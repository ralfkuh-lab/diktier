#Requires -Version 7.0
<#
.SYNOPSIS
    Gepaarte Offline-Leistungsmessung v3 gegen Ultra, Abnahmekriterium 4
    (docs/ultra-alltagstest-plan.md, WP2c/WP2e/WP5; Sol B2, Code-Review W3/W8).

.DESCRIPTION
    Dieselbe WAV-Liste, dasselbe diktier.exe, je Modell ein eigener Prozess mit
    `--transcribe-list --runs <Runs>` (ein ungezählter Warmup im Prozess).
    Durchgänge mit wechselnder Reihenfolge: v3→Ultra, Ultra→v3, v3→Ultra, …

    Ein Urteil zu Kriterium 4 gibt es nur mit dem festen Protokoll:
    - 3 Durchgänge × 3 Läufe (Default), wechselnde Reihenfolge, Daemon beendet;
    - Provenienz: `diktier.exe --version` ist `diktier 0.5.0`; SHA-256 von
      `lib\onnxruntime.dll` neben der Exe gleich `library_sha256` aus
      `versions.toml` im selben Ordner (ohne versions.toml: »nicht belegt«);
      `engine.threads` ist 0 in der Config, die der Kindprozess tatsächlich
      liest (`<APPDATA des Kindes>\diktier\config.toml`, mit `-ModelRoot`
      also `<ModelRoot>\diktier\config.toml`), ausgewertet wie `config.rs`
      mit einem echten TOML-Parser (Python ≥ 3.11); ohne Parser, bei
      ungültiger Datei oder wenn sie sich während der Messung ändert:
      »nicht belegt« (WP2f F1);
    - jeder Prozess hat einen gemessenen Peak (sonst »nicht belegt«);
    - Exitcode und Zeilenstatus passen zusammen (Exit 1 ⇔ `error`-Zeile).
    Andere Durchgangs-/Laufzahlen oder -OhneDaemonPruefung: »Funktionsprobe,
    kein Urteil«, mit denselben Zahlen.

    Gemessen:
    - `infer_ms` je Sekunde Audio (samples / 16000), nur über Dateien, bei
      denen **beide** Modelle in **allen** Läufen `text` liefern.
      Median und p95 (Nearest-Rank) je Modell.
    - Fehler je Modell (`error`-Zeilen, auch gescheiterte Warmups) und
      Ultra-exklusive Fehler (Dateien mit Fehler bei Ultra, nicht bei v3).
    - Peak Working Set je Prozess (GetProcessMemoryInfo nach Prozessende).

    Ausgabe: `<Auswertungsordner>\bench-<zeitstempel>\` mit den Roh-JSONL
    (enthalten Texte, bleiben lokal), `bench.json` und `bench.md` (nur Zahlen).
    Auswertungsordner und Liste müssen unter
    `<Wurzel>\diktier\ultra-test\auswertung\` liegen.

.PARAMETER Evaluation
    Auswertungsordner aus compare-models.ps1 -Prepare. Seine `liste.txt` ist die
    Default-Liste.

.PARAMETER List
    Andere WAV-Liste (UTF-8, eine Datei je Zeile), ebenfalls unter der
    Auswertungswurzel.

.PARAMETER Exe
    diktier.exe ab 0.5.0. Default: die installierte.

.PARAMETER ModelRoot
    Testwurzel: Der Kindprozess bekommt `LOCALAPPDATA` und `APPDATA` darauf
    (Modelle unter `<ModelRoot>\diktier\models\`, Config
    `<ModelRoot>\diktier\config.toml`). Ohne `-ModelRoot` gelten die echten
    `%LOCALAPPDATA%` und `%APPDATA%`.

.PARAMETER OhneDaemonPruefung
    Nur für Tests und Funktionsproben: misst auch neben einem laufenden
    diktier.exe. Ergebnis ist dann nie ein Urteil.

.EXAMPLE
    scripts\bench-models.ps1 -Evaluation "$env:LOCALAPPDATA\diktier\ultra-test\auswertung\20261008-090000"
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Evaluation,
    [string] $List,
    [string] $Exe,
    [string] $ModelRoot,
    [ValidateRange(1, 100)] [int] $Runs = 3,
    [ValidateRange(1, 20)] [int] $Passes = 3,
    [switch] $OhneDaemonPruefung
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot "ultra-test-lib.ps1")

# Das feste Protokoll (Plan, Kriterium 4).
$ProtocolPasses = 3
$ProtocolRuns = 3
$ExpectedVersion = "diktier 0.5.0"

if (-not $OhneDaemonPruefung) {
    $running = @(Get-Process -Name diktier -ErrorAction SilentlyContinue)
    if ($running.Count -gt 0) {
        Write-Host "bench-models.ps1: diktier.exe läuft (PID $(($running | ForEach-Object Id) -join ', '))."
        Write-Host "Für eine faire Messung den Daemon über das Tray-Menü beenden und neu aufrufen. Das Skript beendet ihn nicht."
        exit 3
    }
}

$root = Get-UltraTestRoot $ModelRoot
# Das Kind bekommt die aufgelöste Testwurzel, nie einen relativen Pfad.
$childRoot = if ($ModelRoot) { $root } else { $null }
$evalRoot = Get-EvaluationRoot $root
$evalDir = Assert-UnderEvaluationRoot $Evaluation $evalRoot "-Evaluation"
if (-not (Test-Path -LiteralPath $evalDir -PathType Container)) { throw "Auswertungsordner fehlt: $evalDir" }
$listPath = if ($List) { Assert-UnderEvaluationRoot $List $evalRoot "-List" } else { Join-Path $evalDir "liste.txt" }
if (-not (Test-Path -LiteralPath $listPath -PathType Leaf)) { throw "WAV-Liste fehlt: $listPath" }
$exePath = if ($Exe) { $Exe } else { Get-DefaultExe }
if (-not (Test-Path -LiteralPath $exePath -PathType Leaf)) { throw "diktier.exe nicht gefunden: $exePath" }
$exePath = (Resolve-Path -LiteralPath $exePath).ProviderPath
if ($ModelRoot) { Assert-ModelDirs $root @($V3Key, $UltraKey) }
$files = @(Get-Content -LiteralPath $listPath -Encoding utf8 | ForEach-Object { $_.Trim() } | Where-Object { $_ -ne "" })
if ($files.Count -eq 0) { throw "WAV-Liste ist leer" }

# ------------------------------------------------------------ Provenienz (W8)

function Get-Provenance([string] $ExePath, [string] $ChildModelRoot) {
    $problems = New-Object System.Collections.Generic.List[string]
    $v = Invoke-DiktierLine $ExePath @("--version") $ChildModelRoot
    if ($v.ExitCode -ne 0 -or $v.Output -cne $ExpectedVersion) {
        $problems.Add("--version meldet '$($v.Output)' (Exit $($v.ExitCode)) statt '$ExpectedVersion'")
    }
    $exeDir = Split-Path -Parent $ExePath
    $dll = Join-Path $exeDir "lib\onnxruntime.dll"
    $versions = Join-Path $exeDir "versions.toml"
    $dllSha = if (Test-Path -LiteralPath $dll -PathType Leaf) { (Get-FileHash -Algorithm SHA256 -LiteralPath $dll).Hash.ToLowerInvariant() } else { $null }
    $wantSha = $null
    $ort = "nicht belegt"
    if ($null -eq $dllSha) {
        $problems.Add("lib\onnxruntime.dll fehlt neben der Exe")
    } elseif (-not (Test-Path -LiteralPath $versions -PathType Leaf)) {
        $problems.Add("versions.toml fehlt neben der Exe: ORT-Stand nicht belegt")
    } else {
        $section = $null
        foreach ($line in Get-Content -LiteralPath $versions -Encoding utf8) {
            if ($line -cmatch '^\s*\[([^\]]+)\]\s*$') { $section = $Matches[1]; continue }
            if ($section -ceq "onnxruntime" -and $line -cmatch '^\s*library_sha256\s*=\s*"([0-9a-f]{64})"\s*$') { $wantSha = $Matches[1] }
        }
        if ($null -eq $wantSha) { $problems.Add("versions.toml nennt kein [onnxruntime] library_sha256") }
        elseif ($wantSha -cne $dllSha) { $problems.Add("lib\onnxruntime.dll weicht von versions.toml ab") }
        else { $ort = "gleich versions.toml" }
    }
    # Threads (F1): die Config, die der Kindprozess liest, ausgewertet wie config.rs.
    $threads = Get-EffectiveThreads (Get-ChildConfigPath $ChildModelRoot)
    if ($null -ne $threads.Problem) { $problems.Add($threads.Problem) }
    elseif ($threads.Threads -ne 0) { $problems.Add("engine.threads = $($threads.Threads) statt Default 0 ($($threads.Source))") }
    return [pscustomobject]@{
        Version = $v.Output; OrtSha256 = $dllSha; OrtVersionsToml = $wantSha; Ort = $ort
        Threads = $threads.Threads; ThreadsSource = $threads.Source; Config = $threads.Config
        ConfigState = $threads; Problems = $problems
    }
}

function Format-Threads($Prov) {
    if ($null -eq $Prov.Threads) { return "nicht belegt" }
    return [string] $Prov.Threads
}

$stamp = (Get-Date).ToString("yyyyMMdd-HHmmss")
$out = Join-Path $evalDir "bench-$stamp"
Set-WriteRoot $out $evalRoot
New-WriteDir $out
$prov = Get-Provenance $exePath $childRoot
# Die Liste in normalisierter Form, damit die file-Felder exakt vergleichbar sind.
$benchList = Join-Path $out "liste.txt"
Write-Utf8Lines $benchList $files
Write-Host "== Bench $out ($($files.Count) Dateien, $Passes Durchgänge, je $Runs Läufe)"
Write-Host "   Provenienz: $($prov.Version), ORT $($prov.Ort), Threads $(Format-Threads $prov) ($($prov.Config))"

$procs = New-Object System.Collections.Generic.List[object]
for ($pass = 1; $pass -le $Passes; $pass++) {
    $order = if ($pass % 2 -eq 1) { @($V3Key, $UltraKey) } else { @($UltraKey, $V3Key) }
    foreach ($k in $order) {
        $name = "d$pass-$k"
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $r = Invoke-DiktierList -Exe $exePath -List $benchList -Key $k -ModelRoot $childRoot -Runs $Runs `
            -StdoutPath (Join-Path $out "$name.jsonl") -StderrPath (Join-Path $out "$name.diagnose.txt")
        $sw.Stop()
        Write-Host ("   Durchgang {0} {1}: Exit {2}, {3:F1} s, Peak WS {4} MiB" -f $pass, $k, $r.ExitCode, $sw.Elapsed.TotalSeconds,
            $(if ($null -ne $r.PeakWorkingSet) { Format-Num ($r.PeakWorkingSet / 1MB) 0 } else { "?" }))
        if ($r.ExitCode -notin @(0, 1)) { throw "diktier.exe --model $k endete mit $($r.ExitCode) (siehe $name.diagnose.txt)" }
        $rows = Read-DiktierJsonl (Join-Path $out "$name.jsonl") $files $Runs
        # W3: Exit 1 ohne error-Zeile (oder umgekehrt) ist ein Widerspruch, kein Messergebnis.
        $null = Assert-ExitMatchesRows $r.ExitCode $rows "Durchgang $pass, --model $k"
        $procs.Add([pscustomobject]@{ Pass = $pass; Key = $k; ExitCode = $r.ExitCode; PeakWorkingSet = $r.PeakWorkingSet; Seconds = $sw.Elapsed.TotalSeconds; Rows = $rows })
    }
}

