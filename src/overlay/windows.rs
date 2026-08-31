//! Das Win32-Fenster des Aufnahme-Overlays (SPEC §4.5).
//!
//! **Owner-Thread.** Fensterklasse, `HWND`, DIB und Memory-DC werden
//! ausschließlich auf dem Thread erzeugt, bespielt und zerstört, der
//! [`OverlayWindow::new`] aufgerufen hat — das ist der Overlay-Worker
//! (`daemon::workers::overlay_loop`) bzw. der Spike `--overlay-test`. Fremde
//! Threads fassen das `HWND` nie an, sie schicken Kommandos über einen Channel
//! (Phase-5-Leitentscheidung 2). `AttachThreadInput` wird nicht verwendet.
//!
//! **Fokusregel §4.2.** `WS_EX_NOACTIVATE` (Windows aktiviert das Fenster
//! nie), `SW_SHOWNOACTIVATE` (auch das Zeigen nicht), `WS_EX_TRANSPARENT` plus
//! `WM_NCHITTEST → HTTRANSPARENT` (Klicks gehen hindurch). Kein
//! `SetForegroundWindow`, kein `SetFocus` — nirgends in dieser Datei.
//!
//! **DPI.** Per-Thread PMv2 wird als **allererstes** gesetzt, vor jeder
//! Fenster- oder Monitor-API; prozessweite Awareness (Manifest) bleibt
//! bewusst ausgeklammert (eigenes Folgepaket). Scheitert das Setzen, gibt es
//! kein Overlay — der Daemon läuft ohne weiter (§4.5).
//!
//! Die DPI des Zielmonitors kommt aus `GetDpiForWindow` auf dem **eigenen**
//! Fenster, nachdem es (noch unsichtbar) in die Arbeitsfläche des Zielmonitors
//! geschoben wurde. Nicht aus `GetDpiForMonitor` (Sol-Impl-Review, Blocker 1):
//! Das ist laut Microsoft ausdrücklich nicht DPI-aware und richtet sich nach
//! der **prozessweiten** Awareness — die hier bewusst unaware bleibt, sodass
//! es auf einem 150-%-Monitor 96 lieferte. Und nicht aus `GetDpiForWindow`
//! eines **fremden** Fensters, dessen Rückgabe an dessen Awareness hängt.

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr;
use std::time::{Duration, Instant};

use thiserror::Error;
use windows_sys::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE,
    WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetMonitorInfoW,
    HBITMAP, HDC, HGDIOBJ, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY,
    MONITORINFO, MonitorFromPoint, MonitorFromWindow, SelectObject,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetThreadDpiAwarenessContext,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
    GetForegroundWindow, GetSystemMetrics, GetWindowLongPtrW, HTTRANSPARENT, MSG, PM_REMOVE,
    PeekMessageW, RegisterClassW, SM_CXSCREEN, SM_CYSCREEN, SPI_SETWORKAREA, SW_HIDE,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    ULW_ALPHA, UnregisterClassW, UpdateLayeredWindow, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_NCCREATE,
    WM_NCDESTROY, WM_NCHITTEST, WM_SETTINGCHANGE, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use super::{Canvas, OverlayState, Rect, card_rect, draw_card, history_capacity};

/// Fensterklasse. Prozessweit eindeutig, wie `DiktierTrayOwner` und
/// `DiktierHotkeyDialog`.
const CLASS_NAME: &str = "DiktierOverlay";

/// Obergrenze je `pump()`: nicht abgearbeitete Nachrichten bleiben in der
/// Queue, die Schleife kann so nicht endlos drehen (wie in `tray::windows`).
const MAX_MESSAGES_PER_PUMP: u32 = 64;

/// Referenz-DPI (100 %). Nur als Rückfallwert für die Arbeitsfläche, wenn
/// Windows keine Monitorinfo herausgibt.
const DEFAULT_DPI: u32 = 96;

/// Größe des versteckten Messfensters beim DPI-Bootstrap.
const PROBE_SIZE: i32 = 1;

