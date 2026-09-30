<#
.SYNOPSIS
    Echter TOML-Parser für release.ps1 und bench-models.ps1 (WP2f,
    Nachreview F1/F6). Wird per Dot-Source geladen.

.DESCRIPTION
    `Invoke-RealToml` ruft `scripts\toml-json.py` (Python ≥ 3.11, `tomllib`)
    auf und liefert die Datei als PowerShell-Werte: Tabellen als
    OrderedDictionary mit Ordinal-Vergleich (case-sensitive wie Rust),
    Listen als object[], Ganzzahlen als BigInteger, Text, bool. Das JSON liest
    System.Text.Json, nicht ConvertFrom-Json: Das würde ISO-Texte zu
    Datumswerten machen und Schlüssel ohne Groß-/Kleinschreibung vergleichen.

    Ohne Python ≥ 3.11 ist das Ergebnis `unavailable`, nie ein stilles „gültig“.
    Was daraus folgt, entscheidet der Aufrufer: release.ps1 bricht ab,
    bench-models.ps1 meldet »nicht belegt«.
#>

Set-StrictMode -Version Latest

# Kandidaten der Reihe nach: Programm und vorangestellte Argumente.
$script:TomlPythonCandidates = @(@("python"), @("py", "-3"))
$script:TomlJsonScript = Join-Path $PSScriptRoot "toml-json.py"

function New-OrdinalTable {
    return [System.Collections.Specialized.OrderedDictionary]::new([System.StringComparer]::Ordinal)
}

function ConvertFrom-TaggedToml([System.Text.Json.JsonElement] $Element) {
    $prop = $null
    foreach ($p in $Element.EnumerateObject()) { $prop = $p; break }
    if ($null -eq $prop) { throw "toml-json.py: leeres Element" }
    switch -CaseSensitive -Exact ($prop.Name) {
        "table" {
            $t = New-OrdinalTable
            foreach ($pair in $prop.Value.EnumerateArray()) {
                $items = @($pair.EnumerateArray())
                $t[$items[0].GetString()] = ConvertFrom-TaggedToml $items[1]
            }
            return $t
        }
        "array" {
            $list = New-Object System.Collections.Generic.List[object]
            foreach ($v in $prop.Value.EnumerateArray()) { $list.Add((ConvertFrom-TaggedToml $v)) }
            return , $list.ToArray()
        }
        "string" { return $prop.Value.GetString() }
        "int" { return [System.Numerics.BigInteger]::Parse($prop.Value.GetString(), [System.Globalization.CultureInfo]::InvariantCulture) }
        "bool" { return $prop.Value.GetBoolean() }
        "other" { return [pscustomobject]@{ TomlOther = $prop.Value.GetString() } }
        default { throw "toml-json.py: unbekannter Typ $($prop.Name)" }
    }
}

<#
    Rückgabe: Status `ok` (Value gesetzt), `invalid` (Message: Meldung von
    tomllib) oder `unavailable` (kein Python ≥ 3.11 gefunden). Python: das
    benutzte Programm.
#>
function Invoke-RealToml([string] $Path) {
    $tried = New-Object System.Collections.Generic.List[string]
    foreach ($cand in $script:TomlPythonCandidates) {
        $label = $cand -join " "
        $tried.Add($label)
        $cmd = Get-Command $cand[0] -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if (-not $cmd) { continue }
        $psi = [System.Diagnostics.ProcessStartInfo]::new($cmd.Source)
        foreach ($a in @($cand | Select-Object -Skip 1)) { $psi.ArgumentList.Add($a) }
        $psi.ArgumentList.Add($script:TomlJsonScript)
        $psi.ArgumentList.Add($Path)
        $psi.UseShellExecute = $false
        $psi.CreateNoWindow = $true
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $true
        $psi.StandardOutputEncoding = [System.Text.UTF8Encoding]::new($false)
        $psi.StandardErrorEncoding = [System.Text.UTF8Encoding]::new($false)
        # Kein __pycache__ neben dem Skript.
        $psi.Environment["PYTHONDONTWRITEBYTECODE"] = "1"
        try { $p = [System.Diagnostics.Process]::Start($psi) } catch { continue }
        try {
            $out = $p.StandardOutput.ReadToEndAsync()
            $err = $p.StandardError.ReadToEndAsync()
            if (-not $p.WaitForExit(60000)) { try { $p.Kill() } catch { }; continue }
            $p.WaitForExit()
            $code = $p.ExitCode
            $stdout = $out.GetAwaiter().GetResult()
            $stderr = $err.GetAwaiter().GetResult().Trim()
        } finally {
            $p.Dispose()
        }
        # Kein switch: `continue` darin gälte dem switch, nicht der Schleife.
        if ($code -eq 0) {
            $doc = [System.Text.Json.JsonDocument]::Parse($stdout)
            try { $value = ConvertFrom-TaggedToml $doc.RootElement } finally { $doc.Dispose() }
            return [pscustomobject]@{ Status = "ok"; Value = $value; Message = $null; Python = $label }
        }
        if ($code -eq 1) { return [pscustomobject]@{ Status = "invalid"; Value = $null; Message = $stderr; Python = $label } }
        if ($code -eq 2) { throw "toml-json.py konnte $Path nicht lesen: $stderr" }
        # 3 = Python zu alt, sonst z. B. der Store-Platzhalter: nächster Kandidat.
    }
    return [pscustomobject]@{
        Status = "unavailable"; Value = $null; Python = $null
        Message = "kein Python >= 3.11 mit tomllib gefunden (gesucht: $($tried -join ', '))"
    }
}

