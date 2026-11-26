<#
.SYNOPSIS
Prepares a fresh Windows (Server 2022 / 10 / 11) machine to build and run Kelpie's debug app and daemon for the e2e scripts and the
README screenshots (scripts/e2e/readme_shots.py, boot_smoke.py, window_boot.py, screen.py).

.DESCRIPTION
Run as an administrator, from the desktop session (the display mode and the screen helper need the desktop). Over ssh, pass
-ViaDesktopTask: the script then starts itself again as a one-shot scheduled task in the logged-on desktop session and returns at once;
follow the log (default C:\kelpie-vm\setup.log) until it prints "setup finished" or "setup failed".

What it does, each step skipped when already done (so it can be run again to update and rebuild):
  1. WebView2 runtime (Evergreen bootstrapper).
  2. Visual Studio 2022 Build Tools with the MSVC x64 tools and a Windows SDK, unless vswhere finds MSVC already.
  3. Rust stable (rustup, x86_64-pc-windows-msvc host), pnpm 11 (npm), the Python packages the e2e scripts import (playwright with
     Chromium, websockets, cryptography, psutil, Pillow), tauri-driver (cargo install --locked) and an msedgedriver matching the
     installed WebView2 (MSEDGEDRIVER and PATH, machine-wide).
  4. Clones the repository, or fetches it and checks out -Ref (detached at origin/<Ref>; a checkout with local changes is left alone).
  5. Builds: pnpm install --frozen-lockfile, pnpm build:phone, KELPIE_REQUIRE_PHONE=1 cargo build -p kelpied -p kelpie-screen
     -p kelpie-window, then the debug app with its frontend embedded (pnpm tauri build --debug --no-bundle). With -ScreenRef, the screen
     helper is built from that branch instead (its own checkout and target folder) and copied next to the app.
  6. A standard (non-administrator) local user, -ShareUser, for screen sharing: the app does not share the screen while elevated. Its
     random password is written only to <StateDir>\share-user.secret, readable by Administrators and SYSTEM; it is never printed.
  7. Sets the display to -Width x -Height when the display offers that mode, and logs the modes it offers.
  8. Generates the README demo projects (scripts/e2e/readme_demo.py) into -DemoRoot when that folder does not exist yet.
Each step's time is in <StateDir>\setup.json, next to the tool versions.

.EXAMPLE
powershell -ExecutionPolicy Bypass -File setup_windows_vm.ps1
.EXAMPLE
powershell -ExecutionPolicy Bypass -File setup_windows_vm.ps1 -ViaDesktopTask -ScreenRef feat/screen-no-gpu
#>
param(
    [string]$Repo = "https://github.com/baopeak/kelpie",
    [string]$Dir = "C:\kelpie",
    [string]$Ref = "master",
    [string]$ScreenRef = "",
    [string]$DemoRoot = "C:\kelpie-demo-projects",
    [string]$StateDir = "C:\kelpie-vm",
    [string]$ShareUser = "kelpieshare",
    [int]$Width = 1920,
    [int]$Height = 1080,
    [switch]$SkipBuild,
    [switch]$ViaDesktopTask
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
New-Item -ItemType Directory -Force $StateDir | Out-Null
$Log = Join-Path $StateDir "setup.log"

if ($ViaDesktopTask) {
    # Start this script again in the logged-on desktop session (a scheduled task, interactive, highest privileges) and return.
    $user = (Get-CimInstance Win32_ComputerSystem).UserName
    if (-not $user) { throw "nobody is logged on to the desktop: log on first, or run this script there" }
    $taskArgs = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "`"$PSCommandPath`"", "-Repo", $Repo, "-Dir", "`"$Dir`"", "-Ref", $Ref,
        "-DemoRoot", "`"$DemoRoot`"", "-StateDir", "`"$StateDir`"", "-ShareUser", $ShareUser, "-Width", $Width, "-Height", $Height)
    if ($ScreenRef) { $taskArgs += @("-ScreenRef", $ScreenRef) }
    if ($SkipBuild) { $taskArgs += "-SkipBuild" }
    $name = "kelpie-vm-setup"
    Unregister-ScheduledTask -TaskName $name -Confirm:$false -ErrorAction SilentlyContinue
    $action = New-ScheduledTaskAction -Execute "powershell.exe" -Argument ($taskArgs -join " ")
    $principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Highest
    $settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Hours 4) -AllowStartIfOnBatteries
    Register-ScheduledTask -TaskName $name -Action $action -Principal $principal -Settings $settings | Out-Null
    Start-ScheduledTask -TaskName $name
    Write-Output "started in the desktop session of $user; log: $Log"
    return
}

