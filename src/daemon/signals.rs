//! SIGTERM/SIGINT auf den regulären Quit-Pfad (§5.2).
//!
//! Ohne das endet ein `kill`/`Ctrl+C` mitten im Prozess: X11-Ownership fiele
//! weg, das Clipboard bliebe unbedient und der `SAVE_TARGETS`-Handshake aus
//! §7.1/Phase-2-Erkenntnis liefe nie. Der Handler tut das Minimum, das
//! async-signal-safe ist: ein atomares Flag setzen. Die Event-Loop macht daraus
//! `Event::QuitRequested`.

#[cfg(unix)]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};

    static REQUESTED: AtomicBool = AtomicBool::new(false);

    extern "C" fn on_signal(_signum: libc::c_int) {
        REQUESTED.store(true, Ordering::SeqCst);
    }

    /// `sigaction` statt `signal` (agy B2): definierte Semantik ohne
    /// Handler-Reset, `SA_RESTART` für unterbrochene Syscalls, und kein Cast
    /// eines Funktionszeigers über `usize` in einer eigenen Deklaration.
    ///
    /// SIGHUP bleibt bewusst unangetastet: ein per Autostart gestarteter Daemon
    /// hat kein Terminal, und `nohup` soll weiter greifen.
    pub fn install() {
        // SAFETY: `sa` wird vollständig initialisiert, der Handler schreibt nur
        // ein `AtomicBool` (async-signal-safe), und die Signalnummern sind gültig.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_signal as *const () as usize;
            action.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut());
            libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut());
        }
    }

    /// Konsumiert die Anforderung — ein zweites `QuitRequested` wäre sinnlos.
    pub fn take_quit_request() -> bool {
        REQUESTED.swap(false, Ordering::SeqCst)
    }
}

/// Windows: `SetConsoleCtrlHandler` statt `sigaction` (windows-plan WP5).
///
/// Der Handler läuft in einem **eigenen, vom System eingeschleusten Thread**
/// und tut deshalb dasselbe Minimum wie der Unix-Handler: ein atomares Flag
/// setzen. `TRUE` heißt „behandelt" — für `CTRL_C_EVENT`/`CTRL_BREAK_EVENT`
/// bedeutet das, dass Windows den Prozess **nicht** abschießt und die
/// Event-Loop ihren regulären Quit-Pfad gehen kann (§5.2).
///
/// `CTRL_CLOSE_EVENT` (Konsolenfenster zu) wird genauso quittiert, aber
/// **ohne** auf das Aufräumen zu warten: Windows räumt den Prozess nach seiner
/// Frist ohnehin ab. Das Ack-Warten aus dem Plan bleibt offen (siehe Notizen zu
/// Paket C).
#[cfg(windows)]
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
            // Kein Logger an dieser Stelle (wie im Unix-Zweig läuft `install`
            // vor dem Datei-Log). Ohne Handler endet Ctrl+C hart — das ist eine
            // Meldung wert, aber kein Grund, nicht zu starten.
            eprintln!("diktier: SetConsoleCtrlHandler fehlgeschlagen — Ctrl+C beendet hart");
        }
    }

    /// Konsumiert die Anforderung — ein zweites `QuitRequested` wäre sinnlos.
    pub fn take_quit_request() -> bool {
        REQUESTED.swap(false, Ordering::SeqCst)
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    pub fn install() {}

    pub fn take_quit_request() -> bool {
        false
    }
}

pub use imp::{install, take_quit_request};
