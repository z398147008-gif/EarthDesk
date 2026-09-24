# 下载地球桌面输入法需要内置的第三方文件(没有进版本库,见 THIRD_PARTY.md),放到
#   src-tauri/vendor/ime/
#     rime.dll          librime 1.17.0(BSD-3-Clause),输入法引擎的核心
#     data\             雾凇拼音 rime-ice 的方案和词库(GPL-3.0)+ OpenCC 的简繁/Emoji 数据
# 通常不用手动运行:tools/build-ime.ps1 发现缺文件时会自动调用。

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$librime = "1.17.0"
$build   = "33e7814"
$rimeUrl = "https://github.com/rime/librime/releases/download/$librime/rime-$build-Windows-msvc-x64.7z"
$depsUrl = "https://github.com/rime/librime/releases/download/$librime/rime-deps-$build-Windows-msvc-x64.7z"
$iceUrl  = "https://github.com/iDvel/rime-ice/releases/download/nightly/full.zip"
$zrUrl   = "https://www.7-zip.org/a/7zr.exe"

$root = Join-Path $PSScriptRoot ".."
$dest = Join-Path $root "src-tauri\vendor\ime"
$data = Join-Path $dest "data"
$work = Join-Path $env:TEMP "earthdesk-ime-fetch"

function Get($url, $file) {
    Write-Host "  下载 $url …" -ForegroundColor Gray
    Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $file
}

if (Test-Path $work) { Remove-Item $work -Recurse -Force }
New-Item -ItemType Directory -Force -Path $work, $dest, (Join-Path $data "opencc") | Out-Null

# .7z 要用 7-Zip 解;官网的 7zr.exe 是单文件的命令行版,用完即删。
$zr = Join-Path $work "7zr.exe"
Get $zrUrl $zr

# librime:只要 rime.dll(静态链接了 C 运行库和 lua/octagram/predict 插件)。
$a = Join-Path $work "rime.7z"
Get $rimeUrl $a
& $zr x -y "-o$work\rime" $a | Out-Null
$dll = Get-ChildItem (Join-Path $work "rime") -Recurse -Filter "rime.dll" | Select-Object -First 1
if (-not $dll) { throw "librime 压缩包里没有 rime.dll" }
Copy-Item $dll.FullName (Join-Path $dest "rime.dll") -Force

# OpenCC 数据(简繁转换):在 librime 的依赖包里。
$a = Join-Path $work "deps.7z"
Get $depsUrl $a
& $zr x -y "-o$work\deps" $a | Out-Null
$occ = Get-ChildItem (Join-Path $work "deps") -Recurse -Filter "s2t.json" | Select-Object -First 1
if (-not $occ) { throw "librime 依赖包里没有 OpenCC 数据" }
Copy-Item (Join-Path $occ.DirectoryName "*") (Join-Path $data "opencc") -Force

# 雾凇拼音:只拿微软双拼要用到的方案、词库和 lua 脚本。
$a = Join-Path $work "rime-ice.zip"
Get $iceUrl $a
Expand-Archive -Path $a -DestinationPath (Join-Path $work "ice") -Force
$schema = Get-ChildItem (Join-Path $work "ice") -Recurse -Filter "double_pinyin_mspy.schema.yaml" | Select-Object -First 1
if (-not $schema) { throw "雾凇拼音压缩包里没有 double_pinyin_mspy.schema.yaml" }
$ice = $schema.DirectoryName
foreach ($f in @("default.yaml", "double_pinyin_mspy.schema.yaml", "rime_ice.dict.yaml",
                 "melt_eng.schema.yaml", "melt_eng.dict.yaml",
                 "radical_pinyin.schema.yaml", "radical_pinyin.dict.yaml", "symbols_caps_v.yaml")) {
    Copy-Item (Join-Path $ice $f) (Join-Path $data $f) -Force
}
foreach ($d in @("lua", "en_dicts")) {
    $t = Join-Path $data $d
    if (Test-Path $t) { Remove-Item $t -Recurse -Force }
    Copy-Item (Join-Path $ice $d) $t -Recurse -Force
}
New-Item -ItemType Directory -Force -Path (Join-Path $data "cn_dicts") | Out-Null
foreach ($f in @("8105", "base", "ext", "tencent", "others")) {
    Copy-Item (Join-Path $ice "cn_dicts\$f.dict.yaml") (Join-Path $data "cn_dicts\$f.dict.yaml") -Force
}
Copy-Item (Join-Path $ice "opencc\*") (Join-Path $data "opencc") -Force
Copy-Item (Join-Path $ice "LICENSE") (Join-Path $data "LICENSE-rime-ice.txt") -Force

Remove-Item $work -Recurse -Force
Write-Host "  好了:$dest" -ForegroundColor Green
