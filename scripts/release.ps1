<#
.SYNOPSIS
    Baut das Windows-Release-Bundle, das Zip und die Setup-Exe (Spec §11).

.DESCRIPTION
    Legt an:

        dist\diktier-<version>-win-x64\
          diktier.exe             # cargo build --release --locked
          lib\onnxruntime.dll     # aus lib\, siehe scripts\fetch-ort.ps1
          LICENSES\               # MIT (App), CC-BY-4.0 + NOTICEs (v3, Ultra),
                                  # ONNX-Runtime-MIT, THIRD-PARTY.md
          versions.toml           # App, ORT-ABI + SHA-256, Crate-Pins, Modelle
          README.md               # Kurzanleitung (Kopie der Repo-README)
        dist\diktier-<version>-win-x64.zip
        dist\Diktier_<version>_x64-setup.exe   # installer\diktier.nsi

    Kein PATH, kein System-ORT: die DLL wird über `ort::init_from` relativ zur
    Exe aus `lib\` geladen (§11). Idempotent — das Zielverzeichnis wird vor
    jedem Lauf neu aufgebaut.

    versions.toml nennt `default_model` und je Manifestmodell einen
    `[[models]]`-Block (Schlüssel, Quelle, HF-Revision bzw. Release-Tag,
    Dateien mit Größe und SHA-256), strukturiert aus src\models.toml gelesen.
    Das Bundle-Gate liest die erzeugte Datei zurück und bricht ab, wenn sie
    vom Manifest abweicht (§11, v1.10).

    src\models.toml (vor dem Build) und die erzeugte versions.toml prüft
    zusätzlich ein echter TOML-Parser (Python ≥ 3.11, `tomllib`, über
    scripts\toml-json.py): gültiges TOML und genau dieselben Werte wie der
    Teilmengenparser. Fehlt Python, bricht das Skript ab (WP2f F6).

.PARAMETER TargetDir
    Cargo-Zielverzeichnis (CARGO_TARGET_DIR). Default `target`; für Testläufe
    neben einem laufenden Daemon `target-dev`.

.PARAMETER SkipBuild
    `cargo build` überspringen; das Binary muss dann schon liegen. Auch dann
    muss `diktier.exe --manifest-sha256` gleich dem SHA-256 von
    src\models.toml sein und `--version` die Version aus Cargo.toml melden,
    sonst bricht das Skript ab.

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

. (Join-Path $PSScriptRoot "toml-lib.ps1")

# ------------------------------------------------------------- TOML-Teilmenge
# models.toml und versions.toml verwenden nur: Kommentare, `[tabelle]`,
# `[[liste]]`, `[[liste.unter]]` und `schlüssel = "Text" | Ganzzahl |
# true/false | ["Text", …]` in einer Zeile. Alles andere bricht ab — lieber kein
# Release als ein geratener Wert.
#
# Alle Vergleiche sind case-sensitive wie in Rust (serde/toml, `download.rs`):
# `-ceq`/`-cne`/`-cmatch`, `switch -CaseSensitive`, Tabellen mit Ordinal-
# Vergleich. PowerShell vergleicht sonst ohne Groß-/Kleinschreibung und nähme
# etwa `Default_Model`, `TRUE` oder `source = "GitHub-Release"` an, die Rust
# ablehnt (Code-Review WP2, L2).

function New-TomlTable {
    return [System.Collections.Specialized.OrderedDictionary]::new([System.StringComparer]::Ordinal)
}

# Wie `download::is_safe_component`: beginnt alphanumerisch, nur ASCII-
# Alphanumerik und `.`, `-`, `_`, kein `..`. `\z` statt `$`, das in .NET auch
# vor einem abschließenden Zeilenumbruch passt.
function Test-SafeComponent([string] $Text) {
    return ($Text -cmatch '^[A-Za-z0-9][A-Za-z0-9._-]*\z') -and -not $Text.Contains("..")
}

function ConvertFrom-TomlBasicString([string] $Body) {
    return [regex]::Replace($Body, '\\(["\\])', '$1')
}

