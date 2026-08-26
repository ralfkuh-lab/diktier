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

    const SIGINT: i32 = 2;
    const SIGTERM: i32 = 15;

    unsafe extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }

    extern "C" fn on_signal(_signum: i32) {
        REQUESTED.store(true, Ordering::SeqCst);
    }

    pub fn install() {
        let handler = on_signal as extern "C" fn(i32) as usize;
        // SAFETY: `signal` ist POSIX; der Handler schreibt nur ein AtomicBool.
        // SIGHUP bleibt bewusst unangetastet: ein per Autostart gestarteter
        // Daemon hat kein Terminal, und `nohup` soll weiter greifen.
        unsafe {
            signal(SIGINT, handler);
            signal(SIGTERM, handler);
        }
    }

    /// Konsumiert die Anforderung — ein zweites `QuitRequested` wäre sinnlos.
    pub fn take_quit_request() -> bool {
        REQUESTED.swap(false, Ordering::SeqCst)
    }
}

#[cfg(not(unix))]
mod imp {
    /// Windows bekommt seinen `SetConsoleCtrlHandler` mit dem Windows-Backend.
    pub fn install() {}

    pub fn take_quit_request() -> bool {
        false
    }
}

pub use imp::{install, take_quit_request};
