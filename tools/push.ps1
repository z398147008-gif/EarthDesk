# 把这个项目推到 GitHub:https://github.com/z398147008-gif/EarthDesk
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

git config --local user.name  | Out-Null
if (-not (git config --local user.name))  { git config --local user.name  "z398147008-gif" }
if (-not (git config --local user.email)) { git config --local user.email "z398147008@gmail.com" }

# 仓库地址(已经配好,重复执行也没事)
git remote remove origin 2>$null
git remote add origin https://github.com/z398147008-gif/EarthDesk.git
git branch -M main

# git 异常退出时会留下 .git\index.lock,之后所有提交都会失败。没有 git 在运行时把它清掉。
$lock = Join-Path $root ".git\index.lock"
if ((Test-Path $lock) -and -not (Get-Process git -ErrorAction SilentlyContinue)) {
    Remove-Item $lock -Force -ErrorAction SilentlyContinue
}

# 先把本地的改动记下来(提交),再上传;没有改动就直接上传。
git add -A
if ($LASTEXITCODE -ne 0) {
    Write-Host "  记录本地改动失败(上面有原因),这次没有上传。把这个窗口截图发给我。" -ForegroundColor Yellow
    Read-Host "`n按回车关闭"
    exit 1
}
git diff --cached --quiet
if ($LASTEXITCODE -ne 0) {
    git commit -q -m ("更新 " + (Get-Date -Format "yyyy-MM-dd HH:mm"))
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  提交本地改动失败(上面有原因),这次没有上传。把这个窗口截图发给我。" -ForegroundColor Yellow
        Read-Host "`n按回车关闭"
        exit 1
    }
    Write-Host "  已记录本地改动。" -ForegroundColor Gray
}

Write-Host "  正在上传(约 25 MB,第一次会弹浏览器让你登录 GitHub)…" -ForegroundColor Gray
git push -u origin main

if ($LASTEXITCODE -eq 0) {
    Write-Host ""
    Write-Host "  上传成功:https://github.com/z398147008-gif/EarthDesk" -ForegroundColor Green
    Start-Process "https://github.com/z398147008-gif/EarthDesk"
} else {
    Write-Host ""
    Write-Host "  上传失败。最常见的原因是没登录成功,或者仓库地址写错了。" -ForegroundColor Yellow
    Write-Host "  把上面红色的字截图发我。" -ForegroundColor Yellow
}
Read-Host "`n按回车关闭"