/// `HGDI_ERROR` (`(HGDIOBJ)-1`) hat in windows-sys 0.61 keine Konstante; der
/// Wert ist stabile `wingdi.h`-ABI. Dieselbe Begründung wie bei
/// `NIN_KEYSELECT` in `tray::windows` und `CF_UNICODETEXT` in
/// `inject::windows`. Verglichen wird als `isize`, weil `HGDIOBJ` ein
/// roher Zeiger ist.
const HGDI_ERROR: isize = -1;

#[derive(Debug, Error)]
pub enum OverlayError {
    #[error("Overlay fehlgeschlagen: {0}")]
    Failed(String),
}

fn failed(message: impl Into<String>) -> OverlayError {
    OverlayError::Failed(message.into())
}

/// NUL-terminierter UTF-16-Puffer für die `W`-APIs.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last_error() -> u32 {
    // SAFETY: parameterlos, liest den Fehlercode dieses Threads.
    unsafe { GetLastError() }
}

fn rect_from(raw: &RECT) -> Rect {
    Rect::new(raw.left, raw.top, raw.right, raw.bottom)
}

/// Per-Monitor-V2 für **diesen** Thread. Muss vor jeder Fenster- und
/// Monitor-API laufen, sonst rechnet Windows die Koordinaten virtualisiert um
/// (Leitentscheidung 6). `false` heißt: Windows kennt den Kontext nicht — das
/// Overlay läuft dann in der Awareness des Prozesses weiter und ist auf
/// skalierten Monitoren unscharf, aber nicht kaputt.
fn set_thread_dpi_awareness() -> bool {
    // SAFETY: dokumentierte Konstante, kein Zeiger auf eigenen Speicher; der
    // Rückgabewert ist der vorherige Kontext (NULL = Fehler).
    let previous =
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    !previous.is_null()
}

/// Monitor des fokussierten Fensters; ohne Vordergrundfenster der
/// Primärmonitor (Leitentscheidung 7).
fn foreground_monitor() -> HMONITOR {
    // SAFETY: parameterlos; `NULL` heißt „kein Vordergrundfenster".
    let foreground = unsafe { GetForegroundWindow() };
    if !foreground.is_null() {
        // SAFETY: gültiges (fremdes) Fensterhandle, nur als Schlüssel benutzt —
        // dereferenziert wird es nie (Phase-5-Leitentscheidung 2).
        let monitor = unsafe { MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST) };
        if !monitor.is_null() {
            return monitor;
        }
    }
    // SAFETY: POD-Parameter; `MONITOR_DEFAULTTOPRIMARY` liefert immer einen
    // Monitor, solange überhaupt einer angeschlossen ist.
    unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) }
}

/// Monitor, auf dem ein Rechteck (mehrheitlich) liegt — für den von
/// `WM_DPICHANGED` vorgeschlagenen Rect.
fn monitor_from_rect(rect: Rect) -> HMONITOR {
    let center = POINT {
        x: rect.left + rect.width() / 2,
        y: rect.top + rect.height() / 2,
    };
    // SAFETY: POD-Parameter; `MONITOR_DEFAULTTONEAREST` klemmt auf den
    // nächstgelegenen aktiven Monitor, falls der alte weg ist (Sol Major 9).
    unsafe { MonitorFromPoint(center, MONITOR_DEFAULTTONEAREST) }
}

/// Arbeitsfläche eines Monitors (ohne Taskleiste).
fn monitor_work_area(monitor: HMONITOR) -> Option<Rect> {
    if monitor.is_null() {
        return None;
    }
    // SAFETY: `MONITORINFO` ist POD; genullt plus `cbSize` ist der
    // dokumentierte Ausgangszustand.
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    // SAFETY: `info` ist gültiger, passend dimensionierter Speicher.
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return None;
    }
    Some(rect_from(&info.rcWork))
}

/// Letzter Ausweg, wenn Windows keine Monitorinfo liefert: der Primärbildschirm
/// ohne Taskleistenabzug. Besser eine leicht zu tiefe Karte als gar keine.
fn primary_screen_fallback() -> Rect {
    // SAFETY: parameterlose Lesezugriffe auf Systemmetriken.
    let (width, height) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    Rect::new(0, 0, width.max(1), height.max(1))
}

