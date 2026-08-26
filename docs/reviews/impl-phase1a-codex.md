# Review Phase 1a — STT-Engine-Infrastruktur (codex)

## Kurzfazit

Kein Blocker im Sinn eines grundsätzlich falschen Engine-Ansatzes: `parakeet-rs`
und `ort` sind exakt gepinnt, `ort-sys` ist über das ebenfalls exakt gepinnte
`ort` und `Cargo.lock` auf `2.0.0-rc.13` festgelegt, und der effektive
Feature-Graph enthält `load-dynamic`/`api-28`, aber nicht
`download-binaries`. Auch die Nutzung von `ParakeetTDT` und `hound` ist im
Grundsatz korrekt.

Die Infrastruktur ist dennoch noch nicht abnahmefähig. Der von
`fetch-ort.sh` erzeugte Dev-Aufbau wird vom normalen Binary nicht gefunden,
ein Umgebungsvariablen-Bypass widerspricht dem verbindlichen privaten
Ladeweg, und für Windows fehlt das exakt gepinnte ORT-Artefakt samt
Smoke-Pfad. Außerdem sind die inzwischen bekannte Modellrevision und die
Lizenzunterlagen noch nicht in Manifest/Distribution übernommen.

Verifikation auf Linux: `cargo build --locked`, `cargo test --locked` (30
bestanden, ein Test ignoriert) und `cargo clippy --locked --all-targets
--all-features -- -D warnings` waren erfolgreich. Der separat ausgeführte
ignorierte Test `stt_smoke_silence_is_empty` war ebenfalls erfolgreich. Ein
normaler Lauf ohne `DIKTIER_ORT_LIB` scheitert dagegen reproduzierbar trotz
vorhandener `lib/libonnxruntime.so` mit Exitcode 1. Ein Windows-Build war auf
diesem Rechner mangels installiertem Windows-Target nicht ausführbar.

## Befunde

### Hoch — Das Fetch-Skript und der Resolver erzeugen keinen gemeinsam nutzbaren Dev-Aufbau

- **Stelle:** `scripts/fetch-ort.sh:13-35`, `src/engine.rs:147-165`,
  `docs/SPIKES.md:30-32`
- **Problem:** Das Skript legt die Runtime unter `<Repo>/lib/` ab. Ein mit
  `cargo build` erzeugtes Binary liegt dagegen unter `target/debug/` bzw.
  `target/release/`; der Resolver sucht von dort nur `target/<Profil>/lib/`
  und `target/lib/`. Damit funktioniert der in §11 ausdrücklich genannte
  Dev-Weg `cargo build --release` nicht mit dem Ergebnis des Skripts. Der
  dokumentierte Spike musste folgerichtig `DIKTIER_ORT_LIB` verwenden. Das
  ließ sich mit `target/debug/diktier --transcribe-wav
  testdata/stt/stille.wav` reproduzieren: ORT wird nicht gefunden, Exit 1.
- **Vorschlag:** Einen expliziten Bundle-/Staging-Schritt bereitstellen, der
  Binary und `lib/<fester Name>` in dasselbe Bundle-Verzeichnis kopiert, und
  den Spike über genau dieses Layout laufen lassen. Alternativ muss das
  Fetch-/Release-Skript die Library neben das konkret gebaute Binary legen.
  Der Produktionsresolver sollte dabei weiterhin ausschließlich relativ zu
  `current_exe()` arbeiten.

### Hoch — `DIKTIER_ORT_LIB` umgeht den exklusiven und exakt gepinnten ORT-Ladeweg

- **Stelle:** `src/engine.rs:127-145`
- **Problem:** Ein gesetztes `DIKTIER_ORT_LIB` hat Vorrang vor dem Bundlepfad
  und darf auf jede beliebige Datei im Dateisystem zeigen. Das widerspricht
  §6.1/§11 (Laden ausschließlich aus dem privaten Pfad relativ zu
  `current_exe()`, kein externer ORT) und hebelt das exakte Pinning des
  ORT-Binaries aus. `ort::init_from` akzeptiert zudem Runtime-Minorversionen
  größer als die angeforderte API mit Warnung; der Override garantiert also
  gerade nicht ORT 1.28.0.
- **Vorschlag:** Den Override aus Produktions-Builds entfernen. Falls er für
  den Spike unverzichtbar bleibt, ihn hinter ein nicht standardmäßig
  aktiviertes Dev-/Test-Feature setzen und dort mindestens einen absoluten,
  erwarteten Pfad sowie die gepinnte Artefaktidentität prüfen. Der eigentliche
  Abnahmelauf muss ohne Override aus dem Bundlepfad erfolgen.

### Hoch — Die jetzt bekannte Modellrevision ist nicht im Manifest verankert; Lizenz/Notice fehlen

- **Stelle:** `docs/SPIKES.md:23-28`, `src/models.toml:1-29`,
  `src/download.rs:137-147`, `README.md:18-20`
