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

#[cfg(not(unix))]
mod imp {
    /// Windows bekommt seinen `SetConsoleCtrlHandler` mit dem Windows-Backend.
    pub fn install() {}

    pub fn take_quit_request() -> bool {
        false
    }
}

pub use imp::{install, take_quit_request};
