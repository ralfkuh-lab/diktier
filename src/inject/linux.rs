//! X11-OutputSink: CLIPBOARD + XTEST + _NET_ACTIVE_WINDOW (Spec §7).

use std::time::{Duration, Instant};

use x11rb::atom_manager;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::properties::WmClass;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as XprotoExt, CreateWindowAux, EventMask, KEY_PRESS_EVENT,
    KEY_RELEASE_EVENT, PropMode, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent,
    SelectionRequestEvent, Window, WindowClass,
};
use x11rb::protocol::xtest::ConnectionExt as XtestExt;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, CURRENT_TIME, NONE};

use crate::config::OutputConfig;

use super::protocol::{
    ClipboardHost, ClipboardSnapshot, ModifierState, PumpEvents, apply_leading_space, inject_paste,
};
use super::{CaptureContext, InjectError, InjectOutcome, OutputSink, PasteKey, WindowId};

atom_manager! {
    Atoms: AtomsCookie {
        CLIPBOARD,
        UTF8_STRING,
        TARGETS,
        INCR,
        TEXT,
        TIMESTAMP,
        _NET_ACTIVE_WINDOW,
        DIKTIER_SELECTION: b"DIKTIER_SELECTION",
    }
}

const XK_CONTROL_L: u32 = 0xffe3;
const XK_CONTROL_R: u32 = 0xffe4;
const XK_SHIFT_L: u32 = 0xffe1;
const XK_SHIFT_R: u32 = 0xffe2;
const XK_ALT_L: u32 = 0xffe9;
const XK_ALT_R: u32 = 0xffea;
const XK_SUPER_L: u32 = 0xffeb;
const XK_SUPER_R: u32 = 0xffec;
const XK_META_L: u32 = 0xffe7;
const XK_META_R: u32 = 0xffe8;
const XK_V: u32 = 0x0056;
const XK_LOWER_V: u32 = 0x0076;
const XK_INSERT: u32 = 0xff63;
const XK_KP_INSERT: u32 = 0xff9e;

struct Keycodes {
    shift_l: u8,
    shift_r: u8,
    alt_l: u8,
    alt_r: u8,
    super_l: u8,
    super_r: u8,
    ctrl_l: u8,
    ctrl_r: u8,
    v: u8,
    insert: u8,
}

impl Keycodes {
    fn lookup(conn: &RustConnection) -> Result<Self, InjectError> {
        Ok(Self {
            shift_l: lookup_keysym(conn, XK_SHIFT_L)?,
            shift_r: lookup_keysym_or(conn, XK_SHIFT_R, 0)?,
            alt_l: lookup_keysym(conn, XK_ALT_L)?,
            alt_r: lookup_keysym_or(conn, XK_ALT_R, 0)?,
            super_l: first_keysym(conn, &[XK_SUPER_L, XK_META_L])?,
            super_r: first_keysym(conn, &[XK_SUPER_R, XK_META_R])?,
            ctrl_l: lookup_keysym(conn, XK_CONTROL_L)?,
            ctrl_r: lookup_keysym_or(conn, XK_CONTROL_R, 0)?,
            v: nonzero_key(first_keysym(conn, &[XK_LOWER_V, XK_V])?, "V")?,
            insert: nonzero_key(first_keysym(conn, &[XK_INSERT, XK_KP_INSERT])?, "Insert")?,
        })
    }

    fn pair(&self, key: PasteKey) -> [u8; 2] {
        match key {
            PasteKey::Shift => [self.shift_l, self.shift_r],
            PasteKey::Alt => [self.alt_l, self.alt_r],
            PasteKey::Super => [self.super_l, self.super_r],
            PasteKey::Ctrl => [self.ctrl_l, self.ctrl_r],
            PasteKey::V => [self.v, 0],
            PasteKey::Insert => [self.insert, 0],
        }
    }

    fn primary(&self, key: PasteKey) -> u8 {
        self.pair(key)[0]
    }
}