- **Problem:** `docs/SPIKES.md` nennt inzwischen die Herkunftsrevision
  `8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce`. Trotzdem bleiben alle URLs im
  Manifest leer, und der neue Test schreibt diesen Zustand sogar fest. Nach
  §6.3 müssen ab diesem Zeitpunkt immutable
  `resolve/<git-commit>/...`-URLs im Manifest stehen. Ebenfalls fehlen der
  verlangte NVIDIA-Parakeet-/CC-BY-4.0-Hinweis im README und ein
  `LICENSES/`-Artefakt; das README nennt nur MIT.
- **Vorschlag:** Die vier URLs auf genau diese Revision festlegen, den Test
  auf die exakten URLs umstellen und Modell-Attribution/Lizenztext in README
  und `LICENSES/` ergänzen. Dabei die URL-Inhalte nochmals gegen die vier
  Golden-Set-Hashes prüfen.

### Hoch — Für die verpflichtende Windows-Seite ist weder ORT gepinnt noch ein Smoke-Aufbau vorhanden

- **Stelle:** `Cargo.toml:16-23`, `scripts/fetch-ort.sh:1-38`,
  `src/engine.rs:249-267`
- **Problem:** Das einzige Runtime-Artefakt und dessen Hash gelten nur für
  `linux-x64`; ein gepinntes Windows-x64-ORT-ZIP/DLL-Artefakt und ein
  entsprechender Fetch-/Staging-Weg fehlen vollständig. Der ignorierte
  Smoke-Test konstruiert außerdem fest `lib/libonnxruntime.so`. Damit kann
  das in §12/§13 verlangte Windows-Laden aus dem Bundlepfad ohne PATH-Hack
  mit diesem Stand weder reproduziert noch als Test eingerichtet werden.
- **Vorschlag:** Auch das offizielle Windows-x64-CPU-Artefakt exakt mit URL,
  Version und SHA-256 pinnen, `onnxruntime.dll` in das Spec-Bundlelayout
  stagen und den Smoke-Test den plattformspezifischen festen Namen verwenden
  lassen. Der Windows-Smoke muss ohne `PATH`, `ORT_DYLIB_PATH` und
  `DIKTIER_ORT_LIB` laufen.

### Mittel — Fehler der ORT-Initialisierung werden für den gesamten Prozess dauerhaft gecacht

- **Stelle:** `src/engine.rs:183-196`
- **Problem:** `OnceLock<Result<PathBuf, String>>` speichert auch ein
  fehlgeschlagenes `resolve_ort_lib()` oder `ort::init_from()`. Wird die
  Library danach bereitgestellt oder der Pfad korrigiert, liefert jeder
  weitere Ladeversuch im selben Prozess weiterhin den ersten Fehler. Die
  interne Initialisierung von `ort` benutzt dagegen bewusst eine
  Try-Init-Semantik, bei der ein fehlgeschlagener Ladeversuch die Zelle nicht
  belegt. Der aktuelle Code verbaut damit einen späteren expliziten Retry und
  macht einen transienten Dateisystemfehler zwangsläufig prozessfatal.
- **Vorschlag:** Nur erfolgreiche Initialisierung dauerhaft speichern und
  Versuche mit einem Mutex serialisieren (oder eine geeignete fallible
  Once-Initialisierung verwenden). Einen Test ergänzen: erster Versuch ohne
  Datei schlägt fehl, Datei wird bereitgestellt, zweiter Versuch kann
  erfolgreich initialisieren.

### Mittel — Das Ergebnis von `EnvironmentBuilder::commit()` wird ignoriert

- **Stelle:** `src/engine.rs:187-192`
- **Problem:** `commit()` liefert `false`, wenn bereits eine ORT-Environment
  konfiguriert wurde; dann werden die soeben gesetzten Optionen nicht
  wirksam. Der Code verwirft diesen Wert und speichert trotzdem `Ok(path)`.
  Dadurch meldet `ensure_ort_initialized()` Erfolg, obwohl eine frühere,
  unerwartete ORT-Nutzung den Initialisierungsvertrag bereits verletzt hat.
  Zusammen mit der globalen Dylib-Zelle von `ort` kann `init_from(path)` auch
  nicht nachträglich auf eine andere bereits geladene Library umschalten.
- **Vorschlag:** Beim ersten Diktier-Init `commit() == false` als klaren
  Initialisierungsfehler behandeln. Nur ein von Diktier selbst bereits
  protokollierter erfolgreicher Init darf idempotent als Erfolg gelten; den
  Fall durch einen isolierten Prozesstest absichern.

### Mittel — Lokale Golden-Set-Dateien werden nur nach Größe, nicht nach SHA-256 geprüft

- **Stelle:** `src/download.rs:82-98`, `src/engine.rs:78-99`,
  `src/download.rs:159-176`
- **Problem:** Vor dem Modellladen prüft `check_artifacts()` lediglich
  Existenz und Bytezahl. Eine beschädigte oder vertauschte Datei gleicher
  Größe — insbesondere `vocab.txt` oder `config.json` — gilt damit als
  gültiges Golden Set. Der Manifest-Test kontrolliert nur die im Binary
  eingebetteten Strings, nicht die tatsächlich geladene lokale Kopie. Das
  genügt der Anforderung aus §6.3, dass die Golden-Set-Hashes stimmen, nicht.
