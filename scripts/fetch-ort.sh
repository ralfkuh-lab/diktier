#!/usr/bin/env bash
# Lädt das offizielle ONNX-Runtime-CPU-Release (Linux x64) in der Version,
# die die gepinnte ort-Crate 2.0.0-rc.13 verlangt: ORT 1.28.0.
# Ergebnis: lib/libonnxruntime.so (fester Name, kein Symlink, Spec §11).
#
# Dev-Staging: zusätzlich nach target/debug/lib/ und target/release/lib/,
# damit ./target/<profil>/diktier und das Test-Binary
# (target/<profil>/deps/ → ../lib) ohne Umgebungsvariable finden.
# Bewusst kein Suchpfad ../../lib im Resolver: bei /opt/diktier träfe
# das System-/lib und umginge §11.
set -euo pipefail

ORT_VERSION="1.28.0"
TARBALL="onnxruntime-linux-x64-${ORT_VERSION}.tgz"
URL="https://github.com/microsoft/onnxruntime/releases/download/v${ORT_VERSION}/${TARBALL}"
# SHA-256 des offiziellen GitHub-Release-Tarballs (nicht der .so).
TARBALL_SHA256="a3e1b79d7bb1bf09696ce675f49e4064e6c81f6202b8225624fff0e93f8d6407"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="${ROOT}/lib"
# Die MIT-Lizenz der ONNX Runtime gehört ins Bundle (Spec §11 „LICENSES/").
LICENSE_DEST="${ROOT}/LICENSES/ONNXRUNTIME-LICENSE.txt"
TMP="$(mktemp -d)"
cleanup() { rm -rf "${TMP}"; }
trap cleanup EXIT

echo "Lade ${URL}"
curl -fL --retry 3 -o "${TMP}/${TARBALL}" "${URL}"

echo "${TARBALL_SHA256}  ${TMP}/${TARBALL}" | sha256sum -c -

tar -xzf "${TMP}/${TARBALL}" -C "${TMP}"
EXTRACT="${TMP}/onnxruntime-linux-x64-${ORT_VERSION}"
SRC="${EXTRACT}/lib/libonnxruntime.so"
if [[ ! -e "${SRC}" ]]; then
  echo "Fehler: ${SRC} fehlt im Archiv" >&2
  exit 1
fi

mkdir -p "${DEST}"
# -L: Symlink auflösen, feste Datei unter dem Spec-Namen ablegen.
cp -L "${SRC}" "${DEST}/libonnxruntime.so"
chmod 0644 "${DEST}/libonnxruntime.so"

# Lizenz mitnehmen, solange das Archiv noch ausgepackt ist: das Release-Bundle
# liefert die Library aus und muss ihre Lizenz beilegen (§11).
mkdir -p "$(dirname "${LICENSE_DEST}")"
if [[ -f "${EXTRACT}/LICENSE" ]]; then
  cp "${EXTRACT}/LICENSE" "${LICENSE_DEST}"
  chmod 0644 "${LICENSE_DEST}"
  echo "OK: ${LICENSE_DEST}"
else
  echo "Warnung: LICENSE fehlt im ORT-Archiv — ${LICENSE_DEST} nicht aktualisiert" >&2
fi

# target/ darf noch fehlen (frisches Repo).
for profile in debug release; do
  mkdir -p "${ROOT}/target/${profile}/lib"
  cp -L "${DEST}/libonnxruntime.so" "${ROOT}/target/${profile}/lib/libonnxruntime.so"
  chmod 0644 "${ROOT}/target/${profile}/lib/libonnxruntime.so"
done

echo "OK: ${DEST}/libonnxruntime.so"
echo "OK: ${ROOT}/target/debug/lib/libonnxruntime.so"
echo "OK: ${ROOT}/target/release/lib/libonnxruntime.so"
sha256sum "${DEST}/libonnxruntime.so"
