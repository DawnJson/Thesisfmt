# Collect pagination from Word / WPS through COM automation. ASCII only: the script arrives on stdin in the console code page.
# Inputs (environment): TFMT_PROGID, TFMT_PROC, TFMT_DOC, TFMT_OUT, TFMT_PIDFILE, TFMT_PARAS, TFMT_TABLES.
# Output: UTF-8 JSON written to TFMT_OUT, either the layout or {"error": "..."}.
$ErrorActionPreference = 'Stop'
$utf8 = New-Object Text.UTF8Encoding($false)
function Write-Out($text) { [IO.File]::WriteAllText($env:TFMT_OUT, $text, $utf8) }
function J($s) { ConvertTo-Json -InputObject ([string]$s) -Compress }
# Word returns a non-breaking hyphen as char 30; the engine reads <w:noBreakHyphen/> as '-'
function Clean($s) { [regex]::Replace(([string]$s).Replace([char]30, '-'), '[\x00-\x08\x0b-\x1f]', '') }

$info = @{}
# Physical page of a range; also caches that page's displayed number and number style.
function PageOf($r) {
  $p = [int]$r.Information(3)
  if (-not $info.ContainsKey($p)) {
    $info[$p] = @([int]$r.Information(1), [int]$r.Sections.Item(1).Footers.Item(1).PageNumbers.NumberStyle)
  }
  return $p
}

$app = $null
$doc = $null
$mine = $false
try {
  $before = @(Get-Process -Name $env:TFMT_PROC -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
  $app = New-Object -ComObject $env:TFMT_PROGID
  $new = @(Get-Process -Name $env:TFMT_PROC -ErrorAction SilentlyContinue | Where-Object { $before -notcontains $_.Id } | ForEach-Object { $_.Id })
  if ($new.Count -gt 0) {
    # A process we started: safe to hide, silence and quit. Otherwise we are attached to the user's running instance
    # and must only close our own document.
    $mine = $true
    [IO.File]::WriteAllText($env:TFMT_PIDFILE, ($new -join ','))
    $app.Visible = $false
    $app.DisplayAlerts = 0
    try { $app.AutomationSecurity = 3 } catch {}
  }
  $m = [Type]::Missing
  # ReadOnly, not in recent files, invisible window.
  $doc = $app.Documents.Open($env:TFMT_DOC, $false, $true, $false, $m, $m, $false, $m, $m, $m, $m, $false)
  $doc.Repaginate()
  $doc.Bookmarks.ShowHidden = $true
  $pages = [int]$doc.ComputeStatistics(2)
  $np = [int]$env:TFMT_PARAS
  $nt = [int]$env:TFMT_TABLES

  $pg = New-Object 'int[]' $np
  $rg = New-Object 'object[]' $np
  for ($i = 0; $i -lt $np; $i++) {
    try { $r = $doc.Bookmarks.Item("_tfmt_$i").Range } catch { continue }
    $rg[$i] = $r
    $pg[$i] = PageOf $r
  }
  $paras = New-Object Text.StringBuilder
  for ($i = 0; $i -lt $np; $i++) {
    if ($pg[$i] -eq 0) { continue }
    $next = 0
    for ($j = $i + 1; $j -lt $np; $j++) { if ($pg[$j] -ne 0) { $next = $pg[$j]; break } }
    $e = $pg[$i]
    # The paragraph can only end on a later page when the next one starts on a later page.
    if ($next -eq 0 -or $next -gt $e) {
      $pr = $rg[$i].Paragraphs.Item(1).Range
      $end = $pr.End - 1
      if ($end -ge $pr.Start) { $e = [int]$doc.Range($end, $end).Information(3) }
    }
    $d = $info[$pg[$i]]
    if ($paras.Length -gt 0) { [void]$paras.Append(',') }
    [void]$paras.Append('{"i":' + $i + ',"p":' + $pg[$i] + ',"e":' + $e + ',"d":' + $d[0] + ',"f":' + $d[1] + '}')
  }

  $tables = New-Object Text.StringBuilder
  for ($t = 0; $t -lt $nt; $t++) {
    try {
      $sr = $doc.Bookmarks.Item("_tfmt_t${t}_s").Range
      $s = PageOf $sr
      $e = PageOf ($doc.Bookmarks.Item("_tfmt_t${t}_e").Range)
    } catch { continue }
    # Spanning table: 0-based index of the first row on each later page (row start = start of its first cell).
    $rows = New-Object Text.StringBuilder
    if ($e -gt $s) {
      try {
        $last = -1
        $prev = $s
        foreach ($c in $sr.Tables.Item(1).Range.Cells) {
          $k = [int]$c.RowIndex
          if ($k -eq $last) { continue }
          $last = $k
          $at = $c.Range.Start
          $p = [int]$doc.Range($at, $at).Information(3)
          if ($p -gt $prev) {
            if ($rows.Length -gt 0) { [void]$rows.Append(',') }
            [void]$rows.Append($k - 1)
            $prev = $p
          }
        }
      } catch { $rows.Clear() | Out-Null }
    }
    if ($tables.Length -gt 0) { [void]$tables.Append(',') }
    [void]$tables.Append('{"i":' + $t + ',"s":' + $s + ',"e":' + $e + ',"r":[' + $rows.ToString() + ']}')
  }

  $tocs = New-Object Text.StringBuilder
  foreach ($toc in $doc.TablesOfContents) {
    $fields = @($toc.Range.Fields)
    $code = ''
    $entries = New-Object Text.StringBuilder
    foreach ($f in $fields) {
      if ($f.Type -eq 13 -and $code -eq '') { $code = $f.Code.Text; continue }
      if ($f.Type -ne 37) { continue }
      $line = Clean ($f.Result.Paragraphs.Item(1).Range.Text)
      $k = $line.LastIndexOf("`t")
      $text = if ($k -ge 0) { $line.Substring(0, $k) } else { $line }
      $cached = Clean ($f.Result.Text)
      $bm = ''
      if ($f.Code.Text -match 'PAGEREF\s+(\S+)') { $bm = $Matches[1] }
      $target = 'null'
      try {
        $br = $doc.Bookmarks.Item($bm).Range
        $p = PageOf $br
        $tp = $br.Paragraphs.Item(1).Range
        $target = '{"d":' + $info[$p][0] + ',"f":' + $info[$p][1] + ',"text":' + (J (Clean ($tp.Text))) + ',"list":' + (J ($tp.ListFormat.ListString)) + '}'
      } catch {}
      if ($entries.Length -gt 0) { [void]$entries.Append(',') }
      $bmj = if ($bm) { J $bm } else { 'null' }
      [void]$entries.Append('{"text":' + (J $text) + ',"cached":' + (J $cached) + ',"bm":' + $bmj + ',"target":' + $target + '}')
    }
    if ($tocs.Length -gt 0) { [void]$tocs.Append(',') }
    [void]$tocs.Append('{"code":' + (J $code) + ',"entries":[' + $entries.ToString() + ']}')
  }

  Write-Out ('{"name":' + (J $env:TFMT_NAME) + ',"version":' + (J $app.Version) + ',"pages":' + $pages +
    ',"paras":[' + $paras.ToString() + '],"tables":[' + $tables.ToString() + '],"tocs":[' + $tocs.ToString() + ']}')
} catch {
  Write-Out ('{"error":' + (J $_.Exception.Message) + '}')
} finally {
  if ($doc) { try { $doc.Close(0) } catch {} }
  if ($app -and $mine) { try { $app.Quit() } catch {} }
}
