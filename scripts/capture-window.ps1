# Captures a top-level window of a process to PNG (dev tooling for visual checks).
#   pwsh scripts/capture-window.ps1 -Process kelpie -Out shot.png
# -ClientOnly keeps the window's content area: no frame, no title bar.
param(
  [string]$Process = "kelpie",
  [Parameter(Mandatory = $true)][string]$Out,
  [int]$ProcessId = 0,
  [switch]$ClientOnly,
  # "x,y,w,h" in the saved image's pixels: also print that area's mean brightness (0-255),
  # so a caller can tell a rendered page from an empty one.
  [string]$MeasureRect = ""
)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Win {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
}
"@
[Win]::SetProcessDPIAware() | Out-Null
$p = if ($ProcessId) {
  Get-Process -Id $ProcessId -ErrorAction Stop | Where-Object { $_.MainWindowHandle -ne 0 }
} else {
  Get-Process -Name $Process -ErrorAction Stop | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
}
if (-not $p) { throw "no window for process $Process" }
$h = $p.MainWindowHandle
$r = New-Object Win+RECT
[Win]::GetWindowRect($h, [ref]$r) | Out-Null
$w = $r.Right - $r.Left; $hgt = $r.Bottom - $r.Top
$bmp = New-Object System.Drawing.Bitmap $w, $hgt
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
# PW_RENDERFULLCONTENT (2) captures GPU/WebView content even when partially covered.
[Win]::PrintWindow($h, $hdc, 2) | Out-Null
$g.ReleaseHdc($hdc)
if ($ClientOnly) {
  $c = New-Object Win+RECT
  [Win]::GetClientRect($h, [ref]$c) | Out-Null
  $o = New-Object Win+POINT
  [Win]::ClientToScreen($h, [ref]$o) | Out-Null
  $area = New-Object System.Drawing.Rectangle ($o.X - $r.Left), ($o.Y - $r.Top), $c.Right, $c.Bottom
  $client = $bmp.Clone($area, $bmp.PixelFormat)
  $bmp.Dispose(); $bmp = $client
  $w = $c.Right; $hgt = $c.Bottom
}
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
if ($MeasureRect) {
  $x0, $y0, $mw, $mh = $MeasureRect.Split(",") | ForEach-Object { [int]$_ }
  $sum = 0.0; $n = 0
  for ($y = [Math]::Max(0, $y0); $y -lt [Math]::Min($hgt, $y0 + $mh); $y += 8) {
    for ($x = [Math]::Max(0, $x0); $x -lt [Math]::Min($w, $x0 + $mw); $x += 8) {
      $c = $bmp.GetPixel($x, $y); $sum += 0.299 * $c.R + 0.587 * $c.G + 0.114 * $c.B; $n++
    }
  }
  Write-Output ("mean {0:N0}" -f ($(if ($n) { $sum / $n } else { 0 })))
}
$g.Dispose(); $bmp.Dispose()
Write-Output "$w x $hgt -> $Out"