# F1: Die Config darf sich während der Messung nicht ändern (auch nicht durch
# das Kind, das eine fehlende Default-Config anlegt: die hat threads = 0).
if ($null -ne $prov.Threads) {
    $after = Get-EffectiveThreads $prov.Config
    if ($prov.ConfigState.Exists -and $after.Sha256 -cne $prov.ConfigState.Sha256) {
        $prov.Problems.Add("engine.threads nicht belegt: $($prov.Config) hat sich während der Messung geändert")
    } elseif ($after.Threads -ne $prov.Threads) {
        $prov.Problems.Add("engine.threads nicht belegt: vor der Messung $($prov.Threads), danach $(Format-Threads $after)")
    }
}
$prov.Problems = $prov.Problems.ToArray()

# Je Datei: alle Läufe beider Modelle `text`?
$allText = @{}
$errFiles = @{ $V3Key = @{}; $UltraKey = @{} }
$errLines = @{ $V3Key = 0; $UltraKey = 0 }
foreach ($f in $files) { $allText[$f] = $true }
foreach ($p in $procs) {
    foreach ($row in $p.Rows) {
        if ($row.status -cne "text") { $allText[$row.file] = $false }
        if ($row.status -ceq "error") { $errLines[$p.Key]++; $errFiles[$p.Key][$row.file] = $true }
    }
}
$paired = @($files | Where-Object { $allText[$_] })
$pairedSet = @{}; foreach ($f in $paired) { $pairedSet[$f] = $true }

