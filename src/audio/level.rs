//! Mikrofonpegel für das Aufnahme-Overlay (SPEC §4.5,
//! `docs/overlay-plan.md` Leitentscheidungen 1, 2 und 8).
//!
//! **Warum ein Atomic und nicht der Ring.** Während `recording` liest niemand
//! den SPSC-Ring; seine Slots sind `UnsafeCell`, ein nebenläufiges Peek wäre
//! undefiniertes Verhalten (`super::spsc`). Der einzige Ort mit Live-Samples
//! ist der cpal-Callback — und der darf weder allozieren noch sperren (§6.4).
//! Deshalb ein [`LevelTap`]: der Callback schreibt den Betragspeak seines
//! Buffers per [`LevelTap::publish`] hinein, der Renderer holt ihn per
//! [`LevelTap::take`] wieder heraus.
//!
//! **Generation statt „Gate reicht schon" (Sol-Impl-Review, Major 2 und 3).**
//! Ein Callback prüft `armed` nur beim **Eintritt** in den Gate-Abschnitt. Hat
//! er ihn betreten, läuft er auch nach einem `disarm()` bis zu seinem Publish
//! weiter — beim Stuck-Producer-Pfad sogar noch, wenn längst ein neuer Stream
//! aufnimmt. Ein zweiter `is_armed()`-Test vor dem Publish schlösse das Rennen
//! zwischen Prüfen und Schreiben nicht.
//!
//! Deshalb tragen Peak **und** Stream-Generation gemeinsam in **einem**
//! `AtomicU64`: obere 32 Bit die Generation, untere 32 Bit die f32-Bits des
//! Peaks. Der Callback kennt die Generation, mit der sein Stream gebaut wurde,
//! und publiziert per `compare_exchange_weak` — verglichen wird die **ganze**
//! Zelle. Ein Callback einer alten Generation bricht ab, ein zeitgleicher
//! Generationswechsel lässt jeden laufenden CAS scheitern. Damit gibt es kein
//! Check-then-Act-Fenster mehr.
//!
//! **Wer wechselt wann die Generation?** Genau der Owner-Thread des
//! Aufnahmegeräts, und genau dann, wenn ein Stream entsteht oder verschwindet
//! (`open`, `release`, Stuck-Producer, fehlgeschlagenes `play`). Solange
//! derselbe Stream weiterläuft, reicht [`LevelTap::clear`] — es nullt den Peak
//! und lässt die Generation stehen, damit der laufende Callback weiter
//! publizieren darf.
//!
//! **Normierungs-Vertrag** (reine Funktion, damit testbar):
//!
//! 1. Sample **erst** nach `f32` wandeln ([`super::convert::ToUnitF32`]),
//! 2. pro Frame denselben arithmetischen Kanalmittelwert wie
//!    `downmix_interleaved` bilden — gemessen wird der **ASR-Eingangspegel**,
//!    nicht der lauteste Rohkanal,
//! 3. Betrag nehmen (`abs` kanonisiert `-0.0` zu `+0.0` mit),
//! 4. nicht-endliche Werte (NaN, ±Inf) → `0.0`,
//! 5. auf `[0, 1]` klemmen.
//!
//! Nach diesem Vertrag sind alle Werte kanonische, endliche `f32` in
//! `+0.0..=1.0` — und **nur** auf dieser Domäne ist die u32-Bitordnung
//! ordnungserhaltend. Das ist die Vorbedingung dafür, dass [`publish`] mit
//! `fetch_max` über `f32::to_bits()` arbeiten darf. `Ordering::Relaxed` genügt:
//! Der Pegel ist ein unabhängiger Skalar, er ordnet keine anderen Zugriffe.
//! Einordnung: **lock-frei, nicht wait-frei** — `fetch_max` darf intern
//! CAS-Retries machen; allokationsfrei und ohne Lock ist es trotzdem.
//!
//! **Peak-Hold zwischen zwei Frames.** Der Callback kommt bei WASAPI Shared
//! typisch alle ~10 ms, das Overlay rendert alle ~33 ms. `fetch_max` beim
//! Schreiben und `swap(0)` beim Lesen halten deshalb den lautesten Wert seit
//! dem letzten Frame fest, statt Transienten zu verschlucken.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use super::convert::ToUnitF32;

