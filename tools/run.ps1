
$ErrorActionPreference = "Continue"
$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if (Test-Path $cargoBin) { $env:Path = "$cargoBin;$env:Path" }

Write-Host ""
Write-Host "===========================================" -ForegroundColor White
Write-Host "  地球桌面 - 试运行" -ForegroundColor White
Write-Host "===========================================" -ForegroundColor White

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "  找不到 Rust。请先双击【1-安装环境.bat】。" -ForegroundColor Yellow
    Read-Host "`n按回车关闭"
    exit 1
}

Write-Host "  第一次运行要编译几分钟,之后就快了。" -ForegroundColor Gray
Write-Host "  屏幕刷绿色的 Compiling 是正常的。" -ForegroundColor Gray
Write-Host ""
Write-Host "  跑起来之后:" -ForegroundColor Gray
Write-Host "    - 地球会铺满桌面,两个组件浮在上面" -ForegroundColor Gray
Write-Host "    - Ctrl + Alt + D 进入编辑模式,可以拖动和缩放组件" -ForegroundColor Gray
Write-Host "    - 右下角托盘图标可以退出" -ForegroundColor Gray
Write-Host "    - 关掉这个黑窗口 = 关掉程序" -ForegroundColor Gray
Write-Host ""

Set-Location (Join-Path $PSScriptRoot "..")
cargo tauri dev

Write-Host ""
Write-Host "  程序已退出。" -ForegroundColor Gray
Write-Host "  如果上面有红色的 error,把它截图发给我。" -ForegroundColor Yellow
Read-Host "`n按回车关闭"