Start-Transcript -Path $Log -Force | Out-Null
$timings = [ordered]@{}
$total = [Diagnostics.Stopwatch]::StartNew()

function Step([string]$name, [scriptblock]$body) {
    Write-Output "== $name"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    & $body
    $timings[$name] = [math]::Round($sw.Elapsed.TotalSeconds)
    Write-Output "== $name done in $($timings[$name]) s"
}

# A native command that must succeed (PowerShell 5 does not stop on a native exit code). Its output, stderr included, goes through
# the pipeline so the transcript keeps it; stderr lines are not errors here (git and cargo report progress there).
function Run([string]$exe) {
    $eap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try { & $exe @args 2>&1 | ForEach-Object { "$_" } } finally { $ErrorActionPreference = $eap }
    if ($LASTEXITCODE -ne 0) { throw "$exe $($args -join ' ') failed with exit code $LASTEXITCODE" }
}

function RefreshPath {
    $machine = [Environment]::GetEnvironmentVariable("Path", "Machine")
    $user = [Environment]::GetEnvironmentVariable("Path", "User")
    $env:Path = "$machine;$user;$env:USERPROFILE\.cargo\bin;$env:APPDATA\npm"
}

function AddMachinePath([string]$p) {
    $machine = [Environment]::GetEnvironmentVariable("Path", "Machine")
    if (($machine -split ";") -notcontains $p) { [Environment]::SetEnvironmentVariable("Path", "$machine;$p", "Machine") }
}

function Download([string]$url, [string]$name) {
    $dl = Join-Path $StateDir "downloads"
    New-Item -ItemType Directory -Force $dl | Out-Null
    $file = Join-Path $dl $name
    Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $file
    $file
}

function WebView2Version {
    foreach ($k in "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
        "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}") {
        $pv = (Get-ItemProperty $k -ErrorAction SilentlyContinue).pv
        if ($pv -and $pv -ne "0.0.0.0") { return $pv }
    }
    $null
}

function Msvc {
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path $vswhere)) { return $null }
    & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
}

