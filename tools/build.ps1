
$ErrorActionPreference = "Continue"
$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if (Test-Path $cargoBin) { $env:Path = "$cargoBin;$env:Path" }

Write-Host ""
Write-Host "===========================================" -ForegroundColor White
Write-Host "  地球桌面 - 打包成安装程序" -ForegroundColor White
Write-Host "===========================================" -ForegroundColor White
Write-Host "  装完之后它就是一个普通软件,不需要再开黑窗口。" -ForegroundColor Gray
Write-Host "  编译大约 3-5 分钟。" -ForegroundColor Gray
Write-Host ""

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "  找不到 Rust。请先双击【1-安装环境.bat】。" -ForegroundColor Yellow
    Read-Host "`n按回车关闭"
    exit 1
}

$root = Join-Path $PSScriptRoot ".."
Set-Location $root

# 先和 GitHub 同步,保证打包的是最新代码(没联网时用本地代码继续)。
& (Join-Path $PSScriptRoot "sync.ps1") -Push
if ($LASTEXITCODE -eq 1) {
    Write-Host ""
    Write-Host "  和 GitHub 同步失败,停止打包。把上面的提示截图发给我。" -ForegroundColor Yellow
    Read-Host "`n按回车关闭"
    exit 1
}
Write-Host ""

# 内置的硬件监控服务(温度、风扇、显卡):先编译好,打包时会一起装进去。
& (Join-Path $PSScriptRoot "build-sensors.ps1")
if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Host "  硬件监控服务没有准备好,停止打包。把上面的提示截图发给我。" -ForegroundColor Yellow
    Read-Host "`n按回车关闭"
    exit 1
}

# 地球桌面输入法(引擎 + 64/32 位输入法模块 + 词库):同样先编译好一起装进去。
& (Join-Path $PSScriptRoot "build-ime.ps1")
if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Host "  输入法没有准备好,停止打包。把上面的提示截图发给我。" -ForegroundColor Yellow
    Read-Host "`n按回车关闭"
    exit 1
}

# 版本号每次打包自动刷新:主、次版本取自 tauri.conf.json,第三位是 Git 提交数
# (每次同步 / 更新都会变多),所以新装的一定比旧的新,Windows「应用」里也看得出。
# 只在这次打包时覆盖,不改文件,不会和 GitHub 上的改动冲突。
$conf = Get-Content (Join-Path $root "src-tauri\tauri.conf.json") -Raw -Encoding UTF8 | ConvertFrom-Json
$parts = "$($conf.version)".Split(".")
$count = 0
if (Get-Command git -ErrorAction SilentlyContinue) {
    $n = git rev-list --count HEAD 2>$null
    if ($LASTEXITCODE -eq 0 -and $n) { $count = [int]$n }
}
if ($count -gt 0) {
    $version = "$($parts[0]).$($parts[1]).$count"
} else {
    $version = "$($conf.version)"
}
$override = Join-Path $root "src-tauri\target\version-override.json"
New-Item -ItemType Directory -Force -Path (Split-Path $override) | Out-Null
[System.IO.File]::WriteAllText($override, "{ `"version`": `"$version`" }", (New-Object System.Text.UTF8Encoding $false))
Write-Host "  本次打包的版本号:$version" -ForegroundColor Green
Write-Host ""

cargo tauri build --config "$override"

$ok = ($LASTEXITCODE -eq 0)
$out = Join-Path $root "src-tauri\target\release\bundle\nsis"
$setup = Get-ChildItem -Path $out -Filter "*-setup.exe" -ErrorAction SilentlyContinue |
         Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($ok -and $setup) {
    Write-Host ""
    Write-Host "  打包完成(版本 $version)。安装包是:" -ForegroundColor Green
    Write-Host "  $($setup.FullName)" -ForegroundColor Green
    Write-Host "  正在打开这个文件夹,双击里面的 $($setup.Name) 安装即可。" -ForegroundColor Green
    Start-Process explorer.exe $out
} else {
    Write-Host ""
    Write-Host "  打包失败,没有生成安装包。把上面红色的 Error 截图发给我。" -ForegroundColor Yellow
}
Read-Host "`n按回车关闭"
