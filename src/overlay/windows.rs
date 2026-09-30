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
    AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER,
    BLENDFUNCTION, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection, CreateFontW,
    DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX,
    DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, FF_SWISS, FW_NORMAL, FW_SEMIBOLD,
    GdiFlush, GetMonitorInfoW, HBITMAP, HDC, HFONT, HGDIOBJ, HMONITOR, MONITOR_DEFAULTTONEAREST,
    MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint, MonitorFromWindow, OUT_DEFAULT_PRECIS,
    SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetThreadDpiAwarenessContext,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
    GetForegroundWindow, GetSystemMetrics, GetWindowLongPtrW, HTTRANSPARENT, HWND_TOPMOST, MSG,
    PM_REMOVE, PeekMessageW, RegisterClassW, SM_CXSCREEN, SM_CYSCREEN, SPI_SETWORKAREA, SW_HIDE,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SetWindowLongPtrW, SetWindowPos, ShowWindow, ULW_ALPHA,
    UnregisterClassW, UpdateLayeredWindow, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_NCCREATE,
    WM_NCDESTROY, WM_NCHITTEST, WM_SETTINGCHANGE, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use super::{
    Canvas, NOTICE_DETAIL_LEVEL, NOTICE_DETAIL_PX, NOTICE_TITLE_PX, OverlayState, Rect, card_rect,
    draw_card, draw_notice_card, history_capacity, notice_layout, scale,
};

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

// ------------------------------------------------------------ Hinweistext

/// Schrift der Hinweiskarte. Zeile 1 halbfett: GDI führt „Segoe UI
/// Semibold" als eigene Familie, mit `FW_SEMIBOLD` trifft der Mapper genau
/// diese Datei statt einen künstlichen Fettdruck zu rechnen.
const TITLE_FACE: &str = "Segoe UI Semibold";
const DETAIL_FACE: &str = "Segoe UI";

/// `CLR_INVALID` aus `wingdi.h` — Fehlerwert von `SetTextColor`. Dieselbe
/// Begründung wie bei [`HGDI_ERROR`].
const CLR_INVALID: u32 = 0xFFFF_FFFF;

/// Eigenes GDI-Font-Handle, freigegeben im `Drop`.
struct Font(HFONT);

impl Font {
    /// `px` ist die Zeichenhöhe in Geräte-Pixeln (negativer `cHeight`, also
    /// ohne Zeilenabstand) — die DPI-Skalierung ist schon eingerechnet.
    fn new(face: &str, px: i32, weight: u32) -> Result<Self, OverlayError> {
        let face = wide(face);
        // SAFETY: `face` ist ein lebender, NUL-terminierter UTF-16-Puffer; alle
        // anderen Parameter sind Werte. `ANTIALIASED_QUALITY` heißt Graustufen
        // ohne ClearType — die Luminanz wird unten zur Deckungsmaske.
        let font = unsafe {
            CreateFontW(
                -px.max(1),
                0,
                0,
                0,
                weight as i32,
                0,
                0,
                0,
                u32::from(DEFAULT_CHARSET),
                u32::from(OUT_DEFAULT_PRECIS),
                u32::from(CLIP_DEFAULT_PRECIS),
                u32::from(ANTIALIASED_QUALITY),
                u32::from(DEFAULT_PITCH | FF_SWISS),
                face.as_ptr(),
            )
        };
        if font.is_null() {
            return Err(failed(format!(
                "Schrift nicht erzeugbar: Win32-Fehler {}",
                last_error()
            )));
        }
        Ok(Self(font))
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        // SAFETY: eigenes GDI-Objekt; `FontSelection` hat es vorher aus dem
        // DC genommen (sie wird vor den Fonts abgebaut).
        let deleted = unsafe { DeleteObject(self.0) };
        debug_assert!(deleted != 0, "Overlay: Schrift nicht freigegeben");
    }
}

