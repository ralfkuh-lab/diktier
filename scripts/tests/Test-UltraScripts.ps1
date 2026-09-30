#Requires -Version 7.0
<#
.SYNOPSIS
    Regressionssuite für die Alltagstest-Skripte (Code-Review WP2: W2–W9, L2,
    L3, Blindheit), ohne echte Modelle.

.DESCRIPTION
    Ein Fake-`diktier` (`diktier.cmd` → `fake-diktier.ps1`) liefert feste JSONL
    und Exitcodes je Modell laut `scenario.json`; dazu kleine Dummy-WAVs und
    synthetische `diktier.log`-Dateien. Alles liegt in einem eigenen
    Temp-Verzeichnis (`%TEMP%\diktier-ultra-tests-<zufall>`), das die Suite
    anlegt und am Ende löscht. Nichts unter `%LOCALAPPDATA%\diktier` oder
    `%TEMP%\diktier`; jeder Skriptaufruf bekommt `-ModelRoot` auf die Testwurzel.

    Abgedeckt:
    - W2 Auswertungswurzel: -Resolve/-Judgments/-Evaluation/-List außerhalb
      oder über eine Junction nach außen → Abbruch vor dem ersten Schreiben.
    - W3 gescheiterter Warmup (error-Zeile, Exit 1) in Vorbereitung und
      Benchmark; widersprüchlicher Exitcode bricht beide ab.
    - W4 Inventur: zwei --foreground-Neustarts, Daemon/Foreground gemischt,
      abgeschnittener Loganfang, WAV-Zeit gegen Gate-Zeit an den Grenzen.
    - W5 Kriterium 2: gemeinsamer Fehler, gemeinsamer Stichprobenfehler,
      Wiederholung, echtes exklusives Veto, K5, Schemaprüfung.
    - W6 »Herr Präsident« und »Ich« getrennt.
    - W7 verbindlich/explorativ: kurzer, laufender, verkehrter Zeitraum,
      fremde Namen außerhalb, WAVs ohne Zeitstempel.
    - W8 Kriterium 4: Protokoll, Provenienz, fehlender Peak.
    - W9/L2 release.ps1: Manifest-Abgleich des Binaries, case-sensitive
      Manifestprüfung, COMPLETE/.part.
    - Blindheit: Audio nur über neutrale Aliasse.
    - W2/L3 Vergleichsseite in headless Chrome/Edge (falls vorhanden):
      Urteilsdatei, nur Codes im Browser-Speicher, sichtbare Fehler.
    - WP2f (Nachreview Sol): F1 Thread-Provenienz aus der Config, die das Kind
      liest (APPDATA ≠ LOCALAPPDATA, widersprüchliche Werte); F2 Speichern
      erst nach close, verzögertes close, zweite Änderung, Schreibfehler;
      F3 Reparse-/Hardlink-Schutz je Schreibziel; F4 »Unterschied nur
      Zahlenformat« im Schema 3; F5 Dauer aus den Grenzen, Zeitumstellung,
      Verlängerung nur mit 7-Tage-Mengenstand; F6 echte TOML-Prüfung.

    Aufruf: `pwsh -File scripts\tests\Test-UltraScripts.ps1` (Exit ≠ 0 bei
    einem Fehler). `-KeepTemp` lässt das Temp-Verzeichnis zur Analyse stehen,
    `-SkipBrowser` überspringt den Browser-Teil.
