//! Aufnahme-Gate zwischen cpal-Callback und Ring-Consumer.
//!
//! Der Stream läuft dauerhaft (Owner-Entscheidung Phase 3c: sonst suspendiert
//! das Gerät und jeder Aufnahmestart kostet ~2 s). Ob der Callback die Frames
//! annimmt, entscheidet dieses Gate — und es beweist zusätzlich, **wann kein
//! Callback mehr im Ring steht**.
//!
//! Ein bloßes `armed`-Flag reicht dafür nicht (codex H1 / agy B4): Ein Callback
//! kann `armed == true` gelesen haben und danach vom Scheduler unterbrochen
//! werden; ein stillstehender Write-Cursor beweist nichts. Weil die Ring-Slots
//! `UnsafeCell` sind, wäre ein überlappendes `drain`/`reset` nicht nur ein
//! verlorener Rest, sondern undefiniertes Verhalten.
//!
//! Deshalb betritt der Callback das Gate über einen atomaren In-flight-Zähler
//! **bevor** er `armed` liest, und verlässt es erst nach dem letzten
//! Ringzugriff:
//!
//! ```text
//! Producer: in_flight++ ; armed? ─ja→ push … push ─→ in_flight--
//!                              └─nein→ in_flight--
//! Consumer: armed=false ; warte auf in_flight==0 ; drain/reset
//! ```
//!
//! Sieht der Consumer nach `disarm()` ein `in_flight == 0`, dann gilt: Wer
//! vorher eingetreten war, ist fertig, und wer danach eintritt, sieht
//! `armed == false` und rührt den Ring nicht an.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Pollschritt beim Warten auf den ruhenden Producer.
const IDLE_STEP: Duration = Duration::from_millis(1);

#[derive(Debug, Default)]
pub struct CaptureGate {
    armed: AtomicBool,
    in_flight: AtomicUsize,
}

impl CaptureGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ab jetzt nimmt der Callback Frames an. Nur der Consumer ruft das auf,
    /// und nur während `in_flight == 0` gilt (also nach `wait_idle`).
    pub fn arm(&self) {
        self.armed.store(true, Ordering::Release);
    }

    /// Ab jetzt nimmt der Callback nichts mehr an. Sagt **nicht**, dass gerade
    /// keiner mehr im Ring steht — dafür ist [`CaptureGate::wait_idle`] da.
    pub fn disarm(&self) {
        self.armed.store(false, Ordering::Release);
    }

    pub fn is_armed(&self) -> bool {
        self.armed.load(Ordering::Acquire)
    }

    pub fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }

    /// Producer-Eintritt. `None` heißt: nicht scharf, Finger weg vom Ring.
    #[inline]
    pub fn enter(&self) -> Option<GateGuard<'_>> {
        // Erst anmelden, dann prüfen: Wer den Zähler nicht erhöht hat, kann
        // den Ring auch nicht anfassen — und wer ihn erhöht hat, ist für den
        // Consumer sichtbar, bevor er `armed` liest.
        self.in_flight.fetch_add(1, Ordering::AcqRel);
        if self.armed.load(Ordering::Acquire) {
            Some(GateGuard { gate: self })
        } else {
            self.in_flight.fetch_sub(1, Ordering::AcqRel);
            None
        }
    }

    /// Wartet, bis kein Producer mehr im Gate steht. `false` = Frist abgelaufen;
    /// der Aufrufer darf den Ring dann **nicht** lesen (codex H1).
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        if self.in_flight() == 0 {
            return true;
        }
        let deadline = Instant::now() + timeout;
        loop {
            std::thread::sleep(IDLE_STEP);
            if self.in_flight() == 0 {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
        }
    }
}

/// Solange dieser Guard lebt, zählt der Producer als „im Ring".
#[derive(Debug)]
pub struct GateGuard<'a> {
    gate: &'a CaptureGate,
}

impl Drop for GateGuard<'_> {
    fn drop(&mut self) {
        self.gate.in_flight.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Barrier;
    use std::thread;

    #[test]
    fn disarmed_gate_refuses_entry() {
        let gate = CaptureGate::new();
        assert!(gate.enter().is_none());
        assert_eq!(
            gate.in_flight(),
            0,
            "abgewiesener Eintritt hinterlässt nichts"
        );
        gate.arm();
        let guard = gate.enter().expect("scharf");
        assert_eq!(gate.in_flight(), 1);
        drop(guard);
        assert_eq!(gate.in_flight(), 0);
    }

    /// Der Kern von codex H1: Ein Callback, der das Gate bereits betreten hat,
    /// hält `wait_idle` auf — und zwar deterministisch, nicht heuristisch.
    #[test]
    fn wait_idle_blocks_until_the_producer_left_the_gate() {
        let gate = Arc::new(CaptureGate::new());
        gate.arm();
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));

        let producer = {
            let gate = gate.clone();
            let entered = entered.clone();
            let release = release.clone();
            thread::spawn(move || {
                let guard = gate.enter().expect("Gate war scharf");
                entered.wait(); // Producer steht jetzt im Ring
                release.wait(); // … und bleibt dort, bis der Test freigibt
                drop(guard);
            })
        };

        entered.wait();
        gate.disarm();
        assert_eq!(gate.in_flight(), 1);
        assert!(
            !gate.wait_idle(Duration::from_millis(20)),
            "solange der Producer im Ring steht, ist das Gate nicht ruhig"
        );

        release.wait();
        assert!(
            gate.wait_idle(Duration::from_secs(2)),
            "nach dem Verlassen muss das Gate ruhig werden"
        );
        assert_eq!(gate.in_flight(), 0);
        producer.join().unwrap();
    }

    /// Nach `disarm` kommt niemand mehr herein — auch nicht, wenn er es
    /// zeitgleich versucht.
    #[test]
    fn nobody_enters_after_disarm() {
        let gate = Arc::new(CaptureGate::new());
        gate.arm();
        gate.disarm();
        assert!(gate.enter().is_none());
        assert!(gate.wait_idle(Duration::ZERO));

        let start = Arc::new(Barrier::new(2));
        let handle = {
            let gate = gate.clone();
            let start = start.clone();
            thread::spawn(move || {
                start.wait();
                let mut entries = 0;
                for _ in 0..10_000 {
                    if gate.enter().is_some() {
                        entries += 1;
                    }
                }
                entries
            })
        };
        start.wait();
        let entries: u32 = handle.join().unwrap();
        assert_eq!(entries, 0, "ein entwaffnetes Gate lässt niemanden durch");
        assert_eq!(gate.in_flight(), 0);
    }

    /// Mehrere Producer (cpal kann den Callback-Thread wechseln) zählen sauber.
    #[test]
    fn nested_entries_are_counted() {
        let gate = CaptureGate::new();
        gate.arm();
        let a = gate.enter().unwrap();
        let b = gate.enter().unwrap();
        assert_eq!(gate.in_flight(), 2);
        assert!(!gate.wait_idle(Duration::from_millis(5)));
        drop(a);
        assert_eq!(gate.in_flight(), 1);
        drop(b);
        assert!(gate.wait_idle(Duration::from_millis(5)));
    }
}