// --------------------------------------------------------------- WndProc

/// Was der `WndProc` dem Owner-Thread hinterlässt. Gezeichnet wird **nicht**
/// in der Nachrichtenbehandlung — der nächste `frame()` holt sich das hier ab.
#[derive(Debug, Default)]
struct PendingLayout {
    /// `WM_DPICHANGED`: neue DPI und der von Windows vorgeschlagene Rect.
    dpi_changed: Option<(u32, Rect)>,
    /// `WM_DISPLAYCHANGE`: Monitore, Auflösung oder Arbeitsfläche geändert.
    display_changed: bool,
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        // SAFETY: Für `WM_NCCREATE` garantiert Windows eine gültige
        // `CREATESTRUCTW`; `lpCreateParams` ist der Zeiger aus `CreateWindowExW`.
        let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
        // SAFETY: `hwnd` ist gültig, `GWLP_USERDATA` gehört der Anwendung.
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize) };
        // SAFETY: unveränderte Parameter an die Default-Behandlung.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }

    // §4.2: Klicks gehen durch die Karte hindurch. Das steht hier **zusätzlich**
    // zu `WS_EX_TRANSPARENT` — der Ex-Stil ist primär ein Paint-Ordering-Stil
    // und kein expliziter Hit-Test-Vertrag (Sol Major 8). Der Test braucht
    // keinen Fensterzustand, deshalb vor dem `GWLP_USERDATA`-Zugriff.
    if msg == WM_NCHITTEST {
        return HTTRANSPARENT as LRESULT;
    }

    // SAFETY: `hwnd` ist gültig; der Wert ist entweder 0 (vor `WM_NCCREATE`,
    // nach `WM_NCDESTROY`) oder der oben gesetzte Zeiger.
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<PendingLayout>;
    if raw.is_null() {
        // SAFETY: unveränderte Parameter an die Default-Behandlung.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    // SAFETY: Der Zeiger stammt aus der `Box` in `OverlayWindow`, die länger
    // lebt als das Fenster (`Drop` zerstört erst das Fenster). Der `WndProc`
    // läuft nur auf dem Thread, dem beides gehört.
    let cell = unsafe { &*raw };

    match msg {
        // Skalierung geändert (oder das Fenster auf einen anders skalierten
        // Monitor gewandert): Layout und DIB neu aufbauen (Sol Major 9).
        WM_DPICHANGED => {
            let dpi = (wparam as u32) & 0xFFFF;
            // SAFETY: Für `WM_DPICHANGED` ist `lParam` ein gültiger
            // `RECT`-Zeiger (dokumentiert), der nur gelesen wird.
            let suggested = rect_from(unsafe { &*(lparam as *const RECT) });
            if let Ok(mut pending) = cell.try_borrow_mut() {
                pending.dpi_changed = Some((dpi.max(1), suggested));
            }
            return 0;
        }
        // Monitor abgezogen oder Auflösung geändert.
        WM_DISPLAYCHANGE => {
            if let Ok(mut pending) = cell.try_borrow_mut() {
                pending.display_changed = true;
            }
            return 0;
        }
        // Taskleiste verschoben, ein- oder ausgeblendet: Das meldet Windows
        // **nicht** als `WM_DISPLAYCHANGE`, sondern als Änderung der
        // Arbeitsfläche (Sol-Impl-Review Minor 6). Ohne diesen Zweig läge die
        // Karte bis zum nächsten Einblenden auf der alten Work-Area.
        WM_SETTINGCHANGE => {
            if wparam as u32 == SPI_SETWORKAREA
                && let Ok(mut pending) = cell.try_borrow_mut()
            {
                pending.display_changed = true;
            }
            return 0;
        }
        WM_NCDESTROY => {
            // Ab hier darf niemand mehr über das Fenster an den Zustand.
            // SAFETY: `hwnd` ist noch gültig, letzte Nachricht des Fensters.
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
        }
        _ => {}
    }
    // SAFETY: unveränderte Parameter an die Default-Behandlung.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

// ------------------------------------------------------------- Zeichenfläche

/// Top-down 32-bpp-DIB plus Memory-DC, **wiederverwendet über Frames**. Ein
/// Neuaufbau pro Frame wäre unnötig teuer und fehleranfällig (Sol Major 8);
/// neu gebaut wird nur bei Größenänderung (DPI-/Monitorwechsel).
struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    /// Das GDI-Objekt, das vor unserem Bitmap im DC steckte — es muss vor dem
    /// `DeleteObject` zurückselektiert werden.
    previous: HGDIOBJ,
    bits: *mut u8,
    width: i32,
    height: i32,
}