function ConvertTo-TomlString([string] $Text) {
    return '"' + ($Text -replace '\\', '\\' -replace '"', '\"') + '"'
}

function Read-TomlValue([string] $Raw, [string] $Where) {
    $s = $Raw.Trim()
    $str = '"((?:[^"\\]|\\["\\])*)"'
    if ($s -cmatch "^$str(.*)$") {
        $value = ConvertFrom-TomlBasicString $Matches[1]
        $rest = $Matches[2]
    } elseif ($s -cmatch '^(-?[0-9]+)(.*)$') {
        $value = [long] $Matches[1]
        $rest = $Matches[2]
    } elseif ($s -cmatch '^(true|false)(.*)$') {
        $value = ($Matches[1] -ceq "true")
        $rest = $Matches[2]
    } elseif ($s.StartsWith("[")) {
        $items = New-Object System.Collections.Generic.List[object]
        $rest = $s.Substring(1).TrimStart()
        while (-not $rest.StartsWith("]")) {
            if ($rest -cnotmatch "^$str\s*(,?)\s*(.*)$") { Die "$($Where): Liste nicht lesbar" }
            $items.Add((ConvertFrom-TomlBasicString $Matches[1]))
            $comma = $Matches[2]
            $rest = $Matches[3]
            if (-not $comma -and -not $rest.StartsWith("]")) { Die "$($Where): Komma fehlt in der Liste" }
        }
        $rest = $rest.Substring(1)
        $value = , $items.ToArray()
    } else {
        Die "$($Where): Wert nicht lesbar"
    }
    if ($rest -cnotmatch '^\s*(#.*)?$') { Die "$($Where): Rest hinter dem Wert" }
    return $value
}

function Read-TomlSubset([string] $Path) {
    $root = New-TomlTable
    $current = $root
    $n = 0
    foreach ($line in Get-Content -LiteralPath $Path -Encoding UTF8) {
        $n++
        $where = "$([System.IO.Path]::GetFileName($Path)):$n"
        if ($line -cmatch '^\s*(#.*)?$') { continue }
        if ($line -cmatch '^\s*\[\[\s*([A-Za-z0-9_-]+)(?:\.([A-Za-z0-9_-]+))?\s*\]\]\s*(#.*)?$') {
            $outer = $Matches[1]
            $inner = $Matches[2]
            $table = New-TomlTable
            if (-not $inner) {
                if (-not $root.Contains($outer)) { $root[$outer] = New-Object System.Collections.Generic.List[object] }
                if ($root[$outer] -isnot [System.Collections.Generic.List[object]]) { Die "$($where): $outer ist keine Liste" }
                $root[$outer].Add($table)
            } else {
                $list = $root[$outer]
                if ($list -isnot [System.Collections.Generic.List[object]] -or $list.Count -ceq 0) {
                    Die "$($where): [[$outer.$inner]] ohne vorheriges [[$outer]]"
                }
                $parent = $list[$list.Count - 1]
                if (-not $parent.Contains($inner)) { $parent[$inner] = New-Object System.Collections.Generic.List[object] }
                if ($parent[$inner] -isnot [System.Collections.Generic.List[object]]) { Die "$($where): $inner ist keine Liste" }
                $parent[$inner].Add($table)
            }
            $current = $table
            continue
        }
        if ($line -cmatch '^\s*\[\s*([A-Za-z0-9_-]+)\s*\]\s*(#.*)?$') {
            $name = $Matches[1]
            if ($root.Contains($name)) { Die "$($where): Tabelle [$name] doppelt" }
            $root[$name] = New-TomlTable
            $current = $root[$name]
            continue
        }
        if ($line -cmatch '^\s*([A-Za-z0-9_-]+)\s*=(.*)$') {
            $key = $Matches[1]
            $raw = $Matches[2]
            if ($current.Contains($key)) { Die "$($where): Schlüssel $key doppelt" }
            $current[$key] = Read-TomlValue $raw $where
            continue
        }
        Die "$($where): Zeile nicht lesbar: $line"
    }
    return $root
}

