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
| LibreHardwareMonitor | 0.9.6 | MPL-2.0 | https://github.com/LibreHardwareMonitor/LibreHardwareMonitor |
| PawnIO(`PawnIO_setup.exe`,由 LHM 发行包携带) | 随 LHM 0.9.6 | GPL-2.0-or-later | https://github.com/namazso/PawnIO |
| Microsoft Edge WebView2 Runtime(离线安装包) | 随构建时的最新版 | Microsoft 的[分发条款](https://developer.microsoft.com/microsoft-edge/webview2/) | — |

说明:

* 这两个程序都以**原样、未修改**的二进制形式随安装包分发,对应源码在上表的地址,
  也可以向本项目的维护者索取(GPL-2.0 第 3 条意义上的书面要约)。
* 地球桌面本身只通过本机 HTTP(`127.0.0.1:8085/data.json`)读取 LibreHardwareMonitor
  的数据,不链接它的任何代码;与 PawnIO 的交互只经过 LHM 和驱动的 IOCTL 接口,
  属于 PawnIO 明确豁免的独立模块。
* 仓库里没有这些二进制,`tools/fetch-vendor.ps1` 从上游下载到 `src-tauri/vendor/lhm/`。

## 说明

构图和配色参考了 Apple 的地球壁纸(在自己的 iPad 截图上取点、拟合相机、逐点比色),
但没有使用其中任何一个像素:所有贴图都来自上面列出的公开数据源,着色器是自己写的。
