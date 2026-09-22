
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
cargo tauri build

$ok = ($LASTEXITCODE -eq 0)
$out = Join-Path $root "src-tauri\target\release\bundle\nsis"
$setup = Get-ChildItem -Path $out -Filter "*-setup.exe" -ErrorAction SilentlyContinue |
         Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($ok -and $setup) {
    Write-Host ""
    Write-Host "  打包完成。安装包是:" -ForegroundColor Green
    Write-Host "  $($setup.FullName)" -ForegroundColor Green
    Write-Host "  正在打开这个文件夹,双击里面的 $($setup.Name) 安装即可。" -ForegroundColor Green
    Start-Process explorer.exe $out
} else {
    Write-Host ""
    Write-Host "  打包失败,没有生成安装包。把上面红色的 Error 截图发给我。" -ForegroundColor Yellow
}
Read-Host "`n按回车关闭"
