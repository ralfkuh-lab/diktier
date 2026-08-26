# Diktier

Lokales Push-to-Talk-Diktat für Windows und Linux Mint. Halten, sprechen,
loslassen — der Text landet am Cursor. Läuft offline mit NVIDIA Parakeet.

Status: Spec, noch keine Implementierung. Siehe [docs/SPEC.md](docs/SPEC.md).

## Zielplattformen (v1)

- Windows 10 22H2+ / Windows 11, x64
- Linux Mint 22.x, Cinnamon, **X11**, x86_64

Cinnamon/Wayland ist in v1 kein Supportziel.

Nicht das gleiche Tool wie [Voxtype](https://voxtype.io) auf Omarchy — Diktier
soll dieselbe Erkennungsqualität auf den übrigen Rechnern liefern.

## Lizenz

Die Anwendung steht unter MIT. Siehe [LICENSE](LICENSE).

Die mitgelieferten bzw. heruntergeladenen **Parakeet-Modellartefakte**
(NVIDIA Parakeet TDT 0.6B v3, ONNX-INT8-Konvertierung
[istupakov/parakeet-tdt-0.6b-v3-onnx](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx),
Revision `8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce`) stehen unter
[CC-BY-4.0](LICENSES/CC-BY-4.0.txt). Attribution und Herkunft:
[LICENSES/NOTICE-parakeet.md](LICENSES/NOTICE-parakeet.md).
