# 第三方素材、数据与程序

本项目自己的代码是 MIT(见 LICENSE)。下面这些不是,各按各自的许可使用与再分发。

## 贴图与天文数据(内置在 `src/textures/`)

| 东西 | 来源 | 许可 |
| --- | --- | --- |
| 昼面 / 夜灯 / 云 / 水体掩膜 | NASA Visible Earth — Blue Marble Next Generation、Black Marble | 公有领域,要求注明 NASA |
| 海底地形(用于海水配色与法线图) | NASA Visible Earth — GEBCO 8 bathymetry/topography | 同上 |
| 银河背景 | NASA SVS 4851 "Deep Star Maps 2020"(E. T. Wright) | 公有领域,注明 NASA/Goddard |
| 月面图 | NASA LRO / CGI Moon Kit | 公有领域 |
| 星表 `stars.bin` | d3-celestial 的 8 等星表(源自 HYG Database) | BSD-3-Clause / CC-BY |

## 实时数据(联网获取,不随程序分发)

| 东西 | 来源 | 许可 |
| --- | --- | --- |
| 天气 | [Open-Meteo](https://open-meteo.com) | CC BY 4.0,非商业免费,无需 API key |
| 葵花 9 号可见光 / 红外云图 | [NASA GIBS](https://nasa-gibs.github.io/gibs-api-docs/) | 公有领域,注明 NASA EOSDIS GIBS |
| 全球云图 | [matteason/live-cloud-maps](https://github.com/matteason/live-cloud-maps) | MIT(图像源自 NOAA/NASA/JMA 等) |

## 城市库

`src/data/cities.json` 由 [GeoNames](https://www.geonames.org/) 的 `cities15000`、
`admin1CodesASCII`、`countryInfo` 和各国 `alternateNames` 生成(中日港澳台取中文名),
国家名来自 CLDR / babel。GeoNames 数据按 **CC BY 4.0** 提供,使用时需注明 GeoNames。

## 图标

天气图标是 [Meteocons](https://github.com/basmilius/weather-icons)(Bas Milius,MIT)。

## 随安装包分发的第三方程序

| 程序 | 版本 | 许可 | 源码 |
| --- | --- | --- | --- |
| LibreHardwareMonitorLib(`LibreHardwareMonitorLib.dll` 及其依赖 HidSharp、DiskInfoToolkit、RAMSPDToolkit、BlackSharp.Core、System.Memory 等) | 0.9.6 | MPL-2.0(依赖库各按其自身的开源许可) | https://github.com/LibreHardwareMonitor/LibreHardwareMonitor |
| PawnIO(`PawnIO_setup.exe`,由 LHM 发行包携带) | 随 LHM 0.9.6 | GPL-2.0-or-later | https://github.com/namazso/PawnIO |
| Microsoft Edge WebView2 Runtime(离线安装包) | 随构建时的最新版 | Microsoft 的[分发条款](https://developer.microsoft.com/microsoft-edge/webview2/) | — |

说明:

* 这些文件都以**原样、未修改**的二进制形式随安装包分发,对应源码在上表的地址,
  也可以向本项目的维护者索取(GPL-2.0 第 3 条意义上的书面要约)。
* 地球桌面的硬件监控服务 `EarthDeskSensors.exe`(源码 `sensors/EarthDeskSensors.cs`,MIT)
  作为独立程序调用 LibreHardwareMonitorLib 的公开接口(MPL-2.0 允许与其他许可的代码组合,
  库文件本身未改动);地球桌面主程序只通过本机命名管道读取服务输出的 JSON。
  与 PawnIO 的交互只经过 LHM 库和驱动的 IOCTL 接口,属于 PawnIO 明确豁免的独立模块。
* 仓库里没有这些二进制,`tools/fetch-vendor.ps1` 从上游下载到 `src-tauri/vendor/sensors/`。

## 说明

构图和配色参考了 Apple 的地球壁纸(在自己的 iPad 截图上取点、拟合相机、逐点比色),
但没有使用其中任何一个像素:所有贴图都来自上面列出的公开数据源,着色器是自己写的。
| librime(`ime/rime.dll`,输入法引擎核心,内含 librime-lua / octagram / predict 插件) | 1.17.0 | BSD-3-Clause(插件各按其许可) | https://github.com/rime/librime |
| 雾凇拼音 rime-ice(`ime/data/`:微软双拼方案、词库、lua 脚本) | nightly | GPL-3.0(数据文件,单独分发,不影响地球桌面代码的 MIT 许可) | https://github.com/iDvel/rime-ice |
| OpenCC 数据(`ime/data/opencc/`,简繁转换) | 随 librime 1.17.0 依赖包 | Apache-2.0 | https://github.com/BYVoid/OpenCC |

| Mozc 转换引擎(`ime/earthdesk_mozc.dll`,日语输入;由本仓库 GitHub Actions 从源码编译,内含 Mozc 的 OSS 词典数据) | commit 13c9898 | BSD-3-Clause(词典数据各按其许可,见 Mozc 源码 `src/data/dictionary_oss/README.txt`) | https://github.com/google/mozc |
| Microsoft Visual C++ 运行库(`msvcp140.dll` 等,随 `earthdesk_mozc.dll` 放在同目录) | 随构建机 | Microsoft 可再发行组件许可 | — |

构建时 `tools/fetch-ime.ps1` 还会临时下载 7-Zip 的 `7zr.exe`(LGPL-2.1)用来解压 librime 的发行包,
不随安装包分发。
