#Requires -Version 7.0
<#
.SYNOPSIS
    Verdeckter Modellvergleich v3 gegen Ultra für den Alltagstest
    (docs/ultra-alltagstest-plan.md, »Bewertungsprotokoll«, WP2c/WP2e/WP5).

.DESCRIPTION
    Zwei Schritte:

    -Prepare -From <Start> -To <Ende>                                  (verbindlich, 7 Tage)
    -Prepare -From <Start> -To <Ende> -Verlaengert -Grundlage <Ordner>  (verbindlich, 11 Tage)
    -Prepare -Explorativ [-From …] [-To …]                             (explorativ, nie ein Urteil)
        Verbindlich heißt: Der Zeitraum ist abgelaufen (Ende ≤ jetzt) und
        dauert genau 7 Tage, mit -Verlaengert genau 11, gemessen in Wanduhrzeit
        der lokalen Zeitzone (eine Zeitumstellung im Zeitraum ändert nichts).
        Start, Ende, Tage und die Zeitzone stehen in `vorbereitung.json`.

        Die 7-Tage-Vorbereitung hält zusätzlich den Mengenstand fest
        (`mengenstand_7_tage`: vollständige Paare mit Sprache und Paare mit
        Textunterschied, Letzteres als Obergrenze der entscheidbaren Paare).
        Die einmalige Verlängerung (Plan F2: nur bei fehlenden Mindestmengen)
        verlangt mit -Grundlage genau diese Vorbereitung: gleicher Start,
        erstellt nach Tag 7 und vor Ende der Verlängerung, und mindestens
        eine der beiden Zahlen unter der Mindestmenge (300 bzw. 50). Sonst
        Abbruch vor dem ersten Schreiben (WP2f F5).

        1. Inventur: Läufe laut diktier.log/diktier.log.1 (Gate-Zeilen je
           Sitzung, Startzeilen »Daemon« und »--foreground«) gegen die WAVs im
           Ring. Sitzungen ohne Startzeile (Rotation) werden nicht verschmolzen,
           sondern als mehrdeutig ausgewiesen. WAVs ohne auswertbaren
           Zeitstempel sind ein Inventurproblem und zählen nicht mit.
        2. `diktier.exe --transcribe-list` einmal je Modell. Exitcode und
           Zeilenstatus müssen zusammenpassen (Exit 1 ⇔ mindestens eine Zeile
           `error`). Weiter gehen nur vollständige Paare (beide Modelle `text`),
           jeder Ausschluss wird mit Grund gezählt.
        3. Paare mit Textunterschied bekommen eine zufällige A/B-Zuordnung. Seed,
           Zuordnung und die Audio-Aliasse stehen nur in `schluessel.json`.
        4. `vergleich.html`: selbstenthaltene Seite ohne Netz mit vier Blöcken
           (Paare, Stichprobe gleicher Ausgaben, »Herr Präsident«, »Ich« am
           Anfang). Audio nur über neutrale Aliasse `audio\<ID>.wav` (Hardlink,
           sonst Kopie), damit Aufnahmezeit und Laufnummer nicht sichtbar sind.
           Die Seite speichert direkt in eine Urteilsdatei (File System Access
           API), im Browser liegen höchstens Urteilscodes.

    -Resolve <Auswertungsordner> -Judgments <urteile.json>
        Löst die Zuordnung auf und rechnet die Abnahmekriterien 1–3 und 6.
        Kriterien-Urteile nur für eine verbindliche, abgelaufene Vorbereitung,
        deren gespeicherte Grenzen in der gespeicherten Zeitzone genau 7 bzw.
        11 Wanduhrtage ergeben und zu `tage` passen; eine Verlängerung nur mit
        gültiger 7-Tage-Grundlage (siehe oben). Sonst »explorativ, kein
        Urteil«.

        Urteilsschema (Format 3): je Paar `urteil` (A|B|gleich|unklar),
        `seiten.A/B` mit `kategorien` (K1–K4), `k5` und bei K1 `gravierend`,
        optional `gleicher_fall_wie`, `notiz`. Bei A/B zusätzlich
        `nur_zahlenformat` (bool, Pflicht): true heißt, der einzige Unterschied
        ist das Zahlenformat — das Paar zählt in Kriterium 1 als gleich, auch
        wenn beide Seiten gemeinsame andere Fehler haben (WP2f F4). Schreibt `bericht.md` (lokal, mit
        Texten), `zusammenfassung.md` und `ergebnis.json` (nur Zahlen) in den
        Auswertungsordner.

    Datenschutz: Alle Ordner und Dateien, die das Skript liest oder schreibt,
    liegen unter `<Wurzel>\diktier\ultra-test\auswertung\`, Wurzel ist
    `%LOCALAPPDATA%` oder `-ModelRoot`; sonst Abbruch vor dem ersten Schreiben.
    Auf der Konsole erscheinen nur Zahlen und Pfade, nie ein Transkript.

.PARAMETER Exe
    diktier.exe ab 0.5.0. Default: die installierte unter
    `%LOCALAPPDATA%\Programs\Diktier\`.

.PARAMETER ModelRoot
    Ersatz für `%LOCALAPPDATA%` (Testwurzel): Nur der Kindprozess bekommt es als
    `LOCALAPPDATA` und sucht die Modelle unter `<ModelRoot>\diktier\models\`.
    Aufnahmen, Log und Auswertung hängen ebenfalls daran.

.PARAMETER WavDir
    Ring-Verzeichnis. Default `<Wurzel>\diktier\ultra-test\wav`
    (DIKTIER_DEBUG_WAV_DIR aus Leitentscheidung 5).

.PARAMETER LogDir
    Verzeichnis mit diktier.log und diktier.log.1. Default `<Wurzel>\diktier`.

.PARAMETER From
    Beginn des Zeitraums (lokale Zeit, wenn ohne Zone). Verbindlich Pflicht.

.PARAMETER To
    Ende des Zeitraums (exklusiv). Verbindlich Pflicht.

.PARAMETER Verlaengert
    Verbindlich: Der Zeitraum ist die einmalige Verlängerung auf 11 Tage.
    Verlangt -Grundlage.

.PARAMETER Grundlage
    Nur mit -Verlaengert: Auswertungsordner der verbindlichen
    7-Tage-Vorbereitung desselben Zeitraumbeginns. Ihr Mengenstand muss die
    Mindestmengen verfehlen.

.PARAMETER Explorativ
    Vorbereitung ohne Urteil (jederzeit, Zeitraum optional).

.PARAMETER Seed
    Seed der Zufallszuordnung. Ohne Angabe wird einer gezogen und in
    `schluessel.json` protokolliert.

.PARAMETER Ziffern
    Nur mit -Resolve: Kriterium 6, Ralfs ausdrückliche Antwort zur
    Zahlenschreibweise von Ultra (ja | nein | offen).

.EXAMPLE
    scripts\compare-models.ps1 -Prepare -From "2026-10-01 08:00" -To "2026-10-08 08:00"

.EXAMPLE
    scripts\compare-models.ps1 -Prepare -Explorativ

.EXAMPLE
    scripts\compare-models.ps1 -Resolve "$env:LOCALAPPDATA\diktier\ultra-test\auswertung\20261008-090000" `
        -Judgments "$env:LOCALAPPDATA\diktier\ultra-test\auswertung\20261008-090000\urteile.json" -Ziffern ja
#>
[CmdletBinding(DefaultParameterSetName = "Prepare")]
param(
    [Parameter(ParameterSetName = "Prepare", Mandatory)]
    [Parameter(ParameterSetName = "Explore", Mandatory)] [switch] $Prepare,
    [Parameter(ParameterSetName = "Explore", Mandatory)] [switch] $Explorativ,
    [Parameter(ParameterSetName = "Prepare")]
    [Parameter(ParameterSetName = "Explore")] [string] $Exe,
    [Parameter(ParameterSetName = "Prepare")]
    [Parameter(ParameterSetName = "Explore")] [string] $WavDir,
    [Parameter(ParameterSetName = "Prepare")]
    [Parameter(ParameterSetName = "Explore")] [string] $LogDir,
    [Parameter(ParameterSetName = "Prepare", Mandatory)]
    [Parameter(ParameterSetName = "Explore")] [Nullable[datetime]] $From,
    [Parameter(ParameterSetName = "Prepare", Mandatory)]
    [Parameter(ParameterSetName = "Explore")] [Nullable[datetime]] $To,
    [Parameter(ParameterSetName = "Prepare")] [switch] $Verlaengert,
    [Parameter(ParameterSetName = "Prepare")] [string] $Grundlage,
    [Parameter(ParameterSetName = "Prepare")]
    [Parameter(ParameterSetName = "Explore")] [Nullable[int]] $Seed,
    [Parameter(ParameterSetName = "Prepare")]
    [Parameter(ParameterSetName = "Explore")] [int] $SampleSize = 20,

    [Parameter(ParameterSetName = "Resolve", Mandatory)] [string] $Resolve,
    [Parameter(ParameterSetName = "Resolve", Mandatory)] [string] $Judgments,
    [Parameter(ParameterSetName = "Resolve")] [ValidateSet("ja", "nein", "offen")] [string] $Ziffern = "offen",

    [string] $ModelRoot
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot "ultra-test-lib.ps1")
$SetName = $PSCmdlet.ParameterSetName

