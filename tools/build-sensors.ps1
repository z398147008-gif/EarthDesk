# 编译地球桌面自带的硬件监控服务 EarthDeskSensors.exe(源码在 sensors\)。
#
# 用的是每台 Windows 都自带的 .NET Framework 4.x 编译器(csc.exe),不需要另装 SDK。
# 由 build.ps1(打包)和 run.ps1(试运行)自动调用。

$ErrorActionPreference = "Continue"
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$src  = Join-Path $root "sensors"
$dest = Join-Path $root "src-tauri\vendor\sensors"
$exe  = Join-Path $dest "EarthDeskSensors.exe"

if (-not (Test-Path (Join-Path $dest "LibreHardwareMonitorLib.dll")) -or -not (Test-Path (Join-Path $dest "PawnIO_setup.exe"))) {
    try {
        & (Join-Path $PSScriptRoot "fetch-vendor.ps1")
    } catch {
        Write-Host "  下载出错:$($_.Exception.Message)" -ForegroundColor Yellow
    }
    if (-not (Test-Path (Join-Path $dest "LibreHardwareMonitorLib.dll"))) {
        Write-Host "  硬件监控需要的 LibreHardwareMonitor 库没有下载成功。" -ForegroundColor Yellow
        exit 1
    }
}

Copy-Item (Join-Path $src "setup-sensors.ps1") $dest -Force
Copy-Item (Join-Path $src "EarthDeskSensors.exe.config") $dest -Force

$csc = Join-Path $env:WINDIR "Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path $env:WINDIR "Microsoft.NET\Framework\v4.0.30319\csc.exe" }
if (-not (Test-Path $csc)) {
    if (Test-Path $exe) {
        Write-Host "  没找到 csc.exe,沿用已有的 EarthDeskSensors.exe。" -ForegroundColor Yellow
        exit 0
    }
    Write-Host "  没找到 .NET Framework 编译器 csc.exe,无法编译硬件监控服务。" -ForegroundColor Yellow
    exit 1
}

Write-Host "  编译硬件监控服务 …" -ForegroundColor Gray
$tmp = Join-Path $env:TEMP "EarthDeskSensors-build.exe"
if (Test-Path $tmp) { Remove-Item $tmp -Force }
Push-Location $src
& $csc /nologo /target:exe /platform:anycpu /optimize+ /warn:1 "/out:$tmp" `
    "/reference:$(Join-Path $dest 'LibreHardwareMonitorLib.dll')" `
    /reference:System.ServiceProcess.dll /reference:System.Core.dll `
    EarthDeskSensors.cs
$ok = ($LASTEXITCODE -eq 0) -and (Test-Path $tmp)
Pop-Location

if ($ok) {
    Copy-Item $tmp $exe -Force
    Remove-Item $tmp -Force
    Write-Host "  [OK] EarthDeskSensors.exe" -ForegroundColor Green
    exit 0
}
if (Test-Path $exe) {
    Write-Host "  编译失败,沿用已有的 EarthDeskSensors.exe。把上面的错误截图发给我。" -ForegroundColor Yellow
    exit 0
}
Write-Host "  编译硬件监控服务失败。把上面的错误截图发给我。" -ForegroundColor Yellow
exit 1
