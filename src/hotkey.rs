//! HotkeyBackend: Press/Release (Spec §4.4 / §5.1) über `WH_KEYBOARD_LL`.

use thiserror::Error;

use crate::config::{HotkeyConfig, Modifier};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Press,
    Release,
}

#[derive(Debug, Error)]
pub enum HotkeyError {
    #[error("Hotkey fehlgeschlagen: {0}")]
    Failed(String),
}

/// Die tatsächlich zu greifende Taste (§4.4: „Nur über Config änderbar").
///
/// Diese Struktur ist der Weg von [`HotkeyConfig`] bis in den Virtual-Key des
/// Hooks (codex H3: die validierte Config bestimmt die Taste, nicht ein
/// hartverdrahtetes `F9`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeySpec {
    /// Kanonischer Schlüsselname aus der Config (`F9`, `A`, `Space`, …).
    pub key: String,
    pub modifiers: Vec<Modifier>,
}

impl Default for HotkeySpec {
    fn default() -> Self {
        Self {
            key: "F9".into(),
            modifiers: Vec::new(),
        }
    }
}

impl HotkeySpec {
    pub fn from_config(config: &HotkeyConfig) -> Self {
        Self {
            key: config.key.clone(),
            modifiers: config.modifiers.clone(),
        }
    }

    /// Menschenlesbar für Log und Tooltip: `Ctrl+Shift+F9`.
    pub fn describe(&self) -> String {
        let mut out = String::new();
        for modifier in &self.modifiers {
            out.push_str(modifier_name(*modifier));
            out.push('+');
        }
        out.push_str(&self.key);
        out
    }
}

pub fn modifier_name(modifier: Modifier) -> &'static str {
    match modifier {
        Modifier::Ctrl => "Ctrl",
        Modifier::Shift => "Shift",
        Modifier::Alt => "Alt",
        Modifier::Super => "Super",
    }
}

pub trait HotkeyBackend {
    fn register(&mut self) -> Result<(), HotkeyError>;
    /// §4.4/§5.2: „Hotkey pausieren" muss den Grab wirklich freigeben — sonst
    /// schluckt der Hook die Taste weiter und die fokussierte App sieht sie nie
    /// (codex H3).
    fn unregister(&mut self) -> Result<(), HotkeyError> {
        Ok(())
    }
    fn poll(&mut self) -> Result<Option<HotkeyEvent>, HotkeyError>;
    fn is_registered(&self) -> bool {
        false
    }
    fn backend_name(&self) -> &'static str {
        "stub"
    }
}

/// Auto-Repeat → ein logisches Press, ein logisches Release (Spec §4.4).
#[derive(Debug, Default)]
struct Debounce {
    held: bool,
}

impl Debounce {
    fn on_press(&mut self) -> Option<HotkeyEvent> {
        if self.held {
            None
        } else {
            self.held = true;
            Some(HotkeyEvent::Press)
        }
    }

    fn on_release(&mut self) -> Option<HotkeyEvent> {
        if self.held {
            self.held = false;
            Some(HotkeyEvent::Release)
        } else {
            None
        }
    }
}

/// Vertragsprobe: registriert folgenlos und meldet nie ein Event.
///
/// Seit Phase 5/WP2 gibt es das echte Backend (`win32-ll-hook`) —
/// `new_backend` baut den Stub nirgends mehr. Er bleibt als Prüfstein für die
/// Trait-Defaults im Test.
#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct StubHotkeyBackend;

impl HotkeyBackend for StubHotkeyBackend {
    fn register(&mut self) -> Result<(), HotkeyError> {
        Ok(())
    }

    fn poll(&mut self) -> Result<Option<HotkeyEvent>, HotkeyError> {
        Ok(None)
    }
}

pub enum AnyHotkeyBackend {
    /// Phase 5/WP2: `WH_KEYBOARD_LL` auf eigenem Hook-Thread (§5).
    WinHook(windows::WinHookBackend),
}

impl HotkeyBackend for AnyHotkeyBackend {
    fn register(&mut self) -> Result<(), HotkeyError> {
        match self {
            Self::WinHook(inner) => inner.register(),
        }
    }

    fn unregister(&mut self) -> Result<(), HotkeyError> {
        match self {
            Self::WinHook(inner) => inner.unregister(),
        }
    }

    fn poll(&mut self) -> Result<Option<HotkeyEvent>, HotkeyError> {
        match self {
            Self::WinHook(inner) => inner.poll(),
        }
    }

    fn is_registered(&self) -> bool {
        match self {
            Self::WinHook(inner) => inner.is_registered(),
        }
    }

    fn backend_name(&self) -> &'static str {
        match self {
            Self::WinHook(inner) => inner.backend_name(),
        }
    }
}

/// Hotkey über `WH_KEYBOARD_LL` (Spec §3/§5, Plan WP2).
///
/// `RegisterHotKey` scheidet aus: es meldet kein Release und kann die Taste
/// nicht vor der Zielanwendung verstecken. Der Low-Level-Hook sieht Down und
/// Up, und ein Rückgabewert ≠ 0 schluckt das Ereignis — §4.4: „Der
/// PTT-Hotkey erreicht die fokussierte Anwendung **nie**".
///
/// Der Hook lebt auf einem **eigenen** Thread mit eigener Message-Queue
/// (§5), nie auf dem Tray-Thread: dessen `TrackPopupMenu`-Modalschleife
/// würde den Hook in `LowLevelHooksTimeout` laufen lassen und Windows
/// entfernte ihn stillschweigend.
pub(crate) mod windows {
    use std::cell::RefCell;
    use std::panic::{self, AssertUnwindSafe};
    use std::ptr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{GetLastError, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_LCONTROL, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RWIN, VK_SHIFT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED,
        LLKHF_LOWER_IL_INJECTED, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW,
        SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_APP, WM_KEYDOWN, WM_KEYUP,
        WM_SYSKEYDOWN, WM_SYSKEYUP, WM_USER,
    };

    use super::{Debounce, HotkeyBackend, HotkeyError, HotkeyEvent, HotkeySpec};
    use crate::config::Modifier;

    /// Command-Nachrichten an den Hook-Thread. Der `WM_APP`-Bereich gehört der
    /// Anwendung, und diese Thread-Queue gehört ausschließlich uns.
    const MSG_INSTALL: u32 = WM_APP + 1;
    const MSG_REMOVE: u32 = WM_APP + 2;
    const MSG_STOP: u32 = WM_APP + 3;

