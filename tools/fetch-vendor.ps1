# 下载打包时需要内置的第三方程序(没有进版本库,见 THIRD_PARTY.md):
#   src-tauri/vendor/lhm/  LibreHardwareMonitor 0.9.6 + 它自带的 PawnIO 驱动安装器
# 直接右键"使用 PowerShell 运行",或者在仓库根目录 pwsh tools/fetch-vendor.ps1

$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$version = "v0.9.6"
$url  = "https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/releases/download/$version/LibreHardwareMonitor-net472.zip"
$root = Join-Path $PSScriptRoot ".."
$dest = Join-Path $root "src-tauri\vendor\lhm"
$tmp  = Join-Path $env:TEMP "lhm-$version.zip"
$work = Join-Path $env:TEMP "lhm-$version"

Write-Host "下载 LibreHardwareMonitor $version …" -ForegroundColor Gray
Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $tmp

if (Test-Path $work) { Remove-Item $work -Recurse -Force }
Expand-Archive -Path $tmp -DestinationPath $work -Force

# PawnIO 的安装器就在发行包里(Resources\PawnIO_setup.exe),没有的话单独下一份。
$pawn = Get-ChildItem $work -Recurse -Filter "PawnIO_setup.exe" | Select-Object -First 1
if (-not $pawn) {
    Write-Host "发行包里没有 PawnIO,单独下载 …" -ForegroundColor Gray
    $p = Join-Path $work "PawnIO_setup.exe"
    Invoke-WebRequest -UseBasicParsing -Uri "https://github.com/namazso/PawnIO/releases/latest/download/PawnIO_setup.exe" -OutFile $p
    $pawn = Get-Item $p
}

New-Item -ItemType Directory -Force -Path $dest | Out-Null
$src = Split-Path -Parent (Get-ChildItem $work -Recurse -Filter "LibreHardwareMonitor.exe" | Select-Object -First 1).FullName
Copy-Item "$src\*" $dest -Recurse -Force
Copy-Item $pawn.FullName (Join-Path $dest "PawnIO_setup.exe") -Force
# 调试符号和文档不需要跟着安装包走
Get-ChildItem $dest -Include *.pdb, *.xml -Recurse | Remove-Item -Force
foreach ($lang in @("de","es","fr","it","pl","ru","sv","tr")) {
    $d = Join-Path $dest $lang
    if (Test-Path $d) { Remove-Item $d -Recurse -Force }
}

# 启动参数:开机最小化到托盘、只对本机开 8085 端口的 Web 服务。
@'
<?xml version="1.0" encoding="utf-8"?>
<configuration>
  <appSettings>
    <add key="startMinMenuItem" value="true" />
    <add key="minTrayMenuItem" value="true" />
    <add key="minCloseMenuItem" value="true" />
    <add key="runWebServerMenuItem" value="true" />
    <add key="listenerIp" value="127.0.0.1" />
    <add key="listenerPort" value="8085" />
    <add key="authenticationEnabled" value="false" />
  </appSettings>
</configuration>
'@ | Set-Content -Path (Join-Path $dest "LibreHardwareMonitor.config") -Encoding UTF8

Remove-Item $tmp -Force
Write-Host "好了:$dest" -ForegroundColor Green
