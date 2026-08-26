# Third-Party-Komponenten

Diktier selbst steht unter MIT (`LICENSE-diktier-MIT.txt`). Das Release-Bundle
enthält darüber hinaus fremde Bestandteile.

Diese Liste ist **handgepflegt** und nennt die Bibliotheken, die im Bundle
landen oder den Charakter der Anwendung bestimmen — sie ist kein vollständiger
Abhängigkeitsbaum. Die exakt gebauten Versionen stehen in `versions.toml`, der
vollständige Baum in `Cargo.lock` im Quell-Repository.

## Mitgeliefert als Binärdatei

| Komponente | Datei | Lizenz | Quelle |
|---|---|---|---|
| ONNX Runtime (CPU) | `lib/libonnxruntime.so` | MIT (`ONNXRUNTIME-LICENSE.txt`) | [microsoft/onnxruntime](https://github.com/microsoft/onnxruntime), offizielles Release |

## Nachgeladen zur Laufzeit (nicht im Bundle)

| Komponente | Lizenz | Quelle |
|---|---|---|
| NVIDIA Parakeet TDT 0.6B v3, ONNX-INT8 | CC-BY-4.0 (`CC-BY-4.0.txt`, Attribution in `NOTICE-parakeet.md`) | [istupakov/parakeet-tdt-0.6b-v3-onnx](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx) |

Die vier Artefakte lädt Diktier beim ersten Start (rund 650 MB) und prüft sie
gegen Größe und SHA-256 aus `src/models.toml`.

## Einkompilierte Rust-Crates (Auswahl)

| Crate | Rolle | Lizenz |
|---|---|---|
| `parakeet-rs` | Parakeet-TDT-Decoder, Mel-Frontend | MIT OR Apache-2.0 |
| `ort` | ONNX-Runtime-Bindung (`load-dynamic`, C-API 1.28) | MIT OR Apache-2.0 |
| `ndarray` | Tensoren für die Engine-Schnittstelle | MIT OR Apache-2.0 |
| `cpal` | Audioaufnahme (ALSA/PulseAudio) | Apache-2.0 |
| `rubato` | FFT-Resampling auf 16 kHz | MIT |
| `hound` | WAV-Lesen/-Schreiben | Apache-2.0 |
| `x11rb` | X11-Protokoll (Clipboard, Paste, Fensterkennung) | MIT OR Apache-2.0 |
| `global-hotkey` | globaler Push-to-Talk-Hotkey | Apache-2.0 OR MIT |
| `betrayer` | Tray-Icon über StatusNotifierItem | MIT |
| `zbus` | D-Bus-Anbindung unter `betrayer` | MIT |
| `ureq` | HTTPS-Download der Modellartefakte | MIT OR Apache-2.0 |
| `rustls` | TLS für den Download | Apache-2.0 OR ISC OR MIT |
| `ring` | Kryptoprimitive unter `rustls` | Apache-2.0 AND ISC |
| `webpki-roots` | Wurzelzertifikate (Mozilla-CA-Bundle) | CDLA-Permissive-2.0 |
| `clap` | Kommandozeile | MIT OR Apache-2.0 |
| `serde`, `toml` | Config- und Manifest-Parsing | MIT OR Apache-2.0 |
| `sha2` | SHA-256 der Modellartefakte | MIT OR Apache-2.0 |
| `thiserror` | Fehlertypen | MIT OR Apache-2.0 |
| `libc` | `flock` für die Single-Instance-Sperre | MIT OR Apache-2.0 |

Die vollständigen Lizenztexte der Crates liegen in den jeweiligen
Quellarchiven auf [crates.io](https://crates.io); Apache-2.0, MIT, ISC und
CDLA-Permissive-2.0 erlauben die Weitergabe in dieser Form.

## Systembibliotheken

Nicht im Bundle, vom Betriebssystem erwartet (`readelf -d diktier`):
`libasound.so.2`, `libgcc_s.so.1`, `libm.so.6`, `libc.so.6` —
glibc-Mindestversion siehe `versions.toml`, Abschnitt `[build_host]`.

`libasound.so.2` gehört zum ALSA-Backend von `cpal` und wird beim Start
benötigt. Die Aufnahme läuft im Regelfall trotzdem über PulseAudio: `cpal`
spricht dessen Protokoll direkt über den Unix-Socket (unter PipeWire von
`pipewire-pulse` bedient) und braucht dafür **keine** `libpulse`.

Auf Debian/Ubuntu/Mint deckt das Paket `libasound2t64` (früher `libasound2`)
diese Anforderung ab; auf einem Desktop mit Audio ist es ohnehin installiert.