/// Merkt sich den Font, der vor unserem im DC stand, und selektiert ihn im
/// `Drop` zurück — auf **jedem** Pfad, auch nach einem frühen Fehler. Sonst
/// scheiterte das `DeleteObject` der eigenen Schrift.
struct FontSelection {
    dc: HDC,
    previous: Option<HGDIOBJ>,
}

impl FontSelection {
    fn select(&mut self, font: &Font) -> Result<(), OverlayError> {
        // SAFETY: eigener Memory-DC dieses Threads, eigenes Font-Handle.
        let previous = unsafe { SelectObject(self.dc, font.0) };
        if previous.is_null() || previous as isize == HGDI_ERROR {
            return Err(failed(format!(
                "Schrift nicht selektierbar: Win32-Fehler {}",
                last_error()
            )));
        }
        // Nur der **erste** Vorgänger ist der Default-Font des DC.
        self.previous.get_or_insert(previous);
        Ok(())
    }
}

impl Drop for FontSelection {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            // SAFETY: eigener DC; `previous` stammt aus `SelectObject` auf ihm.
            unsafe { SelectObject(self.dc, previous) };
        }
    }
}

/// Eine Zeile per `DrawTextW` in den DC: einzeilig, feste Rechteckbreite,
/// Ellipse am Ende, kein `&`-Präfix.
fn draw_text_line(
    selection: &mut FontSelection,
    font: &Font,
    text: &str,
    rect: Rect,
    gray: u8,
) -> Result<(), OverlayError> {
    if rect.is_empty() {
        return Ok(());
    }
    selection.select(font)?;
    let color = u32::from(gray) * 0x0001_0101;
    // SAFETY: eigener DC dieses Threads, `color` ist ein `COLORREF`-Wert.
    if unsafe { SetTextColor(selection.dc, color) } == CLR_INVALID {
        return Err(failed(format!(
            "Textfarbe nicht setzbar: Win32-Fehler {}",
            last_error()
        )));
    }
    let text: Vec<u16> = text.encode_utf16().collect();
    let mut raw = RECT {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    };
    // SAFETY: `text` lebt über den Aufruf und wird mit Länge übergeben (ohne
    // NUL, kein `DT_MODIFYSTRING` → GDI schreibt nicht hinein); `raw` ist ein
    // gültiger `RECT`.
    let drawn = unsafe {
        DrawTextW(
            selection.dc,
            text.as_ptr(),
            text.len() as i32,
            &mut raw,
            DT_SINGLELINE | DT_END_ELLIPSIS | DT_LEFT | DT_VCENTER | DT_NOPREFIX,
        )
    };
    if drawn == 0 {
        return Err(failed(format!(
            "DrawTextW fehlgeschlagen: Win32-Fehler {}",
            last_error()
        )));
    }
    Ok(())
}

