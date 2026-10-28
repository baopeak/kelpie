# Captures a top-level window of a process to PNG (dev tooling for visual checks).
#   pwsh scripts/capture-window.ps1 -Process kelpie -Out shot.png
param(
  [string]$Process = "kelpie",
  [Parameter(Mandatory = $true)][string]$Out,
  [int]$ProcessId = 0
)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Win {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
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
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
Write-Output "$w x $hgt -> $Out"