$stats = [ordered]@{}
foreach ($k in @($V3Key, $UltraKey)) {
    $values = New-Object System.Collections.Generic.List[double]
    foreach ($p in $procs | Where-Object Key -eq $k) {
        foreach ($row in $p.Rows) {
            if (-not $pairedSet.ContainsKey($row.file) -or [int64] $row.samples -le 0) { continue }
            $values.Add([double] $row.infer_ms / ([double] $row.samples / 16000.0))
        }
    }
    $mine = @($procs | Where-Object Key -eq $k)
    $peaks = @($mine | ForEach-Object PeakWorkingSet | Where-Object { $null -ne $_ })
    $stats[$k] = [ordered]@{
        messungen = $values.Count
        median_ms_je_s = Get-Median $values.ToArray()
        p95_ms_je_s = Get-Quantile $values.ToArray() 0.95
        fehlerzeilen = $errLines[$k]
        dateien_mit_fehler = $errFiles[$k].Count
        exitcodes = @($mine | ForEach-Object ExitCode)
        peak_working_set_bytes = @($mine | ForEach-Object PeakWorkingSet)
        peak_fehlt = @($mine | Where-Object { $null -eq $_.PeakWorkingSet } | ForEach-Object { "Durchgang $($_.Pass)" })
        peak_working_set_max_bytes = $(if ($peaks.Count) { ($peaks | Measure-Object -Maximum).Maximum } else { $null })
    }
}
$ultraOnlyErrors = @($errFiles[$UltraKey].Keys | Where-Object { -not $errFiles[$V3Key].ContainsKey($_) }).Count
$v3s = $stats[$V3Key]; $us = $stats[$UltraKey]
$ratio = { param($a, $b) if ($null -eq $a -or $null -eq $b -or $b -eq 0) { $null } else { $a / $b - 1 } }
$medianDelta = & $ratio $us.median_ms_je_s $v3s.median_ms_je_s
$p95Delta = & $ratio $us.p95_ms_je_s $v3s.p95_ms_je_s
$limit = 2GB
$checks = [ordered]@{
    median_hoechstens_plus_10_prozent = ($null -ne $medianDelta -and $medianDelta -le 0.10)
    p95_hoechstens_plus_20_prozent = ($null -ne $p95Delta -and $p95Delta -le 0.20)
    keine_ultra_exklusiven_fehler = ($ultraOnlyErrors -eq 0)
    peak_working_set_hoechstens_2_gib = ($null -ne $us.peak_working_set_max_bytes -and $us.peak_working_set_max_bytes -le $limit -and
        $null -ne $v3s.peak_working_set_max_bytes -and $v3s.peak_working_set_max_bytes -le $limit)
}

