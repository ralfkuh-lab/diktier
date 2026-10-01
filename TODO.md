# TODO

## ✅ Modell-Repo `ralfkuh-lab/diktier-models` anlegen (Ralf, privater Account)

Für den Ultra-Alltagstest ([docs/ultra-alltagstest-plan.md](docs/ultra-alltagstest-plan.md),
WP3a Schritt 2). Das geht nur mit `ralfkuh-lab` selbst: Der Account `fsrakul`,
mit dem hier gearbeitet wird, hat bei `ralfkuh-lab` nur Push-Rechte und kann
dort weder ein Repo anlegen noch Immutable Releases einschalten.

Auf einem Rechner, auf dem `gh` als `ralfkuh-lab` angemeldet ist (`gh auth status`):

```bash
# 1. Öffentliches, leeres Repo anlegen (kein README nötig, den ersten Commit macht Claude)
gh repo create ralfkuh-lab/diktier-models --public --description "Modellartefakte für Diktier (Parakeet Ultra int8, CC-BY-4.0)"

# 2. Immutable Releases einschalten (braucht Admin, also ralfkuh-lab)
gh api -X PUT repos/ralfkuh-lab/diktier-models/immutable-releases

#    Prüfen, die Antwort muss "enabled": true enthalten
gh api repos/ralfkuh-lab/diktier-models/immutable-releases

# 3. fsrakul als Collaborator mit Schreibrecht einladen
gh api -X PUT repos/ralfkuh-lab/diktier-models/collaborators/fsrakul -f permission=push
```

Alternativ im Browser:

1. Neues öffentliches Repo `diktier-models`.
2. Unter Settings → General, Abschnitt „Releases“, die Release-Immutability
   einschalten.
3. Unter Settings → Collaborators `fsrakul` mit Write einladen.

**Danach Bescheid geben.** Den Rest macht Claude vom Arbeitsrechner aus:

1. Einladung für `fsrakul` annehmen.
2. Ersten Commit in `diktier-models` pushen (README und NOTICE).
3. Modell-Release `model-parakeet-ultra-0.6b-int8-pc-r1` als Entwurf anlegen,
   hochladen, zurücklesen und prüfen, dann veröffentlichen (nicht als Latest).
4. Auf Ultra umstellen (Umgebungsvariablen, Config, Neustart). Ralfs erstes
   Diktat danach ist das Probediktat, danach folgt der Rückweg-Test.

## 🔍 Headset abgezogen → Diktier bleibt im Fehlerzustand (Tray dauerhaft rot)

Vorfall 2026-10-01 16:14 UTC (0.5.0, `audio.device = "default"`): Ralf hat die
Jabra Evolve2 40 abgezogen. Erholung erst durch Neustart des Daemons.

Ablauf laut `diktier.log`:

1. Lauf 145: Stream lief weiter, lieferte aber nur Nullen → „Gate: leer
   (Regel C)“, nichts eingefügt.
2. Lauf 147: Gerät als verloren erkannt, Neu-Öffnen scheitert in
   `default_input_config()`:
   `Failed to get audio client: Der Threadmodus kann nicht nach dem Einstellen
   geändert werden. (os error -2147417850)` = `RPC_E_CHANGED_MODE` (`0x80010106`).
3. Danach scheitert jeder Hotkey- und Tray-Versuch identisch, Zustand `error`.

Bekannt: Alle Öffnungen laufen auf dem Thread `diktier-audio`. cpal 0.18.2
initialisiert dort COM als STA (`src/host/com.rs`) und holt das Default-Gerät
über `ActivateAudioInterfaceAsync` (`wasapi/device.rs`). Im eigenen Code gibt es
kein `CoInitializeEx`. Das erste Öffnen beim Start klappt, nur das Neu-Öffnen
nach dem Abziehen nicht.

Offen:

- Ursache klären: Abziehen/Einstecken nachstellen, Fehlerquelle (cpal oder
  eigener Ablauf) eingrenzen.
- Fix ableiten, z. B. Gerät beim Neu-Öffnen auf einem frischen Audio-Thread
  anlegen. Spec-Abgleich vorher.
- Lauf 145: Erkennen, dass der Stream nur noch Nullen liefert, statt still
  „leer“ zu melden?