function Assert-Keys($Table, [string[]] $Allowed, [string] $Where) {
    foreach ($key in $Table.Keys) {
        if ($Allowed -cnotcontains $key) { Die "$($Where): unerwarteter Schlüssel $key" }
    }
}

function Get-Text($Table, [string] $Key, [string] $Where) {
    $value = $Table[$Key]
    if ($value -isnot [string] -or $value -ceq "") { Die "$($Where): $Key fehlt oder ist kein Text" }
    return $value
}

# ------------------------------------------------------------ Modell-Manifest

# src\models.toml strukturiert einlesen und prüfen (Spec §6.2, §6.3). Die URL
# jeder Datei muss aus der Herkunft folgen — erraten wird nichts.
function Get-ModelCatalog([string] $Path) {
    $toml = Read-TomlSubset $Path
    Assert-Keys $toml @("default_model", "models") "models.toml"
    $default = Get-Text $toml "default_model" "models.toml"
    if ($toml["models"] -isnot [System.Collections.Generic.List[object]] -or $toml["models"].Count -ceq 0) {
        Die "models.toml: keine [[models]]"
    }
    $models = New-Object System.Collections.Generic.List[object]
    foreach ($m in $toml["models"]) {
        $key = Get-Text $m "key" "models.toml [[models]]"
        $where = "models.toml $key"
        if (-not (Test-SafeComponent $key)) { Die "$($where): Schlüssel ist kein sicherer Verzeichnisname" }
        if (@($models | Where-Object { $_.Key -ceq $key }).Count -gt 0) { Die "$($where): Schlüssel doppelt" }
        Assert-Keys $m @("key", "source", "repository", "revision", "release_tag", "files") $where
        $source = Get-Text $m "source" $where
        $repo = Get-Text $m "repository" $where
        $repoParts = $repo.Split("/")
        if ($repoParts.Count -cne 2 -or -not (Test-SafeComponent $repoParts[0]) -or -not (Test-SafeComponent $repoParts[1])) { Die "$($where): repository $repo" }
        switch -CaseSensitive -Exact ($source) {
            "huggingface" {
                $origin = "revision"
                $ref = Get-Text $m "revision" $where
                if ($ref -cnotmatch '^[0-9a-f]{40}\z') { Die "$($where): revision ist kein voller Commit" }
                if ($m.Contains("release_tag")) { Die "$($where): huggingface ohne release_tag" }
                $base = "https://huggingface.co/$repo/resolve/$ref"
            }
            "github-release" {
                $origin = "release_tag"
                $ref = Get-Text $m "release_tag" $where
                if (-not (Test-SafeComponent $ref)) { Die "$($where): release_tag $ref" }
                if ($m.Contains("revision")) { Die "$($where): github-release ohne revision" }
                $base = "https://github.com/$repo/releases/download/$ref"
            }
            default { Die "$($where): unbekannte Quelle $source" }
        }
        if ($m["files"] -isnot [System.Collections.Generic.List[object]] -or $m["files"].Count -ceq 0) { Die "$($where): keine Dateien" }
        $files = New-Object System.Collections.Generic.List[object]
        foreach ($f in $m["files"]) {
            Assert-Keys $f @("name", "bytes", "sha256", "url") $where
            $name = Get-Text $f "name" $where
            # Wie Rust: sicherer Name, nicht der Abschlussmarker, keine Part-Datei.
            if (-not (Test-SafeComponent $name) -or $name -ceq "COMPLETE" -or $name.EndsWith(".part", [System.StringComparison]::Ordinal)) {
                Die "$($where): Dateiname $name ist nicht zulässig"
            }
            if (@($files | Where-Object { $_.Name -ceq $name }).Count -gt 0) { Die "$($where): Datei $name doppelt" }
            $bytes = $f["bytes"]
            if ($bytes -isnot [long] -or $bytes -le 0) { Die "$($where)/$($name): bytes" }
            $sha = Get-Text $f "sha256" $where
            if ($sha -cnotmatch '^[0-9a-f]{64}\z') { Die "$($where)/$($name): sha256" }
            $url = Get-Text $f "url" $where
            if ($url -cne "$base/$name") { Die "$($where)/$($name): url folgt nicht aus der Herkunft ($url)" }
            $files.Add([pscustomobject]@{ Name = $name; Bytes = $bytes; Sha256 = $sha })
        }
        $models.Add([pscustomobject]@{
            Key = $key; Source = $source; Repository = $repo
            OriginKey = $origin; OriginValue = $ref; Files = $files.ToArray()
        })
    }
    if (@($models | Where-Object { $_.Key -ceq $default }).Count -cne 1) { Die "models.toml: default_model $default steht nicht unter [[models]]" }
    return [pscustomobject]@{ DefaultModel = $default; Models = $models.ToArray() }
}