try {
    Write-Output "Kelpie VM setup on $env:COMPUTERNAME as $env:USERNAME (session $((Get-Process -Id $PID).SessionId)), $(Get-Date -Format s)"
    RefreshPath

    Step "webview2" {
        if (WebView2Version) { Write-Output "WebView2 $(WebView2Version) present"; return }
        $f = Download "https://go.microsoft.com/fwlink/p/?LinkId=2124703" "MicrosoftEdgeWebview2Setup.exe"
        $p = Start-Process $f -ArgumentList "/silent", "/install" -Wait -PassThru
        if (-not (WebView2Version)) { throw "the WebView2 installer exited with $($p.ExitCode) and no runtime is registered" }
        Write-Output "WebView2 $(WebView2Version) installed"
    }

    Step "msvc" {
        if (Msvc) { Write-Output "MSVC found in $(Msvc)"; return }
        $f = Download "https://aka.ms/vs/17/release/vs_BuildTools.exe" "vs_BuildTools.exe"
        $p = Start-Process $f -Wait -PassThru -ArgumentList "--quiet", "--wait", "--norestart", "--nocache",
            "--add", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "--add", "Microsoft.VisualStudio.Component.Windows11SDK.22621"
        if ($p.ExitCode -notin 0, 3010) { throw "Build Tools installer exited with $($p.ExitCode)" }
        if (-not (Msvc)) { throw "Build Tools installed but vswhere finds no MSVC" }
        Write-Output "MSVC installed in $(Msvc) (exit $($p.ExitCode))"
    }

    Step "rust" {
        if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
            $f = Download "https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe" "rustup-init.exe"
            Run $f -y --profile minimal --default-toolchain stable --default-host x86_64-pc-windows-msvc
            RefreshPath
        } else {
            Run rustup update stable
        }
        Run rustup default stable
        Write-Output (rustc --version)
    }

    Step "pnpm" {
        $have = (Get-Command pnpm -ErrorAction SilentlyContinue) -and ((pnpm --version) -like "11.*")
        if (-not $have) { Run npm install -g pnpm@11; RefreshPath }
        Write-Output "pnpm $(pnpm --version)"
    }

    Step "python-packages" {
        Run python -m pip install --upgrade --quiet --disable-pip-version-check playwright websockets cryptography psutil Pillow
        Run python -m playwright install chromium
    }

    Step "tauri-driver" {
        if (Get-Command tauri-driver -ErrorAction SilentlyContinue) { Write-Output "tauri-driver present"; return }
        Run cargo install tauri-driver --locked
    }

    Step "msedgedriver" {
        $ver = WebView2Version
        if (-not $ver) { throw "no WebView2 runtime to match" }
        $dir = "C:\tools\msedgedriver"
        $exe = Join-Path $dir "msedgedriver.exe"
        $current = if (Test-Path $exe) { ((& $exe --version) -split " ")[3] } else { "" }
        if ($current -ne $ver) {
            $zip = Download "https://msedgedriver.microsoft.com/$ver/edgedriver_win64.zip" "edgedriver_win64.zip"
            New-Item -ItemType Directory -Force $dir | Out-Null
            Expand-Archive $zip -DestinationPath $dir -Force
        }
        [Environment]::SetEnvironmentVariable("MSEDGEDRIVER", $exe, "Machine")
        $env:MSEDGEDRIVER = $exe
        AddMachinePath $dir
        RefreshPath
        Write-Output (& $exe --version)
    }

    Step "repository" {
        if (-not (Test-Path (Join-Path $Dir ".git"))) {
            Run git clone $Repo $Dir
        }
        Run git -C $Dir fetch origin --prune
        if (git -C $Dir status --porcelain --untracked-files=no) {
            Write-Output "local changes in ${Dir}: left as they are ($(git -C $Dir rev-parse --short HEAD))"
        } else {
            Run git -C $Dir checkout --quiet --detach "origin/$Ref"
            Write-Output "$Dir at origin/$Ref $(git -C $Dir log -1 --format='%h %s')"
        }
    }

    if (-not $SkipBuild) {
        Step "frontend" {
            Push-Location (Join-Path $Dir "app")
            try {
                Run pnpm install --frozen-lockfile
                Run pnpm build:phone
            } finally { Pop-Location }
        }

        Step "daemon-and-helpers" {
            Push-Location $Dir
            try {
                $env:KELPIE_REQUIRE_PHONE = "1"
                Run cargo build -p kelpied -p kelpie-screen -p kelpie-window
            } finally { Pop-Location }
        }

        Step "debug-app" {
            Push-Location (Join-Path $Dir "app")
            try { Run pnpm tauri build --debug --no-bundle } finally { Pop-Location }
        }

        if ($ScreenRef) {
            Step "screen-helper-from-$ScreenRef" {
                # Its own checkout and target folder: one target folder shared by two checkouts can run the other's binary.
                $sdir = "$Dir-screen"
                if (-not (Test-Path (Join-Path $sdir ".git"))) { Run git clone $Repo $sdir }
                Run git -C $sdir fetch origin --prune
                Run git -C $sdir checkout --quiet --force --detach "origin/$ScreenRef"
                Push-Location $sdir
                try { Run cargo build -p kelpie-screen } finally { Pop-Location }
                Copy-Item (Join-Path $sdir "target\debug\kelpie-screen.exe") (Join-Path $Dir "target\debug\kelpie-screen.exe") -Force
                Write-Output "kelpie-screen.exe from origin/$ScreenRef $(git -C $sdir log -1 --format='%h') copied next to the app"
            }
        }
    }

    Step "share-user" {
        if (Get-LocalUser -Name $ShareUser -ErrorAction SilentlyContinue) { Write-Output "user $ShareUser present"; return }
        $bytes = New-Object byte[] 18
        [Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
        # Letters, digits and a symbol, so any complexity policy takes it.
        $plain = "Kp9!" + ([Convert]::ToBase64String($bytes) -replace "[+/=]", "x")
        $secure = ConvertTo-SecureString $plain -AsPlainText -Force
        New-LocalUser -Name $ShareUser -Password $secure -PasswordNeverExpires -AccountNeverExpires -Description "Kelpie screen sharing (standard user)" | Out-Null
        Add-LocalGroupMember -Group "Users" -Member $ShareUser
        Add-LocalGroupMember -Group "Remote Desktop Users" -Member $ShareUser -ErrorAction SilentlyContinue
        $secret = Join-Path $StateDir "share-user.secret"
        Set-Content -Path $secret -Value $plain -NoNewline
        Remove-Variable plain
        Run icacls $secret /inheritance:r /grant:r "*S-1-5-32-544:F" "*S-1-5-18:F"
        Write-Output "user $ShareUser created (standard); password in $secret"
    }

    Step "display" {
        Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class KelpieDisplay {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct DEVMODE {
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmDeviceName;
        public short dmSpecVersion, dmDriverVersion, dmSize, dmDriverExtra; public int dmFields;
        public int dmPositionX, dmPositionY, dmDisplayOrientation, dmDisplayFixedOutput;
        public short dmColor, dmDuplex, dmYResolution, dmTTOption, dmCollate;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmFormName;
        public short dmLogPixels; public int dmBitsPerPel, dmPelsWidth, dmPelsHeight, dmDisplayFlags, dmDisplayFrequency;
        public int dmICMMethod, dmICMIntent, dmMediaType, dmDitherType, dmReserved1, dmReserved2, dmPanningWidth, dmPanningHeight;
    }
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern bool EnumDisplaySettings(string dev, int mode, ref DEVMODE dm);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int ChangeDisplaySettingsEx(string dev, ref DEVMODE dm, IntPtr hwnd, int flags, IntPtr param);
    public static DEVMODE Blank() { var d = new DEVMODE(); d.dmSize = (short)Marshal.SizeOf(typeof(DEVMODE)); return d; }
}
"@
        $modes = @()
        for ($i = 0; ; $i++) {
            $dm = [KelpieDisplay]::Blank()
            if (-not [KelpieDisplay]::EnumDisplaySettings($null, $i, [ref]$dm)) { break }
            $modes += "$($dm.dmPelsWidth)x$($dm.dmPelsHeight)"
        }
        $modes = $modes | Select-Object -Unique
        Write-Output "display modes: $($modes -join ', ')"
        $cur = [KelpieDisplay]::Blank()
        [void][KelpieDisplay]::EnumDisplaySettings($null, -1, [ref]$cur)
        Write-Output "current: $($cur.dmPelsWidth)x$($cur.dmPelsHeight)"
        if ($cur.dmPelsWidth -eq $Width -and $cur.dmPelsHeight -eq $Height) { return }
        if ($modes -notcontains "${Width}x$Height") { Write-Output "${Width}x$Height is not offered: left as it is"; return }
        $cur.dmPelsWidth = $Width; $cur.dmPelsHeight = $Height; $cur.dmFields = 0x80000 -bor 0x100000
        # CDS_UPDATEREGISTRY: kept after a reboot.
        $r = [KelpieDisplay]::ChangeDisplaySettingsEx($null, [ref]$cur, [IntPtr]::Zero, 1, [IntPtr]::Zero)
        Write-Output "set ${Width}x${Height}: $(if ($r -eq 0) { 'done' } else { "failed ($r; not from the desktop session?)" })"
    }

    Step "demo-projects" {
        if (Test-Path $DemoRoot) { Write-Output "$DemoRoot present"; return }
        Run python -X utf8 (Join-Path $Dir "scripts\e2e\readme_demo.py") $DemoRoot
        Run python (Join-Path $DemoRoot "atlas-commerce\smoke.py")
    }

    $versions = [ordered]@{
        webview2 = WebView2Version
        msvc = Msvc
        rustc = (rustc --version)
        pnpm = (pnpm --version)
        node = (node --version)
        python = (python --version)
        msedgedriver = $env:MSEDGEDRIVER
        tauri_driver = (Get-Command tauri-driver -ErrorAction SilentlyContinue).Source
        kelpie = (git -C $Dir log -1 --format="%h %s")
    }
    [ordered]@{ ok = $true; seconds = [math]::Round($total.Elapsed.TotalSeconds); steps = $timings; versions = $versions } |
        ConvertTo-Json -Depth 4 | Set-Content (Join-Path $StateDir "setup.json")
    Write-Output "setup finished in $([math]::Round($total.Elapsed.TotalMinutes, 1)) min"
} catch {
    [ordered]@{ ok = $false; error = "$_"; steps = $timings } | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $StateDir "setup.json")
    Write-Output "setup failed: $_"
    Stop-Transcript | Out-Null
    exit 1
}
Stop-Transcript | Out-Null