impl Surface {
    fn new(width: i32, height: i32) -> Result<Self, OverlayError> {
        if width <= 0 || height <= 0 {
            return Err(failed(format!("ungültige Overlay-Größe {width}×{height}")));
        }
        // SAFETY: `BITMAPINFO` ist ein reiner POD-Header ohne Zeiger.
        let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // Negativ = top-down: Zeile 0 ist die oberste (wie beim Tray-Icon).
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };

        let mut bits: *mut c_void = ptr::null_mut();
        // SAFETY: `bmi` lebt über den Aufruf; `hdc`/`hSection` dürfen NULL sein
        // (dokumentierte Form „DIB im Prozessspeicher"). `bits` wird gesetzt
        // und gehört danach dem zurückgegebenen `HBITMAP`.
        let bitmap: HBITMAP = unsafe {
            CreateDIBSection(
                ptr::null_mut(),
                &bmi,
                DIB_RGB_COLORS,
                &mut bits,
                ptr::null_mut(),
                0,
            )
        };
        if bitmap.is_null() || bits.is_null() {
            let err = last_error();
            if !bitmap.is_null() {
                // Inkonsistentes Ergebnis (Handle ja, Speicher nein): Das
                // Handle darf trotzdem nicht liegen bleiben (Sol-Impl-Review
                // Minor 5).
                // SAFETY: eigenes, noch nirgends selektiertes GDI-Objekt.
                unsafe { DeleteObject(bitmap) };
            }
            return Err(failed(format!(
                "Overlay-DIB {width}×{height} nicht erzeugbar: Win32-Fehler {err}"
            )));
        }

        // SAFETY: `NULL` heißt „kompatibel zum Bildschirm".
        let dc = unsafe { CreateCompatibleDC(ptr::null_mut()) };
        if dc.is_null() {
            let err = last_error();
            // SAFETY: eigenes, noch nirgends selektiertes GDI-Objekt.
            unsafe { DeleteObject(bitmap) };
            return Err(failed(format!(
                "Overlay-DC nicht erzeugbar: Win32-Fehler {err}"
            )));
        }
        // SAFETY: eigener DC, eigenes Bitmap; der Rückgabewert ist das vorher
        // selektierte Objekt und wird für den Abbau aufgehoben.
        let previous = unsafe { SelectObject(dc, bitmap) };
        // Ohne diese Prüfung stünde im DC weiter das Default-Bitmap (
        // `UpdateLayeredWindow` zeigte dann nicht unser DIB), und der `Drop`
        // würde einen ungültigen Vorgänger zurückselektieren.
        if previous.is_null() || previous as isize == HGDI_ERROR {
            let err = last_error();
            // Abbau in umgekehrter Aufbaufolge.
            // SAFETY: eigener DC und eigenes Bitmap; im DC steckt nach dem
            // gescheiterten `SelectObject` noch das Default-Bitmap.
            unsafe {
                DeleteDC(dc);
                DeleteObject(bitmap);
            }
            return Err(failed(format!(
                "Overlay-DIB nicht in den DC selektierbar: Win32-Fehler {err}"
            )));
        }

        Ok(Self {
            dc,
            bitmap,
            previous,
            bits: bits as *mut u8,
            width,
            height,
        })
    }

    fn len(&self) -> usize {
        (self.width as usize) * (self.height as usize) * 4
    }

    fn pixels(&mut self) -> &mut [u8] {
        // SAFETY: `CreateDIBSection` hat genau `width*height` 32-Bit-Pixel
        // alloziert (bei 32 bpp sind die Zeilen von Haus aus 4-Byte-
        // ausgerichtet), das Bitmap lebt so lange wie diese `Surface`, und
        // niemand sonst hält den Zeiger.
        unsafe { std::slice::from_raw_parts_mut(self.bits, self.len()) }
    }
}