- **Vorschlag:** Die lokale Kopie vor ihrer Freigabe auch per SHA-256 prüfen
  und erst danach als vollständig markieren. Tests für falschen Hash bei
  korrekter Größe und für alle vier gültigen Artefakte ergänzen; die spätere
  Download-Implementierung sollte dieselbe Prüfroutine verwenden.

### Mittel — WAV-I/O-Fehler und Bedienfehler sind im Exitcode nicht unterscheidbar

- **Stelle:** `src/audio.rs:8-14`, `src/audio.rs:45-91`,
  `src/main.rs:100-107`
- **Problem:** Alle Fehler von `WavReader::open()` und beim Sample-Lesen
  werden in `AudioError::Wav(String)` umgewandelt und damit pauschal als
  Exitcode 2 ausgegeben. Darunter fallen aber auch echte Laufzeit-I/O-Fehler
  wie `PermissionDenied` oder ein Lesefehler nach erfolgreichem Öffnen; nach
  §9 müssen diese Exitcode 1 liefern. Durch das frühzeitige Stringifizieren
  geht die `hound::Error::IoError`-Information verloren.
- **Vorschlag:** Format-/Parameterfehler typisiert von `io::Error` trennen.
  Nicht unterstützte Rate, Kanalzahl, Sampleformat und kaputte Eingabe können
  als Bedienfehler 2 gelten; Dateisystem-/Lese-I/O muss als Laufzeitfehler 1
  durchgereicht und jeweils getestet werden.

### Mittel — Die dokumentierten Zeitwerte sind keine fünf warmen Läufe mit geladenem Modell

- **Stelle:** `src/main.rs:117-143`, `docs/SPIKES.md:30-40`
- **Problem:** Die CLI lädt das Modell, führt genau eine Inferenz aus und
  beendet den Prozess. `docs/SPIKES.md` beschreibt fünf separate CLI-Läufe
  und bezeichnet deren Median dennoch als „warm“. §12 fordert fünf warme
  Läufe bei separat ausgewiesenem Modellladen; dafür muss dasselbe geladene
  Modell nach mindestens einem Warmup mehrfach inferieren. Separate Prozesse
  setzen ORT, Sessions und Prozesszustand jedes Mal neu auf.
- **Vorschlag:** Einen kleinen Benchmark-/Spike-Modus oder einen ignorierten
  Test bereitstellen, der einmal lädt, mindestens einmal aufwärmt und danach
  fünf Inferenzzeiten desselben Inputs erfasst. Das Protokoll mit dieser
  Methode neu messen bzw. die aktuellen Werte bis dahin nicht als warm
  bezeichnen.

### Mittel — Kritische Fehlerpfade und der reale Smoke sind nur sehr schmal getestet

- **Stelle:** `src/audio.rs:96-128`, `src/engine.rs:206-288`,
  `src/main.rs:197-238`
- **Problem:** Der reale ignorierte Test deckt nur `stille.wav` ab. Es fehlen
  Smoke-Fälle für die drei Sprach-WAVs, `rauschen.wav` und `< 250 ms` sowie
  Tests für fehlende/falsche ORT-Library, `commit()`-/Retry-Semantik,
  erfolgreiche i16-/f32-WAVs, Stereo, falsche Bittiefe und Sample-Lesefehler.
  Der nachgewiesene Fetch-/Resolver-Mismatch blieb deshalb unentdeckt. Der
  Stub-Test „silence yields empty“ prüft nur, dass ein Stub immer leer
  zurückgibt, nicht einen Silence-Pfad.
- **Vorschlag:** ORT-Auflösung und CLI-Exitcodes bevorzugt in isolierten
  Kindprozessen testen, weil ORT prozessglobale Once-Zellen besitzt. WAV-Fälle
  als schnelle Unit-Tests ergänzen und den ignorierten Realmodell-Smoke über
  alle fünf Fixtures parametrisieren; Qualitäts-/WER-Auswertung kann dabei
  weiterhin separat als Spike-Gate protokolliert werden.

### Niedrig — Die Plattform-CFGs akzeptieren unbeabsichtigt weitere Unix-Ziele als Linux

- **Stelle:** `src/download.rs:51-79`, `src/engine.rs:168-176`
- **Problem:** `cfg(unix)` verwendet auf macOS/BSD den Linux-Datenpfad, und
  `cfg(not(windows))` wählt dort `libonnxruntime.so`. Die Compile-Error-Branch
  greift auf diesen Systemen nicht. Das bricht Windows heute nicht, macht die
  behauptete Plattformgrenze aber unscharf und kann Cross-Builds für nicht
  unterstützte Unix-Ziele irreführend erfolgreich machen.
- **Vorschlag:** Explizit `target_os = "linux"` und `windows` verwenden und
  für alle anderen Ziele einen klaren `compile_error!` bzw. einen bewusst
  unterstützten Pfad definieren.