# Die `[[models]]`-Blöcke für versions.toml, in Manifest-Reihenfolge.
function Get-ModelBlockLines($Catalog) {
    $out = New-Object System.Collections.Generic.List[string]
    $out.Add("# Je Manifestmodell ein Block; Dateien, Größen und SHA-256 aus src\models.toml")
    $out.Add("# (Spec §6.3). Wählbar über engine.model, Default steht in default_model.")
    foreach ($m in $Catalog.Models) {
        $out.Add("[[models]]")
        $out.Add("key = $(ConvertTo-TomlString $m.Key)")
        $out.Add("source = $(ConvertTo-TomlString $m.Source)")
        $out.Add("repository = $(ConvertTo-TomlString $m.Repository)")
        $out.Add("$($m.OriginKey) = $(ConvertTo-TomlString $m.OriginValue)")
        foreach ($f in $m.Files) {
            $out.Add("")
            $out.Add("[[models.files]]")
            $out.Add("name = $(ConvertTo-TomlString $f.Name)")
            $out.Add("bytes = $($f.Bytes)")
            $out.Add("sha256 = $(ConvertTo-TomlString $f.Sha256)")
        }
        $out.Add("")
    }
    return , $out.ToArray()
}

# Anzahl Einträge für Meldungen. `@($liste)` scheitert an einer List[object]
# mit Dictionaries („Argument types do not match").
function Get-EntryCount($Value) {
    if ($null -ceq $Value) { return 0 }
    if ($Value -is [System.Collections.ICollection]) { return $Value.Count }
    return 1
}

# Bundle-Gate (§11): die erzeugte versions.toml zurücklesen und Feld für Feld
# gegen das Manifest prüfen. Jede Abweichung bricht das Release ab.
function Assert-VersionsMatchManifest([string] $Path, $Catalog, [string] $AppVersion) {
    $back = Read-TomlSubset $Path
    $where = "Bundle-Gate"
    if ($null -ceq $back["app"] -or $back["app"]["version"] -cne $AppVersion) { Die "$($where): [app].version ist nicht $AppVersion" }
    if ($back["default_model"] -cne $Catalog.DefaultModel) {
        Die "$($where): default_model $($back["default_model"]) statt $($Catalog.DefaultModel)"
    }
    if ($back.Contains("model")) { Die "$($where): alter [model]-Block" }
    $blocks = $back["models"]
    if ($blocks -isnot [System.Collections.Generic.List[object]] -or $blocks.Count -cne $Catalog.Models.Count) {
        Die "$($where): $(Get-EntryCount $blocks) [[models]]-Blöcke statt $($Catalog.Models.Count)"
    }
    foreach ($m in $Catalog.Models) {
        $found = @($blocks | Where-Object { $_["key"] -ceq $m.Key })
        if ($found.Count -cne 1) { Die "$($where): $($m.Key) steht $($found.Count)-mal in versions.toml" }
        $b = $found[0]
        $w = "$where $($m.Key)"
        Assert-Keys $b @("key", "source", "repository", $m.OriginKey, "files") $w
        if ($b["source"] -cne $m.Source) { Die "$($w): source $($b["source"]) statt $($m.Source)" }
        if ($b["repository"] -cne $m.Repository) { Die "$($w): repository $($b["repository"]) statt $($m.Repository)" }
        if ($b[$m.OriginKey] -cne $m.OriginValue) { Die "$($w): $($m.OriginKey) $($b[$m.OriginKey]) statt $($m.OriginValue)" }
        $files = $b["files"]
        if ($files -isnot [System.Collections.Generic.List[object]] -or $files.Count -cne $m.Files.Count) {
            Die "$($w): $(Get-EntryCount $files) Dateien statt $($m.Files.Count)"
        }
        foreach ($f in $m.Files) {
            $hit = @($files | Where-Object { $_["name"] -ceq $f.Name })
            if ($hit.Count -cne 1) { Die "$($w): Datei $($f.Name) steht $($hit.Count)-mal da" }
            Assert-Keys $hit[0] @("name", "bytes", "sha256") "$w/$($f.Name)"
            if ($hit[0]["bytes"] -cne $f.Bytes) { Die "$($w)/$($f.Name): bytes $($hit[0]["bytes"]) statt $($f.Bytes)" }
            if ($hit[0]["sha256"] -cne $f.Sha256) { Die "$($w)/$($f.Name): sha256 weicht ab" }
        }
    }
}

