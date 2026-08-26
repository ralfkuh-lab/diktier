# Ungetestet. Verifikation auf Windows in Phase 2.
# Analog zu fetch-ort.sh: offizielles ONNX-Runtime-CPU-Release Windows x64
# in der Version, die ort 2.0.0-rc.13 verlangt (ORT 1.28.0).
# Ergebnis: lib/onnxruntime.dll (fester Name, Spec §11).
#
# Dev-Staging: zusätzlich nach target\debug\lib\ und target\release\lib\,
# damit cargo-Binaries und Test-Binaries (deps\ → ..\lib) ohne Env finden.
# Bewusst kein Suchpfad ..\..\lib im Resolver (siehe fetch-ort.sh).

$ErrorActionPreference = "Stop"

$OrtVersion = "1.28.0"
$ZipName = "onnxruntime-win-x64-$OrtVersion.zip"
$Url = "https://github.com/microsoft/onnxruntime/releases/download/v$OrtVersion/$ZipName"
# SHA-256 des offiziellen GitHub-Release-Zips.
$ZipSha256 = "abef733dacbe2f571547a7150b479b5cb9cc0df22f96c24983a42cadb1b4f8bc"

$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
if (-not $Root) {
    $Root = (Get-Location).Path
}
$Dest = Join-Path $Root "lib"
$Tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("diktier-ort-" + [guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $Tmp | Out-Null
try {
    $ZipPath = Join-Path $Tmp $ZipName
    Write-Host "Lade $Url"
    Invoke-WebRequest -Uri $Url -OutFile $ZipPath

    $actual = (Get-FileHash -Algorithm SHA256 -Path $ZipPath).Hash.ToLowerInvariant()
    if ($actual -ne $ZipSha256) {
        throw "SHA-256 stimmt nicht: $actual (erwartet $ZipSha256)"
    }

    Expand-Archive -Path $ZipPath -DestinationPath $Tmp
    $Src = Join-Path $Tmp "onnxruntime-win-x64-$OrtVersion\lib\onnxruntime.dll"
    if (-not (Test-Path $Src)) {
        throw "onnxruntime.dll fehlt im Archiv"
    }

    New-Item -ItemType Directory -Path $Dest -Force | Out-Null
    Copy-Item -Path $Src -Destination (Join-Path $Dest "onnxruntime.dll") -Force

    foreach ($profile in @("debug", "release")) {
        $stage = Join-Path $Root "target\$profile\lib"
        New-Item -ItemType Directory -Path $stage -Force | Out-Null
        Copy-Item -Path $Src -Destination (Join-Path $stage "onnxruntime.dll") -Force
    }

    Write-Host "OK: $(Join-Path $Dest 'onnxruntime.dll')"
}
finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
