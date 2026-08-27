<#
.SYNOPSIS
    Baut das Windows-Release-Bundle, das Zip und die Setup-Exe (Spec §11).

.DESCRIPTION
    Windows-Gegenstück zu scripts/release.sh:

        dist\diktier-<version>-win-x64\
          diktier.exe             # cargo build --release --locked
          lib\onnxruntime.dll     # aus lib\, siehe scripts\fetch-ort.ps1
          LICENSES\               # MIT (App), CC-BY-4.0 + NOTICE (Modell),
                                  # ONNX-Runtime-MIT, THIRD-PARTY.md
          versions.toml           # App, ORT-ABI + SHA-256, Crate-Pins, Modell
          README.md               # Kurzanleitung (Kopie der Repo-README)
        dist\diktier-<version>-win-x64.zip
        dist\Diktier_<version>_x64-setup.exe   # installer\diktier.nsi

    Kein PATH, kein System-ORT: die DLL wird über `ort::init_from` relativ zur
    Exe aus `lib\` geladen (§11). Idempotent — das Zielverzeichnis wird vor
    jedem Lauf neu aufgebaut.

.PARAMETER TargetDir
    Cargo-Zielverzeichnis (CARGO_TARGET_DIR). Default `target`; für Testläufe
    neben einem laufenden Daemon `target-dev`.

.PARAMETER SkipBuild
    `cargo build` überspringen; das Binary muss dann schon liegen.

.PARAMETER SkipInstaller
    Nur Bundle und Zip bauen, kein makensis.
#>
[CmdletBinding()]
param(
    [string] $TargetDir = "target",
    [switch] $SkipBuild,
    [switch] $SkipInstaller
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Die([string] $Message) {
    throw "release.ps1: $Message"
}

$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$Platform = "win-x64"
$Triple = "x86_64-pc-windows-msvc"

# --------------------------------------------------------------------- Version

$CargoToml = Join-Path $Root "Cargo.toml"
$Version = $null
$inPackage = $false
foreach ($line in Get-Content -LiteralPath $CargoToml) {
    if ($line -match '^\s*\[') { $inPackage = ($line -match '^\s*\[package\]') ; continue }
    if ($inPackage -and $line -match '^\s*version\s*=\s*"([^"]+)"') { $Version = $Matches[1]; break }
}
if (-not $Version) { Die "Version aus Cargo.toml nicht lesbar" }

$Name = "diktier-$Version-$Platform"
$Dist = Join-Path $Root "dist"
$Bundle = Join-Path $Dist $Name
$Zip = Join-Path $Dist "$Name.zip"
$Setup = Join-Path $Dist "Diktier_${Version}_x64-setup.exe"

Write-Host "== Diktier $Version ($Platform), TargetDir=$TargetDir"

# ------------------------------------------------------------------ ORT-Library

$OrtDll = Join-Path $Root "lib\onnxruntime.dll"
if (-not (Test-Path -LiteralPath $OrtDll)) {
    Die "lib\onnxruntime.dll fehlt — erst scripts\fetch-ort.ps1 laufen lassen"
}
$FetchOrt = Get-Content -LiteralPath (Join-Path $Root "scripts\fetch-ort.ps1") -Raw
$OrtVersion = if ($FetchOrt -match '\$OrtVersion\s*=\s*"([^"]+)"') { $Matches[1] } else { Die "ORT-Version nicht aus fetch-ort.ps1 lesbar" }
$OrtZipSha = if ($FetchOrt -match '\$ZipSha256\s*=\s*"([^"]+)"') { $Matches[1] } else { Die "ORT-Zip-SHA nicht aus fetch-ort.ps1 lesbar" }
$OrtDllSha = (Get-FileHash -Algorithm SHA256 -LiteralPath $OrtDll).Hash.ToLowerInvariant()

# ------------------------------------------------------------------------ Build

if ($SkipBuild) {
    Write-Host "== Build übersprungen (-SkipBuild)"
} else {
    Write-Host "== cargo build --release --locked (CARGO_TARGET_DIR=$TargetDir)"
    $env:CARGO_TARGET_DIR = $TargetDir
    & cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { Die "cargo build fehlgeschlagen ($LASTEXITCODE)" }
}
$TargetRoot = if ([System.IO.Path]::IsPathRooted($TargetDir)) { $TargetDir } else { Join-Path $Root $TargetDir }
$Exe = Join-Path $TargetRoot "release\diktier.exe"
if (-not (Test-Path -LiteralPath $Exe)) { Die "$Exe fehlt" }

# ----------------------------------------------------------------------- Bundle

Write-Host "== Bundle $Bundle"
if (Test-Path -LiteralPath $Bundle) { Remove-Item -Recurse -Force -LiteralPath $Bundle }
if (Test-Path -LiteralPath $Zip) { Remove-Item -Force -LiteralPath $Zip }
New-Item -ItemType Directory -Force -Path (Join-Path $Bundle "lib") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $Bundle "LICENSES") | Out-Null

Copy-Item -LiteralPath $Exe -Destination (Join-Path $Bundle "diktier.exe") -Force
Copy-Item -LiteralPath $OrtDll -Destination (Join-Path $Bundle "lib\onnxruntime.dll") -Force
Copy-Item -LiteralPath (Join-Path $Root "LICENSE") -Destination (Join-Path $Bundle "LICENSES\LICENSE-diktier-MIT.txt") -Force
Copy-Item -Path (Join-Path $Root "LICENSES\*") -Destination (Join-Path $Bundle "LICENSES") -Force
# Der Empfänger bekommt die Repo-README — sie ist bereits die Windows-Anleitung.
Copy-Item -LiteralPath (Join-Path $Root "README.md") -Destination (Join-Path $Bundle "README.md") -Force

# ----------------------------------------------------------------- versions.toml

$Models = Get-Content -LiteralPath (Join-Path $Root "src\models.toml") -Raw
$ModelKey = if ($Models -match '(?m)^key\s*=\s*"([^"]+)"') { $Matches[1] } else { Die "Modellschlüssel nicht lesbar" }
$ModelUrl = if ($Models -match '(?m)^url\s*=\s*"([^"]+)"') { $Matches[1] } else { Die "Modell-URL nicht lesbar" }
$ModelRepo = ($ModelUrl -replace '^(https://huggingface\.co/[^/]+/[^/]+)/resolve/.*$', '$1')
$ModelRev = ($ModelUrl -replace '^.*/resolve/([^/]+)/.*$', '$1')

# Aufgelöste Version(en) eines Pakets aus Cargo.lock. Steht ein Name mehrfach
# im Lock, werden alle ausgegeben — ein einzelner Wert wäre die falsche Wahrheit.
$LockLines = Get-Content -LiteralPath (Join-Path $Root "Cargo.lock")
function Lock-Version([string] $Crate) {
    $found = @()
    for ($i = 0; $i -lt $LockLines.Count - 1; $i++) {
        if ($LockLines[$i] -eq "name = `"$Crate`"" -and $LockLines[$i + 1] -match '^version\s*=\s*"([^"]+)"') {
            $found += $Matches[1]
        }
    }
    if ($found.Count -eq 0) { Die "Crate $Crate steht nicht in Cargo.lock" }
    if ($found.Count -eq 1) { return "`"$($found[0])`"" }
    return "[" + (($found | ForEach-Object { "`"$_`"" }) -join ", ") + "]"
}