$k4 = Get-Kriterium4 -Passes $Passes -Runs $Runs -ProtocolPasses $ProtocolPasses -ProtocolRuns $ProtocolRuns `
    -NoDaemonCheck:$OhneDaemonPruefung -ProvenanceProblems $prov.Problems `
    -PeakMissing @(foreach ($k in @($V3Key, $UltraKey)) { foreach ($m in $stats[$k].peak_fehlt) { "$m $k" } }) `
    -PairedCount $paired.Count -Checks $checks
$protocol = $k4.Protocol
$missingEvidence = $k4.MissingEvidence
$verdict = $k4.Verdict

$result = [ordered]@{
    format = 2
    erstellt = (Get-Date).ToUniversalTime().ToString("s") + "Z"
    protokoll = [ordered]@{
        fest = ($protocol.Count -eq 0)
        abweichungen = @($protocol)
        durchgaenge = $Passes
        laeufe_je_prozess = $Runs
        reihenfolge = @($procs | ForEach-Object { "$($_.Pass):$($_.Key)" })
    }
    provenienz = [ordered]@{
        version = $prov.Version
        onnxruntime_sha256 = $prov.OrtSha256
        onnxruntime_versions_toml = $prov.OrtVersionsToml
        onnxruntime = $prov.Ort
        threads = $prov.Threads
        threads_quelle = $prov.ThreadsSource
        config = $prov.Config
        probleme = $prov.Problems
    }
    dateien = $files.Count
    gepaarte_dateien = $paired.Count
    modelle = $stats
    ultra_exklusive_fehlerdateien = $ultraOnlyErrors
    median_delta = $medianDelta
    p95_delta = $p95Delta
    pruefungen = $checks
    fehlende_belege = @($missingEvidence)
    kriterium_4 = $verdict
}
Write-Utf8Text (Join-Path $out "bench.json") ($result | ConvertTo-Json -Depth 6)

$pct = { param($x) if ($null -eq $x) { "—" } else { "{0}{1} %" -f $(if ($x -ge 0) { "+" } else { "" }), (Format-Num (100 * $x) 1) } }
$mib = { param($x) if ($null -eq $x) { "—" } else { Format-Num ($x / 1MB) 0 } }
$md = New-Object System.Collections.Generic.List[string]
$md.Add("# Leistung v3 gegen Ultra (Kriterium 4)")
$md.Add("")
$md.Add("Erzeugt von scripts/bench-models.ps1, $($prov.Version), ORT $($prov.Ort), Threads $(Format-Threads $prov). $($files.Count) Dateien, davon $($paired.Count) mit Text bei beiden Modellen in allen Läufen; $Passes Durchgänge in wechselnder Reihenfolge, je Prozess ein Warmup und $Runs Läufe.")
$md.Add("")
$md.Add("| | v3 | Ultra | Ultra gegen v3 |")
$md.Add("|---|---:|---:|---:|")
$md.Add("| Messungen | $($v3s.messungen) | $($us.messungen) | |")
$md.Add("| Median Inferenz je s Audio (ms) | $(Format-Num $v3s.median_ms_je_s 1) | $(Format-Num $us.median_ms_je_s 1) | $(& $pct $medianDelta) |")
$md.Add("| p95 Inferenz je s Audio (ms) | $(Format-Num $v3s.p95_ms_je_s 1) | $(Format-Num $us.p95_ms_je_s 1) | $(& $pct $p95Delta) |")
$md.Add("| Fehlerzeilen | $($v3s.fehlerzeilen) | $($us.fehlerzeilen) | Ultra-exklusiv: $ultraOnlyErrors Dateien |")
$md.Add("| Peak Working Set max. (MiB) | $(& $mib $v3s.peak_working_set_max_bytes) | $(& $mib $us.peak_working_set_max_bytes) | |")
$md.Add("")
$md.Add("| Prüfung | Ergebnis |")
$md.Add("|---|---|")
foreach ($c in $checks.Keys) { $md.Add("| $c | $(if ($checks[$c]) { 'ja' } else { 'nein' }) |") }
$md.Add("")
foreach ($m in $missingEvidence) { $md.Add("- nicht belegt: $m") }
if ($missingEvidence.Count) { $md.Add("") }
$md.Add("Kriterium 4: **$verdict**")
Write-Utf8Lines (Join-Path $out "bench.md") $md.ToArray()

Write-Host "== Ergebnis $(Join-Path $out 'bench.md')"
Write-Host ("   Median {0} → {1} ms/s ({2}), p95 {3} → {4} ms/s ({5}), Fehler {6}/{7}, Peak {8}/{9} MiB" -f
    (Format-Num $v3s.median_ms_je_s 1), (Format-Num $us.median_ms_je_s 1), (& $pct $medianDelta),
    (Format-Num $v3s.p95_ms_je_s 1), (Format-Num $us.p95_ms_je_s 1), (& $pct $p95Delta),
    $v3s.fehlerzeilen, $us.fehlerzeilen, (& $mib $v3s.peak_working_set_max_bytes), (& $mib $us.peak_working_set_max_bytes))
Write-Host "   Kriterium 4: $verdict"