#>
[CmdletBinding()]
param(
    [switch] $KeepTemp,
    [switch] $SkipBrowser,
    # Nur Tests, deren Name auf dieses Muster passt (Entwicklung); die übrigen zählen nicht.
    [string] $Only
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$Scripts = Split-Path -Parent $PSScriptRoot
$Repo = Split-Path -Parent $Scripts
. (Join-Path $Scripts "ultra-test-lib.ps1")
. (Join-Path $Scripts "release.ps1")
$ErrorActionPreference = "Stop"

$Temp = Join-Path ([System.IO.Path]::GetTempPath()) ("diktier-ultra-tests-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Path $Temp | Out-Null
$Pwsh = (Get-Process -Id $PID).Path
$Results = New-Object System.Collections.Generic.List[object]
$Skipped = New-Object System.Collections.Generic.List[string]
$Utc8 = [System.Text.UTF8Encoding]::new($false)
# Ausgaben der Kindprozesse (pwsh, Browser) als UTF-8 lesen; am Ende zurück.
$SavedOutputEncoding = [Console]::OutputEncoding
[Console]::OutputEncoding = $Utc8

# ------------------------------------------------------------- Mini-Harness

function Test-Case([string] $Name, [scriptblock] $Body) {
    if ($Only -and $Name -notmatch $Only) { return }
    try {
        & $Body
        $Results.Add([pscustomobject]@{ Name = $Name; Ok = $true; Error = $null })
        Write-Host "  ok    $Name"
    } catch {
        $Results.Add([pscustomobject]@{ Name = $Name; Ok = $false; Error = $_.Exception.Message })
        Write-Host "  FEHL  $Name`n        $($_.Exception.Message)"
    }
}

# Sichtbar übersprungen, zählt nicht als ok.
function Skip-Case([string] $Name, [string] $Reason) {
    $Skipped.Add("$Name ($Reason)")
    Write-Host "  SKIP  $Name`n        $Reason"
}

function Assert-Eq($Actual, $Expected, [string] $What) {
    if ("$Actual" -cne "$Expected") { throw "$($What): '$Actual' statt '$Expected'" }
}

function Assert-True($Condition, [string] $What) {
    if (-not $Condition) { throw $What }
}

function Assert-Throws([scriptblock] $Body, [string] $Pattern, [string] $What) {
    $threw = $false
    try { & $Body } catch {
        $threw = $true
        if ($Pattern -and $_.Exception.Message -notmatch $Pattern) { throw "$($What): falsche Meldung: $($_.Exception.Message)" }
    }
    if (-not $threw) { throw "$($What): kein Abbruch" }
}

# Skript als eigener pwsh-Prozess: Exitcode und Ausgabe (Write-Host eingeschlossen).
function Invoke-Script([string] $Name, [string[]] $Arguments) {
    $out = & $Pwsh -NoProfile -NonInteractive -File (Join-Path $Scripts $Name) @Arguments 2>&1 | ForEach-Object { "$_" }
    return [pscustomobject]@{ Code = $LASTEXITCODE; Text = ($out -join "`n") }
}

function Assert-ScriptOk($R, [string] $What) {
    if ($R.Code -ne 0) { throw "$What endete mit $($R.Code):`n$($R.Text)" }
}

function Assert-ScriptFails($R, [string] $Pattern, [string] $What) {
    if ($R.Code -eq 0) { throw "$($What): kein Abbruch" }
    if ($Pattern -and $R.Text -notmatch $Pattern) { throw "$($What): Meldung passt nicht:`n$($R.Text)" }
}

function Read-Json([string] $Path) {
    return Get-Content -LiteralPath $Path -Raw -Encoding utf8 | ConvertFrom-Json -AsHashtable
}

function Write-Json([string] $Path, $Value) {
    [System.IO.File]::WriteAllText($Path, ($Value | ConvertTo-Json -Depth 10), $Utc8)
}

# ------------------------------------------------------------- Fake diktier

$FakeScript = @'
# Fake-diktier für scripts\tests\Test-UltraScripts.ps1: liest scenario.json
# neben sich und gibt je Listeneintrag feste JSONL-Zeilen aus.
$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$sc = Get-Content -LiteralPath (Join-Path $PSScriptRoot "scenario.json") -Raw -Encoding utf8 | ConvertFrom-Json -AsHashtable
$a = @($args)
if ($a -contains "--version") { [Console]::Out.WriteLine($sc.version); exit 0 }
if ($a -contains "--manifest-sha256") { [Console]::Out.WriteLine($sc.manifest); exit 0 }
$list = $a[[array]::IndexOf($a, "--transcribe-list") + 1]
# F1: festhalten, mit welchen Benutzerpfaden das Kind lief.
[System.IO.File]::WriteAllText((Join-Path $PSScriptRoot "env-seen.json"),
    (@{ APPDATA = $env:APPDATA; LOCALAPPDATA = $env:LOCALAPPDATA } | ConvertTo-Json), [System.Text.UTF8Encoding]::new($false))
$key = $a[[array]::IndexOf($a, "--model") + 1]
$runs = if ($a -contains "--runs") { [int] $a[[array]::IndexOf($a, "--runs") + 1] } else { 0 }
$m = $sc.models[$key]
$sb = [System.Text.StringBuilder]::new()
$anyError = $false
foreach ($f in [System.IO.File]::ReadAllLines($list)) {
    if (-not $f.Trim()) { continue }
    $base = [System.IO.Path]::GetFileNameWithoutExtension($f)
    $rule = if ($m.files.Contains($base)) { $m.files[$base] } else { $m.default }
    $ids = if ($runs -gt 0) { 1..$runs } else { @(0) }
    foreach ($r in $ids) {
        $isText = $rule.status -eq "text"
        $o = [ordered]@{ file = $f; status = $rule.status; text = $(if ($isText) { $rule.text } else { "" })
            infer_ms = $(if ($isText) { 100.0 } else { $null }); samples = 32000 }
        if ($r -gt 0) { $o.run = $r }
        if ($rule.status -eq "error") { $anyError = $true }
        [void] $sb.Append(($o | ConvertTo-Json -Compress)).Append("`n")
    }
}
[Console]::Out.Write($sb.ToString())
[Console]::Error.WriteLine("fake: $key")
exit $(if ($m.Contains("exit")) { [int] $m.exit } else { [int] $anyError })
'@

function New-Fake([string] $Name) {
    $dir = Join-Path $Temp $Name
    New-Item -ItemType Directory -Path (Join-Path $dir "lib") -Force | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $dir "fake-diktier.ps1"), $FakeScript, $Utc8)
    [System.IO.File]::WriteAllText((Join-Path $dir "diktier.cmd"),
        "@`"$Pwsh`" -NoProfile -NonInteractive -File `"%~dp0fake-diktier.ps1`" %*`r`n@exit /b %ERRORLEVEL%`r`n", [System.Text.Encoding]::ASCII)
    $dll = Join-Path $dir "lib\onnxruntime.dll"
    [System.IO.File]::WriteAllBytes($dll, [byte[]] (1..64))
    $sha = (Get-FileHash -Algorithm SHA256 -LiteralPath $dll).Hash.ToLowerInvariant()
    [System.IO.File]::WriteAllText((Join-Path $dir "versions.toml"), "[onnxruntime]`nversion = `"1.28.0`"`nlibrary_sha256 = `"$sha`"`n", $Utc8)
    return [pscustomobject]@{ Dir = $dir; Exe = (Join-Path $dir "diktier.cmd") }
}

function Set-Scenario($Fake, $V3Files, $UltraFiles, [Nullable[int]] $V3Exit = $null, [Nullable[int]] $UltraExit = $null,
    [string] $Version = "diktier 0.5.0", [string] $Manifest = "") {
    $models = [ordered]@{}
    foreach ($spec in @(@($V3Key, $V3Files, $V3Exit), @($UltraKey, $UltraFiles, $UltraExit))) {
        $m = [ordered]@{ default = @{ status = "text"; text = "Standardtext" }; files = $spec[1] }
        if ($null -ne $spec[2]) { $m.exit = $spec[2] }
        $models[$spec[0]] = $m
    }
    Write-Json (Join-Path $Fake.Dir "scenario.json") ([ordered]@{ version = $Version; manifest = $Manifest; models = $models })
}

# ------------------------------------------------------------ Testdaten

function New-TestRoot([string] $Name) {
    $root = Join-Path $Temp $Name
    foreach ($d in @("diktier\models\$V3Key", "diktier\models\$UltraKey", "diktier\ultra-test\wav")) {
        New-Item -ItemType Directory -Path (Join-Path $root $d) -Force | Out-Null
    }
    return $root
}

function Get-WavName([datetime] $Utc, [int] $Run, [int] $Suffix = 0) {
    $n = "rec_{0}Z_lauf-{1}" -f $Utc.ToString("yyyy-MM-ddTHH-mm-ss-fff", [System.Globalization.CultureInfo]::InvariantCulture), $Run
    if ($Suffix -gt 0) { $n += "-$Suffix" }
    return "$n.wav"
}

function New-Wav([string] $Dir, [string] $Name, [string] $Content = $null) {
    $path = Join-Path $Dir $Name
    if ($null -eq $Content -or $Content -eq "") { $Content = "RIFF-dummy $Name" }
    [System.IO.File]::WriteAllBytes($path, $Utc8.GetBytes($Content))
    return $path
}

function Format-LogLine([datetime] $Utc, [string] $Message) {
    return "{0}Z [+   0.000s] INFO  {1}" -f $Utc.ToString("yyyy-MM-ddTHH:mm:ss", [System.Globalization.CultureInfo]::InvariantCulture), $Message
}

function Write-Log([string] $Root, [string[]] $Lines, [string] $Name = "diktier.log") {
    [System.IO.File]::WriteAllLines((Join-Path $Root "diktier\$Name"), $Lines, $Utc8)
}

function Get-Newest([string] $Root) {
    $eval = Join-Path $Root "diktier\ultra-test\auswertung"
    return (Get-ChildItem -LiteralPath $eval -Directory | Sort-Object Name | Select-Object -Last 1).FullName
}

# Zwei Vorbereitungen in derselben Sekunde hätten denselben Ordnernamen.
function Wait-NextSecond { Start-Sleep -Milliseconds 1100 }

<#
    Standarddatensatz: eine Daemon-Sitzung im Zeitraum, je Aufnahme WAV,
    Dump- und Gate-Zeile. Rollen (Basisname → Rolle) für Szenarien und Urteile.
#>
function New-Dataset([string] $Root, [datetime] $StartUtc) {
    $wavDir = Join-Path $Root "diktier\ultra-test\wav"
    $roles = @("f01", "f02", "f03", "f04", "f05", "f06", "s01", "s02", "s03", "hp", "ich", "rej", "dup")
    $lines = New-Object System.Collections.Generic.List[string]
    $lines.Add((Format-LogLine $StartUtc "diktier 0.5.0 startet (Daemon, Modell $UltraKey)"))
    $byRole = [ordered]@{}
    $run = 0
    $f01Content = $null
    foreach ($role in $roles) {
        $run++
        $t = $StartUtc.AddMinutes($run)
        $name = Get-WavName $t $run
        $content = if ($role -eq "dup") { $f01Content } else { "RIFF $role $run" }
        if ($role -eq "f01") { $f01Content = $content }
        $path = New-Wav $wavDir $name $content
        $lines.Add((Format-LogLine $t "DIKTIER_DEBUG_WAV: $path"))
        $lines.Add((Format-LogLine $t.AddSeconds(2) "Lauf ${run}: Gate: Sprache (Regel B1)"))
        $byRole[$role] = [System.IO.Path]::GetFileNameWithoutExtension($name)
    }
    Write-Log $Root $lines.ToArray()
    $v3 = [ordered]@{}; $ultra = [ordered]@{}
    foreach ($i in 1..6) {
        $r = "f0$i"
        $v3[$byRole[$r]] = @{ status = "text"; text = "Wir treffen uns um $i Uhr" }
        $ultra[$byRole[$r]] = @{ status = "text"; text = "Wir treffen uns um $i Uhr." }
    }
    foreach ($i in 1..3) {
        $v3[$byRole["s0$i"]] = @{ status = "text"; text = "Gleicher Text $i" }
        $ultra[$byRole["s0$i"]] = @{ status = "text"; text = "Gleicher Text $i" }
    }
    $v3[$byRole.hp] = @{ status = "text"; text = "Guten Morgen zusammen" }
    $ultra[$byRole.hp] = @{ status = "text"; text = "Herr Präsident. Guten Morgen zusammen" }
    $v3[$byRole.ich] = @{ status = "text"; text = "Öffne das Menü" }
    $ultra[$byRole.ich] = @{ status = "text"; text = "Ich öffne das Menü" }
    $v3[$byRole.rej] = @{ status = "rejected" }
    $ultra[$byRole.rej] = @{ status = "rejected" }
    $v3[$byRole.dup] = $v3[$byRole.f01]
    $ultra[$byRole.dup] = $ultra[$byRole.f01]
    return [pscustomobject]@{ Roles = $byRole; V3 = $v3; Ultra = $ultra }
}

# Verbindlicher Zeitraum: genau 7 Tage, vor einem Tag zu Ende.
$PeriodFrom = (Get-Date).Date.AddDays(-8).AddHours(8)
$PeriodTo = $PeriodFrom.AddDays(7)
$PeriodArgs = @("-From", $PeriodFrom.ToString("yyyy-MM-dd HH:mm:ss"), "-To", $PeriodTo.ToString("yyyy-MM-dd HH:mm:ss"))
$DataStart = $PeriodFrom.ToUniversalTime().AddHours(1)

# ------------------------------------------------------------- Urteile

function New-Side($Spec) {
    $cats = @(); $k5 = $false; $grave = $null
    if ($Spec) {
        if ($Spec.Contains("k")) { $cats = @($Spec.k) }
        if ($Spec.Contains("k5")) { $k5 = [bool] $Spec.k5 }
        if ($Spec.Contains("g")) { $grave = [bool] $Spec.g }
    }
    $side = [ordered]@{ kategorien = $cats; k5 = $k5 }
    if ("K1" -cin $cats) { $side.gravierend = $(if ($null -eq $grave) { $false } else { $grave }) }
    return $side
}

<#
    Urteile im Schema 3 aus Rollen: $Pairs[Rolle] = @{ win = ultra|v3|gleich|unklar;
    u = @{ k = @(...); k5; g }; v = @{...}; same = Rolle; nz = $true|$false|"omit" }.
    `nz` ist »Unterschied nur Zahlenformat« (F4); bei A/B ohne Angabe $false.
    Nicht genannte Paare: gleich ohne Kategorien. $Sample[Rolle] = @(Kategorien)
    für »fehler«.
#>
function New-Judgments([string] $EvalDir, $Data, $Pairs = @{}, $Sample = @{}, [bool] $Spoken = $false) {
    $key = Read-Json (Join-Path $EvalDir "schluessel.json")
    $roleOf = @{}; foreach ($r in $Data.Roles.Keys) { $roleOf[$Data.Roles[$r]] = $r }
    $idOf = @{}
    foreach ($p in $key.paare) { $idOf[$roleOf[[System.IO.Path]::GetFileNameWithoutExtension($p.file)]] = $p.id }
    $paare = [ordered]@{}
    foreach ($p in $key.paare) {
        $role = $roleOf[[System.IO.Path]::GetFileNameWithoutExtension($p.file)]
        $spec = if ($role -and $Pairs.Contains($role)) { $Pairs[$role] } else { @{ win = "gleich" } }
        $ultraIsA = $p.A -ceq $UltraKey
        $u = New-Side $(if ($spec.Contains("u")) { $spec.u } else { $null })
        $v = New-Side $(if ($spec.Contains("v")) { $spec.v } else { $null })
        $urteil = switch ($spec.win) {
            "ultra" { if ($ultraIsA) { "A" } else { "B" } }
            "v3" { if ($ultraIsA) { "B" } else { "A" } }
            default { $spec.win }
        }
        $j = [ordered]@{ urteil = $urteil; seiten = [ordered]@{ A = $(if ($ultraIsA) { $u } else { $v }); B = $(if ($ultraIsA) { $v } else { $u }) } }
        if ($spec.Contains("nz")) {
            if (-not ($spec.nz -is [string] -and $spec.nz -ceq "omit")) { $j.nur_zahlenformat = [bool] $spec.nz }
        } elseif ($urteil -cin @("A", "B")) {
            $j.nur_zahlenformat = $false
        }
        if ($spec.Contains("same")) { $j.gleicher_fall_wie = $(if ($spec.same -match '^P\d+$') { $spec.same } else { $idOf[$spec.same] }) }
        $j.notiz = "Notiz zu $role"
        $paare[$p.id] = $j
    }
    $stich = [ordered]@{}
    foreach ($s in $key.stichprobe) {
        $role = $roleOf[[System.IO.Path]::GetFileNameWithoutExtension($s.file)]
        $stich[$s.id] = if ($role -and $Sample.Contains($role)) { [ordered]@{ urteil = "fehler"; kategorien = @($Sample[$role]) } } else { [ordered]@{ urteil = "stimmt"; kategorien = @() } }
    }
    $doc = [ordered]@{ format = 3; auswertung = $key.auswertung; paare = $paare; stichprobe = $stich
        herr_praesident = [ordered]@{}; ich = [ordered]@{} }
    foreach ($h in @($key.herr_praesident)) { if ($h) { $doc.herr_praesident[$h.id] = @{ gesprochen = $Spoken } } }
    foreach ($h in @($key.ich)) { if ($h) { $doc.ich[$h.id] = @{ gesprochen = $Spoken } } }
    return $doc
}

function Invoke-Resolve([string] $Root, [string] $EvalDir, $Doc, [string] $Name = "urteile.json") {
    $path = Join-Path $EvalDir $Name
    Write-Json $path $Doc
    $r = Invoke-Script "compare-models.ps1" @("-Resolve", $EvalDir, "-Judgments", $path, "-ModelRoot", $Root)
    $result = $null
    if ($r.Code -eq 0) { $result = Read-Json (Join-Path $EvalDir "ergebnis.json") }
    return [pscustomobject]@{ Run = $r; Result = $result }
}

# ===================================================================== Tests

Write-Host "== Test-UltraScripts ($Temp)"
$Fake = New-Fake "fake"

# ---------------------------------------------------------- W4 Inventur
Write-Host "-- W4 Inventur"

Test-Case "W4: zwei --foreground-Neustarts werden nicht verschmolzen" {
    $root = New-TestRoot "w4a"; $wav = Join-Path $root "diktier\ultra-test\wav"
    $t0 = [datetime]::new(2026, 10, 1, 8, 0, 0, [System.DateTimeKind]::Utc)
    $lines = @()
    foreach ($s in 0..2) {
        $ts = $t0.AddHours($s)
        $kind = if ($s -eq 0) { "Daemon" } else { "--foreground" }
        $lines += Format-LogLine $ts "diktier 0.5.0 startet ($kind, Modell $UltraKey)"
        foreach ($run in 1..2) {
            $tw = $ts.AddMinutes($run)
            $lines += Format-LogLine $tw.AddSeconds(3) "Lauf ${run}: Gate: Sprache"
            # Sitzung 2, Lauf 2 hat keine WAV — sie darf nicht von einer anderen verdeckt werden.
            if (-not ($s -eq 2 -and $run -eq 2)) { New-Wav $wav (Get-WavName $tw $run) | Out-Null }
        }
    }
    Write-Log $root $lines
    $inv = Get-Inventory $wav (Join-Path $root "diktier") $null $null
    Assert-Eq $inv.Sessions.Count 3 "Sitzungen"
    Assert-Eq $inv.Expected 6 "erwartet"
    Assert-Eq $inv.Present 5 "vorhanden"
    Assert-Eq $inv.Missing.Count 1 "fehlend"
    Assert-True ($inv.Missing[0].Sitzung -like "*--foreground*") "fehlender Lauf gehört zur Foreground-Sitzung: $($inv.Missing[0].Sitzung)"
    Assert-Eq $inv.Missing[0].Lauf 2 "fehlender Lauf"
    Assert-Eq $inv.WavsWithoutLog 0 "ohne Logeintrag"
}

Test-Case "W4: gemischte Starts, Dump-Zeile ordnet ereignisbezogen zu" {
    $root = New-TestRoot "w4b"; $wav = Join-Path $root "diktier\ultra-test\wav"
    $t0 = [datetime]::new(2026, 10, 2, 8, 0, 0, [System.DateTimeKind]::Utc)
    $a = New-Wav $wav (Get-WavName $t0.AddMinutes(1) 1)
    # Die WAV der Foreground-Sitzung trägt eine Zeit VOR deren Startzeile
    # (Uhr verstellt); nur die Dump-Zeile ordnet sie richtig zu.
    $b = New-Wav $wav (Get-WavName $t0.AddMinutes(5) 1 2)
    $lines = @(
        (Format-LogLine $t0 "diktier 0.5.0 startet (Daemon, Modell $UltraKey)"),
        (Format-LogLine $t0.AddMinutes(1) "DIKTIER_DEBUG_WAV: $a"),
        (Format-LogLine $t0.AddMinutes(1).AddSeconds(2) "Lauf 1: Gate: Sprache"),
        (Format-LogLine $t0.AddMinutes(10) "diktier 0.5.0 startet (--foreground, Modell $UltraKey)"),
        (Format-LogLine $t0.AddMinutes(11) "DIKTIER_DEBUG_WAV: $b"),
        (Format-LogLine $t0.AddMinutes(11).AddSeconds(2) "Lauf 1: Gate: Sprache")
    )
    Write-Log $root $lines
    $inv = Get-Inventory $wav (Join-Path $root "diktier") $null $null
    Assert-Eq $inv.Expected 2 "erwartet"
    Assert-Eq $inv.Present 2 "vorhanden"
    Assert-Eq $inv.WavsWithSuffix 1 "Namenssuffix"
    Assert-Eq $inv.AmbiguousWavs 0 "mehrdeutig"
}

Test-Case "W4: abgeschnittener Loganfang ist mehrdeutig, nicht fehlend" {
    $root = New-TestRoot "w4c"; $wav = Join-Path $root "diktier\ultra-test\wav"
    $t0 = [datetime]::new(2026, 10, 3, 8, 0, 0, [System.DateTimeKind]::Utc)
    New-Wav $wav (Get-WavName $t0.AddMinutes(1) 41) | Out-Null
    $dumped = New-Wav $wav (Get-WavName $t0.AddMinutes(3) 43)
    New-Wav $wav (Get-WavName $t0.AddMinutes(30) 1) | Out-Null
    # diktier.log.1 beginnt ohne Startzeile (Rotation).
    Write-Log $root @(
        (Format-LogLine $t0.AddMinutes(1).AddSeconds(2) "Lauf 41: Gate: Sprache"),
        (Format-LogLine $t0.AddMinutes(2).AddSeconds(2) "Lauf 42: Gate: Sprache"),
        (Format-LogLine $t0.AddMinutes(3) "DIKTIER_DEBUG_WAV: $dumped"),
        (Format-LogLine $t0.AddMinutes(3).AddSeconds(2) "Lauf 43: Gate: Sprache")
    ) "diktier.log.1"
    Write-Log $root @(
        (Format-LogLine $t0.AddMinutes(20) "diktier 0.5.0 startet (Daemon, Modell $UltraKey)"),
        (Format-LogLine $t0.AddMinutes(30).AddSeconds(2) "Lauf 1: Gate: Sprache")
    )
    $inv = Get-Inventory $wav (Join-Path $root "diktier") $null $null
    Assert-Eq $inv.SessionsWithoutStart 1 "Sitzungen ohne Startzeile"
    Assert-Eq $inv.Expected 2 "erwartet (43 über Dump-Zeile, 1 über Zeit)"
    Assert-Eq $inv.Present 2 "vorhanden"
    Assert-Eq $inv.Missing.Count 0 "fehlend"
    Assert-Eq $inv.AmbiguousRuns 2 "mehrdeutige Läufe (41 ohne Dump-Zeile, 42 ohne WAV)"
    Assert-Eq $inv.AmbiguousWavs 1 "mehrdeutige WAV (41)"
}

Test-Case "W4/W7: WAV-Zeit gegen Gate-Zeit an den Grenzen" {
    $root = New-TestRoot "w4d"; $wav = Join-Path $root "diktier\ultra-test\wav"
    $from = [datetime]::new(2026, 10, 4, 8, 0, 0, [System.DateTimeKind]::Utc)
    $to = $from.AddHours(2)
    $lines = @((Format-LogLine $from.AddHours(-1) "diktier 0.5.0 startet (Daemon, Modell $UltraKey)"))
    # 1: Aufnahme endet vor From, Gate danach → gehört nicht dazu.
    New-Wav $wav (Get-WavName $from.AddSeconds(-1) 1) | Out-Null
    $lines += Format-LogLine $from.AddSeconds(3) "Lauf 1: Gate: Sprache"
    # 2: endet vor To, Gate danach → gehört dazu.
    New-Wav $wav (Get-WavName $to.AddSeconds(-1) 2) | Out-Null
    $lines += Format-LogLine $to.AddSeconds(3) "Lauf 2: Gate: Sprache"
    # 3: ohne WAV, Gate kurz nach From → fehlend und grenznah.
    $lines += Format-LogLine $from.AddSeconds(30) "Lauf 3: Gate: Sprache"
    # 4: ohne WAV, mitten im Zeitraum → fehlend.
    $lines += Format-LogLine $from.AddHours(1) "Lauf 4: Gate: Sprache"
    # Fremde Namen: einer ohne Zeitstempel, einer mit unmöglichem Datum, einer außerhalb.
    New-Wav $wav "aufnahme.wav" | Out-Null
    New-Wav $wav "rec_2026-02-30T08-00-00-000Z_lauf-9.wav" | Out-Null
    New-Wav $wav (Get-WavName $from.AddDays(-3) 7) | Out-Null
    Write-Log $root $lines
    $inv = Get-Inventory $wav (Join-Path $root "diktier") $from $to
    Assert-Eq $inv.Wavs.Count 1 "WAVs im Zeitraum"
    Assert-True ($inv.Wavs[0] -like "*lauf-2.wav") "richtige WAV im Zeitraum"
    Assert-Eq $inv.Expected 3 "erwartet (2 mit WAV, 3 und 4 ohne)"
    Assert-Eq $inv.Present 1 "vorhanden"
    Assert-Eq $inv.Missing.Count 2 "fehlend"
    Assert-Eq $inv.MissingNearBoundary 1 "grenznah"
    Assert-Eq $inv.WavsWithoutTimestamp.Count 2 "ohne Zeitstempel"
    Assert-Eq $inv.WavsOutsidePeriod 2 "außerhalb (Lauf 1 und die alte)"
}

# ------------------------------------------------ W7 verbindlich/explorativ
Write-Host "-- W7 Zeitraum"

$RootMain = New-TestRoot "main"
$Data = New-Dataset $RootMain $DataStart
# Fremde Dateien im Ring: alte Aufnahme vor dem Zeitraum, Name ohne Zeitstempel.
New-Wav (Join-Path $RootMain "diktier\ultra-test\wav") (Get-WavName $PeriodFrom.ToUniversalTime().AddDays(-2) 99) | Out-Null
New-Wav (Join-Path $RootMain "diktier\ultra-test\wav") "umbenannt.wav" | Out-Null
Set-Scenario $Fake $Data.V3 $Data.Ultra
$CommonPrep = @("-Prepare", "-Exe", $Fake.Exe, "-ModelRoot", $RootMain, "-Seed", "42")

Test-Case "W7: laufender Zeitraum wird verbindlich abgelehnt, nichts angelegt" {
    $from = (Get-Date).AddDays(-3); $to = $from.AddDays(7)
    $r = Invoke-Script "compare-models.ps1" ($CommonPrep + @("-From", $from.ToString("yyyy-MM-dd HH:mm:ss"), "-To", $to.ToString("yyyy-MM-dd HH:mm:ss")))
    Assert-ScriptFails $r "läuft noch" "laufender Zeitraum"
    Assert-True (-not (Test-Path (Join-Path $RootMain "diktier\ultra-test\auswertung"))) "Auswertungsordner angelegt"
}

Test-Case "W7: kurzer Zeitraum wird verbindlich abgelehnt" {
    $to = $PeriodFrom.AddDays(3)
    $r = Invoke-Script "compare-models.ps1" ($CommonPrep + @("-From", $PeriodFrom.ToString("yyyy-MM-dd HH:mm:ss"), "-To", $to.ToString("yyyy-MM-dd HH:mm:ss")))
    Assert-ScriptFails $r "genau 7 Tage" "kurzer Zeitraum"
}

Test-Case "W7: verkehrter Zeitraum wird in beiden Modi abgelehnt" {
    $r = Invoke-Script "compare-models.ps1" ($CommonPrep + @("-From", $PeriodTo.ToString("yyyy-MM-dd HH:mm:ss"), "-To", $PeriodFrom.ToString("yyyy-MM-dd HH:mm:ss")))
    Assert-ScriptFails $r "verkehrt" "verbindlich verkehrt"
    $r = Invoke-Script "compare-models.ps1" ($CommonPrep + @("-Explorativ", "-From", $PeriodTo.ToString("yyyy-MM-dd HH:mm:ss"), "-To", $PeriodFrom.ToString("yyyy-MM-dd HH:mm:ss")))
    Assert-ScriptFails $r "verkehrt" "explorativ verkehrt"
}

Test-Case "W7: verbindlich ohne -From/-To ist ein Bedienfehler" {
    $r = Invoke-Script "compare-models.ps1" ($CommonPrep + @("-From", $PeriodFrom.ToString("yyyy-MM-dd HH:mm:ss")))
    Assert-ScriptFails $r "" "ohne -To"
}

$EvalMain = $null
Test-Case "W7/Blindheit: verbindliche Vorbereitung mit Aliassen" {
    $r = Invoke-Script "compare-models.ps1" ($CommonPrep + $PeriodArgs)
    Assert-ScriptOk $r "Prepare"
    $script:EvalMain = Get-Newest $RootMain
    $prep = Read-Json (Join-Path $EvalMain "vorbereitung.json")
    Assert-Eq $prep.modus "verbindlich" "Modus"
    Assert-Eq $prep.zeitraum.tage 7 "Tage"
    Assert-Eq $prep.inventur.wavs_ohne_zeitstempel 1 "ohne Zeitstempel"
    Assert-Eq $prep.inventur.wavs_ausserhalb 1 "außerhalb"
    Assert-Eq $prep.dateien 13 "ausgewertete Dateien (fremde nicht dabei)"
    $list = Get-Content -LiteralPath (Join-Path $EvalMain "liste.txt")
    Assert-True (-not ($list -match "umbenannt|lauf-99")) "fremde Dateien in der Liste"
    Assert-Eq $prep.verschieden 8 "verschiedene Paare"
    Assert-Eq $prep.herr_praesident 1 "Herr Präsident"
    Assert-Eq $prep.ich 1 "Ich"
    Assert-Eq $prep.ausschluesse["Duplikat (bytegleiche WAV)"] 1 "Duplikat"
    # Blindheit: keine Originalnamen, Laufnummern, Zeitstempel oder Modellnamen in der Seite.
    $html = Get-Content -LiteralPath (Join-Path $EvalMain "vergleich.html") -Raw -Encoding utf8
    foreach ($bad in @("rec_", "lauf-", $V3Key, $UltraKey, "file:///")) {
        Assert-True (-not $html.Contains($bad)) "vergleich.html enthält $bad"
    }
    Assert-True ($html.Contains("audio/P001.wav")) "Alias audio/P001.wav fehlt"
    $key = Read-Json (Join-Path $EvalMain "schluessel.json")
    Assert-Eq $key.format 3 "Schlüsselformat"
    Assert-Eq $key.audio.Count (8 + 3 + 1 + 1) "Anzahl Aliasse"
    foreach ($alias in $key.audio.Keys) {
        $file = Join-Path $EvalMain "audio\$alias"
        Assert-True (Test-Path -LiteralPath $file) "Alias $alias fehlt"
        Assert-Eq (Get-FileHash -LiteralPath $file).Hash (Get-FileHash -LiteralPath $key.audio[$alias].datei).Hash "Alias $alias gleich Original"
    }
}
Wait-NextSecond

$EvalExplore = $null
Test-Case "W7: explorativ ergibt nie ein Urteil" {
    $r = Invoke-Script "compare-models.ps1" ($CommonPrep + @("-Explorativ"))
    Assert-ScriptOk $r "Prepare explorativ"
    $script:EvalExplore = Get-Newest $RootMain
    $res = Invoke-Resolve $RootMain $EvalExplore (New-Judgments $EvalExplore $Data)
    Assert-ScriptOk $res.Run "Resolve explorativ"
    Assert-Eq $res.Result.verbindlich $false "verbindlich"
    foreach ($k in @("kriterium_1", "kriterium_2", "kriterium_3", "kriterium_6")) {
        Assert-Eq $res.Result.kriterien[$k] "explorativ, kein Urteil" $k
    }
    $z = Get-Content -LiteralPath (Join-Path $EvalExplore "zusammenfassung.md") -Raw -Encoding utf8
    Assert-True ($z.Contains("Explorativ, kein Urteil")) "Hinweis in der Zusammenfassung"
}
Wait-NextSecond

Test-Case "W7: nachträglich veränderter Zeitraum macht Resolve explorativ" {
    $copy = Join-Path (Split-Path $EvalMain) "manipuliert"
    Copy-Item -LiteralPath $EvalMain -Destination $copy -Recurse
    foreach ($case in @(
            @{ Label = "läuft noch"; Edit = { param($p) $p.zeitraum.bis = (Get-Date).ToUniversalTime().AddDays(1).ToString("yyyy-MM-ddTHH:mm:ssZ") } },
            @{ Label = "widersprechen den Grenzen"; Edit = { param($p) $p.zeitraum.tage = 3 } },
            # F5: Grenzen auf einen vergangenen Tag verkürzt, tage = 7 bleibt stehen.
            @{ Label = "statt 7 Tage"; Edit = { param($p) $p.zeitraum.bis = ([datetime] $p.zeitraum.von).ToUniversalTime().AddDays(1).ToString("yyyy-MM-ddTHH:mm:ssZ") } },
            @{ Label = "Zeitzone"; Edit = { param($p) $p.zeitraum.Remove("zeitzone") } },
            @{ Label = "vor Ablauf"; Edit = { param($p) $p.erstellt = $PeriodTo.ToUniversalTime().AddHours(-1).ToString("yyyy-MM-ddTHH:mm:ssZ") } })) {
        $prep = Read-Json (Join-Path $EvalMain "vorbereitung.json")
        & $case.Edit $prep
        Write-Json (Join-Path $copy "vorbereitung.json") $prep
        $res = Invoke-Resolve $RootMain $copy (New-Judgments $copy $Data)
        Assert-ScriptOk $res.Run "Resolve $($case.Label)"
        Assert-Eq $res.Result.verbindlich $false "verbindlich ($($case.Label))"
        Assert-True ($res.Result.grund_explorativ -like "*$($case.Label)*") "Grund: $($res.Result.grund_explorativ)"
    }
    Remove-Item -LiteralPath $copy -Recurse -Force
}

# ------------------------------------------------------------ W5/W6 Resolve
Write-Host "-- W5/W6 Kriterien 2 und 3"

Test-Case "W5: gemeinsamer K1-Fehler ist kein Veto und nicht exklusiv" {
    $doc = New-Judgments $EvalMain $Data @{ f01 = @{ win = "v3"; u = @{ k = @("K1"); g = $true }; v = @{ k = @("K1"); g = $false } } }
    $res = Invoke-Resolve $RootMain $EvalMain $doc
    Assert-ScriptOk $res.Run "Resolve"
    Assert-Eq $res.Result.verbindlich $true "verbindlich"
    Assert-Eq $res.Result.veto_faelle 0 "Veto"
    Assert-Eq $res.Result.kategorien.K1.ultra_exklusiv_faelle 0 "K1 Ultra-exklusiv"
    Assert-Eq $res.Result.gemeinsame_k1_ultra_gravierend 1 "gemeinsames K1 gravierend"
    Assert-Eq $res.Result.V 1 "V"
    Assert-Eq $res.Result.kriterien.kriterium_2 "erfüllt" "Kriterium 2"
}

Test-Case "W5: echtes exklusives gravierendes K1 ist ein Veto" {
    $doc = New-Judgments $EvalMain $Data @{ f05 = @{ win = "v3"; u = @{ k = @("K1"); g = $true } } }
    $res = Invoke-Resolve $RootMain $EvalMain $doc
    Assert-ScriptOk $res.Run "Resolve"
    Assert-Eq $res.Result.veto_faelle 1 "Veto"
    Assert-Eq $res.Result.kriterien.kriterium_2 "nicht erfüllt" "Kriterium 2"
}

Test-Case "W5: drei exklusive K3-Fälle ergeben eine neue Klasse, Wiederholungen zählen einmal" {
    $three = @{
        f02 = @{ win = "v3"; u = @{ k = @("K3") } }
        f03 = @{ win = "v3"; u = @{ k = @("K3") } }
        f04 = @{ win = "v3"; u = @{ k = @("K3") } }
    }
    $res = Invoke-Resolve $RootMain $EvalMain (New-Judgments $EvalMain $Data $three)
    Assert-ScriptOk $res.Run "Resolve drei Fälle"
    Assert-Eq $res.Result.kategorien.K3.ultra_exklusiv_faelle 3 "drei Fälle"
    Assert-Eq ($res.Result.neue_klassen -join ",") "K3" "neue Klasse"
    Assert-Eq $res.Result.kriterien.kriterium_2 "nicht erfüllt" "Kriterium 2 mit drei Fällen"
    $three.f03.same = "f02"; $three.f04.same = "f03"
    $res = Invoke-Resolve $RootMain $EvalMain (New-Judgments $EvalMain $Data $three)
    Assert-ScriptOk $res.Run "Resolve Wiederholung"
    Assert-Eq $res.Result.kategorien.K3.ultra_exklusiv_faelle 1 "Wiederholungen einmal"
    Assert-Eq @($res.Result.neue_klassen).Count 0 "keine neue Klasse"
    Assert-Eq $res.Result.kriterien.kriterium_2 "erfüllt" "Kriterium 2 mit Wiederholung"
}

Test-Case "W5: gemeinsamer Stichprobenfehler zählt für v3" {
    $three = @{
        f02 = @{ win = "v3"; u = @{ k = @("K2") } }
        f03 = @{ win = "v3"; u = @{ k = @("K2") } }
        f04 = @{ win = "v3"; u = @{ k = @("K2") } }
    }
    $res = Invoke-Resolve $RootMain $EvalMain (New-Judgments $EvalMain $Data $three @{ s01 = @("K2") })
    Assert-ScriptOk $res.Run "Resolve"
    Assert-Eq $res.Result.kategorien.K2.stichprobe 1 "K2 in der Stichprobe"
    Assert-Eq @($res.Result.neue_klassen).Count 0 "keine neue Klasse"
    Assert-Eq $res.Result.kriterien.kriterium_2 "erfüllt" "Kriterium 2"
    # Gegenprobe: v3 hat K2 auf einer Paarseite (gemeinsamer Fehler) → ebenfalls keine neue Klasse.
    $three.f05 = @{ win = "gleich"; u = @{ k = @("K2") }; v = @{ k = @("K2") } }
    $res = Invoke-Resolve $RootMain $EvalMain (New-Judgments $EvalMain $Data $three)
    Assert-Eq @($res.Result.neue_klassen).Count 0 "gemeinsamer Paarfehler"
}

Test-Case "W5/F4: K5 allein auf der schlechteren Seite zählt als gleich" {
    $doc = New-Judgments $EvalMain $Data @{ f06 = @{ win = "ultra"; v = @{ k5 = $true }; nz = $true }; f02 = @{ win = "ultra"; v = @{ k = @("K4") } } }
    $res = Invoke-Resolve $RootMain $EvalMain $doc
    Assert-ScriptOk $res.Run "Resolve"
    Assert-Eq $res.Result.urteile.K5_als_gleich 1 "K5 als gleich"
    Assert-Eq $res.Result.U 1 "U"
    Assert-Eq $res.Result.urteile.paare_mit_k5 1 "Paare mit K5"
    # Ohne die Angabe »nur Zahlenformat« ist derselbe Fall ein Widerspruch.
    $res = Invoke-Resolve $RootMain $EvalMain (New-Judgments $EvalMain $Data @{ f06 = @{ win = "ultra"; v = @{ k5 = $true }; nz = $false } })
    Assert-ScriptFails $res.Run "keine Kategorie K1–K4" "K5 allein ohne »nur Zahlenformat«"
}

Test-Case "F4: gemeinsamer K4, Unterschied nur Zahlenformat zählt als gleich" {
    # Sols Szenario: beide Seiten K4, Ultra gewählt wegen der Ziffern, K5 auf v3.
    $pairs = @{ f01 = @{ win = "ultra"; u = @{ k = @("K4") }; v = @{ k = @("K4"); k5 = $true }; nz = $true } }
    $res = Invoke-Resolve $RootMain $EvalMain (New-Judgments $EvalMain $Data $pairs)
    Assert-ScriptOk $res.Run "Resolve"
    Assert-Eq $res.Result.U 0 "U (kein Erkennungsgewinn)"
    Assert-Eq $res.Result.entscheidbar 0 "entscheidbar"
    Assert-Eq $res.Result.urteile.K5_als_gleich 1 "als gleich"
    # Gegenprobe: dieselben Kategorien, aber Ralf sagt »nicht nur Zahlenformat«.
    $pairs.f01.nz = $false
    $res = Invoke-Resolve $RootMain $EvalMain (New-Judgments $EvalMain $Data $pairs)
    Assert-ScriptOk $res.Run "Resolve Gegenprobe"
    Assert-Eq $res.Result.U 1 "U mit echtem Unterschied"
    Assert-Eq $res.Result.urteile.K5_als_gleich 0 "nicht als gleich"
    $z = Get-Content -LiteralPath (Join-Path $EvalMain "zusammenfassung.md") -Raw -Encoding utf8
    Assert-True ($z.Contains("Unterschied nur Zahlenformat")) "Zeile in der Zusammenfassung"
}

Test-Case "F4: Schemaprüfung für »Unterschied nur Zahlenformat«" {
    foreach ($bad in @(
            @{ Label = "A/B ohne Angabe"; Pairs = @{ f01 = @{ win = "ultra"; v = @{ k = @("K4") }; nz = "omit" } }; Pattern = "ohne Angabe" },
            @{ Label = "ja, aber verschiedene Kategorien"; Pairs = @{ f01 = @{ win = "ultra"; u = @{ k = @("K4") }; v = @{ k = @("K3"); k5 = $true }; nz = $true } }; Pattern = "unterscheiden sich" },
            @{ Label = "ja, aber kein K5 auf der schlechteren Seite"; Pairs = @{ f01 = @{ win = "ultra"; u = @{ k = @("K4") }; v = @{ k = @("K4") }; nz = $true } }; Pattern = "kein K5" },
            @{ Label = "Angabe bei gleich"; Pairs = @{ f01 = @{ win = "gleich"; nz = $false } }; Pattern = "nur zu A oder B" })) {
        $res = Invoke-Resolve $RootMain $EvalMain (New-Judgments $EvalMain $Data $bad.Pairs)
        Assert-ScriptFails $res.Run $bad.Pattern $bad.Label
    }
    $doc = New-Judgments $EvalMain $Data
    $doc.format = 2
    $res = Invoke-Resolve $RootMain $EvalMain $doc
    Assert-ScriptFails $res.Run "nicht Format 3" "altes Schema 2"
}

Test-Case "W5: unvollständige oder widersprüchliche Urteile lösen nicht auf" {
    foreach ($bad in @(
            @{ Label = "Sieger ohne Fehler der anderen Seite"; Pairs = @{ f01 = @{ win = "ultra" } }; Pattern = "keine Kategorie" },
            @{ Label = "gleicher Fall wie sich selbst"; Pairs = @{ f01 = @{ win = "gleich"; same = "f01" } }; Pattern = "kein anderes Paar" })) {
        $res = Invoke-Resolve $RootMain $EvalMain (New-Judgments $EvalMain $Data $bad.Pairs)
        Assert-ScriptFails $res.Run $bad.Pattern $bad.Label
    }
    $doc = New-Judgments $EvalMain $Data
    $first = @($doc.paare.Keys)[0]
    $doc.paare[$first].seiten.A.gravierend = $true
    $res = Invoke-Resolve $RootMain $EvalMain $doc
    Assert-ScriptFails $res.Run "gravierend.*ohne K1" "gravierend ohne K1"
    $doc = New-Judgments $EvalMain $Data @{ f02 = @{ win = "gleich"; same = "f03" }; f03 = @{ win = "gleich"; same = "f02" } }
    $res = Invoke-Resolve $RootMain $EvalMain $doc
    Assert-ScriptFails $res.Run "Kreis" "Kreis"
}

Test-Case "W6: »Herr Präsident« und »Ich« getrennt" {
    $res = Invoke-Resolve $RootMain $EvalMain (New-Judgments $EvalMain $Data @{} @{} $false)
    Assert-ScriptOk $res.Run "Resolve nicht gesprochen"
    Assert-Eq $res.Result.halluzinationen.herr_praesident.ultra 1 "HP Ultra"
    Assert-Eq $res.Result.halluzinationen.ich.ultra 1 "Ich Ultra"
    Assert-Eq $res.Result.halluzinationen.ich.v3 0 "Ich v3"
    Assert-Eq $res.Result.kriterium_3_teile.ich "nicht erfüllt" "Teil Ich"
    Assert-Eq $res.Result.kriterien.kriterium_3 "nicht erfüllt" "Kriterium 3"
    $res = Invoke-Resolve $RootMain $EvalMain (New-Judgments $EvalMain $Data @{} @{} $true)
    Assert-ScriptOk $res.Run "Resolve gesprochen"
    Assert-Eq $res.Result.halluzinationen.ich.gesprochen 1 "Ich gesprochen"
    Assert-True ($res.Result.kriterien.kriterium_3 -like "erfüllt*") "Kriterium 3 gesprochen: $($res.Result.kriterien.kriterium_3)"
    $z = Get-Content -LiteralPath (Join-Path $EvalMain "zusammenfassung.md") -Raw -Encoding utf8
    foreach ($text in @("Wir treffen", "Guten Morgen", "Notiz zu", ".wav", "rec_")) {
        Assert-True (-not $z.Contains($text)) "zusammenfassung.md enthält $text"
    }
}

# ------------------------------------------------------ W2 Auswertungswurzel
Write-Host "-- W2 Auswertungswurzel"

Test-Case "W2: -Judgments außerhalb der Wurzel bricht vor dem Schreiben ab" {
    $outside = Join-Path $Temp "urteile-aussen.json"
    Write-Json $outside (New-Judgments $EvalMain $Data)
    Remove-Item -LiteralPath (Join-Path $EvalMain "bericht.md") -ErrorAction SilentlyContinue
    $r = Invoke-Script "compare-models.ps1" @("-Resolve", $EvalMain, "-Judgments", $outside, "-ModelRoot", $RootMain)
    Assert-ScriptFails $r "Auswertungswurzel" "Judgments außen"
    Assert-True (-not (Test-Path (Join-Path $EvalMain "bericht.md"))) "bericht.md trotzdem geschrieben"
}

Test-Case "W2: kopierter Auswertungsordner außerhalb und Junction nach außen" {
    $copy = Join-Path $Temp "kopie-im-repo"
    Copy-Item -LiteralPath $EvalMain -Destination $copy -Recurse
    Remove-Item -LiteralPath (Join-Path $copy "bericht.md") -ErrorAction SilentlyContinue
    $r = Invoke-Script "compare-models.ps1" @("-Resolve", $copy, "-Judgments", (Join-Path $copy "urteile.json"), "-ModelRoot", $RootMain)
    Assert-ScriptFails $r "Auswertungswurzel" "Resolve außen"
    Assert-True (-not (Test-Path (Join-Path $copy "bericht.md"))) "bericht.md außen geschrieben"
    $junction = Join-Path $RootMain "diktier\ultra-test\auswertung\junction"
    New-Item -ItemType Junction -Path $junction -Target $copy | Out-Null
    try {
        Assert-True (-not (Test-UnderEvaluationRoot $junction (Get-EvaluationRoot $RootMain))) "Junction gilt als innen"
        $r = Invoke-Script "compare-models.ps1" @("-Resolve", $junction, "-Judgments", (Join-Path $junction "urteile.json"), "-ModelRoot", $RootMain)
        Assert-ScriptFails $r "Auswertungswurzel" "Resolve über Junction"
        Assert-True (-not (Test-Path (Join-Path $copy "bericht.md"))) "bericht.md über Junction geschrieben"
    } finally {
        [System.IO.Directory]::Delete($junction)
    }
    $r = Invoke-Script "bench-models.ps1" @("-Evaluation", $copy, "-Exe", $Fake.Exe, "-ModelRoot", $RootMain, "-OhneDaemonPruefung")
    Assert-ScriptFails $r "Auswertungswurzel" "bench außen"
    $r = Invoke-Script "bench-models.ps1" @("-Evaluation", $EvalMain, "-List", (Join-Path $copy "liste.txt"), "-Exe", $Fake.Exe, "-ModelRoot", $RootMain, "-OhneDaemonPruefung")
    Assert-ScriptFails $r "Auswertungswurzel" "bench -List außen"
    Assert-True (@(Get-ChildItem -LiteralPath $EvalMain -Directory -Filter "bench-*").Count -eq 0) "bench-Ordner trotz Abbruch"
}

# ---------------------------------------------------------- W3 Warmup-Fehler
Write-Host "-- W3 Warmup-Fehler"

$RootW3 = New-TestRoot "w3"
$DataW3 = New-Dataset $RootW3 $DataStart
$ultraW3 = [ordered]@{}; foreach ($k in $DataW3.Ultra.Keys) { $ultraW3[$k] = $DataW3.Ultra[$k] }
# Erste freigegebene Datei: der Warmup scheitert, alle folgenden gelingen.
$ultraW3[$DataW3.Roles.f01] = @{ status = "error" }
$EvalW3 = $null

Test-Case "W3: Vorbereitung weist den Warmup-Fehler aus" {
    Set-Scenario $Fake $DataW3.V3 $ultraW3
    $r = Invoke-Script "compare-models.ps1" @("-Prepare", "-Explorativ", "-Exe", $Fake.Exe, "-ModelRoot", $RootW3, "-Seed", "1")
    Assert-ScriptOk $r "Prepare"
    $script:EvalW3 = Get-Newest $RootW3
    $prep = Read-Json (Join-Path $EvalW3 "vorbereitung.json")
    Assert-Eq $prep.exitcodes[$UltraKey] 1 "Exit Ultra"
    Assert-Eq $prep.fehlerzeilen[$UltraKey] 1 "Fehlerzeilen Ultra"
    Assert-Eq $prep.ausschluesse["Fehler nur Ultra"] 1 "Ausschluss"
}

Test-Case "W3: Benchmark zählt den Warmup-Fehler als Ultra-exklusiv" {
    Set-Scenario $Fake $DataW3.V3 $ultraW3
    $r = Invoke-Script "bench-models.ps1" @("-Evaluation", $EvalW3, "-Exe", $Fake.Exe, "-ModelRoot", $RootW3, "-OhneDaemonPruefung")
    Assert-ScriptOk $r "bench"
    $b = Read-Json (Join-Path (Get-ChildItem -LiteralPath $EvalW3 -Directory -Filter "bench-*" | Select-Object -Last 1).FullName "bench.json")
    Assert-Eq $b.ultra_exklusive_fehlerdateien 1 "Ultra-exklusive Fehlerdateien"
    Assert-Eq $b.modelle[$UltraKey].fehlerzeilen 9 "Fehlerzeilen Ultra (3 Durchgänge × 3 Läufe)"
    Assert-Eq $b.pruefungen.keine_ultra_exklusiven_fehler $false "Prüfung"
    Assert-True ($b.kriterium_4 -like "Funktionsprobe*") "kein Urteil: $($b.kriterium_4)"
}

Test-Case "W3: Exit 1 ohne error-Zeile bricht beide Skripte ab" {
    Set-Scenario $Fake $DataW3.V3 $DataW3.Ultra -UltraExit 1
    $r = Invoke-Script "compare-models.ps1" @("-Prepare", "-Explorativ", "-Exe", $Fake.Exe, "-ModelRoot", $RootW3)
    Assert-ScriptFails $r "Exit 1 ohne Zeile mit error" "Prepare"
    $r = Invoke-Script "bench-models.ps1" @("-Evaluation", $EvalW3, "-Exe", $Fake.Exe, "-ModelRoot", $RootW3, "-OhneDaemonPruefung")
    Assert-ScriptFails $r "Exit 1 ohne Zeile mit error" "bench"
    Set-Scenario $Fake $DataW3.V3 $ultraW3 -UltraExit 0
    $r = Invoke-Script "bench-models.ps1" @("-Evaluation", $EvalW3, "-Exe", $Fake.Exe, "-ModelRoot", $RootW3, "-OhneDaemonPruefung", "-Passes", "1", "-Runs", "1")
    Assert-ScriptFails $r "Exit 0 trotz" "bench Exit 0 mit error"
}
Wait-NextSecond

# ------------------------------------------------------------ W8 Kriterium 4
Write-Host "-- W8 Kriterium 4"

Test-Case "W8: Urteil nur mit festem Protokoll und vollständigen Belegen" {
    $ok = [ordered]@{ a = $true; b = $true }
    $base = @{ Passes = 3; Runs = 3; ProtocolPasses = 3; ProtocolRuns = 3; PairedCount = 5; Checks = $ok }
    Assert-Eq (Get-Kriterium4 @base).Verdict "erfüllt" "vollständig"
    Assert-True ((Get-Kriterium4 @base -PeakMissing @("Durchgang 2 $UltraKey")).Verdict -like "nicht belegt (Peak Working Set fehlt: Durchgang 2*") "Peak fehlt"
    Assert-True ((Get-Kriterium4 @base -ProvenanceProblems @("versions.toml fehlt")).Verdict -like "nicht belegt (versions.toml*") "Provenienz"
    $short = $base.Clone(); $short.Passes = 1; $short.Runs = 1
    Assert-True ((Get-Kriterium4 @short).Verdict -like "Funktionsprobe, kein Urteil*") "Kurzprobe"
    Assert-True ((Get-Kriterium4 @base -NoDaemonCheck).Verdict -like "Funktionsprobe, kein Urteil*") "ohne Daemonprüfung"
    $bad = $base.Clone(); $bad.Checks = [ordered]@{ a = $true; b = $false }
    Assert-Eq (Get-Kriterium4 @bad).Verdict "nicht erfüllt" "Prüfung verfehlt"
}

Test-Case "W8: Provenienz prüft Version, ORT gegen versions.toml und Threads" {
    Set-Scenario $Fake $Data.V3 $Data.Ultra -Version "diktier 0.4.1"
    [System.IO.File]::WriteAllText((Join-Path $RootMain "diktier\config.toml"), "[engine]`nthreads = 4`n", $Utc8)
    $versions = Join-Path $Fake.Dir "versions.toml"
    $saved = Get-Content -LiteralPath $versions -Raw
    Remove-Item -LiteralPath $versions
    try {
        $r = Invoke-Script "bench-models.ps1" @("-Evaluation", $EvalMain, "-Exe", $Fake.Exe, "-ModelRoot", $RootMain, "-OhneDaemonPruefung", "-Passes", "1", "-Runs", "1")
        Assert-ScriptOk $r "bench"
        $b = Read-Json (Join-Path (Get-ChildItem -LiteralPath $EvalMain -Directory -Filter "bench-*" | Select-Object -Last 1).FullName "bench.json")
        $problems = $b.provenienz.probleme -join " | "
        Assert-True ($problems -like "*--version*0.4.1*") "Version: $problems"
        Assert-True ($problems -like "*versions.toml fehlt*") "versions.toml: $problems"
        Assert-True ($problems -like "*threads = 4*") "Threads: $problems"
        Assert-Eq $b.provenienz.onnxruntime "nicht belegt" "ORT"
        Assert-Eq $b.protokoll.fest $false "Protokoll"
    } finally {
        [System.IO.File]::WriteAllText($versions, $saved, $Utc8)
        Remove-Item -LiteralPath (Join-Path $RootMain "diktier\config.toml")
        Set-Scenario $Fake $Data.V3 $Data.Ultra
    }
    Start-Sleep -Milliseconds 1100
    $r = Invoke-Script "bench-models.ps1" @("-Evaluation", $EvalMain, "-Exe", $Fake.Exe, "-ModelRoot", $RootMain, "-OhneDaemonPruefung")
    Assert-ScriptOk $r "bench sauber"
    $b = Read-Json (Join-Path (Get-ChildItem -LiteralPath $EvalMain -Directory -Filter "bench-*" | Select-Object -Last 1).FullName "bench.json")
    Assert-Eq @($b.provenienz.probleme).Count 0 "keine Provenienzprobleme"
    Assert-Eq $b.provenienz.onnxruntime "gleich versions.toml" "ORT"
    Assert-Eq @($b.fehlende_belege).Count 0 "Peaks gemessen"
    Assert-Eq $b.protokoll.durchgaenge 3 "Durchgänge"
    Assert-Eq ($b.protokoll.reihenfolge -join ",") "1:$V3Key,1:$UltraKey,2:$UltraKey,2:$V3Key,3:$V3Key,3:$UltraKey" "Reihenfolge"
}

# ------------------------------------------------ F5 Dauer und Verlängerung
Write-Host "-- F5 Dauer und Verlängerung"

Test-Case "F5: Wanduhrtage über die Zeitumstellung" {
    $zone = "W. Europe Standard Time"
    $u = { param([string] $T) [datetime]::Parse($T, [System.Globalization.CultureInfo]::InvariantCulture, [System.Globalization.DateTimeStyles]::AdjustToUniversal -bor [System.Globalization.DateTimeStyles]::AssumeUniversal) }
    # Frühjahr 2026 (29.03.): Mi 08:00 MEZ bis Mi 08:00 MESZ sind 167 h, aber 7 Wanduhrtage.
    Assert-Eq (Get-WallClockDays (& $u "2026-03-25T07:00:00Z") (& $u "2026-04-01T06:00:00Z") $zone) 7 "Frühjahr"
    # Herbst 2025 (26.10.): 169 h, 7 Wanduhrtage.
    Assert-Eq (Get-WallClockDays (& $u "2025-10-22T06:00:00Z") (& $u "2025-10-29T07:00:00Z") $zone) 7 "Herbst"
    # Dieselben Grenzen in UTC gezählt sind kein ganzer Tag mehr.
    Assert-True ((Get-WallClockDays (& $u "2026-03-25T07:00:00Z") (& $u "2026-04-01T06:00:00Z") "UTC") -lt 7) "UTC-Zählung"
    Assert-Eq (Get-WallClockDays (& $u "2026-03-25T07:00:00Z") (& $u "2026-04-01T06:00:00Z") "Keine/Zone") "" "unbekannte Zone"
}

Test-Case "F5: Resolve rechnet die Dauer aus den Grenzen, Zeitumstellung ist kein Kurzlauf" {
    $copy = Join-Path (Split-Path $EvalMain) "zeitumstellung"
    Copy-Item -LiteralPath $EvalMain -Destination $copy -Recurse
    try {
        $prep = Read-Json (Join-Path $EvalMain "vorbereitung.json")
        $prep.zeitraum.von = "2026-03-25T07:00:00Z"
        $prep.zeitraum.bis = "2026-04-01T06:00:00Z"
        $prep.zeitraum.tage = 7
        $prep.zeitraum.zeitzone = "W. Europe Standard Time"
        Write-Json (Join-Path $copy "vorbereitung.json") $prep
        $res = Invoke-Resolve $RootMain $copy (New-Judgments $copy $Data)
        Assert-ScriptOk $res.Run "Resolve"
        Assert-Eq $res.Result.verbindlich $true "verbindlich trotz 167 h"
        # Dieselben UTC-Grenzen, als UTC-Wanduhr gezählt: kein 7-Tage-Zeitraum.
        $prep.zeitraum.zeitzone = "UTC"
        Write-Json (Join-Path $copy "vorbereitung.json") $prep
        $res = Invoke-Resolve $RootMain $copy (New-Judgments $copy $Data)
        Assert-Eq $res.Result.verbindlich $false "UTC-Zählung"
        Assert-True ($res.Result.grund_explorativ -like "*statt 7 Tage*") "Grund: $($res.Result.grund_explorativ)"
    } finally {
        Remove-Item -LiteralPath $copy -Recurse -Force
    }
}

# Eigene Wurzel: Zeitraum vor 13 Tagen, Grundlage 7 Tage, Verlängerung 11 Tage.
$RootF5 = New-TestRoot "f5"
$F5From = (Get-Date).Date.AddDays(-13).AddHours(8)
$DataF5 = New-Dataset $RootF5 $F5From.ToUniversalTime().AddHours(1)
$F5Fmt = { param([datetime] $T) $T.ToString("yyyy-MM-dd HH:mm:ss") }
$F5Base = $null
$F5Ext = $null
Test-Case "F5: 7-Tage-Vorbereitung hält den Mengenstand fest" {
    Set-Scenario $Fake $DataF5.V3 $DataF5.Ultra
    $r = Invoke-Script "compare-models.ps1" @("-Prepare", "-Exe", $Fake.Exe, "-ModelRoot", $RootF5, "-Seed", "5",
        "-From", (& $F5Fmt $F5From), "-To", (& $F5Fmt $F5From.AddDays(7)))
    Assert-ScriptOk $r "Prepare 7 Tage"
    $script:F5Base = Get-Newest $RootF5
    $prep = Read-Json (Join-Path $F5Base "vorbereitung.json")
    Assert-Eq $prep.format 3 "Format"
    Assert-Eq $prep.zeitraum.zeitzone ([System.TimeZoneInfo]::Local.Id) "Zeitzone"
    Assert-Eq $prep.mengenstand_7_tage.vollstaendig_mit_sprache 11 "vollständige Paare"
    Assert-Eq $prep.mengenstand_7_tage.verschieden 8 "Paare mit Unterschied"
    Assert-Eq $prep.mengenstand_7_tage.verlaengerung_zulaessig $true "Verlängerung zulässig"
    Assert-True ($r.Text -like "*Mengenstand nach 7 Tagen*") "Konsole"
}
Wait-NextSecond

function Set-BaseField([scriptblock] $Edit) {
    $path = Join-Path $F5Base "vorbereitung.json"
    $prep = Read-Json $path
    & $Edit $prep
    Write-Json $path $prep
}
$F5ExtArgs = { @("-Prepare", "-Exe", $Fake.Exe, "-ModelRoot", $RootF5, "-Seed", "6", "-Verlaengert",
        "-From", (& $F5Fmt $F5From), "-To", (& $F5Fmt $F5From.AddDays(11))) }

Test-Case "F5: Verlängerung ohne oder mit unzulässiger Grundlage wird abgelehnt" {
    $before = @(Get-ChildItem -LiteralPath (Join-Path $RootF5 "diktier\ultra-test\auswertung") -Directory).Count
    $r = Invoke-Script "compare-models.ps1" (& $F5ExtArgs)
    Assert-ScriptFails $r "verlangt -Grundlage" "ohne Grundlage"
    # Die Grundlage entstand jetzt, also nach Ende der Verlängerung: nicht vorab.
    $r = Invoke-Script "compare-models.ps1" ((& $F5ExtArgs) + @("-Grundlage", $F5Base))
    Assert-ScriptFails $r "nicht vorab" "Grundlage nachträglich"
    # Vorab (Tag 8), aber die Mindestmengen waren nach 7 Tagen erreichbar.
    $saved = [System.IO.File]::ReadAllText((Join-Path $F5Base "vorbereitung.json"))
    try {
        Set-BaseField { param($p)
            $p.erstellt = $F5From.ToUniversalTime().AddDays(8).ToString("yyyy-MM-ddTHH:mm:ssZ")
            $p.vollstaendig_mit_sprache = 300; $p.verschieden = 60
            $p.mengenstand_7_tage.vollstaendig_mit_sprache = 300; $p.mengenstand_7_tage.verschieden = 60 }
        $r = Invoke-Script "compare-models.ps1" ((& $F5ExtArgs) + @("-Grundlage", $F5Base))
        Assert-ScriptFails $r "Mindestmengen nach 7 Tagen erreichbar" "Mengen erreicht"
    } finally {
        [System.IO.File]::WriteAllText((Join-Path $F5Base "vorbereitung.json"), $saved, $Utc8)
    }
    $after = @(Get-ChildItem -LiteralPath (Join-Path $RootF5 "diktier\ultra-test\auswertung") -Directory).Count
    Assert-Eq $after $before "Auswertungsordner trotz Abbruch angelegt"
}

Test-Case "F5: zulässige Verlängerung ist verbindlich, nachträglich erreichte Mengen sperren sie" {
    # Die Grundlage wurde an Tag 8 festgehalten (vorab), die Mengen fehlen.
    Set-BaseField { param($p) $p.erstellt = $F5From.ToUniversalTime().AddDays(8).ToString("yyyy-MM-ddTHH:mm:ssZ") }
    $r = Invoke-Script "compare-models.ps1" ((& $F5ExtArgs) + @("-Grundlage", $F5Base))
    Assert-ScriptOk $r "Prepare Verlängerung"
    $script:F5Ext = Get-Newest $RootF5
    $prep = Read-Json (Join-Path $F5Ext "vorbereitung.json")
    Assert-Eq $prep.zeitraum.tage 11 "Tage"
    Assert-Eq $prep.zeitraum.verlaengert $true "verlängert"
    Assert-Eq $prep.verlaengerung.grundlage (Split-Path -Leaf $F5Base) "Grundlage"
    Assert-Eq $prep.mengenstand_7_tage "" "kein eigener 7-Tage-Stand"
    $res = Invoke-Resolve $RootF5 $F5Ext (New-Judgments $F5Ext $DataF5)
    Assert-ScriptOk $res.Run "Resolve Verlängerung"
    Assert-Eq $res.Result.verbindlich $true "verbindlich: $($res.Result.grund_explorativ)"
    # Grundlage nachträglich auf erreichte Mengen geändert: kein Urteil mehr.
    $saved = [System.IO.File]::ReadAllText((Join-Path $F5Base "vorbereitung.json"))
    try {
        Set-BaseField { param($p)
            $p.vollstaendig_mit_sprache = 300; $p.verschieden = 60
            $p.mengenstand_7_tage.vollstaendig_mit_sprache = 300; $p.mengenstand_7_tage.verschieden = 60 }
        $res = Invoke-Resolve $RootF5 $F5Ext (New-Judgments $F5Ext $DataF5)
        Assert-ScriptOk $res.Run "Resolve mit geänderter Grundlage"
        Assert-Eq $res.Result.verbindlich $false "verbindlich trotz erreichter Mengen"
        Assert-True ($res.Result.grund_explorativ -like "*Verlängerung unzulässig*Mindestmengen*") "Grund: $($res.Result.grund_explorativ)"
    } finally {
        [System.IO.File]::WriteAllText((Join-Path $F5Base "vorbereitung.json"), $saved, $Utc8)
    }
    # Verlängerung ohne Grundlage in der Vorbereitung: kein Urteil.
    $copy = Join-Path (Split-Path $F5Ext) "ohne-grundlage"
    Copy-Item -LiteralPath $F5Ext -Destination $copy -Recurse
    try {
        $p = Read-Json (Join-Path $copy "vorbereitung.json"); $p.verlaengerung = $null
        Write-Json (Join-Path $copy "vorbereitung.json") $p
        $res = Invoke-Resolve $RootF5 $copy (New-Judgments $copy $DataF5)
        Assert-ScriptOk $res.Run "Resolve ohne Grundlage"
        Assert-Eq $res.Result.verbindlich $false "ohne Grundlage verbindlich"
        Assert-True ($res.Result.grund_explorativ -like "*ohne 7-Tage-Grundlage*") "Grund: $($res.Result.grund_explorativ)"
    } finally {
        Remove-Item -LiteralPath $copy -Recurse -Force
    }
}
Wait-NextSecond

# --------------------------------------------------- F3 Schreibziele
Write-Host "-- F3 Reparse-Schutz je Schreibziel"

Test-Case "F3: Junction als innerer Pfadbestandteil wird nicht beschrieben" {
    $root = New-TestRoot "f3"
    $evalRoot = Get-EvaluationRoot $root
    $dir = Join-Path $evalRoot "20260101-000000"
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    $outside = Join-Path $Temp "f3-aussen"
    New-Item -ItemType Directory -Path $outside | Out-Null
    $junction = Join-Path $dir "sub"
    New-Item -ItemType Junction -Path $junction -Target $outside | Out-Null
    try {
        Set-WriteRoot $dir $evalRoot
        Assert-Throws { Write-Utf8Text (Join-Path $junction "x.txt") "geheim" } "Reparse-Point" "Schreiben über Junction"
        Assert-Eq @(Get-ChildItem -LiteralPath $outside).Count 0 "Datei außen angelegt"
        # Ins Leere zeigende Junction: ebenfalls kein Schreibziel.
        Remove-Item -LiteralPath $outside -Recurse -Force
        Assert-Throws { Write-Utf8Text (Join-Path $junction "y.txt") "geheim" } "Reparse-Point" "verwaiste Junction"
        Write-Utf8Text (Join-Path $dir "ok.txt") "normal"
        Assert-True (Test-Path -LiteralPath (Join-Path $dir "ok.txt")) "normales Ziel geschrieben"
    } finally {
        [System.IO.Directory]::Delete($junction)
        $script:WriteRoot = $null
    }
}

Test-Case "F3: vorhandener Hardlink nach außen bricht Resolve vor dem Schreiben ab" {
    $copy = Join-Path (Split-Path $EvalMain) "hardlink"
    Copy-Item -LiteralPath $EvalMain -Destination $copy -Recurse
    $external = Join-Path $Temp "sync-ordner-bericht.md"
    [System.IO.File]::WriteAllText($external, "extern, darf sich nicht ändern", $Utc8)
    $hashBefore = (Get-FileHash -LiteralPath $external).Hash
    try {
        Remove-Item -LiteralPath (Join-Path $copy "bericht.md") -ErrorAction SilentlyContinue
        New-Item -ItemType HardLink -Path (Join-Path $copy "bericht.md") -Target $external | Out-Null
        $ergebnisBefore = if (Test-Path (Join-Path $copy "ergebnis.json")) { (Get-FileHash -LiteralPath (Join-Path $copy "ergebnis.json")).Hash } else { "" }
        $res = Invoke-Resolve $RootMain $copy (New-Judgments $copy $Data)
        Assert-ScriptFails $res.Run "Hardlinks" "Resolve mit Hardlink"
        Assert-Eq (Get-FileHash -LiteralPath $external).Hash $hashBefore "externe Datei verändert"
        $ergebnisAfter = if (Test-Path (Join-Path $copy "ergebnis.json")) { (Get-FileHash -LiteralPath (Join-Path $copy "ergebnis.json")).Hash } else { "" }
        Assert-Eq $ergebnisAfter $ergebnisBefore "ergebnis.json trotz Abbruch geschrieben"
    } finally {
        Remove-Item -LiteralPath $copy -Recurse -Force
        Remove-Item -LiteralPath $external -Force
    }
}

$SymlinkCase = "F3: Datei-Symlink nach außen bricht Resolve vor dem Schreiben ab"
if ($EvalMain -and -not ($Only -and $SymlinkCase -notmatch $Only)) {
$copy = Join-Path (Split-Path $EvalMain) "symlink"
Copy-Item -LiteralPath $EvalMain -Destination $copy -Recurse
$external = Join-Path $Temp "sync-ordner-symlink.md"
[System.IO.File]::WriteAllText($external, "extern, darf sich nicht ändern", $Utc8)
Remove-Item -LiteralPath (Join-Path $copy "bericht.md") -ErrorAction SilentlyContinue
$symlinkError = $null
try { New-Item -ItemType SymbolicLink -Path (Join-Path $copy "bericht.md") -Target $external -ErrorAction Stop | Out-Null }
catch { $symlinkError = $_.Exception.Message }
if ($symlinkError) {
    Skip-Case $SymlinkCase "Datei-Symlink nicht anlegbar ohne Adminrechte/Entwicklermodus: $symlinkError. Abgedeckt über Junction (Pfadbestandteil) und Hardlink (Zieldatei)."
} else {
    Test-Case $SymlinkCase {
        $hashBefore = (Get-FileHash -LiteralPath $external).Hash
        $res = Invoke-Resolve $RootMain $copy (New-Judgments $copy $Data)
        Assert-ScriptFails $res.Run "Reparse-Point" "Resolve mit Symlink"
        Assert-Eq (Get-FileHash -LiteralPath $external).Hash $hashBefore "externe Datei verändert"
    }
}
Remove-Item -LiteralPath $copy -Recurse -Force
Remove-Item -LiteralPath $external -Force
}

# ------------------------------------------------------ F1 Thread-Provenienz
Write-Host "-- F1 Thread-Provenienz"

Test-Case "F1: engine.threads wie config.rs, mit echtem TOML-Parser" {
    $dir = Join-Path $Temp "f1-configs"
    New-Item -ItemType Directory -Path $dir | Out-Null
    $cases = [ordered]@{
        "fehlt"         = @($null, 0, $false)
        "leer"          = @("", 0, $false)
        "ohne engine"   = @("[audio]`nmax_duration_secs = 30`n", 0, $false)
        "ohne threads"  = @("[engine]`nmodel = `"x`"`n", 0, $false)
        "threads 0"     = @("[engine]`nthreads = 0`n", 0, $false)
        "threads 4"     = @("[engine]`nthreads = 4`n", 4, $false)
        # Der frühere Regex hätte hier 0 gemeldet.
        "gepunktet"     = @("engine.threads = 4`n", 4, $false)
        "inline"        = @("engine = { threads = 2 }`n", 2, $false)
        "negativ"       = @("[engine]`nthreads = -3`n", 0, $false)
        "Text"          = @("[engine]`nthreads = `"4`"`n", $null, $true)
        "ungültig"      = @("[engine]`nthreads = 04`n", $null, $true)
    }
    foreach ($label in $cases.Keys) {
        $text, $want, $problem = $cases[$label]
        $path = Join-Path $dir "$($label -replace '\W', '_').toml"
        if ($null -ne $text) { [System.IO.File]::WriteAllText($path, $text, $Utc8) }
        $t = Get-EffectiveThreads $path
        Assert-Eq $t.Threads $want "$label Threads"
        Assert-Eq ($null -ne $t.Problem) $problem "$label Problem ($($t.Problem))"
    }
    $saved = $script:TomlPythonCandidates
    try {
        $script:TomlPythonCandidates = @(, @("kein-python-diktier-test"))
        $t = Get-EffectiveThreads (Join-Path $dir "threads_0.toml")
        Assert-Eq $t.Threads "" "ohne Python keine Zahl"
        Assert-True ($t.Problem -like "*nicht belegt*Python*") "ohne Python: $($t.Problem)"
    } finally {
        $script:TomlPythonCandidates = $saved
    }
}

function Get-LastBench([string] $Dir) {
    return Read-Json (Join-Path (Get-ChildItem -LiteralPath $Dir -Directory -Filter "bench-*" | Sort-Object Name | Select-Object -Last 1).FullName "bench.json")
}

Test-Case "F1: mit -ModelRoot liest das Kind APPDATA der Testwurzel, nicht das echte" {
    $outerApp = Join-Path $Temp "f1-appdata-aussen"
    New-Item -ItemType Directory -Path (Join-Path $outerApp "diktier") -Force | Out-Null
    $savedApp = $env:APPDATA
    $rootConfig = Join-Path $RootMain "diktier\config.toml"
    try {
        Set-Scenario $Fake $Data.V3 $Data.Ultra
        $env:APPDATA = $outerApp
        # Außen threads = 4, in der Testwurzel keine Config: das Kind läuft mit 0.
        [System.IO.File]::WriteAllText((Join-Path $outerApp "diktier\config.toml"), "[engine]`nthreads = 4`n", $Utc8)
        $r = Invoke-Script "bench-models.ps1" @("-Evaluation", $EvalMain, "-Exe", $Fake.Exe, "-ModelRoot", $RootMain, "-OhneDaemonPruefung", "-Passes", "1", "-Runs", "1")
        Assert-ScriptOk $r "bench"
        $b = Get-LastBench $EvalMain
        Assert-Eq $b.provenienz.threads 0 "Threads"
        Assert-Eq $b.provenienz.config $rootConfig "Config-Pfad"
        Assert-True (-not (($b.provenienz.probleme -join " ") -like "*threads*")) "Thread-Problem: $($b.provenienz.probleme -join ' | ')"
        $seen = Read-Json (Join-Path $Fake.Dir "env-seen.json")
        Assert-Eq $seen.APPDATA $RootMain "APPDATA des Kindes"
        Assert-Eq $seen.LOCALAPPDATA $RootMain "LOCALAPPDATA des Kindes"
        Wait-NextSecond
        # Umgekehrt: außen 0, in der Testwurzel 4 → gemeldet.
        [System.IO.File]::WriteAllText((Join-Path $outerApp "diktier\config.toml"), "[engine]`nthreads = 0`n", $Utc8)
        [System.IO.File]::WriteAllText($rootConfig, "[engine]`nthreads = 4`n", $Utc8)
        $r = Invoke-Script "bench-models.ps1" @("-Evaluation", $EvalMain, "-Exe", $Fake.Exe, "-ModelRoot", $RootMain, "-OhneDaemonPruefung", "-Passes", "1", "-Runs", "1")
        Assert-ScriptOk $r "bench"
        $b = Get-LastBench $EvalMain
        Assert-Eq $b.provenienz.threads 4 "Threads der Testwurzel"
        Assert-True (($b.provenienz.probleme -join " ") -like "*engine.threads = 4*") "Problem: $($b.provenienz.probleme -join ' | ')"
    } finally {
        $env:APPDATA = $savedApp
        Remove-Item -LiteralPath $rootConfig -ErrorAction SilentlyContinue
    }
    Wait-NextSecond
}

Test-Case "F1: ohne -ModelRoot zählt das echte APPDATA, nicht LOCALAPPDATA" {
    $local = Join-Path $Temp "f1-local"
    $app = Join-Path $Temp "f1-roaming"
    foreach ($d in @("$local\diktier\ultra-test\auswertung", "$app\diktier")) { New-Item -ItemType Directory -Path $d -Force | Out-Null }
    $eval = Join-Path $local "diktier\ultra-test\auswertung\20260101-000000"
    Copy-Item -LiteralPath $EvalMain -Destination $eval -Recurse
    # Widersprüchlich: unter LOCALAPPDATA 0 (dort liest Rust nicht), unter APPDATA 4.
    [System.IO.File]::WriteAllText((Join-Path $local "diktier\config.toml"), "[engine]`nthreads = 0`n", $Utc8)
    [System.IO.File]::WriteAllText((Join-Path $app "diktier\config.toml"), "[engine]`nthreads = 4`n", $Utc8)
    $savedLocal = $env:LOCALAPPDATA; $savedApp = $env:APPDATA
    try {
        Set-Scenario $Fake $Data.V3 $Data.Ultra
        $env:LOCALAPPDATA = $local; $env:APPDATA = $app
        $r = Invoke-Script "bench-models.ps1" @("-Evaluation", $eval, "-Exe", $Fake.Exe, "-OhneDaemonPruefung", "-Passes", "1", "-Runs", "1")
    } finally {
        $env:LOCALAPPDATA = $savedLocal; $env:APPDATA = $savedApp
    }
    Assert-ScriptOk $r "bench"
    $b = Get-LastBench $eval
    Assert-Eq $b.provenienz.config (Join-Path $app "diktier\config.toml") "Config-Pfad"
    Assert-Eq $b.provenienz.threads 4 "Threads"
    Assert-True (($b.provenienz.probleme -join " ") -like "*engine.threads = 4*") "Problem: $($b.provenienz.probleme -join ' | ')"
    $seen = Read-Json (Join-Path $Fake.Dir "env-seen.json")
    Assert-Eq $seen.APPDATA $app "APPDATA des Kindes"
    Assert-Eq $seen.LOCALAPPDATA $local "LOCALAPPDATA des Kindes"
}

# ---------------------------------------------------------- W9/L2 release.ps1
Write-Host "-- W9/L2/F6 release.ps1"

$ModelsToml = Join-Path $Repo "src\models.toml"
$ModelsText = [System.IO.File]::ReadAllText($ModelsToml)

Test-Case "L2: das echte Manifest besteht die Prüfung" {
    $c = Get-ModelCatalog $ModelsToml
    Assert-Eq $c.Models.Count 2 "Modelle"
}

Test-Case "L2: Negativfixtures, case-sensitive wie Rust" {
    $cases = [ordered]@{
        "Default_Model"              = @("default_model =", "Default_Model =")
        "Quelle GitHub-Release"      = @('source = "github-release"', 'source = "GitHub-Release"')
        "Quelle Huggingface"         = @('source = "huggingface"', 'source = "Huggingface"')
        "URL-Host groß"              = @("https://huggingface.co/", "https://HuggingFace.co/")
        "Revision groß"              = @('revision = "8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce"', 'revision = "8F23F0C03C8761650BDB5B40AAF3E40D2C15F1CE"')
        "sha256 groß"                = @('sha256 = "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d"', 'sha256 = "D58544679EA4BC6AC563D1F545EB7D474BD6CFA467F0A6E2C1DC1C7D37E3C35D"')
        "Schlüssel KEY"              = @("key = ", "KEY = ")
    }
    foreach ($label in $cases.Keys) {
        $old, $new = $cases[$label]
        Assert-True ($ModelsText.Contains($old)) "Fixture $label passt nicht mehr zu src\models.toml"
        $path = Join-Path $Temp "models-$($label -replace '\W', '_').toml"
        [System.IO.File]::WriteAllText($path, $ModelsText.Replace($old, $new), $Utc8)
        Assert-Throws { Get-ModelCatalog $path } "" "Fixture $label"
    }
    # COMPLETE und .part als Dateiname (Rust: validate_model). Name und URL
    # konsistent umbenannt, damit nur die Namensregel greift.
    foreach ($name in @("COMPLETE", "vocab.txt.part")) {
        $text = $ModelsText.Replace('name = "vocab.txt"', "name = `"$name`"").Replace("/vocab.txt`"", "/$name`"")
        $path = Join-Path $Temp "models-name-$name.toml"
        [System.IO.File]::WriteAllText($path, $text, $Utc8)
        Assert-Throws { Get-ModelCatalog $path } "nicht zulässig" "Dateiname $name"
    }
}

Test-Case "F6: echter TOML-Parser prüft Manifest und versions.toml" {
    Assert-Eq (Assert-RealTomlAgrees $ModelsToml) "python" "echtes Manifest"
    # Führende Null: die Teilmenge liest 0700507227 als 700507227, TOML verbietet sie.
    $m = [regex]::Match($ModelsText, 'bytes = (\d+)')
    Assert-True $m.Success "bytes-Zeile im Manifest"
    $bad = Join-Path $Temp "models-fuehrende-null.toml"
    [System.IO.File]::WriteAllText($bad, $ModelsText.Insert($m.Groups[1].Index, "0"), $Utc8)
    $null = Get-ModelCatalog $bad
    Assert-Throws { Assert-RealTomlAgrees $bad } "kein gültiges TOML" "führende Null im Manifest"
    # versions.toml wie release.ps1 sie schreibt, einmal gültig, einmal mit führender Null.
    $catalog = Get-ModelCatalog $ModelsToml
    $lines = @("default_model = `"$($catalog.DefaultModel)`"", "", "[app]", "version = `"0.5.0`"", "") + (Get-ModelBlockLines $catalog)
    $versions = Join-Path $Temp "versions-probe.toml"
    [System.IO.File]::WriteAllLines($versions, $lines, $Utc8)
    Assert-VersionsMatchManifest $versions $catalog "0.5.0"
    Assert-Eq (Assert-RealTomlAgrees $versions) "python" "versions.toml gültig"
    $text = [System.IO.File]::ReadAllText($versions)
    $m = [regex]::Match($text, 'bytes = (\d+)')
    [System.IO.File]::WriteAllText($versions, $text.Insert($m.Groups[1].Index, "0"), $Utc8)
    Assert-VersionsMatchManifest $versions $catalog "0.5.0"
    Assert-Throws { Assert-RealTomlAgrees $versions } "kein gültiges TOML" "führende Null in versions.toml"
}

Test-Case "F6: ohne Python bricht die Prüfung ab, Abweichungen werden gemeldet" {
    $saved = $script:TomlPythonCandidates
    try {
        $script:TomlPythonCandidates = @(, @("kein-python-diktier-test"))
        Assert-Throws { Assert-RealTomlAgrees $ModelsToml } "nicht möglich.*Python 3.11" "ohne Python"
    } finally {
        $script:TomlPythonCandidates = $saved
    }
    $a = New-OrdinalTable; $a["key"] = "x"; $a["bytes"] = [long] 5
    $b = New-OrdinalTable; $b["key"] = "X"; $b["bytes"] = [System.Numerics.BigInteger] 6; $b["extra"] = $true
    $diffs = @(Compare-TomlValue $a $b)
    Assert-Eq $diffs.Count 3 "Abweichungen ($($diffs -join '; '))"
    $c = New-OrdinalTable; $c["key"] = "x"; $c["bytes"] = [System.Numerics.BigInteger] 5
    Assert-Eq @(Compare-TomlValue $a $c).Count 0 "gleich"
}

Test-Case "W9: Manifest-Abgleich des Binaries" {
    $sha = (Get-FileHash -Algorithm SHA256 -LiteralPath $ModelsToml).Hash.ToLowerInvariant()
    Set-Scenario $Fake @{} @{} -Manifest $sha
    Assert-Eq (Assert-ExeMatchesSources $Fake.Exe $ModelsToml "0.5.0") $sha "gleiches Manifest"
    Set-Scenario $Fake @{} @{} -Manifest ("0" * 64)
    Assert-Throws { Assert-ExeMatchesSources $Fake.Exe $ModelsToml "0.5.0" } "weicht" "anderes Manifest"
    Set-Scenario $Fake @{} @{} -Manifest $sha.ToUpperInvariant()
    Assert-Throws { Assert-ExeMatchesSources $Fake.Exe $ModelsToml "0.5.0" } "weicht" "Großbuchstaben"
    Set-Scenario $Fake @{} @{} -Manifest $sha -Version "diktier 0.4.1"
    Assert-Throws { Assert-ExeMatchesSources $Fake.Exe $ModelsToml "0.5.0" } "--version" "alte Version"
}

# -------------------------------------------------- W2/L3 Vergleichsseite
Write-Host "-- W2/L3 Vergleichsseite (Browser)"

$Browser = @("$env:ProgramFiles\Google\Chrome\Application\chrome.exe", "${env:ProgramFiles(x86)}\Google\Chrome\Application\chrome.exe",
    "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe", "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe") |
    Where-Object { $_ -and (Test-Path -LiteralPath $_) } | Select-Object -First 1

<#
    Die echte vergleich.html mit zwei eingeschobenen Skripten: $Pre läuft vor
    dem Seitenskript (Stubs), $Post danach (Bedienung). $Post schreibt sein
    Ergebnis als JSON in <pre id="testergebnis">; --dump-dom liefert es.
#>
function Invoke-Page([string] $Label, [string] $Pre, [string] $Post) {
    $html = Get-Content -LiteralPath (Join-Path $EvalMain "vergleich.html") -Raw -Encoding utf8
    $marker = '<script type="application/json" id="daten">'
    $html = $html.Replace($marker, "<script>$Pre</script>`n$marker")
    $html = $html.Replace("</body>", "<pre id=`"testergebnis`"></pre><script>$Post</script>`n</body>")
    $page = Join-Path $EvalMain "test-$Label.html"
    [System.IO.File]::WriteAllText($page, $html, $Utc8)
    $profile = Join-Path $Temp "browser-$Label"
    $url = ([System.Uri]::new($page)).AbsoluteUri
    # Start-Process mit Umleitung: `&` liefert beim GUI-Programm Chrome keine Ausgabe.
    $domFile = Join-Path $Temp "dom-$Label.html"
    $p = Start-Process -FilePath $Browser -Wait -PassThru -NoNewWindow `
        -RedirectStandardOutput $domFile -RedirectStandardError (Join-Path $Temp "dom-$Label.err.txt") `
        -ArgumentList @("--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check",
            "--user-data-dir=$profile", "--virtual-time-budget=6000", "--dump-dom", $url)
    if ($p.ExitCode -ne 0) { throw "$($Label): Browser endete mit $($p.ExitCode)" }
    $text = [System.IO.File]::ReadAllText($domFile, $Utc8)
    if ($text -notmatch '<pre id="testergebnis">([^<]*)</pre>') { throw "$($Label): kein Testergebnis im DOM" }
    return [System.Net.WebUtility]::HtmlDecode($Matches[1]) | ConvertFrom-Json -AsHashtable
}

$StubFs = @'
window.__writes = [];
window.__failWrite = false;
window.__failClose = false;
window.__holdClose = false;
window.__release = null;
window.showSaveFilePicker = async () => ({
  name: "urteile.json",
  createWritable: async () => {
    if (window.__failWrite) throw new DOMException("gesperrt", "NotAllowedError");
    let buf = "";
    return { write: async t => { buf += t; }, close: async () => {
      // F2: close() kann dauern (window.__holdClose) oder scheitern (window.__failClose,
      // gilt ab dem Beginn dieses close).
      const fail = window.__failClose;
      if (window.__holdClose) await new Promise(res => { window.__release = res; });
      if (fail) throw new DOMException("Datei gesperrt", "NoModificationAllowedError");
      window.__writes.push(buf);
    } };
  },
});
'@

$Operate = @'
(async () => {
  const click = sel => { const n = document.querySelector(sel); if (!n) throw new Error("fehlt: " + sel); n.click(); };
  const r = { errors: [] };
  try {
    click('input[name="P001-u"][value="A"]');
    await new Promise(res => setTimeout(res, 100));
    click('input[name="P001-B-K1"]');
    click('input[name="P001-B-g"][value="true"]');
    click('input[name="P001-nz"][value="false"]');
    const note = document.querySelector("#P001 textarea");
    note.value = "Geheime Notiz"; note.dispatchEvent(new Event("input"));
    await new Promise(res => setTimeout(res, 1200));
  } catch (e) { r.errors.push(String(e)); }
  let stored = null;
  try { stored = localStorage.getItem("diktier-ultra-test:" + JSON.parse(document.getElementById("daten").textContent).auswertung); } catch (e) { r.errors.push("ls " + e); }
  r.stored = stored;
  r.writes = (window.__writes || []).length;
  r.last = (window.__writes || []).slice(-1)[0] || null;
  r.messages = document.getElementById("messages").textContent;
  r.saveInfo = document.getElementById("saveInfo").textContent;
  r.downloads = document.querySelectorAll("a[download]").length;
  r.p001 = document.getElementById("P001").className;
  r.texts = JSON.parse(document.getElementById("daten").textContent).paare.map(p => [p.a, p.b]).flat();
  document.getElementById("testergebnis").textContent = JSON.stringify(r);
})();
'@

if ($SkipBrowser -or -not $Browser) {
    Write-Host "  übersprungen: $(if ($SkipBrowser) { '-SkipBrowser' } else { 'kein Chrome/Edge gefunden' })"
} else {
    Test-Case "W2/L3: Urteilsdatei per Picker, fortlaufend geschrieben, Browser nur Codes" {
        $r = Invoke-Page "normal" $StubFs $Operate
        Assert-Eq ($r.errors -join ";") "" "Bedienfehler"
        Assert-True ($r.writes -ge 2) "Datei nicht fortlaufend geschrieben ($($r.writes))"
        $doc = $r.last | ConvertFrom-Json -AsHashtable
        Assert-Eq $doc.format 3 "Format"
        Assert-Eq $doc.paare.P001.urteil "A" "Urteil in der Datei"
        Assert-Eq $doc.paare.P001.nur_zahlenformat $false "»nur Zahlenformat« in der Datei"
        Assert-True ($r.p001 -like "*done*") "Paar P001 fertig: $($r.p001)"
        Assert-Eq ($doc.paare.P001.seiten.B.kategorien -join ",") "K1" "Kategorie in der Datei"
        Assert-Eq $doc.paare.P001.seiten.B.gravierend $true "gravierend in der Datei"
        Assert-Eq $doc.paare.P001.notiz "Geheime Notiz" "Notiz in der Datei"
        Assert-True ($null -ne $r.stored) "Browser-Codes fehlen"
        Assert-True (-not $r.stored.Contains("Geheime Notiz")) "Notiz im Browser-Speicher"
        Assert-True (-not $r.stored.Contains("notiz")) "Notizfeld im Browser-Speicher"
        foreach ($t in $r.texts) { Assert-True (-not $r.stored.Contains($t)) "Transkript im Browser-Speicher" }
        Assert-True ($r.saveInfo -like "gespeichert in urteile.json*") "Speicheranzeige: $($r.saveInfo)"
        Assert-Eq $r.downloads 0 "Download-Link"
    }

    Test-Case "W2/L3: ohne File System Access API sichtbarer Fehler, kein Download" {
        $r = Invoke-Page "ohne-api" "window.showSaveFilePicker = undefined; window.showOpenFilePicker = undefined;" $Operate
        Assert-True ($r.messages -like "*File System Access API fehlt*") "Fehlermeldung: $($r.messages)"
        Assert-Eq $r.downloads 0 "Download-Link"
        Assert-Eq $r.writes 0 "Schreibvorgänge"
        Assert-True ($r.saveInfo -like "*nicht gespeichert*") "Speicheranzeige: $($r.saveInfo)"
    }

    Test-Case "L3: Schreibfehler der Urteilsdatei ist sichtbar" {
        $r = Invoke-Page "schreibfehler" ($StubFs + "`nwindow.__failWrite = true;") $Operate
        Assert-True ($r.messages -like "*fehlgeschlagen*") "Meldung: $($r.messages)"
        Assert-True ($r.saveInfo -like "*NICHT gespeichert*") "Speicheranzeige: $($r.saveInfo)"
    }

    Test-Case "L3: beschädigter Browser-Stand wird angezeigt und nicht ersetzt" {
        $key = (Read-Json (Join-Path $EvalMain "schluessel.json")).auswertung
        $r = Invoke-Page "kaputt" ($StubFs + "`nlocalStorage.setItem('diktier-ultra-test:$key', '{kaputt');") $Operate
        Assert-True ($r.messages -like "*beschädigt*") "Meldung: $($r.messages)"
        Assert-Eq $r.stored "{kaputt" "Browser-Stand ersetzt"
        Assert-True ($r.writes -ge 1) "Datei trotzdem geschrieben"
    }

    Test-Case "L3: voller Browser-Speicher ist sichtbar" {
        $r = Invoke-Page "voll" ($StubFs + "`nStorage.prototype.setItem = function () { throw new DOMException('voll', 'QuotaExceededError'); };") $Operate
        Assert-True ($r.messages -like "*Browser-Zwischenstand nicht gespeichert*") "Meldung: $($r.messages)"
        Assert-True ($r.writes -ge 1) "Datei trotzdem geschrieben"
    }
    $Lifecycle = @'
(async () => {
  const sleep = ms => new Promise(res => setTimeout(res, ms));
  const click = sel => { const n = document.querySelector(sel); if (!n) throw new Error("fehlt: " + sel); n.click(); };
  const warns = () => { const e = new Event("beforeunload", { cancelable: true }); window.dispatchEvent(e); return e.defaultPrevented; };
  const snap = () => ({ info: document.getElementById("saveInfo").textContent, warns: warns(),
    writes: window.__writes.length, messages: document.getElementById("messages").textContent });
  const note = t => { const n = document.querySelector("#P001 textarea"); n.value = t; n.dispatchEvent(new Event("input")); };
  const r = { errors: [] };
  try {
    r.start = snap();
    window.__holdClose = true;
    click('input[name="P001-u"][value="A"]');          // Picker, erster Schreibvorgang, close hängt
    await sleep(200);
    r.held = snap();
    note("Zweite Notiz");                               // zweite Änderung während des Schreibens
    await sleep(50);
    r.second = snap();
    window.__holdClose = false;
    window.__failClose = true;                          // der nächste Schreibvorgang scheitert
    window.__release();
    await sleep(50);
    r.firstClosed = snap();
    await sleep(700);                                   // entprellter zweiter Schreibvorgang
    r.failed = snap();
    window.__failClose = false;
    note("Dritte Notiz");
    await sleep(700);
    r.recovered = snap();
    r.last = window.__writes.slice(-1)[0] || null;
  } catch (e) { r.errors.push(String(e)); }
  document.getElementById("testergebnis").textContent = JSON.stringify(r);
})();
'@

    Test-Case "F2: gespeichert erst nach close, Warnung bei laufendem, neuem und gescheitertem Schreiben" {
        $r = Invoke-Page "lebenszyklus" $StubFs $Lifecycle
        Assert-Eq ($r.errors -join ";") "" "Bedienfehler"
        Assert-Eq $r.start.warns $false "Warnung ohne Änderung"
        Assert-Eq $r.held.writes 0 "Schreibvorgang vor close gezählt"
        Assert-True ($r.held.info -notlike "gespeichert*") "Anzeige vor close: $($r.held.info)"
        Assert-Eq $r.held.warns $true "Warnung bei laufendem close"
        Assert-Eq $r.second.warns $true "Warnung bei zweiter Änderung"
        Assert-Eq $r.firstClosed.writes 1 "erster Stand geschrieben"
        Assert-True ($r.firstClosed.info -notlike "gespeichert*") "Anzeige nach altem close: $($r.firstClosed.info)"
        Assert-Eq $r.firstClosed.warns $true "Warnung, solange die zweite Änderung fehlt"
        Assert-Eq $r.failed.writes 1 "gescheiterter Schreibvorgang gezählt"
        Assert-True ($r.failed.info -like "*NICHT gespeichert*") "Anzeige nach Fehler: $($r.failed.info)"
        Assert-True ($r.failed.messages -like "*fehlgeschlagen*") "Meldung nach Fehler: $($r.failed.messages)"
        Assert-Eq $r.failed.warns $true "Warnung nach Fehler"
        Assert-True ($r.recovered.info -like "gespeichert in urteile.json*") "Anzeige nach Erfolg: $($r.recovered.info)"
        Assert-Eq $r.recovered.warns $false "Warnung nach Erfolg"
        Assert-True ($r.recovered.messages -notlike "*fehlgeschlagen*") "Fehlermeldung bleibt: $($r.recovered.messages)"
        $doc = $r.last | ConvertFrom-Json -AsHashtable
        Assert-Eq $doc.paare.P001.notiz "Dritte Notiz" "letzter Stand in der Datei"
    }
    if ($EvalMain) { Get-ChildItem -LiteralPath $EvalMain -Filter "test-*.html" | Remove-Item }
}

# ================================================================= Summary

$failed = @($Results | Where-Object { -not $_.Ok })
[Console]::OutputEncoding = $SavedOutputEncoding
if (-not $KeepTemp) {
    Remove-Item -LiteralPath $Temp -Recurse -Force -ErrorAction SilentlyContinue
    if (Test-Path -LiteralPath $Temp) { Write-Host "Hinweis: $Temp ließ sich nicht ganz löschen" }
} else {
    Write-Host "Temp bleibt: $Temp"
}
Write-Host ""
Write-Host "Test-UltraScripts: $($Results.Count) Tests, $($Results.Count - $failed.Count) ok, $($failed.Count) fehlgeschlagen, $($Skipped.Count) übersprungen"
foreach ($f in $failed) { Write-Host "  FEHL $($f.Name)" }
foreach ($sk in $Skipped) { Write-Host "  SKIP $sk" }
exit $(if ($failed.Count) { 1 } else { 0 })
