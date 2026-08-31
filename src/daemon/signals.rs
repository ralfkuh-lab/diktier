//! Ctrl+C / Konsolenfenster-Schließen auf den regulären Quit-Pfad (§5.2).
//!
//! Ohne das endet ein `Ctrl+C` mitten im Prozess: Tray-Icon und Overlay blieben
//! stehen, und ein offenes Delayed-Rendering-Versprechen am Clipboard stürbe mit
//! dem Prozess, statt vorher eingelöst zu werden (§7.1 / `inject::windows`).
//!
//! `SetConsoleCtrlHandler` statt `sigaction`: Der Handler läuft in einem
//! **eigenen, vom System eingeschleusten Thread** und tut deshalb nur das
//! Minimum, das dort sicher ist — ein atomares Flag setzen. Die Event-Loop macht
//! daraus `Event::QuitRequested`.
//!
//! `TRUE` heißt „behandelt" — für `CTRL_C_EVENT`/`CTRL_BREAK_EVENT` bedeutet
//! das, dass Windows den Prozess **nicht** abschießt und die Event-Loop ihren
//! regulären Quit-Pfad gehen kann (§5.2).
//!
//! `CTRL_CLOSE_EVENT` (Konsolenfenster zu) wird genauso quittiert, aber
//! **ohne** auf das Aufräumen zu warten: Windows räumt den Prozess nach seiner
//! Frist ohnehin ab. Das Ack-Warten aus dem Plan bleibt offen (siehe Notizen zu
//! Paket C).

mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};

    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, SetConsoleCtrlHandler,
    };
    use windows_sys::core::BOOL;

    static REQUESTED: AtomicBool = AtomicBool::new(false);

    const HANDLED: BOOL = 1;
    const NOT_HANDLED: BOOL = 0;

    unsafe extern "system" fn on_ctrl_event(event: u32) -> BOOL {
        match event {
            CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT => {
                REQUESTED.store(true, Ordering::SeqCst);
                HANDLED
            }
            // `CTRL_LOGOFF_EVENT`/`CTRL_SHUTDOWN_EVENT` erreichen nur Dienste;
            // die Abmeldung kommt beim Daemon als `WM_ENDSESSION` am
            // Tray-Fenster an (WP4) und setzt dort denselben Quit-Weg in Gang.
            _ => NOT_HANDLED,
        }
    }

    pub fn install() {
        // SAFETY: `on_ctrl_event` hat die von `PHANDLER_ROUTINE` geforderte
        // Signatur und schreibt nur ein `AtomicBool`; `Add = TRUE` fügt den
        // Handler der Liste des Prozesses hinzu.
        if unsafe { SetConsoleCtrlHandler(Some(on_ctrl_event), HANDLED) } == 0 {
            // Kein Logger an dieser Stelle: `install` läuft vor dem Datei-Log.
            // Ohne Handler endet Ctrl+C hart — das ist eine Meldung wert, aber
            // kein Grund, nicht zu starten.
            eprintln!("diktier: SetConsoleCtrlHandler fehlgeschlagen — Ctrl+C beendet hart");
        }
    }

    /// Konsumiert die Anforderung — ein zweites `QuitRequested` wäre sinnlos.
    pub fn take_quit_request() -> bool {
        REQUESTED.swap(false, Ordering::SeqCst)
    }
}

pub use imp::{install, take_quit_request};