# K1–K4 als Mehrfachauswahl je Seite, K5 als eigenes Flag (Bewertungsprotokoll v2.1).
$Categories = [ordered]@{
    K1 = "Inhalt falsch oder ausgelassen"
    K2 = "Zahl-/Datumswert falsch"
    K3 = "Wortverschmelzung/-trennung"
    K4 = "Schreibweise/Interpunktion"
}
$K5Label = "nur Zahlenformat (Ziffer gegen Wort, gleicher Wert)"
$ClassCategories = @("K1", "K2", "K3")
$HerrPraesident = "Herr Präsident"
$InvCulture = [System.Globalization.CultureInfo]::InvariantCulture
$PeriodDays = 7
$ExtendedDays = 11
$MinComplete = 300
$MinDecisive = 50
# 3 seit WP2f: `nur_zahlenformat` im Urteil (F4), Zeitzone und Mengenstand in
# der Vorbereitung (F5). Ältere Auswertungsordner neu vorbereiten.
$Format = 3

function ConvertTo-UtcBound([Nullable[datetime]] $Value) {
    if ($null -eq $Value) { return $null }
    return ([datetime] $Value).ToUniversalTime()
}

function Format-UtcIso([datetime] $Value) {
    return $Value.ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ", $InvCulture)
}

# ConvertFrom-Json macht aus ISO-Zeitstempeln DateTime-Werte; beides annehmen.
function ConvertFrom-UtcIso($Value) {
    if ($null -eq $Value -or $Value -eq "") { return $null }
    if ($Value -is [datetime]) { return $Value.ToUniversalTime() }
    return [datetime]::Parse([string] $Value, $InvCulture, [System.Globalization.DateTimeStyles]::AdjustToUniversal -bor [System.Globalization.DateTimeStyles]::AssumeUniversal)
}

function Format-Utc($Value) {
    $t = ConvertFrom-UtcIso $Value
    if ($null -eq $t) { return "offen" }
    return Format-UtcIso $t
}

# ----------------------------------------------------------------- Prepare

function New-Rng($SeedValue) {
    if ($null -eq $SeedValue) {
        $SeedValue = [System.Security.Cryptography.RandomNumberGenerator]::GetInt32([int]::MaxValue)
    }
    return [pscustomobject]@{ Seed = [int] $SeedValue; Rng = [System.Random]::new([int] $SeedValue) }
}

function Get-Shuffled([object[]] $Items, [System.Random] $Rng) {
    $a = @($Items)
    for ($i = $a.Count - 1; $i -gt 0; $i--) {
        $j = $Rng.Next($i + 1)
        $tmp = $a[$i]; $a[$i] = $a[$j]; $a[$j] = $tmp
    }
    return , $a
}

function Test-StartsWithHp([string] $Text) {
    return $Text.TrimStart().StartsWith($HerrPraesident, [System.StringComparison]::OrdinalIgnoreCase)
}

# Beginnt der Text mit dem Wort »Ich« (nicht »Ichthyologie«)?
function Test-StartsWithIch([string] $Text) {
    return [regex]::IsMatch($Text, '^\s*ich(?![\p{L}\p{N}])', [System.Text.RegularExpressions.RegexOptions]::IgnoreCase)
}

<#
    Prüft den Zeitraum. Verbindlich: beide Grenzen, Start < Ende, abgelaufen,
    genau 7 bzw. 11 Tage Wanduhr in der lokalen Zeitzone (so bleibt eine
    Zeitumstellung im Zeitraum ohne Einfluss). Rückgabe: Modus, UTC-Grenzen,
    Tage und die Zeitzone, in der sie gezählt sind.
#>
function Get-Period {
    $binding = $SetName -ceq "Prepare"
    $fromUtc = ConvertTo-UtcBound $From
    $toUtc = ConvertTo-UtcBound $To
    if ($null -ne $fromUtc -and $null -ne $toUtc -and $fromUtc -ge $toUtc) {
        throw "Zeitraum verkehrt: -From $From liegt nicht vor -To $To"
    }
    $zone = [System.TimeZoneInfo]::Local.Id
    $days = $null
    if ($null -ne $fromUtc -and $null -ne $toUtc) { $days = Get-WallClockDays $fromUtc $toUtc $zone }
    if (-not $Verlaengert -and $Grundlage) { throw "-Grundlage gehört nur zu -Verlaengert" }
    if ($binding) {
        $want = if ($Verlaengert) { $ExtendedDays } else { $PeriodDays }
        if ($days -ne $want) {
            throw "Verbindlicher Zeitraum muss genau $want Tage dauern$(if ($Verlaengert) { ' (Verlängerung)' }), nicht $(Format-Num $days 3). Für andere Zeiträume -Explorativ."
        }
        if ($toUtc -gt [datetime]::UtcNow) {
            throw "Der Zeitraum läuft noch bis $(Format-UtcIso $toUtc); vor Ablauf wird nicht verbindlich ausgewertet (Bewertungsprotokoll). Für einen Zwischenblick -Explorativ."
        }
    }
    return [pscustomobject]@{
        Mode = $(if ($binding) { "verbindlich" } else { "explorativ" })
        FromUtc = $fromUtc; ToUtc = $toUtc; Days = $days; Zone = $zone; Extended = [bool] $Verlaengert
    }
}

<#
    Ist $Base (vorbereitung.json als Hashtable) eine gültige Grundlage für die
    Verlängerung eines Zeitraums, der bei $ExtFromUtc beginnt und bei
    $ExtToUtc endet? Rückgabe: $null oder der Grund, warum nicht (F5).
    $ExtCreatedUtc: Erstellzeit der Verlängerung, falls schon bekannt.
#>
function Get-ExtensionBaseProblem($Base, [string] $BaseId, [datetime] $ExtFromUtc, [datetime] $ExtToUtc, [string] $ExtWavDir, $ExtCreatedUtc) {
    if ($null -eq $Base) { return "7-Tage-Vorbereitung $BaseId fehlt" }
    if ((Get-Field $Base "format") -ne $Format) { return "7-Tage-Vorbereitung $BaseId hat nicht Format $Format" }
    if ((Get-Field $Base "auswertung") -cne $BaseId) { return "7-Tage-Vorbereitung $BaseId trägt eine andere Auswertungs-ID" }
    if ((Get-Field $Base "modus") -cne "verbindlich") { return "7-Tage-Vorbereitung $BaseId ist nicht verbindlich" }
    $z = Get-Field $Base "zeitraum"
    if ((Get-Field $z "verlaengert") -ne $false) { return "Grundlage $BaseId ist selbst eine Verlängerung" }
    $von = ConvertFrom-UtcIso (Get-Field $z "von")
    $bis = ConvertFrom-UtcIso (Get-Field $z "bis")
    if ($null -eq $von -or $null -eq $bis) { return "Grundlage $BaseId ohne Start oder Ende" }
    $d = Get-WallClockDays $von $bis (Get-Field $z "zeitzone")
    if ($null -eq $d) { return "Grundlage $BaseId ohne bekannte Zeitzone" }
    if ($d -ne $PeriodDays) { return "Grundlage $BaseId dauert laut Grenzen $(Format-Num $d 3) statt $PeriodDays Tage" }
    $tage = Get-Field $z "tage"
    if ($null -eq $tage -or [double] $tage -ne $d) { return "Grundlage $($BaseId): gespeicherte tage $tage widersprechen den Grenzen" }
    if ((Format-UtcIso $von) -cne (Format-UtcIso $ExtFromUtc)) { return "Grundlage $BaseId beginnt $(Format-UtcIso $von), die Verlängerung $(Format-UtcIso $ExtFromUtc)" }
    $wav = Get-Field $Base "wav_dir"
    if ($wav -isnot [string] -or -not $wav.Equals($ExtWavDir, [System.StringComparison]::OrdinalIgnoreCase)) { return "Grundlage $BaseId hat ein anderes Aufnahmeverzeichnis" }
    $created = ConvertFrom-UtcIso (Get-Field $Base "erstellt")
    if ($null -eq $created -or $created -lt $bis) { return "Grundlage $BaseId wurde vor Ablauf der 7 Tage erstellt" }
    if ($created -ge $ExtToUtc) { return "Grundlage $BaseId wurde erst nach Ende der Verlängerung erstellt (nicht vorab festgehalten)" }
    if ($null -ne $ExtCreatedUtc -and $created -ge $ExtCreatedUtc) { return "Grundlage $BaseId ist nicht älter als die Verlängerung" }
    $m = Get-Field $Base "mengenstand_7_tage"
    $complete = Get-Field $m "vollstaendig_mit_sprache"
    $diffPairs = Get-Field $m "verschieden"
    if (($complete -isnot [long] -and $complete -isnot [int]) -or ($diffPairs -isnot [long] -and $diffPairs -isnot [int])) {
        return "Grundlage $BaseId ohne 7-Tage-Mengenstand"
    }
    if ($complete -ne (Get-Field $Base "vollstaendig_mit_sprache") -or $diffPairs -ne (Get-Field $Base "verschieden")) {
        return "Mengenstand der Grundlage $BaseId widerspricht ihren Zählungen"
    }
    if ($complete -ge $MinComplete -and $diffPairs -ge $MinDecisive) {
        return "Mindestmengen nach 7 Tagen erreichbar ($complete vollständige Paare ≥ $MinComplete, $diffPairs Paare mit Unterschied ≥ $MinDecisive): Verlängerung unzulässig"
    }
    return $null
}

