# 下载打包时需要内置的第三方程序(没有进版本库,见 THIRD_PARTY.md),放到
#   src-tauri/vendor/sensors/
# 里面只留地球桌面的硬件监控服务真正要用的东西:LibreHardwareMonitor 0.9.6 的
# 库文件(不要它的界面程序)和它自带的 PawnIO 驱动安装器。
# 通常不用手动运行:tools/build-sensors.ps1 发现缺文件时会自动调用。

$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$version = "v0.9.6"
$url  = "https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/releases/download/$version/LibreHardwareMonitor-net472.zip"
$root = Join-Path $PSScriptRoot ".."
$dest = Join-Path $root "src-tauri\vendor\sensors"
$tmp  = Join-Path $env:TEMP "lhm-$version.zip"
$work = Join-Path $env:TEMP "lhm-$version"

# 服务用到的全部程序集(LibreHardwareMonitorLib 及其依赖)。
$libs = @(
    "LibreHardwareMonitorLib.dll",
    "HidSharp.dll",
    "DiskInfoToolkit.dll",
    "RAMSPDToolkit-NDD.dll",
    "BlackSharp.Core.dll",
    "System.Memory.dll",
    "System.Buffers.dll",
    "System.Numerics.Vectors.dll",
    "System.Runtime.CompilerServices.Unsafe.dll"
)

New-Item -ItemType Directory -Force -Path $dest | Out-Null

# 旧版本(1.0.0)下载到过 src-tauri/vendor/lhm,能用就直接从那里拿,省一次下载。
$old = Join-Path $root "src-tauri\vendor\lhm"
$src = $null
$pawnSrc = $null
if ((Test-Path (Join-Path $old "LibreHardwareMonitorLib.dll")) -and (Test-Path (Join-Path $old "PawnIO_setup.exe"))) {
    $src = $old
    $pawnSrc = Join-Path $old "PawnIO_setup.exe"
    Write-Host "使用已有的 src-tauri\vendor\lhm …" -ForegroundColor Gray
} else {
    Write-Host "下载 LibreHardwareMonitor $version …" -ForegroundColor Gray
    Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $tmp
    if (Test-Path $work) { Remove-Item $work -Recurse -Force }
    Expand-Archive -Path $tmp -DestinationPath $work -Force
    Remove-Item $tmp -Force
    $src = Split-Path -Parent (Get-ChildItem $work -Recurse -Filter "LibreHardwareMonitorLib.dll" | Select-Object -First 1).FullName
    # PawnIO 的安装器就在发行包里,没有的话单独下一份。
    $pawn = Get-ChildItem $work -Recurse -Filter "PawnIO_setup.exe" | Select-Object -First 1
    if ($pawn) {
        $pawnSrc = $pawn.FullName
    } else {
        Write-Host "发行包里没有 PawnIO,单独下载 …" -ForegroundColor Gray
        $pawnSrc = Join-Path $work "PawnIO_setup.exe"
        Invoke-WebRequest -UseBasicParsing -Uri "https://github.com/namazso/PawnIO/releases/latest/download/PawnIO_setup.exe" -OutFile $pawnSrc
    }
}

foreach ($f in $libs) {
    Copy-Item (Join-Path $src $f) (Join-Path $dest $f) -Force
}
Copy-Item $pawnSrc (Join-Path $dest "PawnIO_setup.exe") -Force
Write-Host "好了:$dest" -ForegroundColor Green