    /// Handshake und Join: kurz genug, dass ein hängender Hook-Thread den
    /// Daemon nicht festhält, lang genug für einen belasteten Rechner.
    const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);

    /// Nachfrist nach einem Ready-Timeout: Kommt das `Ready::Up` doch noch,
    /// trägt es die Thread-ID — dann lässt sich `MSG_STOP` gezielt posten,
    /// statt sich allein auf das Cancel-Flag zu verlassen.
    const CANCEL_GRACE: Duration = Duration::from_millis(250);

    /// Ein Command wurde nicht bestätigt. Danach ist das Backend irreversibel
    /// defekt (siehe [`WinHookBackend::mark_broken`]).
    const NO_ANSWER: &str = "Hook-Thread antwortet nicht";

    // ------------------------------------------------------------ VK-Mapping

    /// VK_F1 = 0x70, danach fortlaufend bis VK_F24 = 0x87.
    const VK_F1: u16 = 0x70;
    const VK_F24: u16 = 0x87;

    /// Die benannten Tasten aus §8 — **eine** Tabelle für beide Richtungen
    /// ([`virtual_key`] und [`vk_name`]). Die Namen sind exakt die
    /// kanonischen Config-Schlüssel aus `config::NAMED_KEYS`; ein Test hält
    /// beide Listen deckungsgleich.
    const NAMED_KEYS: &[(&str, u16)] = &[
        ("Space", 0x20),      // VK_SPACE
        ("Tab", 0x09),        // VK_TAB
        ("Enter", 0x0d),      // VK_RETURN
        ("Escape", 0x1b),     // VK_ESCAPE
        ("Backspace", 0x08),  // VK_BACK
        ("Insert", 0x2d),     // VK_INSERT
        ("Delete", 0x2e),     // VK_DELETE
        ("Home", 0x24),       // VK_HOME
        ("End", 0x23),        // VK_END
        ("PageUp", 0x21),     // VK_PRIOR
        ("PageDown", 0x22),   // VK_NEXT
        ("Left", 0x25),       // VK_LEFT
        ("Up", 0x26),         // VK_UP
        ("Right", 0x27),      // VK_RIGHT
        ("Down", 0x28),       // VK_DOWN
        ("ScrollLock", 0x91), // VK_SCROLL
        ("Pause", 0x13),      // VK_PAUSE
        // Die **rechte** Strg-Taste als Hotkey-Taste. Sie ist zugleich eine
        // Modifier-Taste — der Live-Zustand muss deshalb maskiert werden,
        // siehe [`ModifierState::mask_hotkey_key`].
        ("RCtrl", 0xa3), // VK_RCONTROL
    ];

    /// Config-Schlüssel → Windows-Virtual-Key (§8-Tabelle).
    ///
    /// Buchstaben liegen auf dem **Großbuchstaben** — so führt Windows den VK
    /// (`VK_A == 'A'`); Shift ist ein Modifier, kein anderer Code.
    pub fn virtual_key(key: &str) -> Option<u16> {
        if let Some(rest) = key.strip_prefix('F')
            && let Ok(n) = rest.parse::<u8>()
            && (1..=24).contains(&n)
        {
            // VK_F1 = 0x70, danach fortlaufend bis VK_F24 = 0x87.
            return Some(VK_F1 + u16::from(n) - 1);
        }
        if key.len() == 1 {
            let c = key.chars().next()?;
            if c.is_ascii_alphabetic() {
                return u16::try_from(u32::from(c.to_ascii_uppercase())).ok();
            }
            if c.is_ascii_digit() {
                // VK_0..VK_9 sind die ASCII-Ziffern.
                return u16::try_from(u32::from(c)).ok();
            }
        }
        NAMED_KEYS
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, vk)| *vk)
    }

    /// Rückrichtung zu [`virtual_key`]: Virtual-Key → kanonischer Config-Name.
    ///
    /// Der „Hotkey ändern…"-Dialog braucht genau das — er sieht in
    /// `WM_KEYDOWN` nur den VK und muss daraus schreiben, was in
    /// `config.toml` stehen soll. Beide Richtungen kommen aus **einer**
    /// Quelle: die benannten Tasten aus [`NAMED_KEYS`], F-Tasten, Buchstaben
    /// und Ziffern aus derselben Rechnung wie in `virtual_key` (rückwärts).
    ///
    /// Rückgabe ist `String` statt `&'static str`: `F1`..`F24`, `A`..`Z` und
    /// `0`..`9` sind gerechnet, nicht gelistet — für sie gäbe es keinen
    /// statischen Namen, ohne eine zweite Liste zu pflegen.
    pub fn vk_name(vk: u16) -> Option<String> {
        if (VK_F1..=VK_F24).contains(&vk) {
            return Some(format!("F{}", vk - VK_F1 + 1));
        }
        // VK_A..VK_Z bzw. VK_0..VK_9 sind die ASCII-Codes.
        if let Ok(byte) = u8::try_from(vk)
            && (byte.is_ascii_uppercase() || byte.is_ascii_digit())
        {
            return Some((byte as char).to_string());
        }
        NAMED_KEYS
            .iter()
            .find(|(_, code)| *code == vk)
            .map(|(name, _)| (*name).to_string())
    }

    // -------------------------------------------------------------- Modifier

    /// Modifier-Zustand, wie ihn der Hook sieht.
    ///
    /// Linke und rechte Taste sind zusammengefasst (`VK_CONTROL`/`VK_SHIFT`/
    /// `VK_MENU` melden beide Seiten, `VK_LWIN`/`VK_RWIN` werden verodert).
    /// Lock-Tasten (Caps/Num/Scroll) kommen hier gar nicht erst vor und können
    /// den Vergleich deshalb auch nicht verfälschen.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct ModifierState {
        pub ctrl: bool,
        pub shift: bool,
        pub alt: bool,
        pub win: bool,
    }

    impl ModifierState {
        /// Der **geforderte** Zustand: was in der Config steht, ist gedrückt —
        /// alles andere nicht. Der Vergleich ist damit exakt, `Shift+F9` ist
        /// bei Config `F9` kein Treffer (Sol-Review).
        pub fn required(modifiers: &[Modifier]) -> Self {
            let mut out = Self::default();
            for modifier in modifiers {
                match modifier {
                    Modifier::Ctrl => out.ctrl = true,
                    Modifier::Shift => out.shift = true,
                    Modifier::Alt => out.alt = true,
                    Modifier::Super => out.win = true,
                }
            }
            out
        }

        /// Live-Abfrage im Hook-Callback: fünf reine Wertaufrufe, kein Lock,
        /// keine Allokation.
        ///
        /// **Bekannte Kollision (AltGr).** Windows erzeugt für AltGr auf
        /// deutschem Layout `VK_RMENU` **plus** ein synthetisches
        /// `VK_LCONTROL`. `VK_CONTROL`/`VK_MENU` fassen beide Seiten zusammen,
        /// hier erscheint AltGr deshalb als `Ctrl+Alt`. Ein konfigurierter
        /// `Ctrl+Alt+<Key>`-Hotkey kann also beim Tippen von `@`, `€`, `\` …
        /// auslösen. Bewusst nicht umgebaut (Sol-Review): AltGr sauber zu
        /// trennen hieße, die LL-Ereignisfolge mitzuführen (linkes Ctrl mit
        /// `LLKHF_INJECTED`-ähnlicher Herkunft), was für den Default `F9`
        /// (und jeden Chord ohne Ctrl+Alt) nichts bringt. Der Default ist
        /// nicht betroffen, weil zusätzliche Modifier den exakten Vergleich
        /// ohnehin verfehlen.
        fn current() -> Self {
            Self {
                ctrl: is_down(VK_CONTROL),
                shift: is_down(VK_SHIFT),
                alt: is_down(VK_MENU),
                win: is_down(VK_LWIN) || is_down(VK_RWIN),
            }
        }

        /// Der eigene Beitrag der Hotkey-**Taste** muss aus dem Live-Zustand
        /// heraus, wenn sie selbst eine Modifier-Taste ist (§8: `RCtrl`).
        ///
        /// `GetAsyncKeyState(VK_CONTROL)` meldet beide Seiten. Bei Hotkey-Taste
        /// `RCtrl` wäre `ctrl` im Moment des eigenen Downs also immer `true`,
        /// und der exakte Vergleich `live() == required` könnte mit
        /// `modifiers = []` nie greifen. Für `VK_RCONTROL` kommt der
        /// Ctrl-Anteil deshalb allein aus `VK_LCONTROL`:
        ///
        /// - `key = "RCtrl", modifiers = []` feuert, obwohl „Ctrl unten" ist;
        /// - `modifiers = ["ctrl"]` heißt dann: die **linke** Strg zusätzlich
        ///   gehalten.
        ///
        /// Reine Regel, damit sie ohne Win32-Aufruf prüfbar bleibt; `side`
        /// liefert den Zustand der anderen Seite und wird nur ausgewertet, wenn
        /// es darauf ankommt.
        pub fn mask_hotkey_key(self, vk: u16, side: impl FnOnce() -> bool) -> Self {
            if vk == VK_RCONTROL {
                return Self {
                    ctrl: side(),
                    ..self
                };
            }
            self
        }

        /// Live-Zustand, wie ihn **dieser** Hotkey braucht: [`current`] plus
        /// die Maskierung aus [`mask_hotkey_key`].
        ///
        /// [`current`]: Self::current
        /// [`mask_hotkey_key`]: Self::mask_hotkey_key
        fn current_for(vk: u16) -> Self {
            Self::current().mask_hotkey_key(vk, || is_down(VK_LCONTROL))
        }
    }

    /// High-Bit von `GetAsyncKeyState`: die Taste ist gerade unten.
    fn is_down(vk: u16) -> bool {
        // SAFETY: `GetAsyncKeyState` nimmt den VK als Wert, schreibt nichts,
        // hat keine Vorbedingungen und liefert für unbekannte Codes 0.
        let state = unsafe { GetAsyncKeyState(i32::from(vk)) };
        (state as u16 & 0x8000) != 0
    }

    // -------------------------------------------------------- Zustandslogik

    /// Was mit einem Tastenereignis geschieht.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Decision {
        /// Nicht unseres: `CallNextHookEx`, die Zielanwendung bekommt es.
        Pass,
        /// Unseres, aber kein neues logisches Event (Auto-Repeat): `return 1`.
        Swallow,
        /// Unseres: `return 1` **und** melden.
        Emit(HotkeyEvent),
    }

    /// Ein Tastenereignis ohne Win32-Typen — damit bleibt die Zustandslogik
    /// ohne einen einzigen API-Aufruf testbar.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct KeyEvent {
        pub vk: u16,
        pub down: bool,
        /// `LLKHF_INJECTED` oder `LLKHF_LOWER_IL_INJECTED` gesetzt.
        pub injected: bool,
    }

    /// Der thread-lokale Zustand des Hooks.
    pub struct HookState {
        vk: u16,
        required: ModifierState,
        /// Das Down zu diesem Hotkey wurde akzeptiert **und geschluckt**. Dann
        /// muss auch das Up geschluckt werden, unabhängig davon, ob die
        /// Modifier inzwischen losgelassen wurden — sonst sieht die
        /// Zielanwendung ein Up ohne Down (Sol-Review).
        accepted_down: bool,
    }

    impl HookState {
        pub fn new(vk: u16, required: ModifierState) -> Self {
            Self {
                vk,
                required,
                accepted_down: false,
            }
        }

        /// §4.4/§5.2: Beim Freigeben (Pause) fällt der Zustand zurück. Ein
        /// gerade gehaltener Key wird danach nicht mehr geschluckt — genau die
        /// Semantik, die die Pause auch sonst hat.
        pub fn reset(&mut self) {
            self.accepted_down = false;
        }

        /// `live` wird nur ausgewertet, wenn es wirklich auf die Modifier
        /// ankommt — der Callback liegt im globalen Eingabepfad, jeder
        /// gesparte Aufruf zählt gegen `LowLevelHooksTimeout`.
        pub fn on_event(
            &mut self,
            event: KeyEvent,
            live: impl FnOnce() -> ModifierState,
        ) -> Decision {
            // Injizierte Events (unser eigenes `SendInput` aus dem Paste-Pfad)
            // dürfen nie als Hotkey gelten und müssen trotzdem weiterlaufen.
            if event.injected || event.vk != self.vk {
                return Decision::Pass;
            }
            if event.down {
                if self.accepted_down {
                    // Auto-Repeat: §4.4 verlangt genau ein logisches Press.
                    return Decision::Swallow;
                }
                if live() == self.required {
                    self.accepted_down = true;
                    return Decision::Emit(HotkeyEvent::Press);
                }
                // Falscher Chord (z. B. Shift+F9 bei Config `F9`) — die Taste
                // gehört der Zielanwendung.
                Decision::Pass
            } else if self.accepted_down {
                self.accepted_down = false;
                Decision::Emit(HotkeyEvent::Release)
            } else {
                Decision::Pass
            }
        }
    }

    // ------------------------------------------------------- Hook-Callback

    struct HookContext {
        state: HookState,
        /// **Bewusst akzeptiertes Risiko** (Sol-Review): `mpsc::Sender::send`
        /// ist der einzige potenziell langsame Schritt im globalen
        /// Eingabepfad — nicht blockierend, aber weder lock- noch
        /// allokationsfrei garantiert. Er läuft nur bei einem echten
        /// Press/Release des Hotkeys (höchstens zwei pro Diktat), nicht bei
        /// jedem Tastendruck des Systems; ein `sync_channel` + `try_send`
        /// bliebe als Folgeschritt, falls `LowLevelHooksTimeout` je zuschlägt.
        events: Sender<HotkeyEvent>,
    }

    thread_local! {
        /// Nur der Hook-Thread liest und schreibt das: Windows ruft einen
        /// Low-Level-Hook auf genau dem Thread auf, der ihn installiert hat,
        /// und zwar während dieser Nachrichten pumpt. Der Callback braucht
        /// deshalb keinen Mutex und blockiert nie.
        static HOOK: RefCell<Option<HookContext>> = const { RefCell::new(None) };
    }

    unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code < 0 {
            // Dokumentierte Pflicht: unter 0 nichts auswerten, nur weiterreichen.
            // SAFETY: unveränderte Parameter weiterreichen; `NULL` als `hhk`
            // ist der für `WH_KEYBOARD_LL` dokumentierte Wert (Windows
            // ignoriert das Handle und nimmt den nächsten Hook der Kette).
            return unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) };
        }
        // `WH_KEYBOARD_LL` kennt genau diese vier Nachrichten. Alles andere
        // wird unverändert weitergereicht, statt es als Up zu deuten — sonst
        // beendete ein unerwarteter Wert ein akzeptiertes `accepted_down`
        // (Sol-Review).
        let down = match wparam as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => true,
            WM_KEYUP | WM_SYSKEYUP => false,
            _ => {
                // SAFETY: wie oben — unveränderte Parameter an die Kette.
                return unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) };
            }
        };
        // SAFETY: Für `nCode >= 0` (`HC_ACTION`) garantiert Windows, dass
        // `lparam` auf eine gültige, ausgerichtete `KBDLLHOOKSTRUCT` zeigt,
        // die für die Dauer des Callbacks lebt. Es wird nur gelesen und sofort
        // kopiert (`KBDLLHOOKSTRUCT` ist `Copy`).
        let raw = unsafe { *(lparam as *const KBDLLHOOKSTRUCT) };
        let event = KeyEvent {
            vk: raw.vkCode as u16,
            down,
            injected: raw.flags & (LLKHF_INJECTED | LLKHF_LOWER_IL_INJECTED) != 0,
        };

        // Ein Unwind über die Win32-Grenze ist undefiniert, und ein Panic hier
        // risse zusätzlich den Hook-Thread mit. `try_borrow_mut` allein reicht
        // dafür nicht (Sol-Review): auch `LocalKey::with` kann panicken (TLS
        // wird gerade zerstört), und `Sender::send` allokiert. Deshalb liegt
        // die **gesamte** Rust-Auswertung in `catch_unwind` und fällt im
        // Zweifel auf `Pass` zurück — die Taste gehört dann der Anwendung.
        // `AssertUnwindSafe` ist zulässig, weil der einzige geteilte Zustand
        // der thread-lokale `HookState` ist: ein halb geänderter `HookState`
        // führt höchstens zu einem verwaisten Up, nie zu Unsicherheit.
        let decision = panic::catch_unwind(AssertUnwindSafe(|| {
            HOOK.try_with(|cell| {
                // Reentranz kann es hier nicht geben (der Callback läuft in
                // `GetMessageW`, die Commands danach), der sichere Ausgang
                // kostet aber nichts.
                let Ok(mut slot) = cell.try_borrow_mut() else {
                    return Decision::Pass;
                };
                let Some(ctx) = slot.as_mut() else {
                    return Decision::Pass;
                };
                // Der Live-Zustand hängt an der konfigurierten Taste: Ist sie
                // selbst eine Modifier-Taste (`RCtrl`), maskiert
                // `current_for` ihren eigenen Beitrag weg.
                let hotkey_vk = ctx.state.vk;
                let decision = ctx
                    .state
                    .on_event(event, || ModifierState::current_for(hotkey_vk));
                if let Decision::Emit(hotkey) = decision
                    && ctx.events.send(hotkey).is_err()
                {
                    // Niemand hört mehr zu — dann nicht weiter schlucken, sonst
                    // verschwindet die Taste stumm aus dem System. Im geordneten
                    // Shutdown kann das nicht passieren: der Empfänger lebt bis
                    // nach dem bestätigten Stop (siehe `Drop`).
                    ctx.state.reset();
                    return Decision::Pass;
                }
                decision
            })
            .unwrap_or(Decision::Pass)
        }))
        .unwrap_or(Decision::Pass);

        match decision {
            Decision::Pass => {
                // SAFETY: wie oben — unveränderte Parameter an die Kette.
                unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
            }
            // §4.4: non-zero heißt „die fokussierte Anwendung sieht die Taste nie".
            Decision::Swallow | Decision::Emit(_) => 1,
        }
    }

    // ---------------------------------------------------------- Hook-Thread

    /// Ergebnis des Startup-Handshakes.
    enum Ready {
        /// Hook steht, der Thread pumpt; `u32` ist seine Thread-ID als Ziel
        /// für `PostThreadMessageW`.
        Up(u32),
        Failed(String),
    }

    fn install_hook() -> Result<HHOOK, String> {
        // SAFETY: `GetModuleHandleW(NULL)` liefert das Modul-Handle des
        // eigenen Prozesses, nimmt einen Nullzeiger als dokumentiertes
        // Argument und überträgt kein Eigentum.
        let module = unsafe { GetModuleHandleW(ptr::null()) };
        // SAFETY: `hook_proc` hat die von `HOOKPROC` geforderte Signatur und
        // lebt so lange wie der Prozess; `dwThreadId = 0` ist hier **gewählt**
        // — es ist keine Vorbedingung der Signatur, sondern der Wert, der den
        // für uns nötigen globalen Desktop-Hook installiert (ein Thread-Filter
        // sähe die Tasten fremder Anwendungen nicht). Der Rückgabewert wird
        // sofort geprüft.
        let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), module, 0) };
        if hook.is_null() {
            return Err(last_error());
        }
        Ok(hook)
    }

    /// Entfernt den Hook und meldet **den Fehlschlag** (Sol-Review): Wird das
    /// Ergebnis ignoriert und das Handle trotzdem genullt, bleibt der Hook bei
    /// einem echten Fehler aktiv und schluckt weiter, während `unregister()`
    /// Erfolg meldet.
    fn remove_hook(hook: &mut HHOOK) -> Result<(), String> {
        if hook.is_null() {
            return Ok(());
        }
        // SAFETY: Das Handle stammt aus einem erfolgreichen
        // `SetWindowsHookExW` auf genau diesem Thread und wurde noch nicht
        // entfernt — genullt wird es nur nach einem erfolgreichen Unhook, ein
        // zweites Unhook desselben Handles ist also ausgeschlossen. Nach einem
        // Fehlschlag bleibt das Handle stehen und darf erneut versucht werden.
        let ok = unsafe { UnhookWindowsHookEx(*hook) };
        if ok == 0 {
            return Err(last_error());
        }
        *hook = ptr::null_mut();
        Ok(())
    }

    /// Letzter Versuch beim Threadende: Ein hier verbliebener Hook lebt bis
    /// zum Prozessende weiter, deshalb wird der Fehlschlag laut gemeldet und
    /// genau einmal wiederholt.
    fn remove_hook_finally(hook: &mut HHOOK) -> Result<(), String> {
        match remove_hook(hook) {
            Ok(()) => Ok(()),
            Err(first) => {
                eprintln!("diktier: Hook entfernen fehlgeschlagen ({first}) — zweiter Versuch");
                match remove_hook(hook) {
                    Ok(()) => Ok(()),
                    Err(second) => {
                        eprintln!(
                            "diktier: Hook bleibt installiert ({second}) — er endet erst mit dem Prozess"
                        );
                        Err(second)
                    }
                }
            }
        }
    }

    /// §4.4: Der Tooltip nennt den nackten Win32-Code — ein Low-Level-Hook hat
    /// keinen „Konflikt" wie `RegisterHotKey`, und eine erfundene Erklärung
    /// wäre irreführend (Sol-Review).
    fn last_error() -> String {
        // SAFETY: parameterlos, liest nur den Fehlercode dieses Threads.
        let code = unsafe { GetLastError() };
        format!("Win32-Fehler {code}")
    }

    fn with_state(f: impl FnOnce(&mut HookState)) {
        HOOK.with(|cell| {
            if let Ok(mut slot) = cell.try_borrow_mut()
                && let Some(ctx) = slot.as_mut()
            {
                f(&mut ctx.state);
            }
        });
    }

    /// Alles, was der Hook-Thread von außen bekommt.
    struct ThreadPorts {
        events: Sender<HotkeyEvent>,
        ready: Sender<Ready>,
        acks: Sender<Result<(), String>>,
        /// `GetMessageW == -1`: der gesicherte Win32-Fehler, damit `poll()`
        /// ihn nennen kann statt nur „Hook-Thread beendet" (Sol-Review).
        status: Sender<String>,
        /// Der Ready-Handshake ist abgelaufen — das Backend gibt es nicht mehr
        /// (oder gleich nicht). Wer das Flag gesetzt sieht, installiert nichts
        /// mehr, entfernt einen schon installierten Hook und endet. Ohne das
        /// überlebte ein knapp zu spät fertiger Thread samt globalem Hook den
        /// fehlgeschlagenen Start (Sol-Review).
        cancel: Arc<AtomicBool>,
    }

    impl ThreadPorts {
        fn cancelled(&self) -> bool {
            self.cancel.load(Ordering::Acquire)
        }
    }

    fn hook_thread(vk: u16, required: ModifierState, ports: ThreadPorts) {
        // `PostThreadMessageW` scheitert, solange der Thread noch keine
        // Nachrichtenqueue hat. Ein `PeekMessageW` erzwingt sie **vor** dem
        // Handshake — danach ist jedes Post gültig (Sol-Review).
        let mut msg = MSG::default();
        // SAFETY: `msg` ist ein vollständig initialisiertes `MSG`;
        // `hwnd = NULL` meint „alle Fenster dieses Threads", `PM_NOREMOVE`
        // entnimmt nichts, und der Filter `WM_USER..=WM_USER` fasst nichts an,
        // was sonst gebraucht würde.
        unsafe { PeekMessageW(&mut msg, ptr::null_mut(), WM_USER, WM_USER, PM_NOREMOVE) };
        // SAFETY: parameterlos, liefert die ID des aufrufenden Threads.
        let thread_id = unsafe { GetCurrentThreadId() };

        // 1. Cancel-Prüfung: Wer hier schon zu spät ist, installiert gar
        // nichts erst.
        if ports.cancelled() {
            return;
        }

        HOOK.with(|cell| {
            *cell.borrow_mut() = Some(HookContext {
                state: HookState::new(vk, required),
                events: ports.events.clone(),
            });
        });

        let mut hook = match install_hook() {
            Ok(hook) => hook,
            Err(err) => {
                let _ = ports.ready.send(Ready::Failed(err));
                clear_context();
                return;
            }
        };
        // 2. Cancel-Prüfung: Der Hook steht, aber niemand wartet mehr darauf —
        // dann muss er sofort wieder weg.
        if ports.cancelled() || ports.ready.send(Ready::Up(thread_id)).is_err() {
            let _ = remove_hook_finally(&mut hook);
            clear_context();
            return;
        }
        // 3. Cancel-Prüfung: Zwischen `send` und dem blockierenden
        // `GetMessageW` kann das Backend aufgegeben haben. Danach hält nur noch
        // das gezielte `MSG_STOP` aus der Nachfrist (`CANCEL_GRACE`), für das
        // das Backend die eben gesendete Thread-ID braucht.
        if ports.cancelled() {
            let _ = remove_hook_finally(&mut hook);
            clear_context();
            return;
        }

        loop {
            // SAFETY: `msg` ist gültig; `hwnd = NULL` mit Filter 0/0 heißt
            // „alle Nachrichten dieses Threads", also auch die geposteten
            // Commands. Der Rückgabewert wird auf -1 und 0 geprüft.
            let rc = unsafe { GetMessageW(&mut msg, ptr::null_mut(), 0, 0) };
            if rc < 0 {
                // -1 = Fehler: der Thread kann nicht weiterpumpen, also fällt
                // der Hook aus. Den Code **sofort** sichern (jeder weitere
                // Win32-Aufruf überschriebe ihn) und ans Backend melden, damit
                // `poll()` ihn nennen kann statt nur „Thread beendet".
                let err = last_error();
                let _ = ports.status.send(format!("GetMessageW: {err}"));
                break;
            }
            if rc == 0 {
                // `WM_QUIT` — geordnetes Ende, kein Fehler.
                break;
            }
            match msg.message {
                MSG_INSTALL => {
                    let result = if hook.is_null() {
                        install_hook().map(|new| hook = new)
                    } else {
                        Ok(())
                    };
                    if result.is_ok() {
                        with_state(HookState::reset);
                    }
                    let _ = ports.acks.send(result);
                }
                MSG_REMOVE => {
                    // §4.4: State und `registered` dürfen erst nach einem
                    // **bestätigten** Unhook fallen — sonst meldete
                    // `unregister()` Erfolg, während der Hook weiterschluckt.
                    let result = remove_hook(&mut hook);
                    if result.is_ok() {
                        with_state(HookState::reset);
                    }
                    let _ = ports.acks.send(result);
                }
                MSG_STOP => break,
                // Der Thread besitzt kein Fenster; alles andere ist irrelevant
                // und braucht weder `TranslateMessage` noch `DispatchMessageW`.
                _ => {}
            }
        }

        // Der Ack zum Stop kommt erst, wenn der Hook wirklich weg ist: `Drop`
        // wartet darauf, bevor er Sender und Empfänger fallen lässt.
        let result = remove_hook_finally(&mut hook);
        clear_context();
        let _ = ports.acks.send(result);
    }

    fn clear_context() {
        HOOK.with(|cell| {
            if let Ok(mut slot) = cell.try_borrow_mut() {
                *slot = None;
            }
        });
    }

    /// Join, der nie länger als `timeout` hängt — ein hängender Owner-Thread
    /// darf den Shutdown nicht blockieren.
    ///
    /// `Builder::spawn` statt `thread::spawn`: Letzteres panickt, wenn das
    /// System keinen Thread mehr hergibt — ausgerechnet im Aufräumpfad
    /// (Sol-Review). Timeout und Spawnfehler werden gemeldet, statt still zu
    /// bleiben; der Hook-Thread lebt dann bis zum Prozessende weiter.
    fn join_timeout(join: JoinHandle<()>, timeout: Duration) {
        let (tx, rx) = mpsc::channel();
        let waiter = thread::Builder::new()
            .name("diktier-hook-join".into())
            .spawn(move || {
                let _ = join.join();
                let _ = tx.send(());
            });
        match waiter {
            Ok(_) => {
                if rx.recv_timeout(timeout).is_err() {
                    eprintln!(
                        "diktier: Hook-Thread endet nicht innerhalb von {} s — aufgegeben",
                        timeout.as_secs_f32()
                    );
                }
            }
            Err(err) => eprintln!("diktier: Join-Thread für den Hook nicht startbar: {err}"),
        }
    }

    // ------------------------------------------------------------- Backend

    /// `HotkeyBackend` über einen persistenten Hook-Thread.
    ///
    /// `register`/`unregister` installieren und entfernen nur den Hook; der
    /// Thread bleibt stehen und wartet auf den nächsten Command. Beide sind
    /// idempotent — der Daemon schickt `HotkeyCmd::{Grab,Ungrab}` bei jedem
    /// Pause-Wechsel, auch mehrfach hintereinander.
    pub struct WinHookBackend {
        chord: String,
        thread_id: u32,
        events: Receiver<HotkeyEvent>,
        acks: Receiver<Result<(), String>>,
        status: Receiver<String>,
        cancel: Arc<AtomicBool>,
        join: Option<JoinHandle<()>>,
        debounce: Debounce,
        registered: bool,
        /// Ein Command blieb unbestätigt. Danach ist **nicht mehr feststellbar**,
        /// zu welchem Command ein später eintreffender Ack gehört — ein
        /// verspäteter Remove-Ack könnte einen noch gar nicht ausgeführten
        /// Install bestätigen (Sol-Review). Deshalb ist das Backend ab dann
        /// irreversibel defekt: Stop ist gepostet, weitere Commands gibt es
        /// nicht, und `register`/`unregister`/`poll` melden den Fehler, damit
        /// §10 greift (Hotkey aus, Tray-Click bleibt).
        broken: bool,
    }

    impl WinHookBackend {
        pub fn try_new(spec: &HotkeySpec) -> Result<Self, HotkeyError> {
            let chord = spec.describe();
            let vk = virtual_key(&spec.key).ok_or_else(|| {
                failed_with(
                    &chord,
                    &format!("hotkey.key {:?} hat keinen Virtual-Key-Code", spec.key),
                )
            })?;
            let required = ModifierState::required(&spec.modifiers);
            let (event_tx, events) = mpsc::channel();
            let (ack_tx, acks) = mpsc::channel();
            let (status_tx, status) = mpsc::channel();
            let (ready_tx, ready_rx) = mpsc::channel();
            let cancel = Arc::new(AtomicBool::new(false));
            let ports = ThreadPorts {
                events: event_tx,
                ready: ready_tx,
                acks: ack_tx,
                status: status_tx,
                cancel: cancel.clone(),
            };
            let join = thread::Builder::new()
                .name("diktier-hook".into())
                .spawn(move || hook_thread(vk, required, ports))
                .map_err(|e| failed_with(&chord, &format!("Hook-Thread: {e}")))?;

            match ready_rx.recv_timeout(HANDSHAKE_TIMEOUT) {
                Ok(Ready::Up(thread_id)) => Ok(Self {
                    chord,
                    thread_id,
                    events,
                    acks,
                    status,
                    cancel,
                    join: Some(join),
                    debounce: Debounce::default(),
                    registered: true,
                    broken: false,
                }),
                Ok(Ready::Failed(err)) => {
                    cancel.store(true, Ordering::Release);
                    drop(ready_rx);
                    join_timeout(join, HANDSHAKE_TIMEOUT);
                    Err(failed_with(&chord, &err))
                }
                Err(_) => {
                    // Der Thread darf ab jetzt nichts mehr installieren und
                    // muss einen schon installierten Hook wieder entfernen —
                    // sonst überlebte ein globaler Hook den fehlgeschlagenen
                    // Start, und ein zweiter Versuch installierte einen
                    // weiteren (Sol-Review).
                    cancel.store(true, Ordering::Release);
                    // Nachfrist: Kommt das Ready knapp zu spät, trägt es die
                    // Thread-ID — dann beendet ein gezieltes `MSG_STOP` auch
                    // den Thread, der schon in `GetMessageW` steht.
                    if let Ok(Ready::Up(thread_id)) = ready_rx.recv_timeout(CANCEL_GRACE) {
                        // SAFETY: wie in `command` — reines Posten von Werten
                        // an die Queue, die dieser Thread vor dem Handshake
                        // erzwungen hat.
                        unsafe { PostThreadMessageW(thread_id, MSG_STOP, 0, 0) };
                    }
                    drop(ready_rx);
                    join_timeout(join, HANDSHAKE_TIMEOUT);
                    Err(failed_with(&chord, NO_ANSWER))
                }
            }
        }

        /// Command an den Hook-Thread und auf seine Bestätigung warten. Die
        /// Queue existiert seit dem Handshake, also kann das Post nur
        /// scheitern, wenn der Thread tot ist.
        fn command(&mut self, message: u32) -> Result<(), HotkeyError> {
            if self.broken {
                return Err(self.failed(NO_ANSWER));
            }
            // Alte Bestätigungen dürfen die nächste Antwort nicht vortäuschen.
            // Nach einem Timeout reicht das Drainen nicht (der alte Ack kann
            // erst danach eintreffen) — dann ist das Backend defekt und kommt
            // gar nicht mehr hierher.
            while self.acks.try_recv().is_ok() {}
            // SAFETY: `thread_id` stammt aus dem Handshake genau dieses
            // Threads; `PostThreadMessageW` kopiert nur Werte, `wparam` und
            // `lparam` sind 0. Der Rückgabewert wird geprüft.
            let posted = unsafe { PostThreadMessageW(self.thread_id, message, 0, 0) };
            if posted == 0 {
                let err = last_error();
                self.mark_broken();
                return Err(self.failed(&err));
            }
            match self.acks.recv_timeout(HANDSHAKE_TIMEOUT) {
                Ok(Ok(())) => Ok(()),
                Ok(Err(err)) => Err(self.failed(&err)),
                Err(_) => {
                    self.mark_broken();
                    Err(self.failed(NO_ANSWER))
                }
            }
        }

        /// Ab hier ist die Zuordnung Command → Ack verloren. Stop posten,
        /// Cancel setzen und nichts mehr an diesen Thread schicken.
        fn mark_broken(&mut self) {
            if self.broken {
                return;
            }
            self.broken = true;
            // §10: Der Hotkey gilt als tot, auch wenn der Hook womöglich noch
            // steht — der Daemon soll den Fehlerzustand sehen.
            self.registered = false;
            self.cancel.store(true, Ordering::Release);
            // SAFETY: wie oben; ein toter Thread liefert 0, was hier nichts
            // mehr ändert.
            unsafe { PostThreadMessageW(self.thread_id, MSG_STOP, 0, 0) };
        }

        fn failed(&self, reason: &str) -> HotkeyError {
            failed_with(&self.chord, reason)
        }

        /// Alles, was vor einem Zustandswechsel noch im Kanal lag, gehört nicht
        /// mehr dazu.
        fn drain_events(&self) {
            while self.events.try_recv().is_ok() {}
        }
    }

    fn failed_with(chord: &str, reason: &str) -> HotkeyError {
        HotkeyError::Failed(format!("Hotkey nicht verfügbar ({chord}): {reason}"))
    }

    impl HotkeyBackend for WinHookBackend {
        fn register(&mut self) -> Result<(), HotkeyError> {
            if self.broken {
                return Err(self.failed(NO_ANSWER));
            }
            if self.registered {
                return Ok(());
            }
            self.command(MSG_INSTALL)?;
            self.drain_events();
            self.debounce = Debounce::default();
            self.registered = true;
            Ok(())
        }

        fn unregister(&mut self) -> Result<(), HotkeyError> {
            if self.broken {
                return Err(self.failed(NO_ANSWER));
            }
            if !self.registered {
                return Ok(());
            }
            // Scheitert das Unhook, meldet der Ack den Win32-Fehler und `?`
            // trägt ihn weiter — `registered` bleibt dann **wahr**, denn der
            // Hook schluckt weiter (Sol-Review).
            self.command(MSG_REMOVE)?;
            self.drain_events();
            self.debounce = Debounce::default();
            self.registered = false;
            Ok(())
        }

        fn is_registered(&self) -> bool {
            self.registered && !self.broken
        }

        fn poll(&mut self) -> Result<Option<HotkeyEvent>, HotkeyError> {
            if self.broken {
                return Err(self.failed(NO_ANSWER));
            }
            loop {
                match self.events.try_recv() {
                    Ok(HotkeyEvent::Press) => {
                        if let Some(event) = self.debounce.on_press() {
                            return Ok(Some(event));
                        }
                    }
                    Ok(HotkeyEvent::Release) => {
                        if let Some(event) = self.debounce.on_release() {
                            return Ok(Some(event));
                        }
                    }
                    Err(TryRecvError::Empty) => return Ok(None),
                    // Der Hook-Thread ist gestorben (`GetMessageW == -1`,
                    // Threadtod) — §10: Hotkey aus, Tray-Click bleibt.
                    //
                    // **Nicht** erfasst: ein Hook, den Windows wegen
                    // `LowLevelHooksTimeout` still aus der Kette genommen hat.
                    // Dann lebt der Thread weiter, der Kanal bleibt offen und
                    // der Hotkey ist trotzdem tot. Bekanntes Restrisiko —
                    // Watchdog bewusst zurückgestellt (siehe Plan/Notes).
                    Err(TryRecvError::Disconnected) => {
                        let reason = self
                            .status
                            .try_recv()
                            .unwrap_or_else(|_| "Hook-Thread beendet".to_string());
                        self.mark_broken();
                        return Err(self.failed(&reason));
                    }
                }
            }
        }

        fn backend_name(&self) -> &'static str {
            "win32-ll-hook"
        }
    }

    impl Drop for WinHookBackend {
        fn drop(&mut self) {
            self.cancel.store(true, Ordering::Release);
            // SAFETY: wie in `command` — reines Posten von Werten an eine
            // Thread-Queue, die seit dem Handshake existiert. Ein toter Thread
            // liefert 0, was hier nichts mehr ändert.
            let posted = unsafe { PostThreadMessageW(self.thread_id, MSG_STOP, 0, 0) };
            // Auf den **bestätigten** Unhook warten, bevor die Kanäle fallen:
            // `events`/`acks` liegen als Felder in `self` und werden erst nach
            // diesem Rumpf zerstört. So kann der Callback nicht mitten im
            // Shutdown ein Down schlucken, dessen Release dann ins Leere ginge
            // (Sol-Review).
            if posted != 0 && !self.broken {
                match self.acks.recv_timeout(HANDSHAKE_TIMEOUT) {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => {
                        eprintln!("diktier: Hook-Thread meldet beim Stop: {err}");
                    }
                    Err(_) => eprintln!("diktier: Hook-Thread bestätigt den Stop nicht"),
                }
            }
            if let Some(join) = self.join.take() {
                join_timeout(join, HANDSHAKE_TIMEOUT);
            }
        }
    }
}

