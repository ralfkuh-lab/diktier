# TODO

## 🔍 Modell-Repo `ralfkuh-lab/diktier-models` anlegen (Ralf, privater Account)

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
