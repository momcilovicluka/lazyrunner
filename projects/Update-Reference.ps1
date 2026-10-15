# Writes the field reference in projects\README.md (between its reference markers) from profile.schema.json. Run it
# after changing the schema; tests\profile.Tests.ps1 fails while the two differ.
# Run: pwsh -NoProfile -File projects\Update-Reference.ps1
$ErrorActionPreference = 'Stop'
$LazyRunnerRoot = Split-Path $PSScriptRoot
. (Join-Path $LazyRunnerRoot 'lib\profiletools.ps1')
$ReferenceStart = '<!-- reference: made from profile.schema.json by projects\Update-Reference.ps1 - change the schema, not this -->'
$ReferenceEnd = '<!-- end of reference -->'

$readme = Join-Path $PSScriptRoot 'README.md'
$text = [IO.File]::ReadAllText($readme).Replace("`r`n", "`n")
$start = $text.IndexOf($ReferenceStart)
$end = $text.IndexOf($ReferenceEnd)
if ($start -lt 0 -or $end -lt $start) { throw "README.md needs the two reference markers:`n$ReferenceStart`n$ReferenceEnd" }
$text = $text.Substring(0, $start + $ReferenceStart.Length) + "`n`n" + (Get-ProfileReference) + "`n`n" + $text.Substring($end)
[IO.File]::WriteAllText($readme, $text)
'projects\README.md: field reference written from profile.schema.json'
