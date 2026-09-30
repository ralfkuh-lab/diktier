#Requires -Version 7.0
<#
.SYNOPSIS
    Gemeinsame Helfer für compare-models.ps1 und bench-models.ps1
    (docs/ultra-alltagstest-plan.md, WP2c/WP2e). Wird per Dot-Source geladen.

.DESCRIPTION
    - Wurzel: `%LOCALAPPDATA%` oder `-ModelRoot`. Mit `-ModelRoot` bekommt der
      Kindprozess `diktier.exe` **beide** Benutzerpfade auf die Testwurzel,
      `LOCALAPPDATA` (Modelle unter `<ModelRoot>\diktier\models\<key>\`) und
      `APPDATA` (Config `<ModelRoot>\diktier\config.toml`, WP2f F1). Ohne
      `-ModelRoot` erbt er beide unverändert. Die Skripte leiten auch ihre
      Default-Pfade (Aufnahmen, Log, Auswertung) von dieser Wurzel ab, damit
      ein Probelauf nichts unter dem echten `%LOCALAPPDATA%\diktier` anlegt.
    - Auswertungswurzel `<Wurzel>\diktier\ultra-test\auswertung\` (Plan,
      »Datenschutz«, »Urteile speichern«): Jeder Ordner und jede Datei, die ein
      Skript liest oder schreibt, muss darunter liegen, sonst Abbruch vor dem
      ersten Schreiben (`Assert-UnderEvaluationRoot`, `Set-WriteRoot`). Alle
      Schreibhelfer prüfen ihr konkretes Ziel vor dem Öffnen noch einmal
      selbst, lexikalisch und auf Reparse-Points in jedem Pfadbestandteil und
      in der Zieldatei; eine vorhandene Zieldatei mit weiteren Hardlinks wird
      nicht beschrieben (WP2f F3).
    - `Invoke-DiktierList` startet `diktier.exe --transcribe-list` mit
      umgeleitetem stdout/stderr (UTF-8, byte-treu in Dateien) und misst das
      Peak Working Set über `GetProcessMemoryInfo` auf dem noch offenen
      Prozesshandle. .NET liefert `PeakWorkingSet64` nach Prozessende als 0.
    - Kein Text aus stdout geht auf die Konsole.
#>

Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot "toml-lib.ps1")

$script:V3Key = "parakeet-tdt-0.6b-v3-int8"
$script:UltraKey = "parakeet-ultra-0.6b-int8-pc"
$script:Utf8NoBom = [System.Text.UTF8Encoding]::new($false)
$script:WriteRoot = $null
$script:WriteEvalRoot = $null

if (-not ("DiktierUltraTest.Native" -as [type])) {
    Add-Type -Namespace DiktierUltraTest -Name Native -MemberDefinition @'
[StructLayout(LayoutKind.Sequential)]
public struct PROCESS_MEMORY_COUNTERS {
    public uint cb;
    public uint PageFaultCount;
    public UIntPtr PeakWorkingSetSize;
    public UIntPtr WorkingSetSize;
    public UIntPtr QuotaPeakPagedPoolUsage;
    public UIntPtr QuotaPagedPoolUsage;
    public UIntPtr QuotaPeakNonPagedPoolUsage;
    public UIntPtr QuotaNonPagedPoolUsage;
    public UIntPtr PagefileUsage;
    public UIntPtr PeakPagefileUsage;
}
[DllImport("psapi.dll", SetLastError = true)]
public static extern bool GetProcessMemoryInfo(IntPtr hProcess, out PROCESS_MEMORY_COUNTERS counters, uint size);
'@
}
# Eigener Typ, damit ein älterer, schon geladener Native-Typ nicht stört.
if (-not ("DiktierUltraTest.FileLinks" -as [type])) {
    Add-Type -Namespace DiktierUltraTest -Name FileLinks -MemberDefinition @'
[StructLayout(LayoutKind.Sequential)]
public struct BY_HANDLE_FILE_INFORMATION {
    public uint FileAttributes;
    public System.Runtime.InteropServices.ComTypes.FILETIME CreationTime;
    public System.Runtime.InteropServices.ComTypes.FILETIME LastAccessTime;
    public System.Runtime.InteropServices.ComTypes.FILETIME LastWriteTime;
    public uint VolumeSerialNumber;
    public uint FileSizeHigh;
    public uint FileSizeLow;
    public uint NumberOfLinks;
    public uint FileIndexHigh;
    public uint FileIndexLow;
}
[DllImport("kernel32.dll", SetLastError = true)]
public static extern bool GetFileInformationByHandle(Microsoft.Win32.SafeHandles.SafeFileHandle hFile, out BY_HANDLE_FILE_INFORMATION info);
'@
}

function Get-UltraTestRoot([string] $ModelRoot) {
    if ($ModelRoot) {
        if (-not (Test-Path -LiteralPath $ModelRoot -PathType Container)) {
            throw "ModelRoot $ModelRoot existiert nicht"
        }
        return (Resolve-Path -LiteralPath $ModelRoot).ProviderPath
    }
    if (-not $env:LOCALAPPDATA) { throw "LOCALAPPDATA ist nicht gesetzt" }
    return $env:LOCALAPPDATA
}

function Get-DefaultExe {
    return (Join-Path $env:LOCALAPPDATA "Programs\Diktier\diktier.exe")
}

# ------------------------------------------------- Umgebung des Kindes (F1)

<#
    Die beiden Benutzerpfade, mit denen `diktier.exe` startet. Mit ModelRoot
    zeigen beide auf die Testwurzel (Modelle über LOCALAPPDATA, Config über
    APPDATA, `config.rs::config_path`), sonst erbt das Kind die eigenen.
#>
function Get-ChildUserDirs([string] $ModelRoot) {
    if ($ModelRoot) { return [pscustomobject]@{ LocalAppData = $ModelRoot; AppData = $ModelRoot } }
    return [pscustomobject]@{ LocalAppData = $env:LOCALAPPDATA; AppData = $env:APPDATA }
}

function Set-ChildEnvironment([System.Diagnostics.ProcessStartInfo] $Psi, [string] $ModelRoot) {
    if ($ModelRoot) {
        $Psi.Environment["LOCALAPPDATA"] = $ModelRoot
        $Psi.Environment["APPDATA"] = $ModelRoot
    }
}

# Die Config, die das Kind liest: `<APPDATA>\diktier\config.toml`. $null, wenn
# APPDATA leer ist (Rust nähme dann einen relativen Pfad: nicht belegbar).
function Get-ChildConfigPath([string] $ModelRoot) {
    $appData = (Get-ChildUserDirs $ModelRoot).AppData
    if (-not $appData) { return $null }
    return (Join-Path $appData "diktier\config.toml")
}

<#
    `engine.threads` so, wie `config.rs` es auswertet: Datei fehlt → das Kind
    legt die Default-Config an (0); Schlüssel oder [engine] fehlt → Default 0;
    Ganzzahl → auf 0..=CPUs begrenzt (negativ wird 0). Gelesen mit einem echten
    TOML-Parser (tomllib). Alles, was Rust ablehnen würde oder was sich nicht
    sicher bestimmen lässt (kein Python, ungültiges TOML, falscher Typ), ist
    Threads = $null mit Problem »nicht belegt« — nie ein angenommener Default.
#>
function Get-EffectiveThreads([string] $ConfigPath) {
    $result = [ordered]@{ Config = $ConfigPath; Exists = $false; Sha256 = $null; Threads = $null; Source = $null; Problem = $null }
    if (-not $ConfigPath) {
        $result.Problem = "engine.threads nicht belegt: APPDATA des Kindprozesses ist leer"
        return [pscustomobject] $result
    }
    if (-not (Test-Path -LiteralPath $ConfigPath -PathType Leaf)) {
        if (Test-Path -LiteralPath $ConfigPath) {
            $result.Problem = "engine.threads nicht belegt: $ConfigPath ist keine Datei"
            return [pscustomobject] $result
        }
        $result.Threads = 0
        $result.Source = "Config fehlt, das Kind legt die Default-Config an (threads = 0)"
        return [pscustomobject] $result
    }
    $result.Exists = $true
    $result.Sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $ConfigPath).Hash.ToLowerInvariant()
    $parsed = Invoke-RealToml $ConfigPath
    if ($parsed.Status -ceq "unavailable") {
        $result.Problem = "engine.threads nicht belegt: $($parsed.Message)"
        return [pscustomobject] $result
    }
    if ($parsed.Status -ceq "invalid") {
        $result.Problem = "engine.threads nicht belegt: config.toml ist kein gültiges TOML ($($parsed.Message))"
        return [pscustomobject] $result
    }
    $root = $parsed.Value
    if (-not $root.Contains("engine")) { $result.Threads = 0; $result.Source = "kein [engine], Default 0"; return [pscustomobject] $result }
    $engine = $root["engine"]
    if ($engine -isnot [System.Collections.IDictionary]) {
        $result.Problem = "engine.threads nicht belegt: engine ist keine Tabelle"
        return [pscustomobject] $result
    }
    if (-not $engine.Contains("threads")) { $result.Threads = 0; $result.Source = "threads fehlt, Default 0"; return [pscustomobject] $result }
    $t = $engine["threads"]
    if ($t -isnot [System.Numerics.BigInteger] -or $t -lt [long]::MinValue -or $t -gt [long]::MaxValue) {
        $result.Problem = "engine.threads nicht belegt: threads ist keine Ganzzahl im i64-Bereich"
        return [pscustomobject] $result
    }
    if ($t -le 0) {
        $result.Threads = 0
        $result.Source = $(if ($t -eq 0) { "threads = 0" } else { "threads = $t, von Rust auf 0 begrenzt" })
    } else {
        # Rust begrenzt nach oben auf available_parallelism; das ist hier nur
        # die Anzeige — für das Protokoll zählt allein »nicht 0«.
        $result.Threads = [int] [System.Numerics.BigInteger]::Min($t, [System.Numerics.BigInteger] [Environment]::ProcessorCount)
        $result.Source = "threads = $t"
    }
    return [pscustomobject] $result
}

# ------------------------------------------------------ Auswertungswurzel (W2)

function Get-EvaluationRoot([string] $Root) {
    return [System.IO.Path]::GetFullPath((Join-Path $Root "diktier\ultra-test\auswertung"))
}

<#
    Ist $Path ein Reparse-Point (Junction, Symlink, auch ein ins Leere
    zeigender)? `File.GetAttributes` folgt dem Link nicht. Fehlt der Eintrag,
    ist das kein Reparse-Point; lässt er sich nicht prüfen (Zugriff verweigert
    o. ä.), gilt er als unsicher.
#>
function Test-ReparseOrUnknown([string] $Path) {
    try {
        $attr = [System.IO.File]::GetAttributes($Path)
    } catch {
        $e = $_.Exception
        while ($e -is [System.Management.Automation.MethodInvocationException] -and $e.InnerException) { $e = $e.InnerException }
        if ($e -is [System.IO.FileNotFoundException] -or $e -is [System.IO.DirectoryNotFoundException]) { return $false }
        return $true
    }
    return [bool] ($attr -band [System.IO.FileAttributes]::ReparsePoint)
}

# Anzahl Hardlinks einer vorhandenen Datei (1 = nur dieser Name).
function Get-HardLinkCount([string] $Path) {
    $h = [System.IO.File]::OpenHandle($Path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read,
        [System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete)
    try {
        $info = New-Object DiktierUltraTest.FileLinks+BY_HANDLE_FILE_INFORMATION
        if (-not [DiktierUltraTest.FileLinks]::GetFileInformationByHandle($h, [ref] $info)) {
            throw "GetFileInformationByHandle für $Path gescheitert"
        }
        return [int] $info.NumberOfLinks
    } finally {
        $h.Dispose()
    }
}

<#
    Liegt $Path (Ordner oder Datei, muss nicht existieren) echt unterhalb von
    $EvalRoot? Vergleich auf dem vollen Pfad ohne Groß-/Kleinschreibung
    (Windows), `..` ist aufgelöst. Kein Bestandteil von der Wurzel
    `<Wurzel>` (drei Ebenen über `…\diktier\ultra-test\auswertung`) bis zum
    Ziel einschließlich darf ein Reparse-Point (Junction/Symlink) sein, sonst
    könnte er nach außen zeigen — das gilt auch für die Zieldatei selbst. Die
    Auswertungswurzel selbst zählt nicht als »darunter«.
#>
function Test-UnderEvaluationRoot([string] $Path, [string] $EvalRoot) {
    $full = [System.IO.Path]::GetFullPath($Path).TrimEnd('\', '/')
    $rootFull = [System.IO.Path]::GetFullPath($EvalRoot).TrimEnd('\', '/')
    if (-not $full.StartsWith($rootFull + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
        return $false
    }
    $top = $rootFull
    foreach ($i in 1..3) {
        $up = [System.IO.Path]::GetDirectoryName($top)
        if (-not $up) { break }
        $top = $up
    }
    $cursor = $full
    while ($cursor -and $cursor.Length -ge $top.Length) {
        if (Test-ReparseOrUnknown $cursor) { return $false }
        $cursor = [System.IO.Path]::GetDirectoryName($cursor)
    }
    return $true
}

function Assert-UnderEvaluationRoot([string] $Path, [string] $EvalRoot, [string] $Label) {
    if (-not (Test-UnderEvaluationRoot $Path $EvalRoot)) {
        throw "$Label $Path liegt nicht unter der Auswertungswurzel $EvalRoot oder führt über einen Reparse-Point (Plan »Datenschutz«); Abbruch vor dem ersten Schreiben"
    }
    return [System.IO.Path]::GetFullPath($Path)
}

# Alle folgenden Schreibhelfer schreiben nur unter diesem Ordner.
function Set-WriteRoot([string] $Dir, [string] $EvalRoot) {
    $script:WriteRoot = Assert-UnderEvaluationRoot $Dir $EvalRoot "Schreibziel"
    $script:WriteEvalRoot = [System.IO.Path]::GetFullPath($EvalRoot)
}

<#
    Vor jedem Öffnen (F3): lexikalisch unter dem Schreibordner, dann dieselbe
    Reparse-Regel wie Test-UnderEvaluationRoot für den konkreten Pfad (jeder
    Bestandteil und die Zieldatei, falls vorhanden). Eine vorhandene Datei mit
    weiteren Hardlinks würde ebenfalls nach außen schreiben: auch Abbruch.
    Grenze: Zwischen Prüfung und Öffnen bleibt ein Zeitfenster; gegen einen
    gleichzeitig arbeitenden Angreifer schützt das nicht, gegen vorab
    liegende Links schon.
#>
function Assert-WriteTarget([string] $Path) {
    if (-not $script:WriteRoot) { throw "interner Fehler: Schreibziel $Path ohne Set-WriteRoot" }
    $full = [System.IO.Path]::GetFullPath($Path)
    $root = $script:WriteRoot.TrimEnd('\', '/')
    if (-not ($full.StartsWith($root + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase) -or
            $full.Equals($root, [System.StringComparison]::OrdinalIgnoreCase))) {
        throw "interner Fehler: Schreibziel $full liegt außerhalb von $root"
    }
    if (-not (Test-UnderEvaluationRoot $full $script:WriteEvalRoot)) {
        throw "Schreibziel $full führt über einen Reparse-Point (Junction/Symlink) oder ist nicht prüfbar; Abbruch vor dem Schreiben (Plan »Datenschutz«)"
    }
    if ([System.IO.File]::Exists($full) -and (Get-HardLinkCount $full) -gt 1) {
        throw "Schreibziel $full hat weitere Hardlinks und würde auch dort schreiben; Abbruch vor dem Schreiben (Plan »Datenschutz«)"
    }
}

function Write-Utf8Lines([string] $Path, [string[]] $Lines) {
    Assert-WriteTarget $Path
    [System.IO.File]::WriteAllLines($Path, $Lines, $script:Utf8NoBom)
}

function Write-Utf8Text([string] $Path, [string] $Text) {
    Assert-WriteTarget $Path
    [System.IO.File]::WriteAllText($Path, $Text, $script:Utf8NoBom)
}

function New-WriteDir([string] $Path) {
    Assert-WriteTarget $Path
    New-Item -ItemType Directory -Path $Path | Out-Null
}

# ------------------------------------------------------------- diktier.exe

<#
    Startet `diktier.exe --transcribe-list <List> --model <Key> [--runs n]`.
    stdout → $StdoutPath, stderr → $StderrPath. Rückgabe: ExitCode und
    PeakWorkingSet (Bytes, $null wenn die Messung scheiterte).
#>
function Invoke-DiktierList {
    param(
        [Parameter(Mandatory)] [string] $Exe,
        [Parameter(Mandatory)] [string] $List,
        [Parameter(Mandatory)] [string] $Key,
        [Parameter(Mandatory)] [string] $StdoutPath,
        [Parameter(Mandatory)] [string] $StderrPath,
        [string] $ModelRoot,
        [int] $Runs = 0
    )
    Assert-WriteTarget $StdoutPath
    Assert-WriteTarget $StderrPath
    $psi = [System.Diagnostics.ProcessStartInfo]::new($Exe)
    foreach ($a in @("--transcribe-list", $List, "--model", $Key)) { $psi.ArgumentList.Add($a) }
    if ($Runs -gt 0) {
        $psi.ArgumentList.Add("--runs")
        $psi.ArgumentList.Add([string] $Runs)
    }
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardOutputEncoding = $script:Utf8NoBom
    $psi.StandardErrorEncoding = $script:Utf8NoBom
    Set-ChildEnvironment $psi $ModelRoot

    $p = [System.Diagnostics.Process]::Start($psi)
    try {
        # Handle vor dem Ende holen, sonst ist es nach dem Exit nicht mehr zu haben.
        $handle = $p.Handle
        $out = $p.StandardOutput.ReadToEndAsync()
        $err = $p.StandardError.ReadToEndAsync()
        $p.WaitForExit()
        Write-Utf8Text $StdoutPath $out.GetAwaiter().GetResult()
        Write-Utf8Text $StderrPath $err.GetAwaiter().GetResult()
        $counters = New-Object DiktierUltraTest.Native+PROCESS_MEMORY_COUNTERS
        $size = [System.Runtime.InteropServices.Marshal]::SizeOf($counters)
        $peak = $null
        if ([DiktierUltraTest.Native]::GetProcessMemoryInfo($handle, [ref] $counters, $size)) {
            $peak = [uint64] $counters.PeakWorkingSetSize
            if ($peak -eq 0) { $peak = $null }
        }
        return [pscustomobject]@{ ExitCode = $p.ExitCode; PeakWorkingSet = $peak }
    } finally {
        $p.Dispose()
    }
}

<#
    Eine Zeile Ausgabe eines kurzen Aufrufs (`--version`), ohne Konsole.
    Rückgabe: ExitCode und stdout (getrimmt).
#>
function Invoke-DiktierLine([string] $Exe, [string[]] $Arguments, [string] $ModelRoot) {
    $psi = [System.Diagnostics.ProcessStartInfo]::new($Exe)
    foreach ($a in $Arguments) { $psi.ArgumentList.Add($a) }
    Set-ChildEnvironment $psi $ModelRoot
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardOutputEncoding = $script:Utf8NoBom
    $p = [System.Diagnostics.Process]::Start($psi)
    try {
        $out = $p.StandardOutput.ReadToEndAsync()
        $null = $p.StandardError.ReadToEndAsync()
        $p.WaitForExit()
        return [pscustomobject]@{ ExitCode = $p.ExitCode; Output = $out.GetAwaiter().GetResult().Trim() }
    } finally {
        $p.Dispose()
    }
}

<#
    JSONL lesen und gegen die Liste prüfen: pro Datei genau $PerFile Zeilen in
    Listenreihenfolge, `file` gleich dem Listeneintrag, Status bekannt. Bricht
    sonst ab (ein unvollständiger Lauf darf nie als Teilergebnis durchgehen).
#>
function Read-DiktierJsonl([string] $Path, [string[]] $Files, [int] $PerFile = 1) {
    $lines = @(Get-Content -LiteralPath $Path -Encoding utf8 | Where-Object { $_ -ne "" })
    if ($lines.Count -ne $Files.Count * $PerFile) {
        throw "$([System.IO.Path]::GetFileName($Path)): $($lines.Count) Zeilen, erwartet $($Files.Count * $PerFile)"
    }
    $rows = New-Object System.Collections.Generic.List[object]
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $row = $lines[$i] | ConvertFrom-Json
        $expected = $Files[[math]::Floor($i / $PerFile)]
        if ($row.file -cne $expected) {
            throw "$([System.IO.Path]::GetFileName($Path)): Zeile $($i + 1) gehört nicht zum Listeneintrag $([math]::Floor($i / $PerFile) + 1)"
        }
        if ($row.status -cnotin @("text", "rejected", "error")) {
            throw "$([System.IO.Path]::GetFileName($Path)): Zeile $($i + 1) hat unbekannten Status"
        }
        if ($PerFile -gt 1 -and $row.run -ne ($i % $PerFile) + 1) {
            throw "$([System.IO.Path]::GetFileName($Path)): Zeile $($i + 1) hat falsches run"
        }
        $rows.Add($row)
    }
    return , $rows.ToArray()
}

<#
    W3: Exitcode und Zeilenstatus müssen zusammenpassen. `--transcribe-list`
    endet mit 1 genau dann, wenn eine Zeile `error` hat (auch ein
    gescheiterter Warmup ist seit WP2e eine `error`-Zeile). Alles andere ist
    ein Widerspruch und bricht ab — nie ein »fehlerfreier« Lauf daraus.
#>
function Assert-ExitMatchesRows([int] $ExitCode, [object[]] $Rows, [string] $Label) {
    if ($ExitCode -notin @(0, 1)) { throw "$Label endete mit $ExitCode" }
    $errors = @($Rows | Where-Object { $_.status -ceq "error" }).Count
    if ($ExitCode -eq 1 -and $errors -eq 0) { throw "$Label meldet Exit 1 ohne Zeile mit error" }
    if ($ExitCode -eq 0 -and $errors -gt 0) { throw "$Label meldet Exit 0 trotz $errors Zeile(n) mit error" }
    return $errors
}

function Assert-ModelDirs([string] $Root, [string[]] $Keys) {
    foreach ($k in $Keys) {
        $dir = Join-Path $Root "diktier\models\$k"
        if (-not (Test-Path -LiteralPath $dir -PathType Container)) {
            throw "Modellverzeichnis fehlt: $dir"
        }
    }
}

# ----------------------------------------------------------- Inventur (W4/W7)

$script:WavNameRe = '^rec_(\d{4})-(\d{2})-(\d{2})T(\d{2})-(\d{2})-(\d{2})-(\d{3})Z_lauf-(\d+)(?:-(\d+))?\.wav$'
$script:LogLineRe = '^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})Z \[[^\]]*\] +[A-Z]+ +(.*)$'
# Beide echten Startzeilen (daemon/mod.rs): »(Daemon, Modell …)« und »(--foreground, Modell …)«.
$script:StartRe = '^diktier \S+ startet \((Daemon|--foreground)[,)]'
$script:GateRe = '^Lauf (\d+): Gate: '
$script:DumpRe = '^DIKTIER_DEBUG_WAV: (.+)$'
# Die Gate-Zeile steht erst nach der Inferenz im Log, die WAV trägt das
# Aufnahmeende. Fehlt die WAV, ist die Gate-Zeit so viel später »grenznah«.
$script:BoundarySlack = [timespan]::FromMinutes(2)

function ConvertFrom-WavName([string] $Name) {
    if ($Name -cnotmatch $script:WavNameRe) { return $null }
    try {
        $t = [datetime]::new([int] $Matches[1], [int] $Matches[2], [int] $Matches[3],
            [int] $Matches[4], [int] $Matches[5], [int] $Matches[6], [int] $Matches[7], [System.DateTimeKind]::Utc)
    } catch { return $null }
    return [pscustomobject]@{ Time = $t; Run = [int] $Matches[8]; Suffixed = [bool] $Matches[9] }
}

<#
    diktier.log.1, dann diktier.log, in Sitzungen zerlegt. Eine Sitzung beginnt
    mit einer Startzeile (Daemon oder --foreground). Zeilen vor der ersten
    Startzeile gehören zu einer Sitzung »ohne Startzeile« (Rotation hat den
    Anfang abgeschnitten); sie wird nie mit einer anderen verschmolzen.
    Je Sitzung: Gate-Zeilen (Lauf → Zeit) und Dump-Zeilen (WAV-Name → Zeit).
#>
function Read-DaemonSessions([string] $Dir) {
    $sessions = New-Object System.Collections.Generic.List[object]
    $files = @()
    $current = $null
    $inv = [System.Globalization.CultureInfo]::InvariantCulture
    $styles = [System.Globalization.DateTimeStyles]::AssumeUniversal -bor [System.Globalization.DateTimeStyles]::AdjustToUniversal
    foreach ($name in @("diktier.log.1", "diktier.log")) {
        $path = Join-Path $Dir $name
        if (-not (Test-Path -LiteralPath $path)) { continue }
        $files += $name
        foreach ($line in [System.IO.File]::ReadLines($path, [System.Text.Encoding]::UTF8)) {
            if ($line -cnotmatch $script:LogLineRe) { continue }
            $t = [datetime]::ParseExact($Matches[1], "yyyy-MM-ddTHH:mm:ss", $inv, $styles)
            $msg = $Matches[2]
            if ($msg -cmatch $script:StartRe) {
                $current = [pscustomobject]@{
                    Index = $sessions.Count; Start = $t; Kind = $Matches[1]; HasStart = $true
                    Gates = @{}; DuplicateRuns = 0; Dumps = @{}
                }
                $sessions.Add($current)
                continue
            }
            if ($null -eq $current) {
                $current = [pscustomobject]@{
                    Index = $sessions.Count; Start = $null; Kind = "ohne Startzeile"; HasStart = $false
                    Gates = @{}; DuplicateRuns = 0; Dumps = @{}
                }
                $sessions.Add($current)
            }
            if ($msg -cmatch $script:GateRe) {
                $run = [int] $Matches[1]
                if ($current.Gates.ContainsKey($run)) { $current.DuplicateRuns++ } else { $current.Gates[$run] = $t }
            } elseif ($msg -cmatch $script:DumpRe) {
                $current.Dumps[[System.IO.Path]::GetFileName($Matches[1].Trim())] = $t
            }
        }
    }
    return [pscustomobject]@{ Files = $files; Sessions = $sessions.ToArray() }
}

function Format-SessionLabel($Session) {
    if ($Session.HasStart) { return "$($Session.Start.ToString('s'))Z ($($Session.Kind))" }
    return "Nr. $($Session.Index + 1) ohne Startzeile"
}

<#
    Inventur für den Zeitraum [FromUtc, ToUtc) (beide optional).

    Zuordnung WAV → Lauf, ereignisbezogen: erst über die Dump-Zeile
    »DIKTIER_DEBUG_WAV: …\<name>« der Sitzung, sonst über die Zeit (letzte
    Startzeile ≤ WAV-Zeit) und die Laufnummer im Namen. In einer Sitzung ohne
    Startzeile und vor der ersten Startzeile gibt es keine Zeitzuordnung:
    solche WAVs und unbelegte Läufe sind »mehrdeutig«, nicht »fehlend«.

    Zeitraum: Eine WAV zählt nach ihrem Namenszeitstempel (Aufnahmeende).
    Ein Lauf ohne WAV zählt nach seiner Gate-Zeit; liegt die höchstens
    BoundarySlack hinter From oder To, ist er »grenznah« (die Aufnahme kann
    vor der Grenze geendet haben). WAVs ohne auswertbaren Zeitstempel sind ein
    Inventurproblem und zählen nie zur Grundgesamtheit (W7).
#>
function Get-Inventory([string] $Dir, [string] $Logs, $FromUtc, $ToUtc) {
    $log = Read-DaemonSessions $Logs
    $inPeriod = {
        param([datetime] $T)
        if ($null -ne $FromUtc -and $T -lt $FromUtc) { return $false }
        if ($null -ne $ToUtc -and $T -ge $ToUtc) { return $false }
        return $true
    }
    $nearBoundary = {
        param([datetime] $T)
        foreach ($b in @($FromUtc, $ToUtc)) {
            if ($null -ne $b -and $T -ge $b -and $T -lt $b + $script:BoundarySlack) { return $true }
        }
        return $false
    }
    $known = @($log.Sessions | Where-Object HasStart)
    $dumpOwner = @{}
    foreach ($s in $log.Sessions) { foreach ($n in $s.Dumps.Keys) { $dumpOwner[$n] = $s } }

    # Erst alle WAVs ihren Läufen zuordnen, auch die außerhalb des Zeitraums:
    # sonst sähe ein Lauf, dessen Aufnahme knapp vor From endete, wie
    # »fehlend« aus. Gezählt werden danach nur WAVs im Zeitraum.
    $runWav = @{}
    $wavs = New-Object System.Collections.Generic.List[string]
    $untimed = New-Object System.Collections.Generic.List[string]
    $counts = [ordered]@{ ohne_logeintrag = 0; mehrdeutig_wav = 0; namenssuffix = 0; ausserhalb = 0 }
    $truncatedFirst = $log.Sessions.Count -gt 0 -and -not $log.Sessions[0].HasStart
    foreach ($f in Get-ChildItem -LiteralPath $Dir -File -Filter *.wav | Sort-Object Name) {
        $info = ConvertFrom-WavName $f.Name
        if ($null -eq $info) { $untimed.Add($f.Name); continue }
        $inside = & $inPeriod $info.Time
        if ($inside) {
            if ($info.Suffixed) { $counts.namenssuffix++ }
            $wavs.Add($f.FullName)
        } else {
            $counts.ausserhalb++
        }
        $session = $null
        if ($dumpOwner.ContainsKey($f.Name)) {
            $session = $dumpOwner[$f.Name]
        } else {
            foreach ($s in $known) { if ($s.Start -le $info.Time) { $session = $s } }
            # Vor der ersten Startzeile, und die erste Sitzung ist abgeschnitten:
            # die WAV kann aus ihr oder aus einer verlorenen stammen.
            if ($null -eq $session -and $truncatedFirst) {
                if ($inside) { $counts.mehrdeutig_wav++ }
                continue
            }
        }
        if ($null -eq $session -or -not $session.Gates.ContainsKey($info.Run)) {
            if ($inside) { $counts.ohne_logeintrag++ }
            continue
        }
        $key = "$($session.Index)|$($info.Run)"
        if ($runWav.ContainsKey($key)) {
            if ($inside) { $counts.mehrdeutig_wav++ }
            continue
        }
        $runWav[$key] = [pscustomobject]@{ Path = $f.FullName; Inside = $inside }
    }

    $expected = 0; $present = 0; $ambiguous = 0; $boundary = 0
    $missing = New-Object System.Collections.Generic.List[object]
    foreach ($s in $log.Sessions) {
        foreach ($run in ($s.Gates.Keys | Sort-Object)) {
            $key = "$($s.Index)|$run"
            if ($runWav.ContainsKey($key)) {
                # Zeitraum nach dem Aufnahmeende der WAV, nicht nach der Gate-Zeit.
                if ($runWav[$key].Inside) { $expected++; $present++ }
                continue
            }
            $gateTime = $s.Gates[$run]
            if (-not (& $inPeriod $gateTime)) { continue }
            if (-not $s.HasStart) { $ambiguous++; continue }
            $expected++
            if (& $nearBoundary $gateTime) { $boundary++ }
            $missing.Add([pscustomobject]@{ Sitzung = (Format-SessionLabel $s); Lauf = $run })
        }
        $ambiguous += $s.DuplicateRuns
    }
    return [pscustomobject]@{
        LogFiles = $log.Files
        Sessions = @($log.Sessions | ForEach-Object { Format-SessionLabel $_ })
        SessionsWithoutStart = @($log.Sessions | Where-Object { -not $_.HasStart }).Count
        Expected = $expected
        Present = $present
        Missing = $missing.ToArray()
        MissingNearBoundary = $boundary
        AmbiguousRuns = $ambiguous
        AmbiguousWavs = $counts.mehrdeutig_wav
        WavsWithoutLog = $counts.ohne_logeintrag
        WavsWithSuffix = $counts.namenssuffix
        WavsOutsidePeriod = $counts.ausserhalb
        WavsWithoutTimestamp = $untimed.ToArray()
        Wavs = $wavs.ToArray()
    }
}

# ------------------------------------------------------- Kriterium 4 (W8)

<#
    Urteil zu Kriterium 4, in dieser Reihenfolge: ohne festes Protokoll
    »Funktionsprobe, kein Urteil«; fehlt ein Beleg (Provenienz, ein Peak je
    Prozess) »nicht belegt« mit Ursache; ohne gepaarte Datei »nicht belegt«;
    sonst erfüllt, wenn alle Prüfungen stimmen.
#>
function Get-Kriterium4 {
    param(
        [int] $Passes, [int] $Runs, [int] $ProtocolPasses, [int] $ProtocolRuns,
        [switch] $NoDaemonCheck,
        [string[]] $ProvenanceProblems = @(),
        [string[]] $PeakMissing = @(),
        [int] $PairedCount,
        [System.Collections.IDictionary] $Checks
    )
    $protocol = New-Object System.Collections.Generic.List[string]
    if ($Passes -ne $ProtocolPasses -or $Runs -ne $ProtocolRuns) { $protocol.Add("$Passes Durchgänge × $Runs Läufe statt $ProtocolPasses × $ProtocolRuns") }
    if ($NoDaemonCheck) { $protocol.Add("ohne Daemonprüfung gemessen") }
    $missing = New-Object System.Collections.Generic.List[string]
    foreach ($p in $ProvenanceProblems) { if ($p) { $missing.Add($p) } }
    foreach ($m in $PeakMissing) { if ($m) { $missing.Add("Peak Working Set fehlt: $m") } }
    $verdict = if ($protocol.Count -gt 0) { "Funktionsprobe, kein Urteil ($($protocol -join '; '))" }
        elseif ($missing.Count -gt 0) { "nicht belegt ($($missing -join '; '))" }
        elseif ($PairedCount -eq 0) { "nicht belegt (keine Datei mit Text bei beiden Modellen)" }
        elseif (@($Checks.Values | Where-Object { -not $_ }).Count -eq 0) { "erfüllt" } else { "nicht erfüllt" }
    return [pscustomobject]@{ Verdict = $verdict; Protocol = $protocol.ToArray(); MissingEvidence = $missing.ToArray() }
}

# ------------------------------------------------------- Zeitraum (W7/F5)

<#
    Dauer eines Zeitraums in Wanduhr-Tagen der Zeitzone $ZoneId (Windows-ID,
    etwa »W. Europe Standard Time«): beide UTC-Grenzen in diese Zone
    umgerechnet, dann die Differenz der Ortszeiten. So dauert Mi 08:00 bis
    Mi 08:00 über eine Zeitumstellung genau 7 Tage, obwohl in UTC 167 bzw.
    169 Stunden dazwischen liegen. $null, wenn die Zone unbekannt ist.
#>
function Get-WallClockDays([datetime] $FromUtc, [datetime] $ToUtc, [string] $ZoneId) {
    if (-not $ZoneId) { return $null }
    try { $tz = [System.TimeZoneInfo]::FindSystemTimeZoneById($ZoneId) } catch { return $null }
    $asUtc = {
        param([datetime] $T)
        if ($T.Kind -eq [System.DateTimeKind]::Local) { return $T.ToUniversalTime() }
        return [datetime]::SpecifyKind($T, [System.DateTimeKind]::Utc)
    }
    $a = [System.TimeZoneInfo]::ConvertTimeFromUtc((& $asUtc $FromUtc), $tz)
    $b = [System.TimeZoneInfo]::ConvertTimeFromUtc((& $asUtc $ToUtc), $tz)
    return ($b - $a).TotalDays
}

# --------------------------------------------------------------- Statistik

# Nearest-Rank-Quantil (p in 0..1) einer unsortierten Zahlenliste.
function Get-Quantile([double[]] $Values, [double] $P) {
    if ($Values.Count -eq 0) { return $null }
    $sorted = $Values | Sort-Object
    $rank = [math]::Ceiling($P * $sorted.Count)
    if ($rank -lt 1) { $rank = 1 }
    return [double] @($sorted)[$rank - 1]
}

function Get-Median([double[]] $Values) {
    if ($Values.Count -eq 0) { return $null }
    $sorted = @($Values | Sort-Object)
    $n = $sorted.Count
    if ($n % 2 -eq 1) { return [double] $sorted[($n - 1) / 2] }
    return ([double] $sorted[$n / 2 - 1] + [double] $sorted[$n / 2]) / 2
}

function Format-Num($Value, [int] $Digits = 2) {
    if ($null -eq $Value) { return "—" }
    return ([double] $Value).ToString("F$Digits", [System.Globalization.CultureInfo]::InvariantCulture)
}
