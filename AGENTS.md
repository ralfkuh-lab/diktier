# Diktier — Agent-Hinweise

Lokales Push-to-Talk-Diktiertool in Rust. Die verbindliche Quelle ist
[docs/SPEC.md](docs/SPEC.md). Abweichungen von der Spec nur nach Rückfrage.

## Nicht tun

- WhisperDictate (`~/dev/Whisper-dictate`) nicht umbauen oder hierher kopieren.
- Keinen Preview-Dialog, kein Whisper, kein HTTP-Server in v1.
- Auf dem PTT-Pfad kein Fenster öffnen, das den Fokus stiehlt.
- TDT-Decoder nicht selbst implementieren — `parakeet-rs` (Fallback: `transcribe-rs`).

## Implementierungsreihenfolge

1. STT-Spike (gleiche WAV gegen Voxtype auf Omarchy)
2. Inject-Spike (Windows + Mint, Fokus bleibt, Umlaute)
3. Daemon + Tray + Config + Autostart

Gates stehen in der Spec. Nicht committen, solange der Auftrag das verbietet.