impl Drop for Surface {
    /// Läuft auf dem Owner-Thread: erst das alte Objekt zurückselektieren,
    /// dann Bitmap und DC freigeben — in genau dieser Reihenfolge.
    fn drop(&mut self) {
        // SAFETY: eigener DC dieses Threads; danach hält er unser Bitmap nicht
        // mehr, es darf gelöscht werden.
        let (restored, deleted, released) = unsafe {
            let restored = SelectObject(self.dc, self.previous);
            let deleted = DeleteObject(self.bitmap);
            let released = DeleteDC(self.dc);
            (restored, deleted, released)
        };
        // Debug-Nachweis: Scheitert hier etwas, leckt GDI-Speicher — im
        // Release bleibt es folgenlos still (Sol-Impl-Review Minor 5).
        debug_assert!(
            !restored.is_null() && restored as isize != HGDI_ERROR,
            "Overlay: altes GDI-Objekt nicht zurückselektiert"
        );
        debug_assert!(deleted != 0, "Overlay: DIB nicht freigegeben");
        debug_assert!(released != 0, "Overlay: Memory-DC nicht freigegeben");
    }
}

// ------------------------------------------------------------------ Fenster

pub struct OverlayWindow {
    hwnd: HWND,
    instance: HINSTANCE,
    class_name: Vec<u16>,
    /// Nur eine selbst registrierte Klasse wird im `Drop` abgemeldet.
    owns_class: bool,
    /// Boxed, damit die Adresse stabil bleibt — der `WndProc` kennt sie über
    /// `GWLP_USERDATA`.
    pending: Box<RefCell<PendingLayout>>,
    surface: Option<Surface>,
    /// Kartenrechteck in Bildschirmkoordinaten.
    rect: Rect,
    dpi: u32,
    visible: bool,
    render: OverlayState,
    last_frame: Instant,
}

impl OverlayWindow {
    /// Baut Klasse und (verstecktes) Fenster auf dem aufrufenden Thread. Der
    /// DPI-Kontext wird dabei als Allererstes gesetzt; scheitert das, gibt es
    /// kein Overlay (Leitentscheidung 6 verlangt einen **geprüften**
    /// PMv2-Bootstrap — ohne ihn wären alle Koordinaten virtualisiert).
    pub fn new() -> Result<Self, OverlayError> {
        if !set_thread_dpi_awareness() {
            return Err(failed(format!(
                "Per-Monitor-DPI (PMv2) nicht setzbar: Win32-Fehler {}",
                last_error()
            )));
        }

        // SAFETY: `GetModuleHandleW(NULL)` liefert das eigene Modul-Handle und
        // überträgt kein Eigentum.
        let instance = unsafe { GetModuleHandleW(ptr::null()) };
        let class_name = wide(CLASS_NAME);
        let window_name = wide("diktier overlay");

        let class = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: ptr::null_mut(),
            hCursor: ptr::null_mut(),
            // Kein Hintergrundpinsel: gemalt wird ausschließlich über
            // `UpdateLayeredWindow`, es gibt keinen `WM_PAINT`-Pfad.
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };
        // SAFETY: `class` ist vollständig initialisiert, `lpszClassName` zeigt
        // in `class_name`, das noch lebt; `wnd_proc` hat die von `WNDPROC`
        // geforderte Signatur.
        let atom = unsafe { RegisterClassW(&class) };
        let owns_class = if atom == 0 {
            let err = last_error();
            if err != ERROR_CLASS_ALREADY_EXISTS {
                return Err(failed(format!(
                    "Fensterklasse {CLASS_NAME} nicht registrierbar: Win32-Fehler {err}"
                )));
            }
            false
        } else {
            true
        };

        let pending = Box::new(RefCell::new(PendingLayout::default()));
        let pending_ptr: *const RefCell<PendingLayout> = &*pending;