/// §4.4/§5: Die **konfigurierte** Taste wird per `WH_KEYBOARD_LL` gegriffen.
/// Es gibt keinen zweiten Weg (`RegisterHotKey` liefert kein Release), also
/// auch keine Fallback-Kette — scheitert der Hook, meldet der Daemon
/// `HotkeyUnavailable` und bleibt per Tray-Click bedienbar (§10).
pub fn new_backend(spec: &HotkeySpec) -> Result<AnyHotkeyBackend, HotkeyError> {
    windows::WinHookBackend::try_new(spec).map(AnyHotkeyBackend::WinHook)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_backend_registers_and_is_idle() {
        let mut backend = StubHotkeyBackend;
        backend.register().unwrap();
        assert_eq!(backend.poll().unwrap(), None);
    }

    #[test]
    fn debounce_collapses_auto_repeat() {
        let mut d = Debounce::default();
        assert_eq!(d.on_press(), Some(HotkeyEvent::Press));
        assert_eq!(d.on_press(), None);
        assert_eq!(d.on_press(), None);
        assert_eq!(d.on_release(), Some(HotkeyEvent::Release));
        assert_eq!(d.on_release(), None);
        assert_eq!(d.on_press(), Some(HotkeyEvent::Press));
        assert_eq!(d.on_release(), Some(HotkeyEvent::Release));
    }

    #[test]
    fn spec_from_config_keeps_key_and_modifiers() {
        let config = HotkeyConfig {
            key: "F7".into(),
            modifiers: vec![Modifier::Ctrl, Modifier::Shift],
            ..HotkeyConfig::default()
        };
        let spec = HotkeySpec::from_config(&config);
        assert_eq!(spec.key, "F7");
        assert_eq!(spec.modifiers, vec![Modifier::Ctrl, Modifier::Shift]);
        assert_eq!(spec.describe(), "Ctrl+Shift+F7");
        assert_eq!(HotkeySpec::default().describe(), "F9");
    }

    /// Ein Backend-Doppel, das die Registrierung mitschreibt — damit ist der
    /// Pause-Pfad ohne Win32 prüfbar.
    #[derive(Debug, Default)]
    struct FakeBackend {
        spec: HotkeySpec,
        registered: bool,
        grabs: u32,
        ungrabs: u32,
        queued: Vec<HotkeyEvent>,
        debounce: Debounce,
    }

    impl HotkeyBackend for FakeBackend {
        fn register(&mut self) -> Result<(), HotkeyError> {
            if !self.registered {
                self.registered = true;
                self.grabs += 1;
            }
            Ok(())
        }

        fn unregister(&mut self) -> Result<(), HotkeyError> {
            if self.registered {
                self.registered = false;
                self.ungrabs += 1;
                self.debounce = Debounce::default();
            }
            Ok(())
        }

        fn poll(&mut self) -> Result<Option<HotkeyEvent>, HotkeyError> {
            // Ein freigegebener Grab liefert nichts mehr — die Taste gehört
            // der fokussierten Anwendung.
            if !self.registered {
                self.queued.clear();
                return Ok(None);
            }
            while !self.queued.is_empty() {
                let raw = self.queued.remove(0);
                let mapped = match raw {
                    HotkeyEvent::Press => self.debounce.on_press(),
                    HotkeyEvent::Release => self.debounce.on_release(),
                };
                if let Some(event) = mapped {
                    return Ok(Some(event));
                }
            }
            Ok(None)
        }

        fn is_registered(&self) -> bool {
            self.registered
        }

        fn backend_name(&self) -> &'static str {
            "fake"
        }
    }

    /// §4.4/§5.2: „Hotkey pausieren" gibt den Grab frei, das Aufheben nimmt ihn
    /// zurück — und dazwischen erreicht die Taste den Daemon nicht (codex H3).
    #[test]
    fn pause_ungrabs_and_resume_regrabs() {
        let mut backend = FakeBackend {
            spec: HotkeySpec::default(),
            ..FakeBackend::default()
        };
        backend.register().unwrap();
        assert!(backend.is_registered());
        assert_eq!((backend.grabs, backend.ungrabs), (1, 0));

        backend.queued = vec![HotkeyEvent::Press, HotkeyEvent::Release];
        assert_eq!(backend.poll().unwrap(), Some(HotkeyEvent::Press));
        assert_eq!(backend.poll().unwrap(), Some(HotkeyEvent::Release));

        // Pause: Grab weg, Tastendrücke erreichen uns nicht mehr.
        backend.unregister().unwrap();
        assert!(!backend.is_registered());
        assert_eq!((backend.grabs, backend.ungrabs), (1, 1));
        backend.queued = vec![HotkeyEvent::Press, HotkeyEvent::Release];
        assert_eq!(backend.poll().unwrap(), None, "pausiert: kein Event");

        // Pause aufheben: neuer Grab, alles wieder wie vorher.
        backend.register().unwrap();
        assert!(backend.is_registered());
        assert_eq!((backend.grabs, backend.ungrabs), (2, 1));
        backend.queued = vec![HotkeyEvent::Press, HotkeyEvent::Release];
        assert_eq!(backend.poll().unwrap(), Some(HotkeyEvent::Press));
        assert_eq!(backend.poll().unwrap(), Some(HotkeyEvent::Release));
        assert_eq!(backend.spec, HotkeySpec::default());
    }

    /// Phase 5/WP2: VK-Mapping, exakter Modifier-Vergleich und die
    /// Zustandsmaschine des Hooks — alles reine Funktionen, kein Win32-Aufruf.
    mod win {
        use super::super::windows::{
            Decision, HookState, KeyEvent, ModifierState, virtual_key, vk_name,
        };
        use super::super::{HotkeyEvent, Modifier};

        const VK_F9: u16 = 0x78;
        /// VK_RCONTROL — die Hotkey-**Taste** `RCtrl` aus §8.
        const VK_RCTRL: u16 = 0xa3;

        /// §4.4: Die Config bestimmt die Taste — bis in den Virtual-Key.
        #[test]
        fn config_key_maps_to_the_exact_virtual_key() {
            assert_eq!(virtual_key("F1"), Some(0x70));
            assert_eq!(virtual_key("F9"), Some(VK_F9));
            assert_eq!(virtual_key("F12"), Some(0x7b));
            assert_eq!(virtual_key("F24"), Some(0x87));
            assert_eq!(
                virtual_key("a"),
                Some(0x41),
                "Buchstaben liegen auf dem Großbuchstaben"
            );
            assert_eq!(virtual_key("A"), Some(0x41));
            assert_eq!(virtual_key("F"), Some(0x46), "kein F-Tasten-Präfix");
            assert_eq!(virtual_key("Z"), Some(0x5a));
            assert_eq!(virtual_key("0"), Some(0x30));
            assert_eq!(virtual_key("7"), Some(0x37));
            assert_eq!(virtual_key("Space"), Some(0x20));
            assert_eq!(virtual_key("Enter"), Some(0x0d));
            assert_eq!(virtual_key("Insert"), Some(0x2d));
            assert_eq!(virtual_key("Delete"), Some(0x2e));
            assert_eq!(virtual_key("Home"), Some(0x24));
            assert_eq!(virtual_key("End"), Some(0x23));
            assert_eq!(virtual_key("PageUp"), Some(0x21));
            assert_eq!(virtual_key("PageDown"), Some(0x22));
            assert_eq!(virtual_key("Left"), Some(0x25));
            assert_eq!(virtual_key("Up"), Some(0x26));
            assert_eq!(virtual_key("Right"), Some(0x27));
            assert_eq!(virtual_key("Down"), Some(0x28));
            assert_eq!(virtual_key("ScrollLock"), Some(0x91));
            assert_eq!(virtual_key("Pause"), Some(0x13));
            assert_eq!(virtual_key("RCtrl"), Some(0xa3), "VK_RCONTROL");
            assert_eq!(virtual_key("F0"), None);
            assert_eq!(virtual_key("F25"), None);
            assert_eq!(virtual_key("Grüße"), None);
        }

        /// Rückrichtung für den „Hotkey ändern…"-Dialog: Der Dialog sieht in
        /// `WM_KEYDOWN` nur den Virtual-Key und schreibt daraus den
        /// Config-Schlüssel. Was `virtual_key` erzeugt, muss `vk_name` wieder
        /// als **denselben** kanonischen Namen liefern — sonst schriebe der
        /// Dialog eine `config.toml`, die der nächste Start ablehnt.
        #[test]
        fn vk_name_is_the_inverse_of_virtual_key() {
            for key in [
                "F1",
                "F9",
                "F12",
                "F24",
                "A",
                "Z",
                "0",
                "7",
                "Space",
                "Tab",
                "Enter",
                "Escape",
                "Backspace",
                "Insert",
                "Delete",
                "Home",
                "End",
                "PageUp",
                "PageDown",
                "Left",
                "Up",
                "Right",
                "Down",
                "ScrollLock",
                "Pause",
                "RCtrl",
            ] {
                let vk = virtual_key(key).unwrap_or_else(|| panic!("{key}: kein VK"));
                assert_eq!(vk_name(vk).as_deref(), Some(key), "{key} (VK {vk:#04x})");
            }
            // Kleingeschriebenes kommt kanonisch zurück — genau das braucht
            // die Config.
            assert_eq!(vk_name(virtual_key("a").unwrap()).as_deref(), Some("A"));
        }

        /// Alles, was der Dialog nicht in eine gültige Config schreiben kann,
        /// muss als „nicht unterstützt" erkennbar sein: Modifier-VKs,
        /// Lock-Tasten, Maustasten, Lücken im VK-Raum.
        #[test]
        fn unknown_virtual_keys_have_no_config_name() {
            for vk in [
                0x00, 0x01, // kein VK / VK_LBUTTON
                0x10, 0x11, 0x12, // VK_SHIFT/CONTROL/MENU
                0x14, 0x90, // VK_CAPITAL, VK_NUMLOCK
                0x5b, 0x5c, // VK_LWIN/VK_RWIN
                0xa2, // VK_LCONTROL — nur die **rechte** Strg ist eine Taste
                0x88, 0xff,
            ] {
                assert_eq!(vk_name(vk), None, "VK {vk:#04x}");
            }
        }

        /// Jeder Schlüssel der §8-Tabelle muss einen VK haben — sonst nimmt
        /// die Config einen Wert an, den der Hook nicht greifen kann.
        #[test]
        fn every_config_key_has_a_virtual_key() {
            for key in [
                "F1",
                "F9",
                "F24",
                "A",
                "Z",
                "0",
                "9",
                "Space",
                "Tab",
                "Enter",
                "Escape",
                "Backspace",
                "Insert",
                "Delete",
                "Home",
                "End",
                "PageUp",
                "PageDown",
                "Left",
                "Up",
                "Right",
                "Down",
                "ScrollLock",
                "Pause",
                "RCtrl",
            ] {
                assert!(virtual_key(key).is_some(), "{key}: VK");
            }
        }

        fn mods(ctrl: bool, shift: bool, alt: bool, win: bool) -> ModifierState {
            ModifierState {
                ctrl,
                shift,
                alt,
                win,
            }
        }

        fn down(vk: u16) -> KeyEvent {
            KeyEvent {
                vk,
                down: true,
                injected: false,
            }
        }

        fn up(vk: u16) -> KeyEvent {
            KeyEvent {
                vk,
                down: false,
                injected: false,
            }
        }

        #[test]
        fn required_modifiers_are_exact() {
            assert_eq!(ModifierState::required(&[]), ModifierState::default());
            assert_eq!(
                ModifierState::required(&[Modifier::Ctrl, Modifier::Shift]),
                mods(true, true, false, false)
            );
            assert_eq!(
                ModifierState::required(&[Modifier::Alt, Modifier::Super]),
                mods(false, false, true, true)
            );
        }

        /// Sol-Review: `Shift+F9` darf bei Config `F9` **nicht** greifen —
        /// der Vergleich ist exakt.
        #[test]
        fn extra_modifiers_are_not_a_match() {
            let mut state = HookState::new(VK_F9, ModifierState::default());
            assert_eq!(
                state.on_event(down(VK_F9), || mods(false, true, false, false)),
                Decision::Pass,
                "Shift+F9 ist kein F9"
            );
            assert_eq!(
                state.on_event(up(VK_F9), || mods(false, true, false, false)),
                Decision::Pass,
                "das Up gehört der Zielanwendung, es gab kein akzeptiertes Down"
            );
            assert_eq!(
                state.on_event(down(VK_F9), ModifierState::default),
                Decision::Emit(HotkeyEvent::Press)
            );
        }

        #[test]
        fn missing_modifiers_are_not_a_match_either() {
            let required = ModifierState::required(&[Modifier::Ctrl, Modifier::Shift]);
            let mut state = HookState::new(VK_F9, required);
            assert_eq!(
                state.on_event(down(VK_F9), || mods(true, false, false, false)),
                Decision::Pass,
                "Ctrl+F9 ist kein Ctrl+Shift+F9"
            );
            assert_eq!(
                state.on_event(down(VK_F9), || mods(true, true, false, false)),
                Decision::Emit(HotkeyEvent::Press),
                "der vollständige Chord trifft"
            );
        }

        /// §8 `RCtrl`: Die Hotkey-Taste ist selbst eine Modifier-Taste. Ihr
        /// eigener Beitrag muss aus dem Live-Zustand heraus, sonst könnte
        /// `modifiers = []` nie greifen — `GetAsyncKeyState(VK_CONTROL)` meldet
        /// beide Seiten.
        #[test]
        fn the_hotkey_keys_own_modifier_contribution_is_masked_away() {
            // Für `RCtrl` kommt der Ctrl-Anteil allein von der linken Seite.
            assert_eq!(
                mods(true, false, false, false).mask_hotkey_key(VK_RCTRL, || false),
                mods(false, false, false, false),
                "RCtrl allein ist kein Ctrl"
            );
            assert_eq!(
                mods(true, true, false, false).mask_hotkey_key(VK_RCTRL, || true),
                mods(true, true, false, false),
                "linke Strg zusätzlich gehalten: Ctrl bleibt"
            );
            // Jede andere Taste bleibt unangetastet — die andere Seite wird
            // dafür nicht einmal abgefragt.
            assert_eq!(
                mods(true, false, false, false).mask_hotkey_key(VK_F9, || panic!("nicht abfragen")),
                mods(true, false, false, false)
            );
        }

        /// `key = "RCtrl", modifiers = []`: Drücken feuert, obwohl „Ctrl unten"
        /// ist — der Live-Zustand ist bereits maskiert (siehe
        /// `ModifierState::current_for`).
        #[test]
        fn right_ctrl_alone_is_a_full_hotkey() {
            let mut state = HookState::new(VK_RCTRL, ModifierState::required(&[]));
            assert_eq!(
                state.on_event(down(VK_RCTRL), ModifierState::default),
                Decision::Emit(HotkeyEvent::Press)
            );
            assert_eq!(
                state.on_event(down(VK_RCTRL), ModifierState::default),
                Decision::Swallow,
                "Auto-Repeat wie bei jeder anderen Taste"
            );
            assert_eq!(
                state.on_event(up(VK_RCTRL), ModifierState::default),
                Decision::Emit(HotkeyEvent::Release)
            );
        }

        /// Bei Hotkey `RCtrl` meint `ctrl` im Live-Zustand die **linke** Strg.
        /// Mit `modifiers = []` ist sie ein zusätzlicher Modifier und der Chord
        /// verfehlt — mit `modifiers = ["ctrl"]` ist sie gefordert.
        #[test]
        fn left_ctrl_is_the_extra_modifier_next_to_right_ctrl() {
            let left_ctrl_down = || mods(true, false, false, false);

            let mut plain = HookState::new(VK_RCTRL, ModifierState::required(&[]));
            assert_eq!(
                plain.on_event(down(VK_RCTRL), left_ctrl_down),
                Decision::Pass,
                "LCtrl+RCtrl ist kein nacktes RCtrl"
            );
            assert_eq!(
                plain.on_event(up(VK_RCTRL), left_ctrl_down),
                Decision::Pass,
                "es gab kein akzeptiertes Down"
            );

            let mut with_ctrl =
                HookState::new(VK_RCTRL, ModifierState::required(&[Modifier::Ctrl]));
            assert_eq!(
                with_ctrl.on_event(down(VK_RCTRL), left_ctrl_down),
                Decision::Emit(HotkeyEvent::Press)
            );
            assert_eq!(
                with_ctrl.on_event(up(VK_RCTRL), left_ctrl_down),
                Decision::Emit(HotkeyEvent::Release)
            );
            assert_eq!(
                with_ctrl.on_event(down(VK_RCTRL), ModifierState::default),
                Decision::Pass,
                "ohne linke Strg trifft der Chord nicht"
            );
        }

        /// Lock-Tasten tauchen im Vergleich gar nicht auf — Caps/Num/Scroll
        /// können den Treffer deshalb nicht verhindern.
        #[test]
        fn lock_keys_are_ignored() {
            // `ModifierState` hat schlicht kein Feld für Caps/Num/Scroll, und
            // `current()` fragt sie nie ab: jeder Lock-Zustand ist derselbe
            // Wert, der Vergleich kann daran nicht scheitern.
            let mut state = HookState::new(VK_F9, ModifierState::default());
            assert_eq!(
                state.on_event(down(VK_F9), ModifierState::default),
                Decision::Emit(HotkeyEvent::Press)
            );
            assert_eq!(
                state.on_event(up(VK_F9), ModifierState::default),
                Decision::Emit(HotkeyEvent::Release)
            );
        }

        /// §4.4: ein logisches Press, ein logisches Release — Auto-Repeat wird
        /// geschluckt, erreicht die Zielanwendung aber trotzdem nie.
        #[test]
        fn auto_repeat_is_swallowed_without_event() {
            let mut state = HookState::new(VK_F9, ModifierState::default());
            assert_eq!(
                state.on_event(down(VK_F9), ModifierState::default),
                Decision::Emit(HotkeyEvent::Press)
            );
            assert_eq!(
                state.on_event(down(VK_F9), ModifierState::default),
                Decision::Swallow
            );
            assert_eq!(
                state.on_event(down(VK_F9), ModifierState::default),
                Decision::Swallow
            );
            assert_eq!(
                state.on_event(up(VK_F9), ModifierState::default),
                Decision::Emit(HotkeyEvent::Release)
            );
            assert_eq!(
                state.on_event(up(VK_F9), ModifierState::default),
                Decision::Pass,
                "ein zweites Up gehört nicht mehr uns"
            );
        }

        /// Sol-Review: Wer das Down geschluckt hat, muss auch das Up schlucken
        /// — sonst sieht die Zielanwendung ein Up ohne Down.
        #[test]
        fn release_after_modifier_change_is_still_swallowed_and_reported() {
            let mut state = HookState::new(VK_F9, ModifierState::required(&[Modifier::Ctrl]));
            assert_eq!(
                state.on_event(down(VK_F9), || mods(true, false, false, false)),
                Decision::Emit(HotkeyEvent::Press)
            );
            // Ctrl wird vor F9 losgelassen — VK_CONTROL ist eine fremde Taste.
            assert_eq!(
                state.on_event(up(0x11), || mods(false, false, false, false)),
                Decision::Pass
            );
            assert_eq!(
                state.on_event(up(VK_F9), || mods(false, false, false, false)),
                Decision::Emit(HotkeyEvent::Release)
            );
        }

        /// Plan WP2: Beim Ungrab fällt der Zustand zurück, ein gehaltener Key
        /// wird danach nicht mehr geschluckt (§5.2-Pause-Semantik).
        ///
        /// **Bewusste Ausnahme zu §4.4** (Sol-Review): Wird während eines
        /// geschluckten Downs pausiert, erreicht das folgende Up die
        /// Zielanwendung ohne vorheriges Down. Das lässt keine Taste hängen —
        /// ein Up ohne Down ist für Windows folgenlos —, ist aber wörtlich ein
        /// Ereignis, das die App vom PTT-Key sieht. Die Alternative („disabled,
        /// pending release": erst das Up schlucken, dann unhooken) hielte den
        /// globalen Hook über die Pause hinaus am Leben und widerspräche der
        /// Pause-Semantik, deshalb bleibt es dabei.
        #[test]
        fn reset_passes_orphaned_up() {
            let mut state = HookState::new(VK_F9, ModifierState::default());
            assert_eq!(
                state.on_event(down(VK_F9), ModifierState::default),
                Decision::Emit(HotkeyEvent::Press)
            );
            state.reset();
            assert_eq!(
                state.on_event(up(VK_F9), ModifierState::default),
                Decision::Pass
            );
            assert_eq!(
                state.on_event(down(VK_F9), ModifierState::default),
                Decision::Emit(HotkeyEvent::Press),
                "danach greift der Hotkey wieder"
            );
        }

        /// Eigene `SendInput`-Events (WP3-Paste-Pfad) sind nie ein Hotkey und
        /// laufen immer an `CallNextHookEx` weiter.
        #[test]
        fn injected_events_never_match() {
            let mut state = HookState::new(VK_F9, ModifierState::default());
            let injected_down = KeyEvent {
                vk: VK_F9,
                down: true,
                injected: true,
            };
            assert_eq!(
                state.on_event(injected_down, ModifierState::default),
                Decision::Pass
            );
            let injected_up = KeyEvent {
                vk: VK_F9,
                down: false,
                injected: true,
            };
            assert_eq!(
                state.on_event(injected_up, ModifierState::default),
                Decision::Pass
            );
        }

        #[test]
        fn other_keys_are_never_touched() {
            let mut state = HookState::new(VK_F9, ModifierState::default());
            assert_eq!(
                state.on_event(down(0x41), ModifierState::default),
                Decision::Pass
            );
            assert_eq!(
                state.on_event(up(0x41), ModifierState::default),
                Decision::Pass
            );
        }
    }

    /// Beide Aufrufe sind idempotent — der Daemon schickt sie bei jedem
    /// Pause-Wechsel, auch mehrfach.
    #[test]
    fn register_and_unregister_are_idempotent() {
        let mut backend = FakeBackend::default();
        backend.register().unwrap();
        backend.register().unwrap();
        assert_eq!(backend.grabs, 1);
        backend.unregister().unwrap();
        backend.unregister().unwrap();
        assert_eq!(backend.ungrabs, 1);
    }
}
