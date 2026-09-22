; 地球桌面 (EarthDesk) installer hooks, included by Tauri's NSIS template.
; The installer runs elevated (perMachine), so the privileged one-time setup
; of the bundled hardware monitor happens here and the app itself never needs
; administrator rights afterwards.

!macro EARTHDESK_CHECK_DOTNET
  ; LibreHardwareMonitor needs .NET Framework 4.7.2+ (built into Windows 10
  ; 1803 and later and every Windows 11, so this almost never fires).
  ReadRegDWORD $0 HKLM "SOFTWARE\Microsoft\NET Framework Setup\NDP\v4\Full" "Release"
  IntCmp $0 461808 dotnet_ok 0 dotnet_ok
    MessageBox MB_YESNO|MB_ICONINFORMATION "硬件监控（温度、风扇、显卡占用）需要 Microsoft .NET Framework 4.8。$\r$\n$\r$\n这台电脑还没有安装。是否现在从微软官网下载并安装？（约 120 MB，国内可直接访问）$\r$\n$\r$\n选“否”也能正常使用地球壁纸和天气，之后可以在“设置 → 硬件监控”里再装。" IDNO dotnet_ok
    DetailPrint "正在下载 .NET Framework 4.8 …"
    nsExec::ExecToLog 'powershell -NoProfile -ExecutionPolicy Bypass -Command "[Net.ServicePointManager]::SecurityProtocol=[Net.SecurityProtocolType]::Tls12; Invoke-WebRequest -UseBasicParsing -Uri https://go.microsoft.com/fwlink/?linkid=2088631 -OutFile $$env:TEMP\ndp48-offline.exe"'
    IfFileExists "$TEMP\ndp48-offline.exe" 0 dotnet_manual
      DetailPrint "正在安装 .NET Framework 4.8 …"
      ExecWait '"$TEMP\ndp48-offline.exe" /passive /norestart'
      Delete "$TEMP\ndp48-offline.exe"
      Goto dotnet_ok
    dotnet_manual:
      MessageBox MB_OK|MB_ICONEXCLAMATION "下载没有成功。将打开微软官网下载页，请手动下载安装“.NET Framework 4.8 运行时”，装好后在地球桌面的“设置 → 硬件监控”里点“一键修复”。"
      ExecShell "open" "https://dotnet.microsoft.com/zh-cn/download/dotnet-framework/net48"
  dotnet_ok:
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; Upgrading over an older copy: stop its hardware monitor so its files can
  ; be replaced.
  IfFileExists "$INSTDIR\lhm\setup-lhm.ps1" 0 +2
    nsExec::ExecToLog 'powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "$INSTDIR\lhm\setup-lhm.ps1" stop'
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro EARTHDESK_CHECK_DOTNET
  DetailPrint "正在配置硬件监控（驱动、开机自启任务、防火墙）…"
  nsExec::ExecToLog 'powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "$INSTDIR\lhm\setup-lhm.ps1" install'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  IfFileExists "$INSTDIR\lhm\setup-lhm.ps1" 0 +2
    nsExec::ExecToLog 'powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "$INSTDIR\lhm\setup-lhm.ps1" uninstall'
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "EarthDesk"
!macroend
