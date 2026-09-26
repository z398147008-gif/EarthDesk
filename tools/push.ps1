# 把这个项目和 GitHub 双向同步(先拉取合并,再上传):https://github.com/z398147008-gif/EarthDesk
# 第一次运行会弹出浏览器让你登录 GitHub,登录完就会自动上传。
$ErrorActionPreference = "Continue"
$root = Join-Path $PSScriptRoot ".."
Set-Location $root

Write-Host ""
Write-Host "===========================================" -ForegroundColor White
Write-Host "  地球桌面 - 上传到 GitHub" -ForegroundColor White
Write-Host "===========================================" -ForegroundColor White
Write-Host ""

if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
    Write-Host "  这台电脑还没装 Git。" -ForegroundColor Yellow
    Write-Host "  去 https://git-scm.com/download/win 下载安装(一路下一步即可),再双击本文件。" -ForegroundColor Yellow
    Write-Host "  或者装 GitHub Desktop:https://desktop.github.com" -ForegroundColor Yellow
    Read-Host "`n按回车关闭"
    exit 1
}

# 先记录本地改动、拉取合并 GitHub 上的新改动,再上传(都在 sync.ps1 里)。
& (Join-Path $PSScriptRoot "sync.ps1") -Push
$code = $LASTEXITCODE
if ($code -eq 0) {
    Write-Host ""
    Write-Host "  同步完成:https://github.com/z398147008-gif/EarthDesk" -ForegroundColor Green
    Start-Process "https://github.com/z398147008-gif/EarthDesk"
} elseif ($code -eq 2) {
    Write-Host ""
    Write-Host "  这次没能和 GitHub 同步。最常见的原因是没联网、没登录成功。" -ForegroundColor Yellow
    Write-Host "  把上面的提示截图发我。" -ForegroundColor Yellow
} else {
    Write-Host ""
    Write-Host "  同步失败,见上面的提示。" -ForegroundColor Yellow
}
Read-Host "`n按回车关闭"