/// Rastert beide Hinweiszeilen per GDI und liefert die Deckungsmaske der
/// Karte: ein Byte je Pixel, `width × height`, top-down.
///
/// Eigenes Top-down-32-bpp-DIB, **schwarz initialisiert**; GDI schreibt Zeile 1
/// weiß (Segoe UI Semibold), Zeile 2 in `NOTICE_DETAIL_LEVEL`-Grau, beide mit
/// `ANTIALIASED_QUALITY` (Graustufen, kein ClearType) und DPI-skalierter
/// Größe. Die Luminanz ist die Deckung. Das Layout-DIB des Fensters wird dabei
/// nicht angefasst, und ein Fenster braucht es nicht — deshalb auch ohne
/// Overlay testbar.
pub fn render_notice_text(
    width: i32,
    height: i32,
    dpi: u32,
    title: &str,
    detail: &str,
) -> Result<Vec<u8>, OverlayError> {
    let mut surface = Surface::new(width, height)?;
    surface.pixels().fill(0);
    let layout = notice_layout(width, height, dpi);
    let title_font = Font::new(TITLE_FACE, scale(NOTICE_TITLE_PX, dpi), FW_SEMIBOLD)?;
    let detail_font = Font::new(DETAIL_FACE, scale(NOTICE_DETAIL_PX, dpi), FW_NORMAL)?;
    {
        // Nach den Fonts angelegt → vor ihnen abgebaut (Drop in umgekehrter
        // Reihenfolge): erst zurückselektieren, dann löschen.
        let mut selection = FontSelection {
            dc: surface.dc,
            previous: None,
        };
        // SAFETY: eigener DC; `TRANSPARENT` lässt den schwarzen Grund stehen.
        if unsafe { SetBkMode(selection.dc, TRANSPARENT as i32) } == 0 {
            return Err(failed(format!(
                "Hintergrundmodus nicht setzbar: Win32-Fehler {}",
                last_error()
            )));
        }
        draw_text_line(&mut selection, &title_font, title, layout.title, 255)?;
        draw_text_line(
            &mut selection,
            &detail_font,
            detail,
            layout.detail,
            NOTICE_DETAIL_LEVEL,
        )?;
    }
    // GDI puffert Zeichenaufrufe; vor dem Lesen der Bits muss alles im DIB
    // stehen. Scheitert der Flush, ist der Text womöglich unvollständig — dann
    // lieber der Glyphen-Fallback mit Warnung (Final-Review).
    // SAFETY: parameterlos.
    if unsafe { GdiFlush() } == 0 {
        return Err(failed(format!(
            "GdiFlush fehlgeschlagen: Win32-Fehler {}",
            last_error()
        )));
    }

    let pixels = surface.pixels();
    let mask = pixels
        .chunks_exact(4)
        .map(|bgra| {
            let (b, g, r) = (u32::from(bgra[0]), u32::from(bgra[1]), u32::from(bgra[2]));
            // Luminanz (Rec. 601, ganzzahlig); bei Graustufen ist sie der Grauwert.
            ((r * 77 + g * 150 + b * 29) >> 8) as u8
        })
        .collect();
    Ok(mask)
}

// ------------------------------------------------------------------ Fenster

