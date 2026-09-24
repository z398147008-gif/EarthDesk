# EarthDesk: set up / remove the built-in hardware monitor service.
#
# Must run elevated. Called by the installer (install / stop / uninstall) and
# by the "repair" button on the settings page (install; one UAC prompt).
#
# Keep this file ASCII-only: Windows PowerShell 5.1 reads a BOM-less script
# in the ANSI code page, and any non-ASCII byte can break parsing.
param([string]$Mode = 'install')

$ErrorActionPreference = 'Continue'
$dir  = Split-Path -Parent $MyInvocation.MyCommand.Path
$exe  = Join-Path $dir 'EarthDeskSensors.exe'
$svc  = 'EarthDeskSensors'
$data = Join-Path $env:ProgramData 'EarthDesk'

# Start: SY/BA full; interactive users (IU) may query, start and stop it, so
# EarthDesk can restart a stuck monitor without a UAC prompt.
$sddl = 'D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;CCLCSWRPWPLOCRRC;;;IU)(A;;CCLCSWLOCRRC;;;SU)'

function Log([string]$m) {
    try {
        New-Item -ItemType Directory -Force -Path $data | Out-Null
        Add-Content -Path (Join-Path $data 'setup.log') -Encoding UTF8 -Value ((Get-Date -Format 'yyyy-MM-dd HH:mm:ss') + " [$Mode] " + $m)
    } catch {}
}

function Stop-Ours {
    if (Get-Service -Name $svc -ErrorAction SilentlyContinue) {
        & sc.exe stop $svc | Out-Null
        for ($i = 0; $i -lt 30; $i++) {
            $s = Get-Service -Name $svc -ErrorAction SilentlyContinue
            if (-not $s -or $s.Status -eq 'Stopped') { break }
            Start-Sleep -Milliseconds 500
        }
    }
    # A hung instance does not answer the stop request; its files still have
    # to be replaceable.
    Get-Process -Name 'EarthDeskSensors' -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and ($_.Path -ieq $exe) } |
        Stop-Process -Force -ErrorAction SilentlyContinue
}

# Version 1.0.0 ran the LibreHardwareMonitor program itself from a logon task,
# with a tray icon and a web server on port 8085. Take all of that away.
function Remove-Legacy {
    $legacy = Join-Path (Split-Path -Parent $dir) 'lhm'
    Stop-ScheduledTask -TaskName 'EarthDesk-LHM' -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName 'EarthDesk-LHM' -Confirm:$false -ErrorAction SilentlyContinue
    Get-Process -Name 'LibreHardwareMonitor' -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and $_.Path.StartsWith($legacy, [System.StringComparison]::OrdinalIgnoreCase) } |
        Stop-Process -Force -ErrorAction SilentlyContinue
    & netsh.exe advfirewall firewall delete rule name=EarthDesk-LHM | Out-Null
}

if ($Mode -eq 'stop') {
    Stop-Ours
    exit 0
}

if ($Mode -eq 'uninstall') {
    Stop-Ours
    & sc.exe delete $svc | Out-Null
    Remove-Legacy
    Log 'removed'
    exit 0
}

Log "installing from $dir"
Remove-Legacy

# 1. PawnIO: the signed driver LibreHardwareMonitor reads the motherboard and
#    CPU sensors through. Install it when missing or older than ours; leave it
#    alone otherwise (other tools such as FanControl use it too).
$pawn = Join-Path $dir 'PawnIO_setup.exe'
if (Test-Path $pawn) {
    $have = $null
    foreach ($k in @('HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PawnIO',
                     'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\PawnIO')) {
        $p = Get-ItemProperty -Path $k -ErrorAction SilentlyContinue
        if ($p -and $p.DisplayVersion) { $have = $p.DisplayVersion; break }
    }
    $ours = (Get-Item $pawn).VersionInfo.ProductVersion
    # The uninstall entry can be missing while the driver is installed and
    # working; do not re-run the installer over it then.
    if (-not $have -and (Test-Path 'HKLM:\SYSTEM\CurrentControlSet\Services\PawnIO')) { $have = $ours }
    $need = -not $have
    if (-not $need) {
        try { $need = ([version]($ours -replace '[^0-9.].*$', '')) -gt ([version]($have -replace '[^0-9.].*$', '')) } catch { $need = $false }
    }
    if ($need) {
        Log "PawnIO: installed '$have', bundled '$ours' -> installing"
        $p = Start-Process -FilePath $pawn -ArgumentList '-install', '-silent' -Wait -PassThru -WindowStyle Hidden
        Log ("PawnIO setup exit code " + $p.ExitCode)
        $now = Get-ItemProperty -Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PawnIO' -ErrorAction SilentlyContinue
        Log ("PawnIO now registered: '" + $(if ($now) { $now.DisplayVersion } else { '' }) + "'")
    } else {
        Log "PawnIO $have already installed"
    }
}

# 2. The service: LocalSystem, starts with Windows, restarted by the service
#    manager if it ever stops unexpectedly.
$bin = '"' + $exe + '"'
Stop-Ours
if (Get-Service -Name $svc -ErrorAction SilentlyContinue) {
    # Not "sc.exe config binPath=": Windows PowerShell 5.1 drops the embedded
    # quotes when passing them to a native program, which would register an
    # unquoted path containing spaces. Win32_Service.Change goes through the
    # API directly.
    $w = Get-CimInstance -ClassName Win32_Service -Filter "Name='$svc'" -ErrorAction SilentlyContinue
    if ($w) {
        $r = Invoke-CimMethod -InputObject $w -MethodName Change -Arguments @{ PathName = $bin; StartMode = 'Automatic' }
        Log ("updated service path, result " + $r.ReturnValue)
    }
} else {
    New-Service -Name $svc -BinaryPathName $bin -DisplayName 'EarthDesk Hardware Sensors' `
        -StartupType Automatic | Out-Null
}
& sc.exe description $svc 'Temperatures, fans and GPU load for EarthDesk (LibreHardwareMonitorLib). No window, no network access.' | Out-Null
& sc.exe failure $svc reset= 86400 actions= restart/3000/restart/10000/restart/30000 | Out-Null
& sc.exe failureflag $svc 1 | Out-Null
& sc.exe sdset $svc $sddl | Out-Null

# 3. Start it now.
Start-Service -Name $svc -ErrorAction SilentlyContinue
$s = Get-Service -Name $svc -ErrorAction SilentlyContinue
Log ("service status: " + $(if ($s) { $s.Status } else { 'missing' }))
exit 0