pub struct X11OutputSink {
    conn: RustConnection,
    root: Window,
    window: Window,
    atoms: Atoms,
    keys: Keycodes,
    output: OutputConfig,
    serve: String,
    we_own: bool,
    /// Server-Zeit der eigenen CLIPBOARD-Übernahme (kein CURRENT_TIME-TOCTOU, codex H1).
    owned_time: u32,
    start: Instant,
}

impl X11OutputSink {
    pub fn new(output: OutputConfig) -> Result<Self, InjectError> {
        let (conn, screen_num) = RustConnection::connect(None).map_err(x11_err)?;
        let root = conn.setup().roots[screen_num].root;
        let atoms = Atoms::new(&conn)
            .map_err(x11_err)?
            .reply()
            .map_err(x11_err)?;
        if conn
            .extension_information(x11rb::protocol::xtest::X11_EXTENSION_NAME)
            .map_err(x11_err)?
            .is_none()
        {
            return Err(InjectError::Failed(
                "XTEST-Erweiterung nicht verfügbar".into(),
            ));
        }
        let _ = conn.xtest_get_version(2, 1).map_err(x11_err)?.reply();

        let window = conn.generate_id().map_err(x11_err)?;
        conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )
        .map_err(x11_err)?;
        conn.flush().map_err(x11_err)?;