/// Anzeigebereich des Pegelmeters: −50 dBFS … 0 dBFS.
pub const DISPLAY_RANGE_DB: f32 = 50.0;

/// Abfall der Peak-Hold-Marke (Leitentscheidung 8).
pub const PEAK_FALL_DB_PER_SEC: f32 = 20.0;

/// Untere 32 Bit der Zelle: die f32-Bits des Peaks.
const PEAK_MASK: u64 = 0x0000_0000_FFFF_FFFF;
/// Obere 32 Bit: die Stream-Generation.
const GENERATION_SHIFT: u32 = 32;

/// Der geteilte Pegel: der cpal-Callback schreibt, der Overlay-Thread liest.
/// `None` im Audio-Pfad heißt „Overlay aus" — dann rechnet der Callback gar
/// nicht erst (`[overlay] enabled = false`, Leitentscheidung 10).
#[derive(Debug)]
pub struct LevelTap {
    /// Generation (obere 32 Bit) und Peak (untere 32 Bit) in **einer** Zelle.
    state: AtomicU64,
    /// Gibt es überhaupt noch einen Consumer? Der Overlay-Thread löscht das
    /// Flag, bevor er stirbt — danach rechnet der Callback den Pegel gar nicht
    /// mehr aus (Sol-Impl-Review, Major 4).
    active: AtomicBool,
}

impl Default for LevelTap {
    fn default() -> Self {
        Self::new()
    }
}

impl LevelTap {
    pub fn new() -> Self {
        Self {
            state: AtomicU64::new(0),
            active: AtomicBool::new(true),
        }
    }

    /// Läuft noch ein Consumer? Genau ein Relaxed-Load pro Callback-Buffer.
    #[inline]
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    /// Der Consumer verabschiedet sich — ab jetzt kostet der Tap im
    /// Audio-Callback nur noch diesen einen Load.
    pub fn deactivate(&self) {
        self.active.store(false, Ordering::Relaxed);
    }

    /// Generation, für die gerade publiziert werden darf.
    pub fn generation(&self) -> u32 {
        (self.state.load(Ordering::Relaxed) >> GENERATION_SHIFT) as u32
    }

    /// Neue Generation, Peak auf 0 — **das** ist der harte Reset: Callbacks
    /// des alten Streams können danach nie wieder publizieren.
    ///
    /// Nur der Owner-Thread des Geräts ruft das auf, und nur dort, wo ein
    /// Stream entsteht oder verschwindet. Der 64-Bit-RMW lässt jeden
    /// zeitgleich laufenden Producer-CAS scheitern — der vergleicht die ganze
    /// Zelle, nicht nur den Peak.
    pub fn bump_generation(&self) -> u32 {
        let next = self.generation().wrapping_add(1);
        self.state
            .swap(u64::from(next) << GENERATION_SHIFT, Ordering::Relaxed);
        next
    }

    /// Producer-Seite: lautester Wert der aktuellen Generation gewinnt, bis
    /// der Renderer ihn abholt.
    ///
    /// Normiert selbst — damit kann kein Aufrufer ein NaN-Bitmuster in die
    /// Zelle bringen, das als riesiger Peak gewänne (Sol Major 2 des
    /// Plan-Reviews). Lock-frei (CAS-Retry möglich), allokationsfrei.
    #[inline]
    pub fn publish(&self, generation: u32, level: f32) {
        let bits = u64::from(normalize(level).to_bits());
        let mut current = self.state.load(Ordering::Relaxed);
        loop {
            if (current >> GENERATION_SHIFT) as u32 != generation {
                // Fremde (alte) Generation: dieser Callback gehört zu einem
                // Stream, den der Owner-Thread längst weggeworfen hat.
                return;
            }
            if bits <= (current & PEAK_MASK) {
                return;
            }
            let next = (current & !PEAK_MASK) | bits;
            match self.state.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(actual) => current = actual,
            }
        }
    }

    /// Consumer-Seite: Peak seit dem letzten Abholen, danach steht der Peak
    /// wieder auf 0. Die Generation bleibt stehen — der laufende Stream darf
    /// weiter publizieren.
    pub fn take(&self) -> f32 {
        let previous = self.state.fetch_and(!PEAK_MASK, Ordering::Relaxed);
        f32::from_bits((previous & PEAK_MASK) as u32)
    }

    /// Peak auf Stille, Generation unverändert. Für die Stellen, an denen
    /// derselbe Stream weiterläuft und das Gate nachweislich ruhig ist
    /// (`start()` vor `arm()`, `stop()` nach `wait_idle()`).
    pub fn clear(&self) {
        self.state.fetch_and(!PEAK_MASK, Ordering::Relaxed);
    }
}