$RustcVersion = (& rustc -V) -join ""
$CargoVersion = (& cargo -V) -join ""
$BuildHost = "$([System.Environment]::OSVersion.VersionString) ($((Get-CimInstance Win32_OperatingSystem).Caption))"

$lines = New-Object System.Collections.Generic.List[string]
$lines.Add("# Von scripts\release.ps1 erzeugt (Spec §11). Nicht von Hand pflegen.")
$lines.Add("")
$lines.Add("[app]")
$lines.Add("name = `"diktier`"")
$lines.Add("version = `"$Version`"")
$lines.Add("platform = `"$Platform`"")
$lines.Add("target = `"$Triple`"")
$lines.Add("")
$lines.Add("[onnxruntime]")
$lines.Add("# CPU-Release von microsoft/onnxruntime, geladen über scripts\fetch-ort.ps1.")
$lines.Add("# ABI: C-API 1.28 (ort-Feature api-28), Laden per ort::init_from aus lib\.")
$lines.Add("version = `"$OrtVersion`"")
$lines.Add("abi = `"api-28`"")
$lines.Add("build = `"onnxruntime-win-x64-$OrtVersion (CPU, offizielles GitHub-Release)`"")
$lines.Add("# Diese Builds setzen mindestens SSE4.2/AVX2 voraus — Haswell aufwärts (§11).")
$lines.Add("zip_sha256 = `"$OrtZipSha`"")
$lines.Add("library_sha256 = `"$OrtDllSha`"")
$lines.Add("")
$lines.Add("[model]")
$lines.Add("key = `"$ModelKey`"")
$lines.Add("repository = `"$ModelRepo`"")
$lines.Add("revision = `"$ModelRev`"")
$lines.Add("# Größen und SHA-256 der vier Artefakte: src\models.toml bzw. Spec §6.3.")
$lines.Add("")
$lines.Add("[crates]")
$lines.Add("# Aufgelöste Versionen aus Cargo.lock — die Pins stehen in Cargo.toml.")
foreach ($crate in @("parakeet-rs", "ort", "cpal", "rubato", "ureq", "rustls", "ring",
        "windows-sys", "clap", "serde", "toml", "toml_edit", "sha2", "thiserror", "hound")) {
    $lines.Add("$crate = $(Lock-Version $crate)")
}
$lines.Add("")
$lines.Add("[toolchain]")
$lines.Add("rustc = `"$RustcVersion`"")
$lines.Add("cargo = `"$CargoVersion`"")
$lines.Add("")
$lines.Add("[build_host]")
$lines.Add("os = `"$BuildHost`"")
[System.IO.File]::WriteAllLines((Join-Path $Bundle "versions.toml"), $lines, (New-Object System.Text.UTF8Encoding($false)))

# ------------------------------------------------------------------ Selbstprüfung

Write-Host "== Selbstprüfung"
foreach ($expected in @("diktier.exe", "lib\onnxruntime.dll", "versions.toml", "README.md",
        "LICENSES\LICENSE-diktier-MIT.txt", "LICENSES\CC-BY-4.0.txt",
        "LICENSES\NOTICE-parakeet.md", "LICENSES\ONNXRUNTIME-LICENSE.txt",
        "LICENSES\THIRD-PARTY.md")) {
    if (-not (Test-Path -LiteralPath (Join-Path $Bundle $expected))) { Die "Bundle unvollständig: $expected" }
}

# --------------------------------------------------------------------------- Zip

Write-Host "== Zip $Zip"
Compress-Archive -Path $Bundle -DestinationPath $Zip -CompressionLevel Optimal -Force

# ----------------------------------------------------------------------- Setup

if ($SkipInstaller) {
    Write-Host "== Installer übersprungen (-SkipInstaller)"
} else {
    $candidates = @(
        (Join-Path $env:LOCALAPPDATA "tauri\NSIS\makensis.exe"),
        "makensis",
        (Join-Path ${env:ProgramFiles(x86)} "NSIS\makensis.exe")
    )
    $MakeNsis = $null
    foreach ($candidate in $candidates) {
        $resolved = (Get-Command $candidate -ErrorAction SilentlyContinue)
        if ($resolved) { $MakeNsis = $resolved.Source; break }
    }
    if (-not $MakeNsis) {
        Die "makensis nicht gefunden (gesucht: $($candidates -join ', ')) — NSIS 3.x installieren"
    }

    Write-Host "== makensis $MakeNsis"
    if (Test-Path -LiteralPath $Setup) { Remove-Item -Force -LiteralPath $Setup }
    $Nsi = Join-Path $Root "installer\diktier.nsi"
    & $MakeNsis "/DVERSION=$Version" "/DSRCDIR=$Bundle" "/DOUTFILE=$Setup" $Nsi
    if ($LASTEXITCODE -ne 0) { Die "makensis fehlgeschlagen ($LASTEXITCODE)" }
    if (-not (Test-Path -LiteralPath $Setup)) { Die "$Setup wurde nicht erzeugt" }
}

# -------------------------------------------------------------------- Ergebnis

function Show([string] $Label, [string] $Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return }
    $size = "{0:N1} MB" -f ((Get-Item -LiteralPath $Path).Length / 1MB)
    $sha = (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
    Write-Host ""
    Write-Host "$Label $Path"
    Write-Host "  Größe:   $size"
    Write-Host "  SHA-256: $sha"
}

Write-Host ""
Write-Host "Bundle:  $Bundle"
Show "Zip:    " $Zip
Show "Setup:  " $Setup