        // §4.2: `WS_EX_NOACTIVATE` (nie aktivieren), `WS_EX_TRANSPARENT`
        // (durchklickbar), `WS_EX_TOOLWINDOW` (kein Taskleisteneintrag,
        // kein Alt-Tab), `WS_EX_TOPMOST` (über dem Zielfenster),
        // `WS_EX_LAYERED` (Voraussetzung für `UpdateLayeredWindow`).
        // Ohne `WS_VISIBLE`: gezeigt wird erst nach dem ersten erfolgreichen
        // `UpdateLayeredWindow`.
        // SAFETY: Alle Zeiger zeigen auf lebende, NUL-terminierte Puffer;
        // `pending_ptr` erreicht den `WndProc` als `lpCreateParams` in
        // `WM_NCCREATE`, und die Box lebt länger als das Fenster (siehe `Drop`).
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE
                    | WS_EX_TRANSPARENT,
                class_name.as_ptr(),
                window_name.as_ptr(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                pending_ptr as *const c_void,
            )
        };
        if hwnd.is_null() {
            let err = last_error();
            if owns_class {
                // SAFETY: eigene Klasse, es existiert kein Fenster dazu.
                unsafe { UnregisterClassW(class_name.as_ptr(), instance) };
            }
            return Err(failed(format!(
                "Overlay-Fenster nicht erzeugbar: Win32-Fehler {err}"
            )));
        }

        Ok(Self {
            hwnd,
            instance,
            class_name,
            owns_class,
            pending,
            surface: None,
            rect: Rect::new(0, 0, 0, 0),
            dpi: DEFAULT_DPI,
            visible: false,
            render: OverlayState::new(),
            last_frame: Instant::now(),
        })
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Kartenlage und Skalierung — fürs Log und für den Spike.
    pub fn describe(&self) -> String {
        format!(
            "{}×{} @ {}/{} · {} dpi",
            self.rect.width(),
            self.rect.height(),
            self.rect.left,
            self.rect.top,
            self.dpi
        )
    }

    /// Karte auf dem Monitor des fokussierten Fensters einblenden.
    ///
    /// Reihenfolge nach Leitentscheidung 4 und Sol-Impl-Review Blocker 1:
    /// Zielmonitor bestimmen, das noch **versteckte** Fenster dorthin
    /// schieben, dessen DPI messen, damit Layout und DIB rechnen, den ersten
    /// Frame präsentieren — und erst danach `SW_SHOWNOACTIVATE`. Sonst blitzte
    /// ein leeres oder falsch skaliertes Fenster auf.
    pub fn show(&mut self) -> Result<(), OverlayError> {
        if self.visible {
            return Ok(());
        }
        let work = monitor_work_area(foreground_monitor()).unwrap_or_else(primary_screen_fallback);
        let dpi = self.probe_dpi_in(work)?;
        self.render.clear();
        self.apply_layout(card_rect(work, dpi), dpi)?;
        self.last_frame = Instant::now();
        self.render.push(0.0, Duration::ZERO);
        self.present()?;
        // SAFETY: eigenes Fenster dieses Threads. `SW_SHOWNOACTIVATE` — das
        // Fenster nimmt keinen Fokus (§4.2).
        unsafe { ShowWindow(self.hwnd, SW_SHOWNOACTIVATE) };
        self.visible = true;
        Ok(())
    }

    /// Karte ausblenden. Die Historie leert sich dabei: das nächste Diktat
    /// fängt mit einer leeren Karte an.
    pub fn hide(&mut self) {
        if !self.visible {
            return;
        }
        // SAFETY: eigenes Fenster dieses Threads.
        unsafe { ShowWindow(self.hwnd, SW_HIDE) };
        self.visible = false;
        self.render.clear();
    }

    /// Ein Frame: Pegel einhängen, Karte neu zeichnen, anzeigen. Unsichtbar
    /// passiert nichts.
    pub fn frame(&mut self, level: f32) -> Result<(), OverlayError> {
        if !self.visible {
            return Ok(());
        }
        self.apply_pending_layout()?;
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.last_frame);
        self.last_frame = now;
        self.render.push(level, elapsed);
        self.present()
    }

    /// `PeekMessageW`-Pump. Nicht blockierend — der Worker darf nicht in
    /// `GetMessageW` hängen, sonst käme kein `Hide` mehr an.
    pub fn pump(&mut self) {
        // SAFETY: `MSG` ist POD; `PeekMessageW` füllt die Struktur.
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        for _ in 0..MAX_MESSAGES_PER_PUMP {
            // SAFETY: `msg` ist gültiger Speicher; `hWnd = NULL` holt die
            // Nachrichten **dieses** Threads — dort gehört nur unser Fenster uns.
            if unsafe { PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) } == 0 {
                break;
            }
            // Kein `TranslateMessage`: das Overlay verarbeitet keine Eingabe.
            // SAFETY: `msg` stammt unverändert aus `PeekMessageW`.
            unsafe { DispatchMessageW(&msg) };
        }
    }

    /// DPI-Bootstrap (Sol-Impl-Review Blocker 1): Das noch versteckte eigene
    /// Fenster als 1×1-Rechteck in die Arbeitsfläche des Zielmonitors
    /// schieben und dort `GetDpiForWindow` fragen.
    ///
    /// Warum über das eigene Fenster: Nur dessen Rückgabe hängt an **unserer**
    /// per-Thread-PMv2-Awareness. `GetDpiForMonitor` richtet sich nach der
    /// prozessweiten Awareness (hier unaware → immer 96), und ein fremdes
    /// Vordergrundfenster liefert seine eigene.
    ///
    /// `SWP_NOACTIVATE` ist Pflicht (§4.2), `SWP_NOZORDER` lässt das
    /// Topmost-Band unberührt.
    fn probe_dpi_in(&self, work: Rect) -> Result<u32, OverlayError> {
        let x = work.left + work.width().max(1) / 2;
        let y = work.top + work.height().max(1) / 2;
        // SAFETY: eigenes Fenster dieses Threads; `NULL` als
        // `hWndInsertAfter` ist mit `SWP_NOZORDER` bedeutungslos.
        let moved = unsafe {
            SetWindowPos(
                self.hwnd,
                ptr::null_mut(),
                x,
                y,
                PROBE_SIZE,
                PROBE_SIZE,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )
        };
        if moved == 0 {
            return Err(failed(format!(
                "Overlay nicht auf den Zielmonitor setzbar: Win32-Fehler {}",
                last_error()
            )));
        }
        // SAFETY: eigenes Fenster dieses Threads; 0 heißt „ungültiges Fenster".
        let dpi = unsafe { GetDpiForWindow(self.hwnd) };
        if dpi == 0 {
            return Err(failed(format!(
                "Monitor-DPI nicht lesbar: Win32-Fehler {}",
                last_error()
            )));
        }
        Ok(dpi)
    }

    /// Was der `WndProc` hinterlassen hat, in Layout umsetzen (Sol Major 9).
    fn apply_pending_layout(&mut self) -> Result<(), OverlayError> {
        let (dpi_changed, display_changed) = match self.pending.try_borrow_mut() {
            Ok(mut pending) => (
                pending.dpi_changed.take(),
                std::mem::take(&mut pending.display_changed),
            ),
            Err(_) => return Ok(()),
        };

        if let Some((dpi, suggested)) = dpi_changed {
            // Der Vorschlag von Windows skaliert nur den **alten** Rect. Für
            // die Karte gilt aber weiter „unten mittig in der Arbeitsfläche",
            // und die ändert sich mit der Skalierung mit. Deshalb bestimmt der
            // Vorschlag den Monitor, die Geometrie kommt wie beim Einblenden
            // aus `card_rect`.
            let work = monitor_work_area(monitor_from_rect(suggested))
                .unwrap_or_else(primary_screen_fallback);
            return self.apply_layout(card_rect(work, dpi), dpi);
        }
        if display_changed {
            // SAFETY: eigenes Fenster; `MONITOR_DEFAULTTONEAREST` klemmt auf
            // den nächstgelegenen aktiven Monitor, falls der alte weg ist.
            let monitor = unsafe { MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST) };
            let work = monitor_work_area(monitor).unwrap_or_else(primary_screen_fallback);
            // Das Fenster steht schon auf diesem Monitor und ist per-Monitor-
            // aware — hier genügt also `GetDpiForWindow` ohne Verschieben.
            // SAFETY: eigenes Fenster dieses Threads.
            let dpi = match unsafe { GetDpiForWindow(self.hwnd) } {
                0 => self.dpi,
                dpi => dpi,
            };
            return self.apply_layout(card_rect(work, dpi), dpi);
        }
        Ok(())
    }

    /// Kartenrechteck übernehmen und, wenn sich die Größe geändert hat, DIB
    /// und Memory-DC neu aufbauen.
    fn apply_layout(&mut self, rect: Rect, dpi: u32) -> Result<(), OverlayError> {
        let (width, height) = (rect.width().max(1), rect.height().max(1));
        let needs_surface = match &self.surface {
            Some(surface) => surface.width != width || surface.height != height,
            None => true,
        };
        if needs_surface {
            // Erst das alte Paar abbauen, dann das neue anlegen — beides auf
            // dem Owner-Thread.
            self.surface = None;
            self.surface = Some(Surface::new(width, height)?);
        }
        self.rect = rect;
        self.dpi = dpi;
        self.render
            .set_capacity(history_capacity(width, height, dpi));
        Ok(())
    }

    /// Karte zeichnen und per `UpdateLayeredWindow` anzeigen — der einzige
    /// Anzeigeweg, es gibt keinen `WM_PAINT`-Pfad.
    fn present(&mut self) -> Result<(), OverlayError> {
        let Self {
            hwnd,
            surface,
            rect,
            dpi,
            render,
            ..
        } = self;
        let Some(surface) = surface.as_mut() else {
            return Err(failed("Overlay-DIB fehlt"));
        };
        let (width, height) = (surface.width, surface.height);
        {
            let pixels = surface.pixels();
            let mut canvas = Canvas::new(pixels, width, height)
                .ok_or_else(|| failed("Overlay-Puffer zu klein"))?;
            draw_card(&mut canvas, *dpi, render);
        }

        let position = POINT {
            x: rect.left,
            y: rect.top,
        };
        let size = SIZE {
            cx: width,
            cy: height,
        };
        let source = POINT { x: 0, y: 0 };
        // Premultipliziertes Alpha aus dem DIB, keine zusätzliche
        // Gesamttransparenz (Leitentscheidung 4).
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        // SAFETY: eigenes Fenster und eigener DC dieses Threads; alle Zeiger
        // zeigen auf lebende lokale Strukturen. `hdcDst = NULL` heißt „gegen
        // den Bildschirm" (dokumentiert). Der Aufruf verschiebt und
        // dimensioniert das Fenster gleich mit — ohne es zu aktivieren.
        let ok = unsafe {
            UpdateLayeredWindow(
                *hwnd,
                ptr::null_mut(),
                &position,
                &size,
                surface.dc,
                &source,
                0,
                &blend,
                ULW_ALPHA,
            )
        };
        if ok == 0 {
            return Err(failed(format!(
                "UpdateLayeredWindow fehlgeschlagen: Win32-Fehler {}",
                last_error()
            )));
        }
        Ok(())
    }
}

impl Drop for OverlayWindow {
    /// Läuft auf dem Owner-Thread — der Overlay-Worker legt das Fenster am
    /// Ende seiner eigenen Schleife ab, der Spike beim Verlassen von
    /// `overlay_test`.
    fn drop(&mut self) {
        // GDI zuerst: danach zeigt nichts mehr auf den DIB.
        self.surface = None;
        if !self.hwnd.is_null() {
            // SAFETY: eigenes Fenster dieses Threads; `WM_NCDESTROY` löscht
            // dabei den `GWLP_USERDATA`-Zeiger, die Box lebt bis danach.
            unsafe { DestroyWindow(self.hwnd) };
            self.hwnd = ptr::null_mut();
        }
        if self.owns_class {
            // SAFETY: eigene Klasse, ihr einziges Fenster ist zerstört.
            unsafe { UnregisterClassW(self.class_name.as_ptr(), self.instance) };
            self.owns_class = false;
        }
    }
}