# Unabhängige Gegenprüfung mit einem echten TOML-Parser (WP2f F6): Die Datei
# muss laut tomllib gültig sein, und tomllib muss genau dasselbe lesen wie der
# Teilmengenparser oben. Etwa `bytes = 0700507227` nähme die Teilmenge als
# 700507227 an, TOML verbietet die führende Null. Ohne Python ≥ 3.11 bricht
# das Release ab — kein stiller Verzicht.
function Assert-RealTomlAgrees([string] $Path) {
    $name = [System.IO.Path]::GetFileName($Path)
    $real = Invoke-RealToml $Path
    if ($real.Status -ceq "unavailable") { Die "echte TOML-Prüfung von $name nicht möglich: $($real.Message). Python 3.11 oder neuer installieren (tomllib)." }
    if ($real.Status -ceq "invalid") { Die "$name ist laut tomllib kein gültiges TOML: $($real.Message)" }
    $diffs = @(Compare-TomlValue (Read-TomlSubset $Path) $real.Value)
    if ($diffs.Count -gt 0) { Die "$($name): Teilmengenparser und tomllib lesen verschieden: $(($diffs | Select-Object -First 5) -join '; ')" }
    return $real.Python
}

# §9 (v1.10) `--manifest-sha256`: Das Binary muss genau das Manifest aus
# src\models.toml eingebettet haben — auch bei -SkipBuild, wo ein älteres oder
# anders gebautes diktier.exe liegen kann (Code-Review WP2, W9). Dazu die
# Version aus `--version` gegen Cargo.toml.
#
# Aufruf über ProcessStartInfo mit umgeleitetem stdout und Warten aufs Ende:
# diktier.exe ist ein Windows-Subsystem-Programm, auf das `& exe` nicht wartet
# (die Pipe schließt, bevor es schreibt).
function Invoke-ExeLines([string] $ExePath, [string[]] $Arguments) {
    $psi = [System.Diagnostics.ProcessStartInfo]::new($ExePath)
    foreach ($a in $Arguments) { $psi.ArgumentList.Add($a) }
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $p = [System.Diagnostics.Process]::Start($psi)
    try {
        $out = $p.StandardOutput.ReadToEndAsync()
        $null = $p.StandardError.ReadToEndAsync()
        $p.WaitForExit()
        $lines = @($out.GetAwaiter().GetResult() -split "`r?`n" | Where-Object { $_ -ne "" })
        return [pscustomobject]@{ ExitCode = $p.ExitCode; Lines = $lines }
    } finally {
        $p.Dispose()
    }
}