/// Was die Karte gerade zeigt. Der Wechsel tauscht nur den Inhalt — das
/// Fenster bleibt dabei sichtbar und wird nie aktiviert (§4.5).
enum Content {
    /// Mikrofonpegel, jeder Frame neu gezeichnet.
    Level,
    /// Hinweis. `mask` ist die GDI-Textmaske zur aktuellen Größe; `None` =
    /// Textaufbau gescheitert → nur die Warn-Glyphe (§4.5 Fallback).
    Notice {
        title: &'static str,
        detail: &'static str,
        mask: Option<Vec<u8>>,
    },
}

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
    content: Content,
    /// Grund, wenn der Hinweistext nicht aufgebaut werden konnte — der Worker
    /// holt ihn ab und loggt ihn ([`OverlayWindow::take_text_warning`]).
    text_warning: Option<String>,
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
            content: Content::Level,
            text_warning: None,
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

    /// Pegelkarte zeigen.
    ///
    /// Unsichtbar: auf dem Monitor des fokussierten Fensters einblenden
    /// ([`Self::appear`]). Steht gerade der Hinweis, wird nur der Inhalt
    /// getauscht — kein Ausblenden, kein Umpositionieren, keine Aktivierung
    /// (§4.5); die Waveform fängt leer an.
    pub fn show_level(&mut self) -> Result<(), OverlayError> {
        let was_notice = matches!(self.content, Content::Notice { .. });
        if self.visible && !was_notice {
            return Ok(());
        }
        self.content = Content::Level;
        self.render.clear();
        self.last_frame = Instant::now();
        self.render.push(0.0, Duration::ZERO);
        if self.visible {
            return self.present();
        }
        self.appear()
    }

    /// Hinweiskarte zeigen (§4.5). Aus der Pegelkarte heraus tauscht das nur
    /// den Inhalt; ist die Karte nicht sichtbar, blendet sie wie beim Pegel
    /// auf dem Monitor des Vordergrundfensters ein.
    ///
    /// Scheitert der Text, ist das **kein** Fehler: Die Karte steht mit der
    /// Warn-Glyphe allein, den Grund liefert [`Self::take_text_warning`].
    pub fn show_notice(
        &mut self,
        title: &'static str,
        detail: &'static str,
    ) -> Result<(), OverlayError> {
        self.content = Content::Notice {
            title,
            detail,
            mask: None,
        };
        self.render.clear();
        if self.visible {
            self.rebuild_notice_text();
            return self.present();
        }
        self.appear()
    }

    /// Einblenden. Reihenfolge nach Leitentscheidung 4 und Sol-Impl-Review
    /// Blocker 1: Zielmonitor bestimmen, das noch **versteckte** Fenster
    /// dorthin schieben, dessen DPI messen, damit Layout und DIB rechnen, den
    /// ersten Frame präsentieren — und erst danach `SW_SHOWNOACTIVATE`. Sonst
    /// blitzte ein leeres oder falsch skaliertes Fenster auf.
    fn appear(&mut self) -> Result<(), OverlayError> {
        let work = monitor_work_area(foreground_monitor()).unwrap_or_else(primary_screen_fallback);
        let dpi = self.probe_dpi_in(work)?;
        self.apply_layout(card_rect(work, dpi), dpi)?;
        self.rebuild_notice_text();
        self.present()?;
        // SAFETY: eigenes Fenster dieses Threads. `SW_SHOWNOACTIVATE` — das
        // Fenster nimmt keinen Fokus (§4.2).
        unsafe { ShowWindow(self.hwnd, SW_SHOWNOACTIVATE) };
        self.visible = true;
        Ok(())
    }

    /// Den letzten Grund abholen, aus dem der Hinweistext nicht aufgebaut
    /// werden konnte (einmalig je Fehlschlag).
    pub fn take_text_warning(&mut self) -> Option<String> {
        self.text_warning.take()
    }

    /// Textmaske zur aktuellen Größe und DPI neu rastern — nur im
    /// Hinweis-Modus. Ein Fehler lässt `mask = None` (nur Glyphe) und merkt
    /// sich den Grund.
    fn rebuild_notice_text(&mut self) {
        let (width, height, dpi) = match &self.surface {
            Some(surface) => (surface.width, surface.height, self.dpi),
            None => return,
        };
        if let Content::Notice {
            title,
            detail,
            mask,
        } = &mut self.content
        {
            *mask = match render_notice_text(width, height, dpi, title, detail) {
                Ok(text) => Some(text),
                Err(err) => {
                    self.text_warning = Some(err.to_string());
                    None
                }
            };
        }
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
    /// passiert nichts. Der Hinweis ist statisch — er wird nur nach einer
    /// Layoutänderung (DPI, Monitor, Arbeitsfläche) neu gerastert und gezeigt.
    pub fn frame(&mut self, level: f32) -> Result<(), OverlayError> {
        if !self.visible {
            return Ok(());
        }
        let relayout = self.apply_pending_layout()?;
        if matches!(self.content, Content::Notice { .. }) {
            if relayout {
                self.rebuild_notice_text();
                return self.present();
            }
            return Ok(());
        }
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
    /// `SWP_NOACTIVATE` ist Pflicht (§4.2). `HWND_TOPMOST` behauptet bei
    /// jedem Einblenden das Topmost-Band neu: Windows kann ein Fenster aus
    /// dem Band nehmen und dabei `WS_EX_TOPMOST` stehen lassen (beobachtet
    /// 2026-09-07 nach Stunden Laufzeit — Karte lag unter dem maximierten
    /// Zielfenster, Log meldete „sichtbar"). Der Ex-Stil aus
    /// `CreateWindowExW` allein reicht dagegen nicht.
    fn probe_dpi_in(&self, work: Rect) -> Result<u32, OverlayError> {
        let x = work.left + work.width().max(1) / 2;
        let y = work.top + work.height().max(1) / 2;
        // SAFETY: eigenes Fenster dieses Threads; `HWND_TOPMOST` ist ein
        // dokumentierter Pseudo-Handle für `hWndInsertAfter`.
        let moved = unsafe {
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                x,
                y,
                PROBE_SIZE,
                PROBE_SIZE,
                SWP_NOACTIVATE,
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
    /// `true`, wenn sich das Layout geändert haben kann.
    fn apply_pending_layout(&mut self) -> Result<bool, OverlayError> {
        let (dpi_changed, display_changed) = match self.pending.try_borrow_mut() {
            Ok(mut pending) => (
                pending.dpi_changed.take(),
                std::mem::take(&mut pending.display_changed),
            ),
            Err(_) => return Ok(false),
        };

        if let Some((dpi, suggested)) = dpi_changed {
            // Der Vorschlag von Windows skaliert nur den **alten** Rect. Für
            // die Karte gilt aber weiter „unten mittig in der Arbeitsfläche",
            // und die ändert sich mit der Skalierung mit. Deshalb bestimmt der
            // Vorschlag den Monitor, die Geometrie kommt wie beim Einblenden
            // aus `card_rect`.
            let work = monitor_work_area(monitor_from_rect(suggested))
                .unwrap_or_else(primary_screen_fallback);
            self.apply_layout(card_rect(work, dpi), dpi)?;
            return Ok(true);
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
            self.apply_layout(card_rect(work, dpi), dpi)?;
            return Ok(true);
        }
        Ok(false)
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
            content,
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
            match content {
                Content::Level => draw_card(&mut canvas, *dpi, render),
                Content::Notice { mask, .. } => {
                    draw_notice_card(&mut canvas, *dpi, mask.as_deref());
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::{NOTICE_DETAIL_LEVEL, card_rect};
    use crate::state::Notice;

    /// Karte in Kartengröße der Arbeitsfläche 1920×1040 bei `dpi`.
    fn card_size(dpi: u32) -> (i32, i32) {
        let card = card_rect(Rect::new(0, 0, 1920, 1040), dpi);
        (card.width(), card.height())
    }

    /// Die GDI-Maske (ohne Fenster): Text steht nur in den beiden
    /// Zeilenrechtecken, Zeile 1 erreicht volle Deckung, Zeile 2 höchstens
    /// ihr Grau. Außerhalb bleibt alles schwarz (schwarz initialisiert).
    #[test]
    fn notice_text_lands_only_inside_its_two_lines() {
        for dpi in [96, 144, 192] {
            let (w, h) = card_size(dpi);
            let notice = Notice::PartialRestore;
            let mask = render_notice_text(w, h, dpi, notice.title(), notice.detail())
                .expect("GDI-Text rastert");
            assert_eq!(mask.len(), (w * h) as usize);
            let layout = notice_layout(w, h, dpi);
            let inside = |rect: Rect, x: i32, y: i32| {
                x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom
            };
            let (mut title_max, mut detail_max, mut inked) = (0u8, 0u8, 0usize);
            for y in 0..h {
                for x in 0..w {
                    let value = mask[(y * w + x) as usize];
                    if inside(layout.title, x, y) {
                        title_max = title_max.max(value);
                    } else if inside(layout.detail, x, y) {
                        detail_max = detail_max.max(value);
                    } else {
                        assert_eq!(value, 0, "dpi {dpi}: Text außerhalb bei {x}/{y}");
                    }
                    if value > 0 {
                        inked += 1;
                    }
                }
            }
            assert!(
                title_max >= 250,
                "dpi {dpi}: Zeile 1 zu blass ({title_max})"
            );
            assert!(detail_max > 0, "dpi {dpi}: Zeile 2 fehlt");
            assert!(
                detail_max <= NOTICE_DETAIL_LEVEL + 1,
                "dpi {dpi}: Zeile 2 heller als ihr Grau ({detail_max})"
            );
            // Graustufen-Kantenglättung: es gibt Zwischenwerte, nicht nur 0/255.
            assert!(
                mask.iter().any(|&v| v > 0 && v < 200),
                "dpi {dpi}: keine geglätteten Kanten"
            );
            assert!(inked > 100, "dpi {dpi}: kaum Text ({inked} Pixel)");
        }
    }

    /// Ein überlanger Text wird am Zeilenende gekürzt (`DT_END_ELLIPSIS`),
    /// nie über das Rechteck hinaus gezeichnet und nie umgebrochen.
    #[test]
    fn overlong_notice_text_is_cut_inside_the_line() {
        let (w, h) = card_size(96);
        let long = "Zwischenablage teilweise wiederhergestellt und noch sehr viel mehr Text, \
                    der niemals in eine Zeile passt";
        let mask = render_notice_text(w, h, 96, long, long).expect("GDI-Text rastert");
        let layout = notice_layout(w, h, 96);
        for y in 0..h {
            for x in layout.title.right..w {
                assert_eq!(mask[(y * w + x) as usize], 0, "über den Rand bei {x}/{y}");
            }
        }
    }

    /// Hilfstest für die Sichtprüfung ohne Fenster (clipboard-restore-plan
    /// WP3): legt alle Hinweiskarten plus den Glyphen-Fallback bei 96/144/192
    /// dpi als PNG unter `target/overlay-notice/` ab, über einem mittelgrauen
    /// Grund komponiert. Fasst weder Fenster noch Fokus noch Zwischenablage an.
    ///
    /// `cargo test notice_card_png_snapshots -- --ignored`
    #[test]
    #[ignore = "Hilfstest: schreibt PNGs nach target/overlay-notice/"]
    fn notice_card_png_snapshots() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("overlay-notice");
        std::fs::create_dir_all(&dir).expect("Ausgabeordner");
        let background = [128u32, 128, 128];
        for dpi in [96u32, 144, 192] {
            let (w, h) = card_size(dpi);
            let gap = scale(8, dpi);
            let rows = Notice::ALL.len() as i32 + 1;
            let (out_w, out_h) = (w + 2 * gap, rows * (h + gap) + gap);
            let mut rgb = vec![0u8; (out_w * out_h * 3) as usize];
            for pixel in rgb.chunks_exact_mut(3) {
                pixel.copy_from_slice(&[128, 128, 128]);
            }
            for row in 0..rows {
                let mut buffer = vec![0u8; (w * h * 4) as usize];
                let mut canvas = Canvas::new(&mut buffer, w, h).expect("Puffer passt");
                match Notice::ALL.get(row as usize) {
                    Some(notice) => {
                        let mask = render_notice_text(w, h, dpi, notice.title(), notice.detail())
                            .expect("GDI-Text rastert");
                        draw_notice_card(&mut canvas, dpi, Some(&mask));
                    }
                    // Letzte Zeile: Fallback „nur Glyphe".
                    None => draw_notice_card(&mut canvas, dpi, None),
                }
                let top = gap + row * (h + gap);
                for y in 0..h {
                    for x in 0..w {
                        let src = ((y * w + x) * 4) as usize;
                        let alpha = u32::from(buffer[src + 3]);
                        let dst = (((top + y) * out_w + gap + x) * 3) as usize;
                        // Premultipliziert: out = src + bg × (1 − a).
                        for (channel, offset) in [(0usize, 2usize), (1, 1), (2, 0)] {
                            let value = u32::from(buffer[src + offset])
                                + background[channel] * (255 - alpha) / 255;
                            rgb[dst + channel] = value.min(255) as u8;
                        }
                    }
                }
            }
            let path = dir.join(format!("notice-{dpi}dpi.png"));
            let file = std::fs::File::create(&path).expect("PNG anlegen");
            let mut encoder =
                png::Encoder::new(std::io::BufWriter::new(file), out_w as u32, out_h as u32);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("PNG-Kopf");
            writer.write_image_data(&rgb).expect("PNG-Daten");
            eprintln!("{}", path.display());
        }
    }
}
