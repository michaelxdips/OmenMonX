param([Parameter(Mandatory = $true)][string]$Path)
# Colour histogram of a capture.
#
# The point is to prove a theme actually applied and the frame is not blank. A
# PrintWindow capture of an elevated window returns a single-colour bitmap, which
# this reports as "distinct colours: 1" instead of being mistaken for a dark theme.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$bmp = [System.Drawing.Bitmap]::FromFile($Path)
$counts = @{}
$amber = 0; $red = 0; $green = 0; $blue = 0; $dark = 0; $light = 0; $total = 0
for ($y = 0; $y -lt $bmp.Height; $y += 3) {
    for ($x = 0; $x -lt $bmp.Width; $x += 3) {
        $c = $bmp.GetPixel($x, $y)
        $total++
        $key = '{0:X2}{1:X2}{2:X2}' -f $c.R, $c.G, $c.B
        if ($counts.ContainsKey($key)) { $counts[$key]++ } else { $counts[$key] = 1 }
        if ($c.R -gt 0xD0 -and $c.G -lt 0x60 -and $c.B -lt 0x80) { $amber++ }
        if ($c.R -gt 0xC0 -and $c.G -lt 0x70 -and $c.B -lt 0x70) { $red++ }
        if ($c.G -gt 0x90 -and $c.R -lt 0x90 -and $c.B -lt 0xA0) { $green++ }
        if ($c.B -gt 0xB0 -and $c.R -lt 0x90) { $blue++ }
        if ($c.R -lt 0x30 -and $c.G -lt 0x30 -and $c.B -lt 0x38) { $dark++ }
        if ($c.R -gt 0xE0 -and $c.G -gt 0xE0 -and $c.B -gt 0xE0) { $light++ }
    }
}
'sampled pixels: {0}' -f $total
'distinct colours: {0}' -f $counts.Count
'dark (chrome/bg):  {0:P1}' -f ($dark / $total)
'light (light bg):  {0:P1}' -f ($light / $total)
"accent-ish: amber=$amber red=$red green=$green blue=$blue"
if ($counts.Count -le 2) { 'WARNING: blank or single-colour frame - capture failed' }
'--- top 6 colours ---'
$counts.GetEnumerator() | Sort-Object Value -Descending | Select-Object -First 6 |
    ForEach-Object { '{0}  {1,7}  {2:P1}' -f $_.Key, $_.Value, ($_.Value / $total) }
$bmp.Dispose()
