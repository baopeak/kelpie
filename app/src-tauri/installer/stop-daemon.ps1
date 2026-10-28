# Stops the Kelpie programs that would keep the installer from replacing files — never the
# daemon copy the sessions run on (`<data dir>\daemon\a` or `\b`, see daemon_runtime.rs):
# installing over a running Kelpie leaves every session running.
#
#   -Mode install    programs directly in the install folder: a daemon started by Kelpie 0.1.x
#                    (before daemon copies) and its console hosts. Stopped gracefully first, so
#                    its sessions come back interrupted and agents resume.
#   -Mode uninstall  everything under the install folder and the daemon copies, which are then
#                    removed.
#
# Exit code 3: such a daemon runs and this installer was started from one of its sessions (it
# would be stopped along with it); the installer explains and stops.
param(
  [Parameter(Mandatory = $true)][string]$InstallDir,
  [string]$DataDir = '',
  [ValidateSet('install', 'uninstall')][string]$Mode = 'install'
)
$ErrorActionPreference = 'SilentlyContinue'
$fs = New-Object -ComObject Scripting.FileSystemObject
# One spelling for every path (8.3 short names where enabled): Windows may report either.
function Short([string]$path) { if ($path -and $fs.FolderExists($path)) { $fs.GetFolder($path).ShortPath.TrimEnd('\') } }
$dir = Short $InstallDir
$copies = Short (Join-Path $DataDir 'daemon')
if (-not $dir -and -not $copies) { exit 0 }

function Test-Under([string]$parent, [string]$root) {
  $root -and $parent.StartsWith("$root\", [StringComparison]::OrdinalIgnoreCase)
}
function Get-Ours {
  @(Get-CimInstance Win32_Process -Filter "Name='kelpied.exe' OR Name='OpenConsole.exe' OR Name='kelpie-stt.exe'" | Where-Object {
      if (-not $_.ExecutablePath -or -not $fs.FileExists($_.ExecutablePath)) { return $false }
      $parent = $fs.GetParentFolderName($fs.GetFile($_.ExecutablePath).ShortPath)
      if ($dir -and $parent -ieq $dir) { return $true }
      $Mode -eq 'uninstall' -and ((Test-Under $parent $dir) -or ($copies -and ($parent -ieq $copies -or (Test-Under $parent $copies))))
    })
}

$daemon = @(Get-Ours | Where-Object { $_.Name -eq 'kelpied.exe' })
if ($daemon.Count -gt 0) {
  if ($env:KELPIE_SESSION_ID) { exit 3 }
  & $daemon[0].ExecutablePath stop --wait --timeout-ms 15000 | Out-Null
  # Kelpie 0.1.x ignores --wait: give it the time a graceful stop takes to save its sessions.
  $daemon | ForEach-Object { Wait-Process -Id $_.ProcessId -Timeout 15 }
}
$deadline = (Get-Date).AddSeconds(3)
do {
  $left = Get-Ours
  if ($left.Count -eq 0) { break }
  Start-Sleep -Milliseconds 250
} while ((Get-Date) -lt $deadline)
$left | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
if ($Mode -eq 'uninstall' -and $DataDir) {
  Start-Sleep -Milliseconds 500
  Remove-Item -LiteralPath (Join-Path $DataDir 'daemon') -Recurse -Force
}
exit 0