function Read-PrepFile([string] $Dir) {
    $path = Join-Path $Dir "vorbereitung.json"
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { return $null }
    return Get-Content -LiteralPath $path -Raw -Encoding utf8 | ConvertFrom-Json -AsHashtable
}

# Neutraler Alias im Auswertungsordner: Hardlink, sonst Kopie (anderes Laufwerk).
function New-AudioAlias([string] $Original, [string] $Alias) {
    Assert-WriteTarget $Alias
    try {
        New-Item -ItemType HardLink -Path $Alias -Target $Original -ErrorAction Stop | Out-Null
        return "hardlink"
    } catch {
        Copy-Item -LiteralPath $Original -Destination $Alias
        return "kopie"
    }
}

function Invoke-Prepare {
    $root = Get-UltraTestRoot $ModelRoot
    $evalRoot = Get-EvaluationRoot $root
    $period = Get-Period
    $exePath = if ($Exe) { $Exe } else { Get-DefaultExe }
    if (-not (Test-Path -LiteralPath $exePath -PathType Leaf)) { throw "diktier.exe nicht gefunden: $exePath" }
    $exePath = (Resolve-Path -LiteralPath $exePath).ProviderPath
    $wavDir = if ($WavDir) { $WavDir } else { Join-Path $root "diktier\ultra-test\wav" }
    if (-not (Test-Path -LiteralPath $wavDir -PathType Container)) { throw "Aufnahmeverzeichnis fehlt: $wavDir" }
    $wavDir = (Resolve-Path -LiteralPath $wavDir).ProviderPath
    $logDir = if ($LogDir) { $LogDir } else { Join-Path $root "diktier" }
    if ($ModelRoot) { Assert-ModelDirs $root @($V3Key, $UltraKey) }

    # F5: Verlängerung nur mit gültiger 7-Tage-Grundlage, geprüft vor jedem Schreiben.
    $extension = $null
    if ($Verlaengert) {
        if (-not $Grundlage) { throw "-Verlaengert verlangt -Grundlage <Auswertungsordner der 7-Tage-Vorbereitung> (Plan F2: Verlängerung nur bei fehlenden Mindestmengen)" }
        $baseDir = Assert-UnderEvaluationRoot $Grundlage $evalRoot "-Grundlage"
        $base = Read-PrepFile $baseDir
        $baseId = Split-Path -Leaf $baseDir
        $problem = Get-ExtensionBaseProblem $base $baseId $period.FromUtc $period.ToUtc $wavDir $null
        if ($problem) { throw "Verlängerung abgelehnt: $problem" }
        $extension = [ordered]@{
            grundlage = $baseId
            grundlage_erstellt = Format-UtcIso (ConvertFrom-UtcIso $base.erstellt)
            vollstaendig_mit_sprache = $base.mengenstand_7_tage.vollstaendig_mit_sprache
            verschieden = $base.mengenstand_7_tage.verschieden
        }
        Write-Host "== Verlängerung auf Grundlage $baseId (7 Tage: $($extension.vollstaendig_mit_sprache) vollständige Paare, $($extension.verschieden) mit Unterschied)"
    }

    $stamp = (Get-Date).ToString("yyyyMMdd-HHmmss")
    $out = Join-Path $evalRoot $stamp
    if (Test-Path -LiteralPath $out) { throw "Auswertungsordner existiert schon: $out" }
    Set-WriteRoot $out $evalRoot
    New-Item -ItemType Directory -Force -Path $evalRoot | Out-Null
    New-WriteDir $out
    Write-Host "== Auswertungsordner $out ($($period.Mode))"

    # 1. Inventur
    $inv = Get-Inventory $wavDir $logDir $period.FromUtc $period.ToUtc
    Write-Host "== Inventur ($(if ($inv.LogFiles.Count) { $inv.LogFiles -join ', ' } else { 'kein Log gefunden' }), $($inv.Sessions.Count) Sitzungen, davon ohne Startzeile $($inv.SessionsWithoutStart))"
    Write-Host "   erwartet $($inv.Expected), vorhanden $($inv.Present), fehlend $($inv.Missing.Count) (davon grenznah $($inv.MissingNearBoundary)), mehrdeutige Läufe $($inv.AmbiguousRuns)"
    foreach ($m in $inv.Missing) { Write-Host "   fehlt: Sitzung $($m.Sitzung), Lauf $($m.Lauf)" }
    Write-Host "   WAVs ohne Logeintrag $($inv.WavsWithoutLog), mehrdeutig $($inv.AmbiguousWavs), mit Namenssuffix $($inv.WavsWithSuffix), außerhalb des Zeitraums $($inv.WavsOutsidePeriod)"
    Write-Host "   WAVs ohne auswertbaren Zeitstempel (Inventurproblem, nicht gezählt) $($inv.WavsWithoutTimestamp.Count)"
    if ($inv.Wavs.Count -eq 0) { throw "keine WAV im Zeitraum" }

    # Bytegleiche Dateien nur einmal werten.
    $seenHash = @{}
    $duplicate = @{}
    foreach ($w in $inv.Wavs) {
        $h = (Get-FileHash -Algorithm SHA256 -LiteralPath $w).Hash
        if ($seenHash.ContainsKey($h)) { $duplicate[$w] = $true } else { $seenHash[$h] = $true }
    }

    $files = [string[]] $inv.Wavs
    $list = Join-Path $out "liste.txt"
    Write-Utf8Lines $list $files

    # 2. Beide Modelle
    $raw = @{}
    $exitCodes = [ordered]@{}
    $errorRows = [ordered]@{}
    foreach ($k in @($V3Key, $UltraKey)) {
        Write-Host "== $k über $($files.Count) Dateien"
        $r = Invoke-DiktierList -Exe $exePath -List $list -Key $k -ModelRoot $(if ($ModelRoot) { $root } else { $null }) `
            -StdoutPath (Join-Path $out "roh-$k.jsonl") -StderrPath (Join-Path $out "diagnose-$k.txt")
        $exitCodes[$k] = $r.ExitCode
        Write-Host "   Exitcode $($r.ExitCode)"
        if ($r.ExitCode -notin @(0, 1)) {
            throw "diktier.exe --model $k endete mit $($r.ExitCode) (siehe diagnose-$k.txt)"
        }
        $raw[$k] = Read-DiktierJsonl (Join-Path $out "roh-$k.jsonl") $files 1
        $errorRows[$k] = Assert-ExitMatchesRows $r.ExitCode $raw[$k] "diktier.exe --model $k"
    }

    # 3. Paare
    $excluded = [ordered]@{}
    $same = New-Object System.Collections.Generic.List[object]
    $diff = New-Object System.Collections.Generic.List[object]
    $hp = New-Object System.Collections.Generic.List[object]
    $ich = New-Object System.Collections.Generic.List[object]
    for ($i = 0; $i -lt $files.Count; $i++) {
        $f = $files[$i]
        $v = $raw[$V3Key][$i]
        $u = $raw[$UltraKey][$i]
        $reason = $null
        if ($duplicate.ContainsKey($f)) { $reason = "Duplikat (bytegleiche WAV)" }
        elseif ($v.status -eq "error" -and $u.status -eq "error") { $reason = "Fehler bei beiden" }
        elseif ($v.status -eq "error") { $reason = "Fehler nur v3" }
        elseif ($u.status -eq "error") { $reason = "Fehler nur Ultra" }
        elseif ($v.status -eq "rejected" -and $u.status -eq "rejected") { $reason = "Gate-Ablehnung (beide)" }
        elseif ($v.status -ne $u.status) { $reason = "Gate uneinheitlich" }
        elseif ($v.text -eq "" -and $u.text -eq "") { $reason = "beide leer (keine Sprache erkannt)" }
        if (-not $reason) {
            $vHp = Test-StartsWithHp $v.text
            $uHp = Test-StartsWithHp $u.text
            if ($vHp -or $uHp) { $hp.Add([pscustomobject]@{ File = $f; V3 = $vHp; Ultra = $uHp }) }
            # »Ich«: nur wenn genau ein Modell damit beginnt (Plan v2.1, W6).
            $vIch = Test-StartsWithIch $v.text
            $uIch = Test-StartsWithIch $u.text
            if ($vIch -xor $uIch) { $ich.Add([pscustomobject]@{ File = $f; V3 = $vIch; Ultra = $uIch }) }
            $entry = [pscustomobject]@{ File = $f; V3 = $v.text; Ultra = $u.text }
            if ($v.text -ceq $u.text) { $same.Add($entry) } else { $diff.Add($entry) }
        } else {
            if (-not $excluded.Contains($reason)) { $excluded[$reason] = 0 }
            $excluded[$reason]++
        }
    }
    $complete = $same.Count + $diff.Count
    Write-Host "== Paare: vollständig mit Sprache $complete (gleich $($same.Count), verschieden $($diff.Count))"
    foreach ($k in $excluded.Keys) { Write-Host "   ausgeschlossen: $k $($excluded[$k])" }
    # F5: Mengenstand nach 7 Tagen, vor jedem Urteil festgehalten. Die
    # entscheidbaren Paare kennt erst das Urteil; die Paare mit Unterschied
    # sind ihre Obergrenze.
    $quantity = $null
    if ($period.Mode -ceq "verbindlich" -and -not $period.Extended) {
        $quantity = [ordered]@{
            vollstaendig_mit_sprache = $complete
            verschieden = $diff.Count
            mindest_vollstaendig = $MinComplete
            mindest_entscheidbar = $MinDecisive
            verlaengerung_zulaessig = ($complete -lt $MinComplete -or $diff.Count -lt $MinDecisive)
        }
        Write-Host "   Mengenstand nach 7 Tagen: $complete vollständige Paare (Mindestmenge $MinComplete), $($diff.Count) mit Unterschied (Obergrenze entscheidbar, Mindestmenge $MinDecisive); Verlängerung zulässig: $(if ($quantity.verlaengerung_zulaessig) { 'ja' } else { 'nein' })"
    }

    # 4. Zufall: Reihenfolge, A/B, Stichprobe, Audioblöcke
    $r = New-Rng $Seed
    $audioDir = Join-Path $out "audio"
    New-WriteDir $audioDir
    $audioKey = [ordered]@{}
    $alias = {
        param([string] $Id, [string] $File)
        $name = "$Id.wav"
        $how = New-AudioAlias $File (Join-Path $audioDir $name)
        $audioKey[$name] = [ordered]@{ datei = $File; art = $how }
        return "audio/$name"
    }

    $keyPairs = New-Object System.Collections.Generic.List[object]
    $htmlPairs = New-Object System.Collections.Generic.List[object]
    $n = 0
    foreach ($p in (Get-Shuffled $diff.ToArray() $r.Rng)) {
        $n++
        $id = "P{0:D3}" -f $n
        $ultraIsA = $r.Rng.Next(2) -eq 1
        $a = if ($ultraIsA) { $UltraKey } else { $V3Key }
        $b = if ($ultraIsA) { $V3Key } else { $UltraKey }
        $keyPairs.Add([ordered]@{ id = $id; file = $p.File; A = $a; B = $b })
        $htmlPairs.Add([ordered]@{
                id = $id
                a = $(if ($ultraIsA) { $p.Ultra } else { $p.V3 })
                b = $(if ($ultraIsA) { $p.V3 } else { $p.Ultra })
                audio = (& $alias $id $p.File)
            })
    }
    # Erst zuweisen: Get-Shuffled gibt das Array als ein Objekt aus.
    $shuffledSame = Get-Shuffled $same.ToArray() $r.Rng
    $sample = @($shuffledSame | Select-Object -First $SampleSize)
    $keySample = New-Object System.Collections.Generic.List[object]
    $htmlSample = New-Object System.Collections.Generic.List[object]
    $n = 0
    foreach ($s in $sample) {
        $n++
        $id = "S{0:D2}" -f $n
        $keySample.Add([ordered]@{ id = $id; file = $s.File })
        $htmlSample.Add([ordered]@{ id = $id; text = $s.V3; audio = (& $alias $id $s.File) })
    }
    $blocks = @{}
    foreach ($spec in @(@{ Name = "herr_praesident"; Prefix = "H"; Items = $hp }, @{ Name = "ich"; Prefix = "I"; Items = $ich })) {
        $keyList = New-Object System.Collections.Generic.List[object]
        $htmlList = New-Object System.Collections.Generic.List[object]
        $n = 0
        foreach ($h in (Get-Shuffled $spec.Items.ToArray() $r.Rng)) {
            $n++
            $id = "{0}{1:D2}" -f $spec.Prefix, $n
            $keyList.Add([ordered]@{ id = $id; file = $h.File; v3 = $h.V3; ultra = $h.Ultra })
            # Nur Audio, keine Texte: die Frage ist, ob gesprochen wurde.
            $htmlList.Add([ordered]@{ id = $id; audio = (& $alias $id $h.File) })
        }
        $blocks[$spec.Name] = [pscustomobject]@{ Key = $keyList.ToArray(); Html = $htmlList.ToArray() }
    }

    $keyDoc = [ordered]@{
        format = $Format
        auswertung = $stamp
        seed = $r.Seed
        modelle = [ordered]@{ v3 = $V3Key; ultra = $UltraKey }
        paare = $keyPairs.ToArray()
        stichprobe = $keySample.ToArray()
        herr_praesident = $blocks["herr_praesident"].Key
        ich = $blocks["ich"].Key
        audio = $audioKey
    }
    Write-Utf8Text (Join-Path $out "schluessel.json") ($keyDoc | ConvertTo-Json -Depth 6)

    $prep = [ordered]@{
        format = $Format
        auswertung = $stamp
        modus = $period.Mode
        erstellt = Format-UtcIso ([datetime]::UtcNow)
        exe = $exePath
        exe_version = (Get-Item -LiteralPath $exePath).VersionInfo.ProductVersion
        wav_dir = $wavDir
        zeitraum = [ordered]@{
            von = $(if ($period.FromUtc) { Format-UtcIso $period.FromUtc } else { $null })
            bis = $(if ($period.ToUtc) { Format-UtcIso $period.ToUtc } else { $null })
            tage = $period.Days
            zeitzone = $period.Zone
            verlaengert = $period.Extended
        }
        mengenstand_7_tage = $quantity
        verlaengerung = $extension
        inventur = [ordered]@{
            logdateien = $inv.LogFiles
            sitzungen = $inv.Sessions.Count
            sitzungen_ohne_startzeile = $inv.SessionsWithoutStart
            erwartet = $inv.Expected
            vorhanden = $inv.Present
            fehlend = $inv.Missing.Count
            fehlend_grenznah = $inv.MissingNearBoundary
            fehlend_laeufe = @($inv.Missing | ForEach-Object { [ordered]@{ sitzung = $_.Sitzung; lauf = $_.Lauf } })
            mehrdeutige_laeufe = $inv.AmbiguousRuns
            wavs_mehrdeutig = $inv.AmbiguousWavs
            wavs_ohne_logeintrag = $inv.WavsWithoutLog
            wavs_mit_namenssuffix = $inv.WavsWithSuffix
            wavs_ausserhalb = $inv.WavsOutsidePeriod
            wavs_ohne_zeitstempel = $inv.WavsWithoutTimestamp.Count
        }
        dateien = $files.Count
        exitcodes = $exitCodes
        fehlerzeilen = $errorRows
        ausschluesse = $excluded
        vollstaendig_mit_sprache = $complete
        gleich = $same.Count
        verschieden = $diff.Count
        stichprobe = $sample.Count
        herr_praesident = $hp.Count
        ich = $ich.Count
    }
    Write-Utf8Text (Join-Path $out "vorbereitung.json") ($prep | ConvertTo-Json -Depth 6)

    $data = [ordered]@{
        format = $Format
        auswertung = $stamp
        modus = $period.Mode
        ordner = $out
        kategorien = $Categories
        k5 = $K5Label
        paare = $htmlPairs.ToArray()
        stichprobe = $htmlSample.ToArray()
        herr_praesident = $blocks["herr_praesident"].Html
        ich = $blocks["ich"].Html
    }
    # EscapeHtml maskiert < > & ' " als \uXXXX: kein Text kann den <script>-Block
    # verlassen. Im Browser landen die Texte nur über textContent im DOM.
    $json = $data | ConvertTo-Json -Depth 6 -Compress -EscapeHandling EscapeHtml
    $html = (Get-Content -LiteralPath (Join-Path $PSScriptRoot "compare-models.html") -Raw -Encoding utf8)
    if (-not $html.Contains("/*DATEN*/{}")) { throw "compare-models.html: Platzhalter fehlt" }
    $html = $html.Replace("/*DATEN*/{}", $json)
    Write-Utf8Text (Join-Path $out "vergleich.html") $html

    Write-Host "== Seite $(Join-Path $out 'vergleich.html')"
    Write-Host "   $($htmlPairs.Count) Paare, $($htmlSample.Count) Stichprobe, $($blocks['herr_praesident'].Html.Count) »Herr Präsident«, $($blocks['ich'].Html.Count) »Ich«"
    Write-Host "   Die Seite fragt beim ersten Urteil nach der Urteilsdatei: $(Join-Path $out 'urteile.json') anlegen."
}

# ----------------------------------------------------------------- Resolve

function Get-Wilson([int] $K, [int] $N, [double] $Z = 1.96) {
    if ($N -eq 0) { return $null }
    $p = $K / $N
    $z2 = $Z * $Z
    $den = 1 + $z2 / $N
    $center = ($p + $z2 / (2 * $N)) / $den
    $half = $Z * [math]::Sqrt($p * (1 - $p) / $N + $z2 / (4 * $N * $N)) / $den
    return [pscustomobject]@{ Low = $center - $half; High = $center + $half }
}

# Text als Daten in Markdown: Metazeichen maskiert, Zeilenumbrüche sichtbar.
function ConvertTo-MdText([string] $Text) {
    if ($Text -eq "") { return "*(leer)*" }
    $t = [regex]::Replace($Text, '([\\`*_{}\[\]()<>#+\-.!|~])', '\$1')
    return ($t -replace "`r`n|`r|`n", " ⏎ " -replace "`t", " ⇥ ")
}

# Das Komma hält ein leeres Array als Wert (sonst rollt PowerShell es zu $null aus).
function Get-Field($Table, [string] $Name) {
    if ($Table -is [System.Collections.IDictionary] -and $Table.Contains($Name)) { return , $Table[$Name] }
    return $null
}

<#
    Eine Seite eines Paars: `kategorien` (Teilmenge K1–K4, ohne Doppel), `k5`
    (bool), `gravierend` (bool genau dann, wenn K1 markiert ist). Rückgabe:
    normalisierte Seite oder $null mit Problemen in $Problems.
#>
function Read-Side($Side, [string] $Where, $Problems) {
    if ($Side -isnot [System.Collections.IDictionary]) { $Problems.Add("$($Where): Seite fehlt"); return $null }
    $cats = Get-Field $Side "kategorien"
    if ($null -eq $cats) { $Problems.Add("$($Where): kategorien fehlt"); return $null }
    $list = @($cats)
    foreach ($c in $list) {
        if ($c -isnot [string] -or $c -cnotin $Categories.Keys) { $Problems.Add("$($Where): unbekannte Kategorie $c"); return $null }
    }
    if (@($list | Select-Object -Unique).Count -ne $list.Count) { $Problems.Add("$($Where): Kategorie doppelt"); return $null }
    $k5 = Get-Field $Side "k5"
    if ($k5 -isnot [bool]) { $Problems.Add("$($Where): k5 fehlt"); return $null }
    $g = Get-Field $Side "gravierend"
    if ("K1" -cin $list) {
        if ($g -isnot [bool]) { $Problems.Add("$($Where): K1 ohne »gravierend«"); return $null }
    } elseif ($null -ne $g) {
        $Problems.Add("$($Where): »gravierend« ohne K1"); return $null
    }
    return [pscustomobject]@{ Kategorien = [string[]] $list; K5 = $k5; Gravierend = ($g -eq $true) }
}

<#
    Verbindlich ausgewertet wird nur eine verbindliche Vorbereitung eines
    abgelaufenen Zeitraums von genau 7 bzw. 11 Tagen, erstellt nach dessen Ende
    (Bewertungsprotokoll »Verbindlicher Lauf«). Die Dauer kommt aus den
    gespeicherten UTC-Grenzen, gezählt in Wanduhrtagen der gespeicherten
    Zeitzone (F5); `tage` muss dazu passen. Eine Verlängerung braucht ihre
    7-Tage-Grundlage im selben Auswertungsbaum. Rückgabe: $null = verbindlich,
    sonst der Grund für »explorativ, kein Urteil«.
#>
function Get-NonBindingReason($Prep, [string] $EvalRoot) {
    if ((Get-Field $Prep "modus") -cne "verbindlich") { return "Vorbereitung ist explorativ" }
    $z = Get-Field $Prep "zeitraum"
    $von = ConvertFrom-UtcIso (Get-Field $z "von")
    $bis = ConvertFrom-UtcIso (Get-Field $z "bis")
    if ($null -eq $von -or $null -eq $bis) { return "Zeitraum ohne Start oder Ende" }
    if ($bis -le $von) { return "Zeitraum verkehrt" }
    if ($bis -gt [datetime]::UtcNow) { return "Zeitraum läuft noch (Ende $(Format-UtcIso $bis))" }
    $zone = Get-Field $z "zeitzone"
    $days = Get-WallClockDays $von $bis $zone
    if ($null -eq $days) { return "Zeitzone des Zeitraums fehlt oder ist unbekannt ('$zone')" }
    $extended = (Get-Field $z "verlaengert") -eq $true
    $want = if ($extended) { $ExtendedDays } else { $PeriodDays }
    if ($days -ne $want) { return "Zeitraum dauert laut Grenzen $(Format-Num $days 3) statt $want Tage" }
    $tage = Get-Field $z "tage"
    if ($null -eq $tage -or ($tage -isnot [long] -and $tage -isnot [int] -and $tage -isnot [double]) -or [double] $tage -ne $days) {
        return "gespeicherte tage '$tage' widersprechen den Grenzen ($(Format-Num $days 3) Tage)"
    }
    $erstellt = ConvertFrom-UtcIso (Get-Field $Prep "erstellt")
    if ($null -eq $erstellt -or $erstellt -lt $bis) { return "Vorbereitung vor Ablauf des Zeitraums erstellt" }
    if ($extended) {
        $ext = Get-Field $Prep "verlaengerung"
        $baseId = Get-Field $ext "grundlage"
        if ($baseId -isnot [string] -or $baseId -cnotmatch '^\d{8}-\d{6}\z') { return "Verlängerung ohne 7-Tage-Grundlage" }
        $baseDir = Join-Path $EvalRoot $baseId
        if (-not (Test-UnderEvaluationRoot $baseDir $EvalRoot)) { return "Grundlage $baseId der Verlängerung liegt nicht sicher unter der Auswertungswurzel" }
        $base = Read-PrepFile $baseDir
        $problem = Get-ExtensionBaseProblem $base $baseId $von $bis (Get-Field $Prep "wav_dir") $erstellt
        if ($problem) { return "Verlängerung unzulässig: $problem" }
        $m = Get-Field $base "mengenstand_7_tage"
        if ((Get-Field $ext "vollstaendig_mit_sprache") -ne (Get-Field $m "vollstaendig_mit_sprache") -or
                (Get-Field $ext "verschieden") -ne (Get-Field $m "verschieden")) {
            return "Verlängerung unzulässig: festgehaltener Mengenstand weicht von der Grundlage $baseId ab"
        }
    }
    return $null
}

function Invoke-Resolve {
    $root = Get-UltraTestRoot $ModelRoot
    $evalRoot = Get-EvaluationRoot $root
    $dir = Assert-UnderEvaluationRoot $Resolve $evalRoot "-Resolve"
    $judgPath = Assert-UnderEvaluationRoot $Judgments $evalRoot "-Judgments"
    if (-not (Test-Path -LiteralPath $dir -PathType Container)) { throw "Auswertungsordner fehlt: $dir" }
    if (-not (Test-Path -LiteralPath $judgPath -PathType Leaf)) { throw "Urteilsdatei fehlt: $judgPath" }
    Set-WriteRoot $dir $evalRoot
    # F3: alle drei Ausgaben vorab prüfen, damit ein Link nach außen abbricht,
    # bevor die erste davon geschrieben ist.
    foreach ($outName in @("ergebnis.json", "zusammenfassung.md", "bericht.md")) { Assert-WriteTarget (Join-Path $dir $outName) }
    foreach ($need in @("schluessel.json", "vorbereitung.json", "liste.txt")) {
        if (-not (Test-Path -LiteralPath (Join-Path $dir $need))) { throw "$need fehlt in $dir" }
    }
    $key = Get-Content -LiteralPath (Join-Path $dir "schluessel.json") -Raw -Encoding utf8 | ConvertFrom-Json -AsHashtable
    $prep = Get-Content -LiteralPath (Join-Path $dir "vorbereitung.json") -Raw -Encoding utf8 | ConvertFrom-Json -AsHashtable
    $judg = Get-Content -LiteralPath $judgPath -Raw -Encoding utf8 | ConvertFrom-Json -AsHashtable
    if ((Get-Field $key "format") -ne $Format -or (Get-Field $prep "format") -ne $Format) {
        throw "Auswertungsordner hat nicht Format $Format (vor WP2e vorbereitet?) — neu vorbereiten"
    }
    if ((Get-Field $judg "auswertung") -cne $key.auswertung) {
        throw "urteile.json gehört zu Auswertung '$(Get-Field $judg 'auswertung')', nicht zu '$($key.auswertung)'"
    }
    if ((Get-Field $judg "format") -ne $Format) { throw "urteile.json hat nicht Format $Format" }
    $files = @(Get-Content -LiteralPath (Join-Path $dir "liste.txt") -Encoding utf8 | Where-Object { $_ -ne "" })
    $raw = @{}
    foreach ($k in @($key.modelle.v3, $key.modelle.ultra)) {
        $rows = Read-DiktierJsonl (Join-Path $dir "roh-$k.jsonl") $files 1
        $byFile = @{}
        foreach ($row in $rows) { $byFile[$row.file] = $row }
        $raw[$k] = $byFile
    }
    $v3 = $key.modelle.v3
    $ultra = $key.modelle.ultra

    # ------------------------------------------ Vollständigkeit und Schema
    $pj = Get-Field $judg "paare"; if ($null -eq $pj) { $pj = @{} }
    $sj = Get-Field $judg "stichprobe"; if ($null -eq $sj) { $sj = @{} }
    $problems = New-Object System.Collections.Generic.List[string]
    $pairIds = @($key.paare | ForEach-Object { $_.id })
    $pairs = New-Object System.Collections.Generic.List[object]
    foreach ($p in $key.paare) {
        $j = Get-Field $pj $p.id
        $u = Get-Field $j "urteil"
        if ($u -cnotin @("A", "B", "gleich", "unklar")) { $problems.Add("$($p.id): Urteil fehlt"); continue }
        $sides = Get-Field $j "seiten"
        $sa = Read-Side (Get-Field $sides "A") "$($p.id)/A" $problems
        $sb = Read-Side (Get-Field $sides "B") "$($p.id)/B" $problems
        if ($null -eq $sa -or $null -eq $sb) { continue }
        $same = Get-Field $j "gleicher_fall_wie"
        if ($null -ne $same -and ($same -isnot [string] -or $same -cnotin $pairIds -or $same -ceq $p.id)) {
            $problems.Add("$($p.id): »gleicher Fall wie« $same ist kein anderes Paar"); continue
        }
        $ultraSide = if ($p.A -ceq $ultra) { $sa } else { $sb }
        $v3Side = if ($p.A -ceq $ultra) { $sb } else { $sa }
        $winner = $null
        # F4: Ob der Unterschied nur das Zahlenformat ist, sagt Ralf bei A/B
        # ausdrücklich; abgeleitet wird es nicht (gemeinsame Fehler stehen
        # auf beiden Seiten).
        $numberOnly = Get-Field $j "nur_zahlenformat"
        if ($u -cin @("A", "B")) {
            $winner = $p[$u]
            $loserSide = if ($winner -ceq $ultra) { $v3Side } else { $ultraSide }
            if ($numberOnly -isnot [bool]) { $problems.Add("$($p.id): $u besser ohne Angabe »Unterschied nur Zahlenformat« (ja/nein)"); continue }
            if ($numberOnly) {
                if (-not $loserSide.K5) { $problems.Add("$($p.id): »Unterschied nur Zahlenformat«, aber die schlechtere Seite hat kein K5"); continue }
                $ca = @($sa.Kategorien | Sort-Object) -join ","
                $cb = @($sb.Kategorien | Sort-Object) -join ","
                if ($ca -cne $cb) { $problems.Add("$($p.id): »Unterschied nur Zahlenformat«, aber die Kategorien K1–K4 der Seiten unterscheiden sich"); continue }
            } elseif ($loserSide.Kategorien.Count -eq 0) {
                $problems.Add("$($p.id): $u besser, aber die andere Seite hat keine Kategorie K1–K4 (nur Zahlenformat? Dann »Unterschied nur Zahlenformat: ja«)"); continue
            }
        } elseif ($null -ne $numberOnly) {
            $problems.Add("$($p.id): »Unterschied nur Zahlenformat« gehört nur zu A oder B"); continue
        }
        $pairs.Add([pscustomobject]@{
                Id = $p.id; File = $p.file; Urteil = $u; Winner = $winner
                Ultra = $ultraSide; V3 = $v3Side; NumberOnly = ($numberOnly -eq $true)
                SameAs = $same; Notiz = (Get-Field $j "notiz")
            })
    }
    $sampleRows = New-Object System.Collections.Generic.List[object]
    foreach ($s in $key.stichprobe) {
        $j = Get-Field $sj $s.id
        $u = Get-Field $j "urteil"
        if ($u -cnotin @("stimmt", "fehler")) { $problems.Add("$($s.id): Urteil fehlt"); continue }
        $rawCats = Get-Field $j "kategorien"
        $cats = @($rawCats | Where-Object { $null -ne $_ })
        if ($u -ceq "fehler") {
            if ($cats.Count -eq 0) { $problems.Add("$($s.id): Fehler ohne Kategorie"); continue }
            if (@($cats | Where-Object { $_ -cnotin $Categories.Keys }).Count -gt 0) { $problems.Add("$($s.id): unbekannte Kategorie"); continue }
        } elseif ($cats.Count -gt 0) {
            $problems.Add("$($s.id): »stimmt« mit Kategorie"); continue
        }
        $sampleRows.Add([pscustomobject]@{ Id = $s.id; File = $s.file; Urteil = $u; Kategorien = [string[]] $cats })
    }
    $audioBlocks = @{}
    foreach ($name in @("herr_praesident", "ich")) {
        $answers = Get-Field $judg $name; if ($null -eq $answers) { $answers = @{} }
        $rows = New-Object System.Collections.Generic.List[object]
        $items = Get-Field $key $name
        foreach ($h in @($items)) {
            if ($null -eq $h) { continue }
            $g = Get-Field (Get-Field $answers $h.id) "gesprochen"
            if ($g -isnot [bool]) { $problems.Add("$($h.id): Antwort fehlt"); continue }
            $rows.Add([pscustomobject]@{ Id = $h.id; File = $h.file; V3 = [bool] $h.v3; Ultra = [bool] $h.ultra; Gesprochen = $g })
        }
        $audioBlocks[$name] = $rows.ToArray()
    }
    if ($problems.Count -gt 0) {
        foreach ($p in $problems | Select-Object -First 20) { Write-Host "   $p" }
        throw "$($problems.Count) Urteile fehlen oder sind unvollständig — keine Auflösung"
    }

    # »gleicher Fall wie«: Ketten auf ihren Anfang zurückführen (Kreise sind ein Fehler).
    $byId = @{}; foreach ($p in $pairs) { $byId[$p.Id] = $p }
    $groupOf = @{}
    foreach ($p in $pairs) {
        $seen = @{}
        $cur = $p
        while ($null -ne $cur.SameAs) {
            if ($seen.ContainsKey($cur.Id)) { throw "»gleicher Fall wie« bildet einen Kreis über $($p.Id)" }
            $seen[$cur.Id] = $true
            $cur = $byId[$cur.SameAs]
        }
        $groupOf[$p.Id] = $cur.Id
    }

    # ------------------------------------------------------ Kriterium 1
    $wins = @{ $v3 = 0; $ultra = 0 }
    $counts = [ordered]@{ A_oder_B = 0; gleich = 0; unklar = 0; K5_als_gleich = 0; paare_mit_k5 = 0 }
    foreach ($p in $pairs) {
        if ($p.Ultra.K5 -or $p.V3.K5) { $counts.paare_mit_k5++ }
        if ($null -ne $p.Winner) {
            # K5 zählt nie als Erkennungsgewinn: »Unterschied nur Zahlenformat« = gleich,
            # auch wenn beide Seiten gemeinsame andere Fehler haben (F4).
            if ($p.NumberOnly) { $counts.K5_als_gleich++ }
            else { $counts.A_oder_B++; $wins[$p.Winner]++ }
        } else {
            $counts[$p.Urteil]++
        }
    }
    $U = $wins[$ultra]; $V = $wins[$v3]; $decisive = $U + $V
    $complete = [int] $prep.vollstaendig_mit_sprache
    $minOk = ($complete -ge $MinComplete) -and ($decisive -ge $MinDecisive)
    $wilson = Get-Wilson $U $decisive
    $quote = if ($decisive -gt 0) { $U / $decisive } else { $null }
    $k1 = if (-not $minOk) { "nicht belegt" } elseif ($U -ge 2 * $V) { "erfüllt" } else { "nicht erfüllt" }

    # ------------------------------------------------------ Kriterium 2
    # Ultra-exklusiv: Kategorie auf der Ultra-Seite, nicht auf der v3-Seite
    # desselben Paars; Wiederholungen (»gleicher Fall wie«) zählen einmal. v3
    # »hat« eine Kategorie, wenn sie auf einer v3-Seite oder in der Stichprobe
    # gleicher Ausgaben vorkommt (gemeinsame Fehler zählen für beide).
    $catStats = [ordered]@{}
    foreach ($c in $Categories.Keys) {
        $catStats[$c] = [ordered]@{
            v3_seiten = 0; ultra_seiten = 0; stichprobe = 0
            ultra_exklusiv_faelle = 0; v3_exklusiv_faelle = 0; neue_klasse = $false
        }
    }
    $ultraExcl = @{}; $v3Excl = @{}
    foreach ($c in $Categories.Keys) { $ultraExcl[$c] = @{}; $v3Excl[$c] = @{} }
    $vetoGroups = @{}
    $sharedK1UltraGrave = 0
    foreach ($p in $pairs) {
        foreach ($c in $Categories.Keys) {
            $onU = $c -cin $p.Ultra.Kategorien
            $onV = $c -cin $p.V3.Kategorien
            if ($onU) { $catStats[$c].ultra_seiten++ }
            if ($onV) { $catStats[$c].v3_seiten++ }
            if ($onU -and -not $onV) { $ultraExcl[$c][$groupOf[$p.Id]] = $true }
            if ($onV -and -not $onU) { $v3Excl[$c][$groupOf[$p.Id]] = $true }
        }
        $k1U = "K1" -cin $p.Ultra.Kategorien
        $k1V = "K1" -cin $p.V3.Kategorien
        if ($k1U -and -not $k1V -and $p.Ultra.Gravierend) { $vetoGroups[$groupOf[$p.Id]] = $true }
        if ($k1U -and $k1V -and $p.Ultra.Gravierend) { $sharedK1UltraGrave++ }
    }
    foreach ($s in $sampleRows) { foreach ($c in $s.Kategorien) { $catStats[$c].stichprobe++ } }
    $newClasses = @()
    foreach ($c in $Categories.Keys) {
        $catStats[$c].ultra_exklusiv_faelle = $ultraExcl[$c].Count
        $catStats[$c].v3_exklusiv_faelle = $v3Excl[$c].Count
        if ($c -cin $ClassCategories) {
            $v3Has = $catStats[$c].v3_seiten -gt 0 -or $catStats[$c].stichprobe -gt 0
            if ($ultraExcl[$c].Count -ge 3 -and -not $v3Has) {
                $catStats[$c].neue_klasse = $true
                $newClasses += $c
            }
        }
    }
    $veto = $vetoGroups.Count
    $k2 = if ($newClasses.Count -eq 0 -and $veto -eq 0) { "erfüllt" } else { "nicht erfüllt" }

    # ------------------------------------------------------ Kriterium 3
    # Getrennt für »Herr Präsident« und »Ich«: Ein Modell halluziniert, wenn
    # sein Text so beginnt und es laut Audio nicht gesprochen war.
    $hall = [ordered]@{}
    $k3Parts = [ordered]@{}
    foreach ($name in @("herr_praesident", "ich")) {
        $h = [ordered]@{ v3 = 0; ultra = 0; faelle = $audioBlocks[$name].Count; gesprochen = 0 }
        foreach ($row in $audioBlocks[$name]) {
            if ($row.Gesprochen) { $h.gesprochen++; continue }
            if ($row.V3) { $h.v3++ }
            if ($row.Ultra) { $h.ultra++ }
        }
        $hall[$name] = $h
        $k3Parts[$name] = if ($h.ultra -gt $h.v3) { "nicht erfüllt" }
            elseif ($h.ultra -eq 0 -and $h.v3 -eq 0) { "erfüllt (0 gegen 0: keine Verschlechterung beobachtet)" }
            else { "erfüllt" }
    }
    $k3 = if (@($k3Parts.Values | Where-Object { $_ -like "nicht*" }).Count -gt 0) { "nicht erfüllt" } else { "erfüllt" }

    # Stichprobe gleicher Ausgaben
    $sOk = @($sampleRows | Where-Object { $_.Urteil -ceq "stimmt" }).Count
    $sBad = $sampleRows.Count - $sOk

    $k6 = switch ($Ziffern) { "ja" { "erfüllt (Ralf: Ja)" } "nein" { "nicht erfüllt (Ralf: Nein)" } default { "offen (Ralfs ausdrückliches Ja steht aus)" } }

    # ------------------------------------------------ verbindlich oder nicht
    $nonBinding = Get-NonBindingReason $prep $evalRoot
    $verdicts = [ordered]@{ kriterium_1 = $k1; kriterium_2 = $k2; kriterium_3 = $k3; kriterium_6 = $k6 }
    if ($null -ne $nonBinding) {
        foreach ($k in @($verdicts.Keys)) { $verdicts[$k] = "explorativ, kein Urteil" }
    }

    # ------------------------------------------------------ ergebnis.json
    $pct = { param($x) if ($null -eq $x) { "—" } else { (Format-Num (100 * $x) 1) + " %" } }
    $result = [ordered]@{
        format = $Format
        auswertung = $key.auswertung
        verbindlich = ($null -eq $nonBinding)
        grund_explorativ = $nonBinding
        vollstaendig_mit_sprache = $complete
        urteile = $counts
        U = $U; V = $V; entscheidbar = $decisive
        mindestmengen = $minOk
        quote = $quote
        wilson = $(if ($wilson) { [ordered]@{ unten = $wilson.Low; oben = $wilson.High } } else { $null })
        kategorien = $catStats
        neue_klassen = @($newClasses)
        veto_faelle = $veto
        gemeinsame_k1_ultra_gravierend = $sharedK1UltraGrave
        halluzinationen = $hall
        kriterium_3_teile = $k3Parts
        stichprobe = [ordered]@{ faelle = $sampleRows.Count; stimmt = $sOk; fehler = $sBad }
        kriterien = $verdicts
    }
    Write-Utf8Text (Join-Path $dir "ergebnis.json") ($result | ConvertTo-Json -Depth 6)

    # ---------------------------------------------------- zusammenfassung.md
    $z = New-Object System.Collections.Generic.List[string]
    $z.Add("# Alltagstest Ultra — Kennzahlen")
    $z.Add("")
    $z.Add("Auswertung $($key.auswertung), erzeugt von scripts/compare-models.ps1. Nur Zahlen: keine Transkripte, Pfade oder Notizen.")
    $z.Add("")
    if ($null -ne $nonBinding) {
        $z.Add("**Explorativ, kein Urteil:** $nonBinding. Kriterien-Urteile gibt es nur für eine verbindliche Vorbereitung nach Ablauf des Zeitraums (Bewertungsprotokoll).")
        $z.Add("")
    }
    $z.Add("## Datenbasis")
    $z.Add("")
    $z.Add("| Größe | Wert |")
    $z.Add("|---|---:|")
    $z.Add("| Modus | $($prep.modus) |")
    $z.Add("| Zeitraum (UTC) | $(Format-Utc $prep.zeitraum.von) bis $(Format-Utc $prep.zeitraum.bis)$(if ($prep.zeitraum.verlaengert) { " (verlängert, Grundlage $(Get-Field (Get-Field $prep 'verlaengerung') 'grundlage'))" }) |")
    $z.Add("| Sitzungen laut Log (ohne Startzeile) | $($prep.inventur.sitzungen) ($($prep.inventur.sitzungen_ohne_startzeile)) |")
    $z.Add("| Läufe laut Log (erwartet) | $($prep.inventur.erwartet) |")
    $z.Add("| davon mit WAV | $($prep.inventur.vorhanden) |")
    $z.Add("| fehlend (davon grenznah) | $($prep.inventur.fehlend) ($($prep.inventur.fehlend_grenznah)) |")
    $z.Add("| mehrdeutige Läufe | $($prep.inventur.mehrdeutige_laeufe) |")
    $z.Add("| WAVs ohne Logeintrag | $($prep.inventur.wavs_ohne_logeintrag) |")
    $z.Add("| WAVs mehrdeutig | $($prep.inventur.wavs_mehrdeutig) |")
    $z.Add("| WAVs ohne Zeitstempel (nicht gezählt) | $($prep.inventur.wavs_ohne_zeitstempel) |")
    $z.Add("| ausgewertete Dateien | $($prep.dateien) |")
    foreach ($r in $prep.ausschluesse.Keys) { $z.Add("| ausgeschlossen: $r | $($prep.ausschluesse[$r]) |") }
    $z.Add("| vollständige Paare mit Sprache | $complete |")
    $z.Add("| davon gleiche Ausgabe | $($prep.gleich) |")
    $z.Add("| davon verschieden (verdeckt beurteilt) | $($prep.verschieden) |")
    $z.Add("")
    $z.Add("## Urteile der verschiedenen Paare")
    $z.Add("")
    $z.Add("| Urteil | Anzahl |")
    $z.Add("|---|---:|")
    $z.Add("| Ultra besser (U, ohne K5) | $U |")
    $z.Add("| v3 besser (V, ohne K5) | $V |")
    $z.Add("| A/B mit »Unterschied nur Zahlenformat« (K5, zählt als gleich) | $($counts.K5_als_gleich) |")
    $z.Add("| gleich | $($counts.gleich) |")
    $z.Add("| unklar | $($counts.unklar) |")
    $z.Add("| Paare mit K5-Markierung (Frage »Ziffern«) | $($counts.paare_mit_k5) |")
    $z.Add("")
    $z.Add("Siegquote Ultra U/(U+V): $(& $pct $quote), 95-%-Wilson-Intervall $(if ($wilson) { (& $pct $wilson.Low) + ' bis ' + (& $pct $wilson.High) } else { '—' }) (n = $decisive).")
    $z.Add("")
    $z.Add("## Fehlerkategorien")
    $z.Add("")
    $z.Add("| Kategorie | v3-Seiten | Ultra-Seiten | Stichprobe (beide) | v3-exklusiv (Fälle) | Ultra-exklusiv (Fälle) | neue Fehlerklasse Ultra |")
    $z.Add("|---|---:|---:|---:|---:|---:|---|")
    foreach ($c in $Categories.Keys) {
        $s = $catStats[$c]
        $flag = if ($c -cin $ClassCategories) { if ($s.neue_klasse) { "ja" } else { "nein" } } else { "—" }
        $z.Add("| $c $($Categories[$c]) | $($s.v3_seiten) | $($s.ultra_seiten) | $($s.stichprobe) | $($s.v3_exklusiv_faelle) | $($s.ultra_exklusiv_faelle) | $flag |")
    }
    $z.Add("")
    $z.Add("Ultra-exklusiv: Kategorie auf der Ultra-Seite und nicht auf der v3-Seite desselben Paars; als »gleicher Fall« markierte Wiederholungen zählen einmal. »Neue Fehlerklasse«: ≥ 3 solcher Fälle in K1–K3, und v3 hat die Kategorie weder auf einer Paarseite noch in der Stichprobe.")
    $z.Add("")
    $z.Add("Veto-Fälle (Ultra-exklusives K1, auf der Ultra-Seite gravierend): $veto. Gemeinsames K1 mit gravierender Ultra-Seite (kein Veto): $sharedK1UltraGrave.")
    $z.Add("")
    $z.Add("## Halluzinationen")
    $z.Add("")
    $z.Add("| Typ | Fälle | laut Audio gesprochen | v3 | Ultra | Ergebnis |")
    $z.Add("|---|---:|---:|---:|---:|---|")
    $z.Add("| beginnt mit »Herr Präsident« | $($hall.herr_praesident.faelle) | $($hall.herr_praesident.gesprochen) | $($hall.herr_praesident.v3) | $($hall.herr_praesident.ultra) | $($k3Parts.herr_praesident) |")
    $z.Add("| beginnt mit »Ich« (nur eins der Modelle) | $($hall.ich.faelle) | $($hall.ich.gesprochen) | $($hall.ich.v3) | $($hall.ich.ultra) | $($k3Parts.ich) |")
    $z.Add("")
    $z.Add("## Stichprobe gleicher Ausgaben")
    $z.Add("")
    $z.Add("$($sampleRows.Count) Aufnahmen: $sOk stimmen, $sBad mit gemeinsamem Fehler ($(@($Categories.Keys | ForEach-Object { "$_ $($catStats[$_].stichprobe)" }) -join ', ')).")
    $z.Add("")
    $z.Add("## Abnahmekriterien")
    $z.Add("")
    $z.Add("| Nr. | Kriterium | Ergebnis |")
    $z.Add("|---|---|---|")
    $z.Add("| — | Mindestmengen (≥ $MinComplete vollständige Paare mit Sprache: $complete; ≥ $MinDecisive entscheidbare Paare: $decisive) | $(if ($minOk) { 'erfüllt' } else { 'nicht erfüllt' }) |")
    $z.Add("| 1 | U ≥ 2·V ($U gegen $V) | $($verdicts.kriterium_1) |")
    $z.Add("| 2 | keine neue Fehlerklasse ($(if ($newClasses) { $newClasses -join ', ' } else { 'keine' })), kein Veto ($veto) | $($verdicts.kriterium_2) |")
    $z.Add("| 3 | Halluzinationen Ultra ≤ v3, »Herr Präsident« $($hall.herr_praesident.ultra) gegen $($hall.herr_praesident.v3), »Ich« $($hall.ich.ultra) gegen $($hall.ich.v3) | $($verdicts.kriterium_3) |")
    $z.Add("| 4 | Leistung | siehe bench-models.ps1 |")
    $z.Add("| 5 | Betrieb | aus Log und Rückweg-Gate (WP3b) |")
    $z.Add("| 6 | Zahlenschreibweise (Paare mit K5: $($counts.paare_mit_k5)) | $($verdicts.kriterium_6) |")
    $z.Add("")
    $z.Add("Der Vergleich ist nur teilweise blind: Ralf hat Ultra live erlebt, und Ultra schreibt Zahlen als Ziffern.")
    Write-Utf8Lines (Join-Path $dir "zusammenfassung.md") $z.ToArray()

    # --------------------------------------------------------- bericht.md
    $sideText = { param($s) $t = @($s.Kategorien); if ($s.K5) { $t += "K5" }; if ($s.Gravierend) { $t += "gravierend" }; if ($t.Count) { $t -join ", " } else { "keine" } }
    $b = New-Object System.Collections.Generic.List[string]
    $b.Add("# Alltagstest Ultra — lokaler Detailbericht $($key.auswertung)")
    $b.Add("")
    $b.Add("**Enthält Transkripte, Pfade und Notizen. Nur lokal, nie ins Repo.** Kennzahlen: zusammenfassung.md.")
    $b.Add("")
    $b.Add("## Verschiedene Paare")
    $b.Add("")
    foreach ($d in $pairs) {
        $rowV = $raw[$v3][$d.File]; $rowU = $raw[$ultra][$d.File]
        $verdict = if ($null -ne $d.Winner) { "$(if ($d.Winner -ceq $ultra) { 'Ultra' } else { 'v3' }) besser$(if ($d.NumberOnly) { ' (Unterschied nur Zahlenformat, zählt als gleich)' })" } else { $d.Urteil }
        $b.Add("### $($d.Id) — $verdict")
        $b.Add("")
        $b.Add("- Datei: $(ConvertTo-MdText ([System.IO.Path]::GetFileName($d.File)))")
        $b.Add("- v3 ($(& $sideText $d.V3)): $(ConvertTo-MdText $rowV.text)")
        $b.Add("- Ultra ($(& $sideText $d.Ultra)): $(ConvertTo-MdText $rowU.text)")
        if ($d.SameAs) { $b.Add("- gleicher Fall wie $($d.SameAs) (Fallgruppe $($groupOf[$d.Id]))") }
        if ($d.Notiz) { $b.Add("- Notiz: $(ConvertTo-MdText $d.Notiz)") }
        $b.Add("")
    }
    $b.Add("## Stichprobe gleicher Ausgaben")
    $b.Add("")
    foreach ($s in $sampleRows) {
        $b.Add("- $($s.Id) $(ConvertTo-MdText ([System.IO.Path]::GetFileName($s.File))): $(if ($s.Urteil -ceq 'stimmt') { 'stimmt' } else { "Fehler $($s.Kategorien -join ', ')" }) — $(ConvertTo-MdText $raw[$v3][$s.File].text)")
    }
    foreach ($spec in @(@{ Name = "herr_praesident"; Title = "»Herr Präsident«" }, @{ Name = "ich"; Title = "»Ich« am Anfang" })) {
        $b.Add("")
        $b.Add("## $($spec.Title)")
        $b.Add("")
        foreach ($h in $audioBlocks[$spec.Name]) {
            $b.Add("- $($h.Id) $(ConvertTo-MdText ([System.IO.Path]::GetFileName($h.File))): v3 $(if ($h.V3) { 'ja' } else { 'nein' }), Ultra $(if ($h.Ultra) { 'ja' } else { 'nein' }), gesprochen $(if ($h.Gesprochen) { 'ja' } else { 'nein' })")
        }
    }
    Write-Utf8Lines (Join-Path $dir "bericht.md") $b.ToArray()

    Write-Host "== Aufgelöst: $(Join-Path $dir 'zusammenfassung.md') (nur Zahlen), $(Join-Path $dir 'bericht.md') (lokal, mit Texten)"
    if ($null -ne $nonBinding) { Write-Host "   explorativ, kein Urteil: $nonBinding" }
    Write-Host "   U $U, V $V, Quote $(& $pct $quote), Mindestmengen $(if ($minOk) { 'erfüllt' } else { 'nicht erfüllt' }), neue Klassen $(if ($newClasses) { $newClasses -join ',' } else { 'keine' }), Veto $veto"
    Write-Host "   Halluzinationen »Herr Präsident« v3 $($hall.herr_praesident.v3) / Ultra $($hall.herr_praesident.ultra), »Ich« v3 $($hall.ich.v3) / Ultra $($hall.ich.ultra)"
    Write-Host "   Kriterium 1: $($verdicts.kriterium_1) · 2: $($verdicts.kriterium_2) · 3: $($verdicts.kriterium_3) · 6: $($verdicts.kriterium_6)"
}

if ($SetName -ceq "Resolve") { Invoke-Resolve } else { Invoke-Prepare }