pub fn new_tap() -> Arc<LevelTap> {
    Arc::new(LevelTap::new())
}

/// Schritt 3–5 des Normierungs-Vertrags. Idempotent.
#[inline]
pub fn normalize(value: f32) -> f32 {
    // `abs()` zieht `-0.0` auf `+0.0` und macht aus NaN wieder NaN.
    let magnitude = value.abs();
    if !magnitude.is_finite() {
        return 0.0;
    }
    magnitude.clamp(0.0, 1.0)
}

/// Betragspeak über einen kompletten Callback-Buffer, gemessen auf dem
/// Kanalmittelwert je Frame (Schritt 1–5).
///
/// Läuft im cpal-Callback: keine Allokation, kein Lock, O(n) über den Buffer,
/// der ohnehin schon angefasst wird.
#[inline]
pub fn buffer_peak<T: ToUnitF32>(data: &[T], channels: usize) -> f32 {
    let channels = channels.max(1);
    let mut peak = 0.0_f32;
    for frame in data.chunks_exact(channels) {
        let mut sum = 0.0_f32;
        for sample in frame {
            sum += sample.to_unit_f32();
        }
        let level = normalize(sum / channels as f32);
        if level > peak {
            peak = level;
        }
    }
    peak
}

/// Linearer Pegel → Balkenhöhe `0..1` über `clamp((dBFS + 50) / 50, 0, 1)`.
///
/// Pegel ≤ 0 (und alles nicht-Endliche) landet direkt auf dem Stille-Floor —
/// `log10(0)` wird nie gerechnet.
pub fn bar_height(level: f32) -> f32 {
    let level = normalize(level);
    if level <= 0.0 {
        return 0.0;
    }
    let dbfs = 20.0 * level.log10();
    ((dbfs + DISPLAY_RANGE_DB) / DISPLAY_RANGE_DB).clamp(0.0, 1.0)
}

