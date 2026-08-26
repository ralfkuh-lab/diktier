#!/usr/bin/env bash
# Baut das Linux-Release-Bundle nach Spec §11:
#
#   dist/diktier-<version>-linux-x64/
#     diktier                 # cargo build --release --locked
#     lib/libonnxruntime.so   # fester Name, kein Symlink
#     LICENSES/               # MIT (App), CC-BY-4.0 + NOTICE (Modell),
#                             # ONNX-Runtime-MIT, THIRD-PARTY.md
#     versions.toml           # App, ORT-ABI, Crate-Pins, Modell, Build-Host
#     README.md               # Kurzanleitung für Empfänger ohne Repository
#   dist/diktier-<version>-linux-x64.tar.gz
#
# Kein PATH, kein LD_LIBRARY_PATH, kein System-ORT: die Library wird über
# `ort::init_from` relativ zu `current_exe()` geladen (§11).
#
# Idempotent: das Zielverzeichnis wird vor jedem Lauf neu aufgebaut.
#
# Umgebung:
#   SKIP_BUILD=1   überspringt `cargo build --release` (Binary muss liegen)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "${ROOT}"

TARGET_TRIPLE="x86_64-unknown-linux-gnu"
PLATFORM="linux-x64"

# --------------------------------------------------------------- Hilfsfunktionen

die() {
  echo "release.sh: $*" >&2
  exit 1
}

# Version der Crate aus Cargo.toml (erste `version = "…"` im [package]-Block).
crate_version() {
  awk '/^\[package\]/{p=1;next} /^\[/{p=0} p && /^version *=/{gsub(/[",]/,"");print $3;exit}' \
    "${ROOT}/Cargo.toml"
}

