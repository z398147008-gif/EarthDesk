# 输入法诊断:把引擎能不能启动、为什么不能启动写进 输入法诊断结果.txt(由 诊断输入法.bat 调用)。
# 不改动任何设置,只读系统日志、试着在临时目录里跑一次引擎。

$ErrorActionPreference = "Continue"
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$out  = Join-Path $root "输入法诊断结果.txt"
$lines = New-Object System.Collections.Generic.List[string]
function Add($s) { $lines.Add([string]$s); Write-Host $s }

Add "== 时间 $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')  Windows $([Environment]::OSVersion.VersionString)  ACP=$([Text.Encoding]::Default.CodePage)"

$installed = Join-Path ${env:ProgramFiles} "地球桌面\ime"
Add "== 安装目录 $installed"
Get-ChildItem $installed -ErrorAction SilentlyContinue | ForEach-Object { Add ("   {0,12}  {1}" -f $_.Length, $_.Name) }

Add "== 正在运行的引擎"
Get-Process EarthDeskIME -ErrorAction SilentlyContinue | ForEach-Object { Add "   pid $($_.Id)  $($_.Path)" }

Add "== 注册表"
$clsid = "HKLM:\SOFTWARE\Classes\CLSID\{6F1A7C52-3B8E-4D0A-9E61-2C5D8B7E4A13}\InprocServer32"
Add ("   64 位: " + (Get-ItemProperty $clsid -ErrorAction SilentlyContinue).'(default)')
Add ("   Run:   " + (Get-ItemProperty "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Run" -ErrorAction SilentlyContinue).EarthDeskIME)

Add "== 最近两天引擎崩溃记录(系统「应用程序」日志)"
try {
    Get-WinEvent -FilterHashtable @{ LogName = "Application"; StartTime = (Get-Date).AddDays(-2); Id = 1000, 1001, 1026 } -ErrorAction Stop |
        Where-Object { $_.Message -match "EarthDeskIME|EarthDeskTSF|rime.dll|earthdesk_mozc" } |
        Select-Object -First 6 | ForEach-Object {
            Add "--- $($_.TimeCreated)  事件 $($_.Id)"
            ($_.Message -split "`n" | Select-Object -First 14) | ForEach-Object { Add ("   " + $_.TrimEnd()) }
        }
} catch { Add "   (没有记录)" }

Add "== 引擎日志 %APPDATA%\EarthDesk\ime"
$ud = Join-Path $env:APPDATA "EarthDesk\ime"
Get-ChildItem $ud -Recurse -ErrorAction SilentlyContinue | Select-Object -First 40 | ForEach-Object { Add ("   {0,10}  {1}" -f $_.Length, $_.FullName.Substring($ud.Length)) }
$log = @(Get-Content (Join-Path $ud "ime.log") -Encoding UTF8 -Tail 3000 -ErrorAction SilentlyContinue)
$log | Select-Object -Last 30 | ForEach-Object { Add ("   log: " + $_) }
# 每个程序最近的记录(最后 30 行常常只有一个程序,别的程序的问题就看不到了)
$apps = $log | ForEach-Object { if ($_ -match 'dll \[([^\]]+)\]') { $Matches[1] } } | Select-Object -Unique
foreach ($app in $apps) {
    Add "   --- $app"
    $log | Where-Object { $_ -like "*dll ``[$app``]*" } | Select-Object -Last 25 | ForEach-Object { Add ("   log: " + $_) }
}
Get-ChildItem $ud -Filter "rime.*" -File -ErrorAction SilentlyContinue | ForEach-Object {
    Add "   --- $($_.Name)"
    Get-Content $_.FullName -Tail 20 | ForEach-Object { Add ("   " + $_) }
}

function Try-Engine($dir, $label) {
    Add "== 试运行引擎:$label ($dir)"
    $exe = Join-Path $dir "EarthDeskIME.exe"
    if (-not (Test-Path $exe)) { Add "   没有 $exe"; return }
    $user = Join-Path $env:TEMP ("edime-" + $label)
    if (Test-Path $user) { Remove-Item $user -Recurse -Force -ErrorAction SilentlyContinue }
    New-Item -ItemType Directory -Force -Path $user | Out-Null
    $env:EARTHDESK_IME_USER = $user
    $o = Join-Path $env:TEMP "edime-out.txt"; $e = Join-Path $env:TEMP "edime-err.txt"
    $p = Start-Process -FilePath $exe -ArgumentList "--cli", "nihk{space}" -WorkingDirectory $dir -NoNewWindow -PassThru `
        -RedirectStandardOutput $o -RedirectStandardError $e
    if (-not $p.WaitForExit(300000)) { Add "   5 分钟没结束,强制结束"; $p.Kill() }
    Add ("   退出码 {0} (0x{0:X8})" -f $p.ExitCode)
    Get-Content $o -Encoding UTF8 -ErrorAction SilentlyContinue | Select-Object -Last 8 | ForEach-Object { Add ("   out: " + $_) }
    Get-Content $e -ErrorAction SilentlyContinue | Select-Object -Last 15 | ForEach-Object { Add ("   err: " + $_) }
    Get-Content (Join-Path $user "ime.log") -Encoding UTF8 -ErrorAction SilentlyContinue | Select-Object -Last 12 | ForEach-Object { Add ("   log: " + $_) }
    Get-ChildItem $user -Filter "rime.*" -File -ErrorAction SilentlyContinue | ForEach-Object {
        Get-Content $_.FullName -Tail 10 | ForEach-Object { Add ("   rime: " + $_) }
    }
    Remove-Item Env:\EARTHDESK_IME_USER
}

Write-Host ""
Write-Host "  接下来试运行两次引擎,每次约 1-2 分钟(要重新部署词库),请等它跑完…" -ForegroundColor Gray
Try-Engine $installed "installed"

# 同一套文件放到纯英文路径下再试一次,看是不是中文路径的问题。
$ascii = Join-Path $env:PUBLIC "edime-test"
if (Test-Path $installed) {
    if (Test-Path $ascii) { Remove-Item $ascii -Recurse -Force -ErrorAction SilentlyContinue }
    Copy-Item $installed $ascii -Recurse -Force
    Try-Engine $ascii "ascii"
    Remove-Item $ascii -Recurse -Force -ErrorAction SilentlyContinue
}

$lines | Out-File -FilePath $out -Encoding utf8
Write-Host ""
Write-Host "  好了,结果在 $out ,告诉我一声就行。" -ForegroundColor Green
Read-Host "`n按回车关闭"