        let keys = Keycodes::lookup(&conn)?;
        Ok(Self {
            conn,
            root,
            window,
            atoms,
            keys,
            output,
            serve: String::new(),
            we_own: false,
            owned_time: CURRENT_TIME,
            start: Instant::now(),
        })
    }

    pub fn active_window_id(&self) -> Option<WindowId> {
        self.read_active_window()
    }

    fn read_active_window(&self) -> Option<WindowId> {
        let reply = self
            .conn
            .get_property(
                false,
                self.root,
                self.atoms._NET_ACTIVE_WINDOW,
                AtomEnum::WINDOW,
                0,
                1,
            )
            .ok()?
            .reply()
            .ok()?;
        if reply.format != 32 || reply.value_len == 0 {
            return None;
        }
        let wid = reply.value32()?.next()?;
        if wid == 0 {
            None
        } else {
            Some(WindowId(u64::from(wid)))
        }
    }

    fn drain_events(&mut self, out: &mut PumpEvents) -> Result<(), InjectError> {
        loop {
            match self.conn.poll_for_event().map_err(x11_err)? {
                None => break,
                Some(Event::SelectionRequest(ev)) => {
                    if self.handle_selection_request(&ev)? {
                        out.reads += 1;
                    }
                }
                Some(Event::SelectionClear(ev)) => {
                    if ev.selection == self.atoms.CLIPBOARD {
                        self.we_own = false;
                        out.lost_ownership = true;
                    }
                }
                Some(Event::Error(err)) => {
                    return Err(InjectError::Failed(format!("X11-Fehler: {err:?}")));
                }
                Some(_) => {}
            }
        }
        Ok(())
    }

    /// TARGETS zählt nicht als Read des Transkripts — nur Daten-Targets.
    /// MULTIPLE ist in v1 bewusst nicht implementiert (Orchestrator: dokumentierte Lücke).
    fn handle_selection_request(
        &mut self,
        ev: &SelectionRequestEvent,
    ) -> Result<bool, InjectError> {
        if ev.selection != self.atoms.CLIPBOARD || !self.we_own {
            self.refuse_selection(ev)?;
            return Ok(false);
        }
        let property = if ev.property == 0 {
            ev.target
        } else {
            ev.property
        };
        if ev.target == self.atoms.TARGETS {
            let targets = [
                self.atoms.TARGETS,
                self.atoms.UTF8_STRING,
                u32::from(AtomEnum::STRING),
                self.atoms.TEXT,
                self.atoms.TIMESTAMP,
            ];
            if self
                .write_property32(ev.requestor, property, AtomEnum::ATOM.into(), &targets)
                .is_err()
            {
                self.refuse_selection(ev)?;
                return Ok(false);
            }
            self.notify_selection(ev, property)?;
            return Ok(false);
        }
        if ev.target == self.atoms.TIMESTAMP {
            if self
                .write_property32(
                    ev.requestor,
                    property,
                    u32::from(AtomEnum::INTEGER),
                    &[self.owned_time],
                )
                .is_err()
            {
                self.refuse_selection(ev)?;
                return Ok(false);
            }
            self.notify_selection(ev, property)?;
            return Ok(false);
        }
        if ev.target == self.atoms.UTF8_STRING
            || ev.target == u32::from(AtomEnum::STRING)
            || ev.target == self.atoms.TEXT
        {
            let bytes = if ev.target == u32::from(AtomEnum::STRING) {
                to_latin1_lossy(&self.serve)
            } else {
                self.serve.as_bytes().to_vec()
            };
            // Kein INCR in v1: Transkripte sind praktisch klein (60-s-Cap).
            // Zu große Payloads ablehnen statt das Protokoll zu brechen (codex M1).
            if !self.data_fits(bytes.len()) {
                let _ = self.conn.delete_property(ev.requestor, property);
                self.refuse_selection(ev)?;
                return Ok(false);
            }
            let ty = if ev.target == u32::from(AtomEnum::STRING) {
                u32::from(AtomEnum::STRING)
            } else {
                self.atoms.UTF8_STRING
            };
            if self
                .write_property8(ev.requestor, property, ty, &bytes)
                .is_err()
            {
                self.refuse_selection(ev)?;
                return Ok(false);
            }
            self.notify_selection(ev, property)?;
            return Ok(true);
        }
        self.refuse_selection(ev)?;
        Ok(false)
    }

    fn data_fits(&self, nbytes: usize) -> bool {
        let padded = nbytes.saturating_add(3) & !3;
        padded.saturating_add(64) < self.conn.maximum_request_bytes()
    }

    fn write_property8(
        &self,
        window: Window,
        property: Atom,
        ty: Atom,
        data: &[u8],
    ) -> Result<(), InjectError> {
        self.conn
            .change_property8(PropMode::REPLACE, window, property, ty, data)
            .map_err(x11_err)?
            .check()
            .map_err(x11_err)
    }

    fn write_property32(
        &self,
        window: Window,
        property: Atom,
        ty: Atom,
        data: &[u32],
    ) -> Result<(), InjectError> {
        self.conn
            .change_property32(PropMode::REPLACE, window, property, ty, data)
            .map_err(x11_err)?
            .check()
            .map_err(x11_err)
    }

    fn notify_selection(
        &self,
        ev: &SelectionRequestEvent,
        property: Atom,
    ) -> Result<(), InjectError> {
        let event = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: ev.time,
            requestor: ev.requestor,
            selection: ev.selection,
            target: ev.target,
            property,
        };
        self.conn
            .send_event(false, ev.requestor, EventMask::NO_EVENT, event)
            .map_err(x11_err)?
            .check()
            .map_err(x11_err)?;
        self.conn.flush().map_err(x11_err)?;
        Ok(())
    }

    fn next_server_time(&mut self) -> Result<u32, InjectError> {
        self.write_property8(
            self.window,
            self.atoms.DIKTIER_SELECTION,
            u32::from(AtomEnum::STRING),
            b"t",
        )?;
        self.conn.flush().map_err(x11_err)?;
        let deadline = Instant::now() + Duration::from_millis(200);
        loop {
            while let Some(ev) = self.conn.poll_for_event().map_err(x11_err)? {
                match ev {
                    Event::PropertyNotify(p) if p.window == self.window => {
                        return Ok(p.time);
                    }
                    Event::SelectionRequest(r) => {
                        let _ = self.handle_selection_request(&r)?;
                    }
                    Event::SelectionClear(c) if c.selection == self.atoms.CLIPBOARD => {
                        self.we_own = false;
                    }
                    Event::Error(err) => {
                        return Err(InjectError::Failed(format!("X11-Fehler: {err:?}")));
                    }
                    _ => {}
                }
            }
            if Instant::now() >= deadline {
                return Ok(CURRENT_TIME);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn refuse_selection(&self, ev: &SelectionRequestEvent) -> Result<(), InjectError> {
        self.notify_selection(ev, u32::from(AtomEnum::NONE))
    }

    fn convert_clipboard(&mut self, target: Atom) -> Result<Option<Vec<u8>>, InjectError> {
        let prop = self.atoms.DIKTIER_SELECTION;
        let _ = self.conn.delete_property(self.window, prop);
        self.conn
            .convert_selection(
                self.window,
                self.atoms.CLIPBOARD,
                target,
                prop,
                CURRENT_TIME,
            )
            .map_err(x11_err)?;
        self.conn.flush().map_err(x11_err)?;
        let deadline = Instant::now() + Duration::from_millis(400);
        loop {
            while let Some(ev) = self.conn.poll_for_event().map_err(x11_err)? {
                match ev {
                    Event::SelectionNotify(n)
                        if n.selection == self.atoms.CLIPBOARD && n.target == target =>
                    {
                        if n.property == 0 {
                            return Ok(None);
                        }
                        return self.read_property(n.property);
                    }
                    Event::SelectionRequest(r) => {
                        let _ = self.handle_selection_request(&r)?;
                    }
                    Event::SelectionClear(c) if c.selection == self.atoms.CLIPBOARD => {
                        self.we_own = false;
                    }
                    Event::Error(err) => {
                        return Err(InjectError::Failed(format!("X11-Fehler: {err:?}")));
                    }
                    _ => {}
                }
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            // Eventloop-Takt, kein Restore-Sleep (Spec §7.1 Punkt 6).
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn read_property(&mut self, property: Atom) -> Result<Option<Vec<u8>>, InjectError> {
        let reply = self
            .conn
            .get_property(
                true,
                self.window,
                property,
                AtomEnum::ANY,
                0,
                256 * 1024 / 4,
            )
            .map_err(x11_err)?
            .reply()
            .map_err(x11_err)?;
        if reply.type_ == 0 {
            return Ok(None);
        }
        // INCR-Angebot: kein Restore-Versprechen in v1 (codex M1). Kein volles INCR.
        if reply.type_ == self.atoms.INCR || reply.bytes_after > 0 {
            return Ok(None);
        }
        Ok(Some(reply.value))
    }

    fn keymap(&self) -> Result<[u8; 32], InjectError> {
        Ok(self
            .conn
            .query_keymap()
            .map_err(x11_err)?
            .reply()
            .map_err(x11_err)?
            .keys)
    }

    fn fake_key(&self, press: bool, keycode: u8) -> Result<(), InjectError> {
        if keycode == 0 {
            return Ok(());
        }
        let ty = if press {
            KEY_PRESS_EVENT
        } else {
            KEY_RELEASE_EVENT
        };
        self.conn
            .xtest_fake_input(ty, keycode, CURRENT_TIME, NONE, 0, 0, 0)
            .map_err(x11_err)?
            .check()
            .map_err(x11_err)?;
        Ok(())
    }

    fn any_down(map: &[u8; 32], codes: [u8; 2]) -> bool {
        codes
            .into_iter()
            .filter(|c| *c != 0)
            .any(|c| keymap_down(map, c))
    }
}

impl ClipboardHost for X11OutputSink {
    fn mark_start(&mut self) {
        self.start = Instant::now();
    }

    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    fn current_window(&self) -> Option<WindowId> {
        self.read_active_window()
    }

    fn wm_class(&self, window: WindowId) -> Option<(String, String)> {
        let wid = u32::try_from(window.0).ok()?;
        let wm = WmClass::get(&self.conn, wid).ok()?.reply().ok()??;
        Some((
            String::from_utf8_lossy(wm.instance()).into_owned(),
            String::from_utf8_lossy(wm.class()).into_owned(),
        ))
    }

    fn snapshot_clipboard(&mut self) -> Result<ClipboardSnapshot, InjectError> {
        let mut dummy = PumpEvents::default();
        self.drain_events(&mut dummy)?;
        let owner = self
            .conn
            .get_selection_owner(self.atoms.CLIPBOARD)
            .map_err(x11_err)?
            .reply()
            .map_err(x11_err)?;
        if owner.owner != self.window {
            self.we_own = false;
        }
        if self.we_own && owner.owner == self.window {
            return Ok(ClipboardSnapshot::Text(self.serve.clone()));
        }
        if owner.owner == 0 {
            return Ok(ClipboardSnapshot::Text(String::new()));
        }
        if let Some(bytes) = self.convert_clipboard(self.atoms.UTF8_STRING)? {
            return match String::from_utf8(bytes) {
                Ok(text) => Ok(ClipboardSnapshot::Text(text)),
                Err(_) => Ok(ClipboardSnapshot::NonText),
            };
        }
        if let Some(bytes) = self.convert_clipboard(u32::from(AtomEnum::STRING))? {
            let text = bytes.iter().copied().map(char::from).collect();
            return Ok(ClipboardSnapshot::Text(text));
        }
        Ok(ClipboardSnapshot::NonText)
    }

    fn become_owner(&mut self, text: String) -> Result<(), InjectError> {
        self.serve = text;
        let time = self.next_server_time()?;
        self.conn
            .set_selection_owner(self.window, self.atoms.CLIPBOARD, time)
            .map_err(x11_err)?
            .check()
            .map_err(x11_err)?;
        let owner = self
            .conn
            .get_selection_owner(self.atoms.CLIPBOARD)
            .map_err(x11_err)?
            .reply()
            .map_err(x11_err)?;
        if owner.owner != self.window {
            self.we_own = false;
            return Err(InjectError::Failed(
                "CLIPBOARD-Ownership nicht übernommen".into(),
            ));
        }
        self.owned_time = time;
        self.we_own = true;
        Ok(())
    }

    fn still_owner(&mut self) -> Result<bool, InjectError> {
        let owner = self
            .conn
            .get_selection_owner(self.atoms.CLIPBOARD)
            .map_err(x11_err)?
            .reply()
            .map_err(x11_err)?;
        let ours = owner.owner == self.window;
        if !ours {
            self.we_own = false;
        }
        Ok(ours && self.we_own)
    }

    fn set_serve_text(&mut self, text: String) {
        self.serve = text;
    }

    fn release_ownership(&mut self) -> Result<(), InjectError> {
        if !self.still_owner()? {
            return Ok(());
        }
        self.conn
            .set_selection_owner(NONE, self.atoms.CLIPBOARD, self.owned_time)
            .map_err(x11_err)?
            .check()
            .map_err(x11_err)?;
        self.conn.flush().map_err(x11_err)?;
        self.we_own = false;
        Ok(())
    }

    fn query_modifiers(&self) -> Result<ModifierState, InjectError> {
        let map = self.keymap()?;
        Ok(ModifierState {
            shift: Self::any_down(&map, self.keys.pair(PasteKey::Shift)),
            alt: Self::any_down(&map, self.keys.pair(PasteKey::Alt)),
            super_key: Self::any_down(&map, self.keys.pair(PasteKey::Super)),
            ctrl: Self::any_down(&map, self.keys.pair(PasteKey::Ctrl)),
        })
    }

    fn key_down(&mut self, key: PasteKey) -> Result<(), InjectError> {
        match key {
            PasteKey::V | PasteKey::Insert => {
                self.fake_key(true, self.keys.primary(key))?;
            }
            other => {
                let map = self.keymap()?;
                let codes = self.keys.pair(other);
                if keymap_down(&map, codes[0]) || keymap_down(&map, codes[1]) {
                    // bereits unten
                } else {
                    self.fake_key(true, self.keys.primary(other))?;
                }
            }
        }
        self.conn.flush().map_err(x11_err)?;
        Ok(())
    }

    fn key_up(&mut self, key: PasteKey) -> Result<(), InjectError> {
        match key {
            PasteKey::V | PasteKey::Insert => {
                self.fake_key(false, self.keys.primary(key))?;
            }
            other => {
                let map = self.keymap()?;
                for code in self.keys.pair(other) {
                    if keymap_down(&map, code) {
                        self.fake_key(false, code)?;
                    }
                }
            }
        }
        self.conn.flush().map_err(x11_err)?;
        Ok(())
    }

    fn pump(&mut self, timeout: Duration) -> Result<PumpEvents, InjectError> {
        let deadline = Instant::now() + timeout;
        let mut out = PumpEvents::default();
        loop {
            self.drain_events(&mut out)?;
            if Instant::now() >= deadline {
                break;
            }
            let remain = deadline.saturating_duration_since(Instant::now());
            // Eventloop-Takt, kein sleep über die Restore-Wartezeit (Spec §7.1 Punkt 6).
            std::thread::sleep(remain.min(Duration::from_millis(5)));
        }
        Ok(out)
    }
}

impl OutputSink for X11OutputSink {
    fn paste(&mut self, text: &str, ctx: &CaptureContext) -> Result<InjectOutcome, InjectError> {
        let output = self.output.clone();
        inject_paste(self, text, ctx, &output)
    }

    fn copy_only(&mut self, text: &str) -> Result<(), InjectError> {
        let text = apply_leading_space(text, self.output.leading_space);
        self.become_owner(text)
    }

    fn current_window_id(&self) -> Option<WindowId> {
        self.read_active_window()
    }

    fn serve_for(&mut self, duration: Duration) -> Result<(), InjectError> {
        let _ = self.pump(duration)?;
        Ok(())
    }

    fn serve_until_read(&mut self, timeout: Duration) -> Result<u32, InjectError> {
        super::protocol::serve_restored_until_read(self, timeout)
    }
}

impl Drop for X11OutputSink {
    fn drop(&mut self) {
        if self.we_own {
            let _ = self
                .conn
                .set_selection_owner(NONE, self.atoms.CLIPBOARD, CURRENT_TIME);
        }
        let _ = self.conn.destroy_window(self.window);
        let _ = self.conn.flush();
    }
}

fn keymap_down(keys: &[u8; 32], keycode: u8) -> bool {
    if keycode == 0 {
        return false;
    }
    let i = usize::from(keycode);
    let byte = i / 8;
    if byte >= keys.len() {
        return false;
    }
    keys[byte] & (1 << (i % 8)) != 0
}

fn nonzero_key(code: u8, name: &str) -> Result<u8, InjectError> {
    if code == 0 {
        Err(InjectError::Failed(format!(
            "Keycode für {name} nicht gefunden"
        )))
    } else {
        Ok(code)
    }
}

fn first_keysym(conn: &RustConnection, keysyms: &[u32]) -> Result<u8, InjectError> {
    for keysym in keysyms {
        let code = lookup_keysym_or(conn, *keysym, 0)?;
        if code != 0 {
            return Ok(code);
        }
    }
    Ok(0)
}

fn lookup_keysym(conn: &RustConnection, keysym: u32) -> Result<u8, InjectError> {
    let code = lookup_keysym_or(conn, keysym, 0)?;
    if code == 0 {
        Err(InjectError::Failed(format!(
            "Keysym {keysym:#x} hat keinen Keycode"
        )))
    } else {
        Ok(code)
    }
}

fn lookup_keysym_or(conn: &RustConnection, keysym: u32, fallback: u8) -> Result<u8, InjectError> {
    let setup = conn.setup();
    let min = setup.min_keycode;
    let max = setup.max_keycode;
    let count = max - min + 1;
    let mapping = conn
        .get_keyboard_mapping(min, count)
        .map_err(x11_err)?
        .reply()
        .map_err(x11_err)?;
    let per = usize::from(mapping.keysyms_per_keycode);
    if per == 0 {
        return Ok(fallback);
    }
    for (i, chunk) in mapping.keysyms.chunks(per).enumerate() {
        if chunk.contains(&keysym) {
            return Ok(min + i as u8);
        }
    }
    Ok(fallback)
}

fn to_latin1_lossy(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| if (c as u32) <= 0xff { c as u8 } else { b'?' })
        .collect()
}

fn x11_err<E: std::fmt::Display>(err: E) -> InjectError {
    InjectError::Failed(format!("X11: {err}"))
}