/// Abfall der Peak-Hold-Marke über die **gemessene** Zeit seit dem letzten
/// Frame — nicht ein fester Abzug pro Frame: der Rendertakt jittert
/// (Scheduling, Nachrichtenfluten, Sessionwechsel).
///
/// Gerechnet wird in der Anzeige-Domäne (Balkenhöhe): 20 dB/s sind bei 50 dB
/// Anzeigebereich 0,4 Höhe pro Sekunde.
pub fn decay_peak(peak: f32, elapsed: Duration) -> f32 {
    let peak = normalize(peak);
    if peak <= 0.0 {
        return 0.0;
    }
    let secs = elapsed.as_secs_f32();
    if !secs.is_finite() || secs <= 0.0 {
        return peak;
    }
    let fall = (PEAK_FALL_DB_PER_SEC / DISPLAY_RANGE_DB) * secs;
    (peak - fall).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Alle neun Sampleformate landen im Einheitsbereich — inklusive der
    /// `MIN`-Werte, an denen ein `abs()` im Integerraum übergelaufen wäre, und
    /// der Offset-Binary-Mittelpunkte `2^(n-1)`.
    #[test]
    fn every_sample_format_normalizes_into_the_unit_range() {
        assert!((buffer_peak(&[i8::MIN], 1) - 1.0).abs() < 1e-6);
        assert!((buffer_peak(&[i16::MIN], 1) - 1.0).abs() < 1e-6);
        assert!((buffer_peak(&[i32::MIN], 1) - 1.0).abs() < 1e-6);
        assert!((buffer_peak(&[i64::MIN], 1) - 1.0).abs() < 1e-5);
        assert!((buffer_peak(&[i8::MAX], 1) - 127.0 / 128.0).abs() < 1e-6);
        assert!((buffer_peak(&[i16::MAX], 1) - 32767.0 / 32768.0).abs() < 1e-6);

        // Offset-Binary: Mittelpunkt ist Stille, beide Enden Vollausschlag.
        assert!(buffer_peak(&[128_u8], 1) < 1e-6);
        assert!((buffer_peak(&[0_u8], 1) - 1.0).abs() < 1e-6);
        assert!((buffer_peak(&[u8::MAX], 1) - 127.0 / 128.0).abs() < 1e-6);
        assert!(buffer_peak(&[32_768_u16], 1) < 1e-6);
        assert!((buffer_peak(&[0_u16], 1) - 1.0).abs() < 1e-6);
        assert!(buffer_peak(&[1_u32 << 31], 1) < 1e-5);
        assert!((buffer_peak(&[0_u32], 1) - 1.0).abs() < 1e-5);

        assert!((buffer_peak(&[-0.75_f32], 1) - 0.75).abs() < 1e-6);
        assert!((buffer_peak(&[-0.75_f64], 1) - 0.75).abs() < 1e-6);

        // Und nichts davon verlässt [0, 1].
        for value in [
            buffer_peak(&[i32::MIN], 1),
            buffer_peak(&[i64::MIN], 1),
            buffer_peak(&[0_u32], 1),
        ] {
            assert!((0.0..=1.0).contains(&value), "{value}");
        }
    }

    /// Native Float-Streams dürfen alles liefern. Nach der Normierung ist
    /// nichts davon mehr da (Sol Major 2).
    #[test]
    fn hostile_float_input_normalizes_to_the_silence_floor_or_gets_clamped() {
        assert_eq!(normalize(f32::NAN), 0.0);
        assert_eq!(normalize(-f32::NAN), 0.0);
        assert_eq!(normalize(f32::INFINITY), 0.0);
        assert_eq!(normalize(f32::NEG_INFINITY), 0.0);
        assert_eq!(normalize(4.2), 1.0);
        assert_eq!(normalize(-4.2), 1.0);

        // `-0.0` wird zu `+0.0` kanonisiert — sonst gewönne sein Bitmuster
        // (0x8000_0000) jedes `fetch_max`.
        assert_eq!(normalize(-0.0_f32).to_bits(), 0);
        assert_eq!(normalize(0.0_f32).to_bits(), 0);

        // Subnormals sind endlich und im Bereich: sie überleben unverändert
        // und bleiben ordnungserhaltend (Bits < jede normale Zahl).
        let subnormal = f32::from_bits(1);
        assert_eq!(normalize(subnormal), subnormal);
        assert_eq!(normalize(-subnormal), subnormal);
        assert!(normalize(subnormal).to_bits() < 0.5_f32.to_bits());

        // Dasselbe über den Produktionsweg.
        assert_eq!(buffer_peak(&[f32::NAN, 2.0, -3.0], 1), 1.0);
        assert_eq!(buffer_peak(&[f32::NAN], 1), 0.0);
        assert_eq!(buffer_peak(&[f32::INFINITY], 1), 0.0);
    }

    /// Sol Minor 11: Die Bitordnung wird **nur** über der
    /// Nach-Normierungs-Domäne behauptet — kanonische, endliche `0.0..=1.0`.
    /// Dort ist `to_bits()` monoton, und genau darauf baut `fetch_max`.
    #[test]
    fn bit_order_matches_float_order_on_the_normalized_domain() {
        let mut previous = normalize(0.0);
        let mut previous_bits = previous.to_bits();
        for step in 0..=1_000_u32 {
            let value = normalize(step as f32 / 1_000.0);
            let bits = value.to_bits();
            assert!(
                value >= previous,
                "Testfolge ist nicht monoton: {previous} → {value}"
            );
            assert!(
                bits >= previous_bits,
                "Bitordnung gebrochen: {value} ({bits:#010x}) < {previous} ({previous_bits:#010x})"
            );
            previous = value;
            previous_bits = bits;
        }
        // Sonderfälle gezielt statt zufällig (NaN ist ungeordnet, `-0.0 ==
        // +0.0` bei verschiedenen Bits) — siehe Test oben.
        assert_eq!(normalize(1.0).to_bits(), 1.0_f32.to_bits());
    }

    /// Peak-Hold über den Kanal: der lauteste Wert bleibt bis zum Abholen
    /// stehen, danach steht der Tap wieder auf Stille.
    #[test]
    fn the_peak_is_held_until_it_is_taken() {
        let tap = new_tap();
        let generation = tap.generation();
        assert_eq!(tap.take(), 0.0);

        tap.publish(generation, 0.2);
        tap.publish(generation, 0.9);
        tap.publish(generation, 0.4);
        assert!((tap.take() - 0.9).abs() < 1e-6, "der Peak muss gewinnen");
        assert_eq!(tap.take(), 0.0, "take() räumt den Peak ab");

        tap.publish(generation, 0.3);
        tap.clear();
        assert_eq!(tap.take(), 0.0, "clear() setzt auf Stille zurück");

        // Auch ein feindlicher Wert kann den Tap nicht vollaussteuern.
        tap.publish(generation, f32::NAN);
        assert_eq!(tap.take(), 0.0);
    }

    /// Sol-Impl-Review Major 2: Ein Callback publiziert **nur** in die
    /// Generation, mit der sein Stream gebaut wurde. Ein alter Callback kann
    /// den Pegel eines neuen Streams deshalb nie anfassen — und zwar ohne
    /// Check-then-Act-Fenster, weil der CAS die ganze Zelle vergleicht.
    #[test]
    fn only_the_current_generation_may_publish() {
        let tap = new_tap();
        let old = tap.generation();
        tap.publish(old, 0.7);
        assert!((tap.take() - 0.7).abs() < 1e-6);

        let new = tap.bump_generation();
        assert_ne!(new, old, "der Wechsel muss sichtbar sein");
        tap.publish(old, 1.0);
        assert_eq!(tap.take(), 0.0, "die alte Generation kommt nicht durch");
        tap.publish(new, 0.4);
        assert!((tap.take() - 0.4).abs() < 1e-6);

        // `clear`/`take` lassen die Generation stehen — der laufende Stream
        // darf danach weiter publizieren.
        tap.publish(new, 0.5);
        tap.clear();
        assert_eq!(tap.generation(), new);
        tap.publish(new, 0.6);
        assert!((tap.take() - 0.6).abs() < 1e-6);

        // Der Wechsel nullt den Peak mit — ohne zweiten Schreibzugriff.
        tap.publish(new, 0.9);
        let newest = tap.bump_generation();
        assert_eq!(tap.take(), 0.0, "bump_generation ist der Reset");
        assert_eq!(tap.generation(), newest);
    }

    /// Sol-Impl-Review Major 4: Ohne Consumer wird nichts mehr publiziert.
    #[test]
    fn a_deactivated_tap_reports_itself_as_inactive() {
        let tap = new_tap();
        assert!(tap.is_active(), "frisch ist der Tap aktiv");
        tap.deactivate();
        assert!(!tap.is_active());
        // Das Flag ist der einzige Prüfpunkt im Callback — der Tap selbst
        // bleibt benutzbar, wird aber nicht mehr gefüttert (siehe
        // `capture::push_if_armed`).
        assert!(!tap.is_active());
    }

    /// Auch unter echter Nebenläufigkeit: Was ein Producer nach dem
    /// Generationswechsel schreibt, erreicht die neue Generation nicht.
    #[test]
    fn a_racing_old_producer_never_reaches_the_new_generation() {
        use std::sync::Barrier;

        let tap = new_tap();
        let old = tap.generation();
        let switched = Arc::new(Barrier::new(2));
        let producer = {
            let (tap, switched) = (tap.clone(), switched.clone());
            std::thread::spawn(move || {
                switched.wait();
                for _ in 0..10_000 {
                    tap.publish(old, 1.0);
                }
            })
        };
        let new = tap.bump_generation();
        switched.wait();
        producer.join().unwrap();
        assert_eq!(tap.take(), 0.0, "kein einziger Wert der alten Generation");
        assert_eq!(tap.generation(), new);
    }

    /// Sol Minor 10: Gemessen wird der ASR-Eingangspegel (Kanalmittelwert),
    /// nicht der lauteste Rohkanal. Sonst zeigte das Overlay Aktivität,
    /// während beim Decoder Stille ankommt.
    #[test]
    fn the_peak_follows_the_channel_mean_not_the_loudest_channel() {
        // Gleichlauf: Mittelwert == Kanal.
        let in_phase = [0.5_f32, 0.5, -0.8, -0.8];
        assert!((buffer_peak(&in_phase, 2) - 0.8).abs() < 1e-6);

        // Gegenphase: der Decoder hört nichts, das Overlay zeigt nichts.
        let anti_phase = [0.9_f32, -0.9, 0.5, -0.5];
        assert!(
            buffer_peak(&anti_phase, 2) < 1e-6,
            "Gegenphase muss ~0 ergeben"
        );

        // Einseitiges Signal: halbiert, wie der Downmix es auch tut.
        let one_sided = [1.0_f32, 0.0];
        assert!((buffer_peak(&one_sided, 2) - 0.5).abs() < 1e-6);

        // Unvollständiges Frame am Buffer-Ende bleibt liegen (wie im Ring).
        assert_eq!(buffer_peak(&[1.0_f32], 2), 0.0);
        assert_eq!(buffer_peak::<f32>(&[], 1), 0.0);
    }

    /// Leitentscheidung 8: dB-Abbildung inklusive der Ränder — und **nie**
    /// `log10(0)`.
    #[test]
    fn db_mapping_covers_silence_full_scale_and_subnormals() {
        assert_eq!(bar_height(0.0), 0.0);
        assert_eq!(bar_height(-0.0), 0.0);
        assert_eq!(bar_height(f32::NAN), 0.0);
        assert_eq!(bar_height(f32::INFINITY), 0.0);
        assert_eq!(bar_height(f32::from_bits(1)), 0.0, "Subnormal ist Stille");
        assert!((bar_height(1.0) - 1.0).abs() < 1e-6);
        assert!((bar_height(2.0) - 1.0).abs() < 1e-6, "geklemmt auf 0 dBFS");

        // −50 dBFS ist der Boden, −25 dBFS die Mitte.
        assert!(bar_height(10.0_f32.powf(-50.0 / 20.0)).abs() < 1e-5);
        assert!((bar_height(10.0_f32.powf(-25.0 / 20.0)) - 0.5).abs() < 1e-5);
        assert!(bar_height(0.000_01) == 0.0, "unter dem Boden bleibt 0");

        // Monoton über den ganzen Anzeigebereich.
        let mut last = 0.0;
        for step in 1..=100 {
            let height = bar_height(step as f32 / 100.0);
            assert!(height >= last, "nicht monoton bei {step}");
            assert!((0.0..=1.0).contains(&height));
            last = height;
        }
    }

    /// Der Abfall hängt an der gemessenen Zeit, nicht an der Framerate: zehn
    /// 10-ms-Schritte fallen so weit wie ein 100-ms-Schritt.
    #[test]
    fn peak_hold_falls_with_the_measured_elapsed_time() {
        let one_step = decay_peak(1.0, Duration::from_millis(100));
        let mut many_steps = 1.0;
        for _ in 0..10 {
            many_steps = decay_peak(many_steps, Duration::from_millis(10));
        }
        assert!(
            (one_step - many_steps).abs() < 1e-5,
            "{one_step} vs {many_steps}"
        );
        // 20 dB/s auf 50 dB Anzeigebereich = 0,4 Höhe pro Sekunde.
        assert!((decay_peak(1.0, Duration::from_secs(1)) - 0.6).abs() < 1e-6);

        // Lange Pause (Transcribing/Injecting): die Marke läuft leer und
        // bleibt dort — kein negativer Wert.
        assert_eq!(decay_peak(1.0, Duration::from_secs(30)), 0.0);
        assert_eq!(decay_peak(0.0, Duration::from_secs(1)), 0.0);
        assert_eq!(decay_peak(f32::NAN, Duration::from_millis(20)), 0.0);
        // Ein stehengebliebener Zeitgeber darf die Marke nicht bewegen.
        assert_eq!(decay_peak(0.5, Duration::ZERO), 0.5);
        assert!((0.0..=1.0).contains(&decay_peak(2.0, Duration::from_millis(20))));
    }
}