# Aufgelöste Version(en) eines Pakets aus Cargo.lock als TOML-Wert.
#
# Steht ein Name mehrfach im Lock (z. B. `thiserror` 1.x transitiv neben
# unserem 2.x), werden **alle** Versionen als Array ausgegeben — ein einzelner
# Wert würde die falsche Wahrheit dokumentieren.
lock_version() {
  awk -v want="name = \"$1\"" '
    $0 == want { getline; gsub(/[",]/, ""); found[n++] = $3 }
    END {
      if (n == 0) exit 1
      if (n == 1) { printf "\"%s\"", found[0]; exit }
      printf "["
      for (i = 0; i < n; i++) printf "%s\"%s\"", (i ? ", " : ""), found[i]
      printf "]"
    }
  ' "${ROOT}/Cargo.lock"
}

# Wert aus src/models.toml (key = "…").
manifest_value() {
  awk -v want="$1" '
    $1 == want && $2 == "=" { gsub(/^[^"]*"|"[^"]*$/, ""); print; exit }
  ' "${ROOT}/src/models.toml"
}

VERSION="$(crate_version)"
[[ -n "${VERSION}" ]] || die "Version aus Cargo.toml nicht lesbar"

NAME="diktier-${VERSION}-${PLATFORM}"
DIST="${ROOT}/dist"
BUNDLE="${DIST}/${NAME}"
TARBALL="${DIST}/${NAME}.tar.gz"

# ------------------------------------------------------------------- ORT-Library

if [[ ! -f "${ROOT}/lib/libonnxruntime.so" ]]; then
  echo "== ORT-Library fehlt — scripts/fetch-ort.sh"
  "${ROOT}/scripts/fetch-ort.sh"
fi
ORT_SO="${ROOT}/lib/libonnxruntime.so"
[[ -f "${ORT_SO}" ]] || die "lib/libonnxruntime.so fehlt auch nach fetch-ort.sh"
[[ ! -L "${ORT_SO}" ]] || die "lib/libonnxruntime.so ist ein Symlink (§11: feste Datei)"

ORT_VERSION="$(awk -F'"' '/^ORT_VERSION=/{print $2}' "${ROOT}/scripts/fetch-ort.sh")"
ORT_TARBALL_SHA="$(awk -F'"' '/^TARBALL_SHA256=/{print $2}' "${ROOT}/scripts/fetch-ort.sh")"
ORT_SO_SHA="$(sha256sum "${ORT_SO}" | cut -d' ' -f1)"

# ------------------------------------------------------------------------- Build

if [[ "${SKIP_BUILD:-0}" == "1" ]]; then
  echo "== Build übersprungen (SKIP_BUILD=1)"
else
  echo "== cargo build --release --locked"
  cargo build --release --locked
fi
BIN="${ROOT}/target/release/diktier"
[[ -x "${BIN}" ]] || die "target/release/diktier fehlt"

# ------------------------------------------------------------------------ Bundle

echo "== Bundle ${BUNDLE}"
rm -rf "${BUNDLE}" "${TARBALL}"
mkdir -p "${BUNDLE}/lib" "${BUNDLE}/LICENSES"

install -m 0755 "${BIN}" "${BUNDLE}/diktier"
# -L: falls lib/ doch je einen Symlink enthält, landet die echte Datei im Bundle.
cp -L "${ORT_SO}" "${BUNDLE}/lib/libonnxruntime.so"
chmod 0644 "${BUNDLE}/lib/libonnxruntime.so"

install -m 0644 "${ROOT}/LICENSE" "${BUNDLE}/LICENSES/LICENSE-diktier-MIT.txt"
for file in "${ROOT}"/LICENSES/*; do
  install -m 0644 "${file}" "${BUNDLE}/LICENSES/$(basename "${file}")"
done
[[ -f "${BUNDLE}/LICENSES/ONNXRUNTIME-LICENSE.txt" ]] ||
  die "LICENSES/ONNXRUNTIME-LICENSE.txt fehlt — scripts/fetch-ort.sh erneut laufen lassen"
[[ -f "${BUNDLE}/LICENSES/THIRD-PARTY.md" ]] ||
  die "LICENSES/THIRD-PARTY.md fehlt"

# -------------------------------------------------------------- versions.toml

MODEL_KEY="$(manifest_value key)"
# Immutable HF-Revision steht in der ersten Download-URL (…/resolve/<rev>/…).
MODEL_REV="$(awk -F'/resolve/' '/^url *=/{split($2,a,"/"); print a[1]; exit}' "${ROOT}/src/models.toml")"
MODEL_REPO="$(awk -F'"' '/^url *=/{print $2; exit}' "${ROOT}/src/models.toml" |
  sed -E 's#^(https://huggingface.co/[^/]+/[^/]+)/resolve/.*#\1#')"

RUSTC_VERSION="$(rustc -V)"
CARGO_VERSION="$(cargo -V)"
# `sed -n 1p` statt `head -1`: `head` schließt die Pipe früh, das gäbe unter
# `set -o pipefail` einen SIGPIPE-Abbruch (Exit 141).
GLIBC_VERSION="$(ldd --version | sed -n '1p')"
BUILD_HOST="$(. /etc/os-release 2>/dev/null && echo "${PRETTY_NAME}" || uname -sr)"

{
  echo "# Von scripts/release.sh erzeugt (Spec §11). Nicht von Hand pflegen."
  echo
  echo "[app]"
  echo "name = \"diktier\""
  echo "version = \"${VERSION}\""
  echo "platform = \"${PLATFORM}\""
  echo "target = \"${TARGET_TRIPLE}\""
  echo
  echo "[onnxruntime]"
  echo "# CPU-Release von microsoft/onnxruntime, geladen über scripts/fetch-ort.sh."
  echo "# ABI: C-API 1.28 (ort-Feature api-28), Laden per ort::init_from aus lib/."
  echo "version = \"${ORT_VERSION}\""
  echo "abi = \"api-28\""
  echo "build = \"onnxruntime-linux-x64-${ORT_VERSION} (CPU, offizielles GitHub-Release)\""
  echo "# Diese Builds setzen mindestens SSE4.2/AVX2 voraus — Haswell aufwärts (§11)."
  echo "tarball_sha256 = \"${ORT_TARBALL_SHA}\""
  echo "library_sha256 = \"${ORT_SO_SHA}\""
  echo
  echo "[model]"
  echo "key = \"${MODEL_KEY}\""
  echo "repository = \"${MODEL_REPO}\""
  echo "revision = \"${MODEL_REV}\""
  echo "# Größen und SHA-256 der vier Artefakte: src/models.toml bzw. Spec §6.3."
  echo
  echo "[crates]"
  echo "# Aufgelöste Versionen aus Cargo.lock — die Pins stehen in Cargo.toml."
  for crate in parakeet-rs ort cpal rubato x11rb global-hotkey betrayer ureq \
    rustls ring clap serde toml sha2 thiserror hound libc; do
    version="$(lock_version "${crate}")" || die "Crate ${crate} steht nicht in Cargo.lock"
    printf '%s = %s\n' "${crate}" "${version}"
  done
  echo
  echo "[toolchain]"
  echo "rustc = \"${RUSTC_VERSION}\""
  echo "cargo = \"${CARGO_VERSION}\""
  echo
  echo "[build_host]"
  echo "os = \"${BUILD_HOST}\""
  echo "glibc = \"${GLIBC_VERSION}\""
  echo "# Ältere glibc als hier gebaut wird nicht unterstützt."
} >"${BUNDLE}/versions.toml"
chmod 0644 "${BUNDLE}/versions.toml"

# ------------------------------------------------------------------- README

# Gekürzte Fassung der Repo-README: Wer nur den Tarball bekommt, soll ohne
# Repository loslegen können. Bewusst keine relativen Repo-Links — im Bundle
# existiert nur `LICENSES/`.
cat >"${BUNDLE}/README.md" <<EOF
# Diktier ${VERSION} — Linux (x86_64)

Lokales Push-to-Talk-Diktat: F9 halten, sprechen, loslassen — der Text landet
am Cursor. Läuft offline mit NVIDIA Parakeet.

Dieser Ordner ist ein **Bundle**: Binary, ONNX Runtime (\`lib/\`), Lizenzen und
\`versions.toml\` gehören zusammen und bleiben beieinander.

## Installation

\`\`\`bash
./diktier --install-autostart   # Eintrag in ~/.config/autostart/
./diktier --foreground          # erster Start, Logs im Terminal
\`\`\`

Der Ordner darf liegen, wo er will — die ONNX Runtime wird immer aus \`lib/\`
neben der Binary geladen, nie aus dem System, nie über \`PATH\` oder
\`LD_LIBRARY_PATH\`. Nach einem Verschieben genügt ein erneutes
\`./diktier --install-autostart\`: der Eintrag wird aktualisiert, nicht
verdoppelt. Entfernen: \`./diktier --remove-autostart\`.

Voraussetzungen: X11-Sitzung (Cinnamon), ein Tray mit StatusNotifierItem und
\`libasound2t64\`. Wayland wird in v1 nicht unterstützt.

## Erster Start: Modell-Download

Beim ersten Start fehlt das Sprachmodell. Diktier lädt es selbst nach
\`~/.local/share/diktier/models/${MODEL_KEY}/\`:

- **rund 650 MB**, vier Dateien, einmalig
- Quelle: ${MODEL_REPO}
  (feste Revision \`${MODEL_REV}\`)
- jede Datei wird gegen Größe **und** SHA-256 geprüft, bevor sie gültig wird
- der Tray zeigt „Lade Modell …", der Fortschritt steht im Log
- Lizenz der Artefakte: CC-BY-4.0, siehe \`LICENSES/NOTICE-parakeet.md\`

Wer die vier Dateien schon hat, kopiert sie einfach dorthin — der Download
entfällt dann.

## Das erste Diktat

1. Warten, bis der Tray-Tooltip \`idle\` zeigt.
2. Cursor dorthin setzen, wo der Text hin soll.
3. **F9 halten**, sprechen, loslassen.

Der Fokus wandert dabei nie. Wechselst du während der Aufnahme das Fenster,
wird nicht eingefügt — der Text liegt dann in der Zwischenablage. Ein
Tray-Linksklick nimmt ebenfalls auf, fügt aber bewusst nichts ein.

## Konfiguration

\`~/.config/diktier/config.toml\` entsteht beim ersten Start mit Defaults
(Hotkey, Aufnahmegerät, Cap, Paste-Verhalten). Änderungen wirken nach einem
Neustart des Daemons. Der Tray öffnet den Ordner über sein Menü.

## Wenn etwas klemmt

- **F9 belegt** → Tray zeigt \`error\`, das Log nennt „Hotkey-Registrierung":
  andere Taste in \`config.toml\`, neu starten. Der Tray-Linksklick geht weiter.
- **„unterstützt nur X11"** → die Sitzung läuft unter Wayland
  (\`echo \$XDG_SESSION_TYPE\` muss \`x11\` sagen).
- **Text kommt nicht an, ist aber in der Zwischenablage** → das Zielfenster hat
  den Paste abgelehnt oder der Fokus wechselte. Nichts geht verloren.
- **„diktier läuft bereits"** → es läuft schon eine Instanz (Autostart); der
  zweite Start endet absichtlich wirkungslos mit Exit 0.
- **Log**: \`~/.local/state/diktier/diktier.log\` (rotiert bei 2 MiB nach
  \`diktier.log.1\`). Transkripte, Zwischenablage-Inhalte und Fenstertitel
  stehen dort nie drin.

## Weiteres

Versionen aller Bestandteile: \`versions.toml\`. Lizenzen und Fremdbestandteile:
\`LICENSES/\` (insbesondere \`THIRD-PARTY.md\`). Vollständige Dokumentation,
Spezifikation und Quellcode liegen im Projekt-Repository (\`docs/SPEC.md\`).
EOF
chmod 0644 "${BUNDLE}/README.md"

# --------------------------------------------------------------------- Prüfungen

echo "== Selbstprüfung"
for expected in diktier lib/libonnxruntime.so versions.toml README.md \
  LICENSES/LICENSE-diktier-MIT.txt LICENSES/CC-BY-4.0.txt \
  LICENSES/NOTICE-parakeet.md LICENSES/ONNXRUNTIME-LICENSE.txt \
  LICENSES/THIRD-PARTY.md; do
  [[ -e "${BUNDLE}/${expected}" ]] || die "Bundle unvollständig: ${expected}"
done
# Läuft ohne ORT und ohne Modell: belegt, dass das Binary im Bundle startet.
"${BUNDLE}/diktier" --version >/dev/null || die "${BUNDLE}/diktier --version schlug fehl"

# ------------------------------------------------------------------------ Tarball

echo "== Tarball ${TARBALL}"
# Sortiert und ohne Host-Benutzer: gleicher Input → gleiches Archiv.
tar --sort=name --owner=0 --group=0 --numeric-owner \
  -czf "${TARBALL}" -C "${DIST}" "${NAME}"

echo
echo "Bundle:  ${BUNDLE}"
echo "Tarball: ${TARBALL}"
du -h "${TARBALL}" | cut -f1 | sed 's/^/Größe:   /'
sha256sum "${TARBALL}"
