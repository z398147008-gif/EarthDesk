# 编译地球桌面输入法(源码在 ime\),结果放到 src-tauri\vendor\ime\,打包时装到 安装目录\ime\:
#   EarthDeskIME.exe     后台引擎(每个登录用户一个,不带管理员权限)
#   EarthDeskTSF.dll     64 位程序里用的输入法模块
#   EarthDeskTSF32.dll   32 位程序(老版微信、QQ 等)里用的输入法模块
#   rime.dll, data\      librime 和雾凇拼音词库(tools\fetch-ime.ps1 下载)
#
# 由 build.ps1(打包)、run.ps1(试运行)和 5-试用输入法.bat 自动调用。

$ErrorActionPreference = "Continue"
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$ime  = Join-Path $root "ime"
$dest = Join-Path $root "src-tauri\vendor\ime"
$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if (Test-Path $cargoBin) { $env:Path = "$cargoBin;$env:Path" }

New-Item -ItemType Directory -Force -Path $dest | Out-Null

# ---- 第三方文件
if (-not (Test-Path (Join-Path $dest "rime.dll")) -or -not (Test-Path (Join-Path $dest "data\double_pinyin_mspy.schema.yaml"))) {
    Write-Host "  下载输入法引擎和词库(约 25 MB,只下一次)…" -ForegroundColor Gray
    try {
        & (Join-Path $PSScriptRoot "fetch-ime.ps1")
    } catch {
        Write-Host "  下载出错:$($_.Exception.Message)" -ForegroundColor Yellow
    }
    if (-not (Test-Path (Join-Path $dest "rime.dll"))) {
        Write-Host "  输入法需要的 librime / 雾凇拼音没有下载成功(要能访问 github.com)。" -ForegroundColor Yellow
        exit 1
    }
}

# 覆盖一个可能正被别的程序加载着的文件:DLL / exe 在用时不能覆盖但能改名,
# 所以先把旧的改名挪开(下次再删)。
function Put($src, $dst) {
    Get-ChildItem -Path (Split-Path $dst) -Filter ((Split-Path $dst -Leaf) + ".old*") -ErrorAction SilentlyContinue |
        ForEach-Object { Remove-Item $_.FullName -Force -ErrorAction SilentlyContinue }
    try {
        Copy-Item $src $dst -Force -ErrorAction Stop
    } catch {
        $old = "$dst.old" + [DateTime]::Now.Ticks
        Rename-Item $dst (Split-Path $old -Leaf) -ErrorAction SilentlyContinue
        Copy-Item $src $dst -Force
    }
}

# ---- 日语引擎(可选):Mozc 的转换引擎 earthdesk_mozc.dll,由本仓库的 GitHub Actions
# (.github/workflows/mozc.yml)编译并发布到 Release。没有它输入法照样能打中文。
$mozcTag = "mozc-13c9898"
$mozcUrl = "https://github.com/z398147008-gif/EarthDesk/releases/download/$mozcTag/earthdesk_mozc.zip"
$mozcStamp = Join-Path $dest "earthdesk_mozc.tag"
$haveMozc = (Test-Path (Join-Path $dest "earthdesk_mozc.dll")) -and (Test-Path $mozcStamp) -and ((Get-Content $mozcStamp -ErrorAction SilentlyContinue) -eq $mozcTag)
if (-not $haveMozc) {
    Write-Host "  下载日语引擎(Mozc)…" -ForegroundColor Gray
    $zip = Join-Path $env:TEMP "earthdesk_mozc.zip"
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        $ProgressPreference = "SilentlyContinue"
        Invoke-WebRequest -UseBasicParsing -Uri $mozcUrl -OutFile $zip -ErrorAction Stop
        $tmp = Join-Path $env:TEMP "earthdesk_mozc"
        if (Test-Path $tmp) { Remove-Item $tmp -Recurse -Force }
        Expand-Archive -Path $zip -DestinationPath $tmp -Force
        Get-ChildItem $tmp -File | ForEach-Object { Put $_.FullName (Join-Path $dest $_.Name) }
        Set-Content -Path $mozcStamp -Value $mozcTag -Encoding ascii
        Remove-Item $zip, $tmp -Recurse -Force -ErrorAction SilentlyContinue
        Write-Host "  [OK] 日语引擎" -ForegroundColor Green
    } catch {
        Write-Host "  日语引擎还没有下载到(要先在 GitHub 上跑一次「Mozc for 地球桌面输入法」),这次先只有中文。" -ForegroundColor Yellow
    }
}

# ---- 编译
Write-Host "  编译输入法 …" -ForegroundColor Gray
$manifest = Join-Path $ime "Cargo.toml"
cargo build --release --manifest-path $manifest -p ime-engine -p ime-tsf
$ok64 = ($LASTEXITCODE -eq 0)

# 32 位模块:第一次需要装 32 位的 Rust 标准库(几十 MB)。失败不影响 64 位程序里打字。
$ok32 = $false
if ($ok64) {
    $targets = & rustup target list --installed 2>$null
    if (-not ($targets -match "i686-pc-windows-msvc")) {
        Write-Host "  安装 32 位编译支持(只做一次)…" -ForegroundColor Gray
        rustup target add i686-pc-windows-msvc
    }
    cargo build --release --manifest-path $manifest -p ime-tsf --target i686-pc-windows-msvc
    $ok32 = ($LASTEXITCODE -eq 0)
}

$rel = Join-Path $ime "target\release"
$rel32 = Join-Path $ime "target\i686-pc-windows-msvc\release"
if ($ok64) {
    Put (Join-Path $rel "EarthDeskIME.exe") (Join-Path $dest "EarthDeskIME.exe")
    Put (Join-Path $rel "EarthDeskTSF.dll") (Join-Path $dest "EarthDeskTSF.dll")
}
if ($ok32) {
    Put (Join-Path $rel32 "EarthDeskTSF.dll") (Join-Path $dest "EarthDeskTSF32.dll")
} elseif ($ok64) {
    Write-Host "  32 位输入法模块没编译成功:32 位的老程序里暂时用不了地球桌面输入法。" -ForegroundColor Yellow
}
Copy-Item (Join-Path $root "src-tauri\icons\icon.ico") (Join-Path $dest "ime.ico") -Force

if ($ok64) {
    Write-Host "  [OK] 输入法" -ForegroundColor Green
    exit 0
}
if (Test-Path (Join-Path $dest "EarthDeskIME.exe")) {
    Write-Host "  输入法编译失败,沿用上次编译好的。把上面红色的 error 截图发给我。" -ForegroundColor Yellow
    exit 0
}
Write-Host "  输入法编译失败。把上面红色的 error 截图发给我。" -ForegroundColor Yellow
exit 1
