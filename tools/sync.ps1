# 和 GitHub 同步(https://github.com/z398147008-gif/EarthDesk):先把本地改动记下来,
# 再拉取合并 GitHub 上的新改动(比如 Claude 推上去的),-Push 时最后再上传。
# 4-上传到GitHub、3-打包、2-运行 都先调用它,所以打包出来的一定是最新的代码。
#
# 退出码:0 已同步(或本来就是最新);2 没法同步(没装 Git、没联网),用本地代码继续;
#         1 同步失败(冲突等),调用方应该停下来。
param([switch]$Push)
$ErrorActionPreference = "Continue"
$root = Join-Path $PSScriptRoot ".."
Set-Location $root

if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
    Write-Host "  这台电脑没装 Git,没法和 GitHub 同步,用本地代码继续。" -ForegroundColor Yellow
    Write-Host "  (装 Git:https://git-scm.com/download/win ,一路下一步)" -ForegroundColor Yellow
    exit 2
}
if (-not (Test-Path (Join-Path $root ".git"))) {
    Write-Host "  这个文件夹不是 Git 仓库,没法同步,用本地代码继续。" -ForegroundColor Yellow
    exit 2
}

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

# 先把本地的改动记下来(提交),合并时才不会丢。
git add -A
if ($LASTEXITCODE -ne 0) {
    Write-Host "  记录本地改动失败(上面有原因)。把这个窗口截图发给 Claude。" -ForegroundColor Yellow
    exit 1
}
git diff --cached --quiet
if ($LASTEXITCODE -ne 0) {
    git commit -q -m ("更新 " + (Get-Date -Format "yyyy-MM-dd HH:mm"))
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  提交本地改动失败(上面有原因)。把这个窗口截图发给 Claude。" -ForegroundColor Yellow
        exit 1
    }
    Write-Host "  已记录本地改动。" -ForegroundColor Gray
}

Write-Host "  正在同步 GitHub 上的新改动…" -ForegroundColor Gray
$before = git rev-parse HEAD
git fetch origin
if ($LASTEXITCODE -ne 0) {
    Write-Host "  连不上 GitHub(上面有原因),这次没有同步,用本地代码继续。" -ForegroundColor Yellow
    exit 2
}
git rev-parse --verify -q origin/main | Out-Null
if ($LASTEXITCODE -eq 0) {
    git merge --no-edit origin/main
    if ($LASTEXITCODE -ne 0) {
        $conflicts = git diff --name-only --diff-filter=U
        git merge --abort 2>$null
        Write-Host ""
        Write-Host "  同步失败:你本地改的和 GitHub 上的改到了同一处,需要手动合并。" -ForegroundColor Yellow
        if ($conflicts) {
            Write-Host "  冲突的文件:" -ForegroundColor Yellow
            $conflicts | ForEach-Object { Write-Host "    $_" -ForegroundColor Yellow }
        }
        Write-Host "  本地文件没有被改动。把这个窗口截图发给 Claude。" -ForegroundColor Yellow
        exit 1
    }
    if ((git rev-parse HEAD) -ne $before) {
        Write-Host "  已同步 GitHub 上的新改动:" -ForegroundColor Green
        git -c core.quotepath=false diff --stat $before HEAD | Select-Object -Last 12 | ForEach-Object { Write-Host "    $_" }
    } else {
        Write-Host "  GitHub 上没有新改动,本地已是最新。" -ForegroundColor Gray
    }
}

# Claude 的改动有时先放在单独的分支上(claude/...),合并进 main 之前这里拉不到。
# 提醒一下,免得以为已经是最新。
$pending = @(git branch -r --no-merged HEAD 2>$null | ForEach-Object { $_.Trim() } | Where-Object { $_ -like "origin/claude/*" })
if ($pending.Count -gt 0) {
    Write-Host "  注意:GitHub 上还有没合并进 main 的分支(不会打包进去):" -ForegroundColor Yellow
    $pending | ForEach-Object {
        $msg = git -c core.quotepath=false log -1 --format="%cd  %s" --date=format:"%m-%d %H:%M" $_
        Write-Host "    $_   $msg" -ForegroundColor Yellow
    }
    Write-Host "  需要的话让 Claude 把它合并进 main。" -ForegroundColor Yellow
}

if ($Push) {
    $ahead = git rev-list --count origin/main..HEAD 2>$null
    if (-not $ahead -or [int]$ahead -gt 0) {
        Write-Host "  正在上传本地改动(第一次会弹浏览器让你登录 GitHub)…" -ForegroundColor Gray
        git push -u origin main
        if ($LASTEXITCODE -ne 0) {
            Write-Host "  上传失败(上面有原因),本地代码不受影响。" -ForegroundColor Yellow
            exit 2
        }
        Write-Host "  已上传。" -ForegroundColor Green
    }
}

$head = git -c core.quotepath=false log -1 --format="%h  %cd  %s" --date=format:"%Y-%m-%d %H:%M"
Write-Host "  当前代码版本:$head" -ForegroundColor Gray
exit 0