<#
    Vergleicht einen Wert des Teilmengenparsers aus release.ps1
    (OrderedDictionary, List[object], Array, long, string, bool) mit dem
    tomllib-Ergebnis. Rückgabe: die Abweichungen als Texte (ausgerollt,
    also mit @() einsammeln), keine = gleich.
#>
function Compare-TomlValue($Subset, $Real, [string] $Where = "") {
    $diffs = New-Object System.Collections.Generic.List[string]
    $at = if ($Where) { $Where } else { "(Wurzel)" }
    if ($Subset -is [System.Collections.IDictionary]) {
        if ($Real -isnot [System.Collections.IDictionary]) { $diffs.Add("$($at): Tabelle gegen $(Get-TomlKind $Real)"); return $diffs.ToArray() }
        $sk = @($Subset.Keys | ForEach-Object { [string] $_ })
        $rk = @($Real.Keys | ForEach-Object { [string] $_ })
        foreach ($k in $sk) { if ($rk -cnotcontains $k) { $diffs.Add("$($at): Schlüssel $k fehlt bei tomllib") } }
        foreach ($k in $rk) { if ($sk -cnotcontains $k) { $diffs.Add("$($at): Schlüssel $k fehlt beim Teilmengenparser") } }
        foreach ($k in $sk) {
            if ($rk -ccontains $k) {
                foreach ($d in (Compare-TomlValue $Subset[$k] $Real[$k] $(if ($Where) { "$Where.$k" } else { $k }))) { $diffs.Add($d) }
            }
        }
    } elseif ($Subset -is [System.Collections.IList]) {
        if ($Real -isnot [System.Collections.IList]) { $diffs.Add("$($at): Liste gegen $(Get-TomlKind $Real)"); return $diffs.ToArray() }
        if ($Subset.Count -ne $Real.Count) { $diffs.Add("$($at): $($Subset.Count) gegen $($Real.Count) Einträge"); return $diffs.ToArray() }
        for ($i = 0; $i -lt $Subset.Count; $i++) {
            foreach ($d in (Compare-TomlValue $Subset[$i] $Real[$i] "$at[$i]")) { $diffs.Add($d) }
        }
    } elseif ($Subset -is [bool]) {
        if ($Real -isnot [bool] -or $Real -ne $Subset) { $diffs.Add("$($at): $Subset gegen $Real") }
    } elseif ($Subset -is [long] -or $Subset -is [int]) {
        if ($Real -isnot [System.Numerics.BigInteger] -or $Real -ne [System.Numerics.BigInteger] $Subset) { $diffs.Add("$($at): Zahl $Subset gegen $Real") }
    } elseif ($Subset -is [string]) {
        if ($Real -isnot [string] -or $Real -cne $Subset) { $diffs.Add("$($at): Text weicht ab") }
    } else {
        $diffs.Add("$($at): unbekannter Typ $($Subset.GetType().Name)")
    }
    return $diffs.ToArray()
}

function Get-TomlKind($Value) {
    if ($null -eq $Value) { return "nichts" }
    if ($Value -is [System.Collections.IDictionary]) { return "Tabelle" }
    if ($Value -is [System.Collections.IList]) { return "Liste" }
    return $Value.GetType().Name
}