function Assert-ExeMatchesSources([string] $ExePath, [string] $ModelsTomlPath, [string] $AppVersion) {
    $want = (Get-FileHash -Algorithm SHA256 -LiteralPath $ModelsTomlPath).Hash.ToLowerInvariant()
    $got = Invoke-ExeLines $ExePath @("--manifest-sha256")
    if ($got.ExitCode -cne 0) { Die "$ExePath --manifest-sha256 endete mit $($got.ExitCode) (Binary vor 0.5.0?)" }
    if ($got.Lines.Count -cne 1 -or $got.Lines[0].Trim() -cne $want) {
        Die "Manifest im Binary ($($got.Lines -join ' ')) weicht von src\models.toml ($want) ab — neu bauen, nicht -SkipBuild"
    }
    $ver = Invoke-ExeLines $ExePath @("--version")
    if ($ver.ExitCode -cne 0 -or $ver.Lines.Count -cne 1 -or $ver.Lines[0].Trim() -cne "diktier $AppVersion") {
        Die "$ExePath --version meldet '$($ver.Lines -join ' ')' statt 'diktier $AppVersion'"
    }
    return $want
}

# Per Dot-Source geladen (scripts\tests\Test-UltraScripts.ps1): nur die
# Funktionen oben bereitstellen, nichts bauen.
if ($MyInvocation.InvocationName -eq ".") { return }

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

# F6: vor dem Build, damit ein ungültiges Manifest gar nicht erst eingebettet wird.
$ModelsToml = Join-Path $Root "src\models.toml"
$TomlPython = Assert-RealTomlAgrees $ModelsToml
Write-Host "== src\models.toml: gültiges TOML laut tomllib ($TomlPython), gleich gelesen wie die Teilmenge"

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
$ManifestSha = Assert-ExeMatchesSources $Exe (Join-Path $Root "src\models.toml") $Version
Write-Host "== Manifest im Binary = src\models.toml (SHA-256 $ManifestSha), --version $Version"

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

$ModelsToml = Join-Path $Root "src\models.toml"
$Catalog = Get-ModelCatalog $ModelsToml
# Gegenprobe zum Default der Config (config::DEFAULT_MODEL); cargo test hält
# beide gleich, das Release-Skript läuft aber ohne Tests.
$ConfigRs = Get-Content -LiteralPath (Join-Path $Root "src\config.rs") -Raw
$ConfigDefault = if ($ConfigRs -match 'pub const DEFAULT_MODEL: &str = "([^"]+)";') { $Matches[1] } else { Die "DEFAULT_MODEL nicht in src\config.rs" }
if ($ConfigDefault -cne $Catalog.DefaultModel) {
    Die "default_model $($Catalog.DefaultModel) in models.toml, aber DEFAULT_MODEL $ConfigDefault in config.rs"
}
Write-Host "== Modelle: $(($Catalog.Models | ForEach-Object { $_.Key }) -join ', ') (Default $($Catalog.DefaultModel))"

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
$lines.Add("# Default für engine.model (Spec §6.2, §8).")
$lines.Add("default_model = $(ConvertTo-TomlString $Catalog.DefaultModel)")
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
foreach ($line in (Get-ModelBlockLines $Catalog)) { $lines.Add($line) }
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
$VersionsToml = Join-Path $Bundle "versions.toml"
[System.IO.File]::WriteAllLines($VersionsToml, $lines, (New-Object System.Text.UTF8Encoding($false)))

# ------------------------------------------------------------------ Bundle-Gate

Write-Host "== Bundle-Gate: versions.toml gegen src\models.toml"
Assert-VersionsMatchManifest $VersionsToml $Catalog $Version
$null = Assert-RealTomlAgrees $VersionsToml
Write-Host "   versions.toml: gültiges TOML laut tomllib, gleich gelesen wie die Teilmenge"
foreach ($m in $Catalog.Models) {
    Write-Host "   ok $($m.Key): $($m.Source) $($m.OriginKey)=$($m.OriginValue), $($m.Files.Count) Dateien"
}

# ------------------------------------------------------------------ Selbstprüfung

Write-Host "== Selbstprüfung"
foreach ($expected in @("diktier.exe", "lib\onnxruntime.dll", "versions.toml", "README.md",
        "LICENSES\LICENSE-diktier-MIT.txt", "LICENSES\CC-BY-4.0.txt",
        "LICENSES\NOTICE-parakeet.md", "LICENSES\NOTICE-parakeet-ultra.md",
        "LICENSES\ONNXRUNTIME-LICENSE.txt", "LICENSES\THIRD-PARTY.md")) {
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
