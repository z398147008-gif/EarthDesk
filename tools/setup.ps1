
# 外部程序偶尔往 stderr 写无害的提示,不要因为这个中断整个脚本
$ErrorActionPreference = "Continue"
$ProgressPreference = "SilentlyContinue"

function Say($msg)  { Write-Host "  $msg" -ForegroundColor Gray }
function Step($msg) { Write-Host "`n>> $msg" -ForegroundColor Cyan }
function Good($msg) { Write-Host "  [OK] $msg" -ForegroundColor Green }
function Warn($msg) { Write-Host "  [!] $msg" -ForegroundColor Yellow }

Write-Host ""
Write-Host "===========================================" -ForegroundColor White
Write-Host "  地球桌面 - 第 1 步:安装编译环境" -ForegroundColor White
Write-Host "===========================================" -ForegroundColor White
Say "这一步只需要做一次。全程大约 20-30 分钟,大部分时间它自己在下载。"
Say "中途可以去干别的,但不要关掉这个窗口。"

# ---------------------------------------------------------------- WebView2
Step "检查 WebView2(组件的显示引擎)"
$wv = "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"
if (Test-Path $wv) {
    Good "已安装"
} else {
    Say "没找到,正在下载安装..."
    $tmp = Join-Path $env:TEMP "MicrosoftEdgeWebview2Setup.exe"
    try {
        Invoke-WebRequest "https://go.microsoft.com/fwlink/p/?LinkId=2124703" -OutFile $tmp -ErrorAction Stop
    } catch {
        Warn "下载失败:$($_.Exception.Message)"
        Warn "请手动下载安装 WebView2:https://developer.microsoft.com/microsoft-edge/webview2/"
        Read-Host "`n按回车关闭"
        exit 1
    }
    Start-Process -FilePath $tmp -ArgumentList "/silent","/install" -Wait
    Good "装好了"
}

# ---------------------------------------------------- Visual Studio C++ 工具
Step "检查 C++ 生成工具(Rust 在 Windows 上靠它链接程序)"
$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
$haveVc = $false
if (Test-Path $vswhere) {
    $found = & $vswhere -products * -latest `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath 2>$null
    if ($found) { $haveVc = $true }
}

if ($haveVc) {
    Good "已安装"
} else {
    Say "没找到。这一项最大(几个 GB),也最花时间。"
    $winget = Get-Command winget -ErrorAction SilentlyContinue
    if (-not $winget) {
        Warn "你的系统没有 winget,我没法自动装。"
        Warn "请手动下载并安装:https://aka.ms/vs/17/release/vs_BuildTools.exe"
        Warn "安装界面里勾选【使用 C++ 的桌面开发】,装完再运行一次这个脚本。"
        Read-Host "`n按回车关闭"
        exit 1
    }
    Say "正在通过 winget 安装,请耐心等待,进度条不动也是正常的..."
    winget install --id Microsoft.VisualStudio.2022.BuildTools -e --silent `
        --accept-package-agreements --accept-source-agreements `
        --override "--quiet --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
    if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne -1978335189) {
        Warn "winget 返回了错误码 $LASTEXITCODE。"
        Warn "如果它说「已经安装」,那没问题,继续就行。否则请手动安装:"
        Warn "https://aka.ms/vs/17/release/vs_BuildTools.exe (勾选【使用 C++ 的桌面开发】)"
    }
    Good "这一步完成"
}

# ---------------------------------------------------------------------- Rust
Step "检查 Rust"
$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if (Test-Path $cargoBin) { $env:Path = "$cargoBin;$env:Path" }

if (Get-Command cargo -ErrorAction SilentlyContinue) {
    Good ((cargo --version) -join "")
} else {
    Say "没装,正在下载 rustup..."
    $tmp = Join-Path $env:TEMP "rustup-init.exe"
    try {
        Invoke-WebRequest "https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe" -OutFile $tmp -ErrorAction Stop
    } catch {
        Warn "下载失败:$($_.Exception.Message)"
        Warn "请手动从 https://rustup.rs 安装 Rust,再运行一次这个脚本。"
        Read-Host "`n按回车关闭"
        exit 1
    }
    Say "正在安装 Rust(几分钟)..."
    & $tmp -y --default-toolchain stable --profile default --no-modify-path | Out-Null
    $env:Path = "$cargoBin;$env:Path"
    # 让以后的新窗口也能找到 cargo
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($userPath -notlike "*$cargoBin*") {
        [Environment]::SetEnvironmentVariable("Path", "$userPath;$cargoBin", "User")
    }
    Good ((cargo --version) -join "")
}

# ------------------------------------------------------------------ tauri-cli
Step "检查 Tauri 打包工具"
if (Get-Command cargo-tauri -ErrorAction SilentlyContinue) {
    Good "已安装"
} else {
    Say "正在编译安装 tauri-cli,这一步大约 5-10 分钟,屏幕会刷很多绿色的 Compiling..."
    cargo install tauri-cli --version "^2" --locked
    if ($LASTEXITCODE -ne 0) {
        Warn "tauri-cli 安装失败。上面红色的 error 是原因,截图发给我。"
        Read-Host "`n按回车关闭"
        exit 1
    }
    Good "装好了"
}

Write-Host ""
Write-Host "===========================================" -ForegroundColor Green
Write-Host "  环境装好了。" -ForegroundColor Green
Write-Host "  接下来双击【2-运行.bat】" -ForegroundColor Green
Write-Host "===========================================" -ForegroundColor Green
Read-Host "`n按回车关闭"
