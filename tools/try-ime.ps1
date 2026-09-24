# 不打安装包,直接在这台电脑上试用地球桌面输入法(由 5-试用输入法.bat 调用)。
# 注册输入法要管理员权限,会弹一次"是否允许此应用对你的设备进行更改"。

$ErrorActionPreference = "Continue"
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$dest = Join-Path $root "src-tauri\vendor\ime"
$exe  = Join-Path $dest "EarthDeskIME.exe"

Write-Host ""
Write-Host "===========================================" -ForegroundColor White
Write-Host "  地球桌面 - 试用输入法" -ForegroundColor White
Write-Host "===========================================" -ForegroundColor White
Write-Host "  第一次要编译几分钟、下载约 25 MB 词库。" -ForegroundColor Gray
Write-Host ""

& (Join-Path $PSScriptRoot "build-ime.ps1")
if ($LASTEXITCODE -ne 0 -or -not (Test-Path $exe)) {
    Read-Host "`n按回车关闭"
    exit 1
}

# 旧的引擎还在跑的话先让它退出(它会先保存用户词库)。
& taskkill /IM EarthDeskIME.exe 2>$null | Out-Null
Start-Sleep -Milliseconds 800
& taskkill /F /IM EarthDeskIME.exe 2>$null | Out-Null

Write-Host "  注册输入法(需要管理员权限)…" -ForegroundColor Gray
try {
    $p = Start-Process -FilePath $exe -ArgumentList "--register" -Verb RunAs -Wait -PassThru
    if ($p.ExitCode -ne 0) { Write-Host "  注册返回了错误码 $($p.ExitCode),日志在 %APPDATA%\EarthDesk\ime\ime.log" -ForegroundColor Yellow }
} catch {
    Write-Host "  没有得到管理员权限,输入法没注册上。" -ForegroundColor Yellow
    Read-Host "`n按回车关闭"
    exit 1
}

Write-Host "  加到你的键盘列表、部署词库(第一次约一分钟)…" -ForegroundColor Gray
$p = Start-Process -FilePath $exe -ArgumentList "--enable","--deploy" -Wait -PassThru
Start-Process -FilePath $exe

Write-Host ""
Write-Host "  [OK] 装好了。" -ForegroundColor Green
Write-Host "    - Win + 空格 切换到【地球桌面输入法】(第一次可能要在任务栏右下角的输入法图标里选)" -ForegroundColor Gray
Write-Host "    - 微软双拼;Shift 切换中/英;空格上屏第一个候选,数字键选词,- = 翻页" -ForegroundColor Gray
Write-Host "    - 打不出字的话把 %APPDATA%\EarthDesk\ime\ime.log 发给我" -ForegroundColor Gray
Read-Host "`n按回车关闭"
