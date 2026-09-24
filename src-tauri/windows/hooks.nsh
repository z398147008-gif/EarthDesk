; 地球桌面 (EarthDesk) installer hooks, included by Tauri's NSIS template.
; The installer runs elevated (perMachine), so the privileged one-time setup
; of the built-in hardware monitor happens here and the app itself never needs
; administrator rights afterwards.
;
; The hardware monitor is our own small Windows service (sensors\
; EarthDeskSensors.exe, built on LibreHardwareMonitorLib): no window, no tray
; icon, no network port. See sensors\setup-sensors.ps1.

!macro EARTHDESK_CHECK_DOTNET
  ; The service needs .NET Framework 4.7.2+ (built into Windows 10 1803 and
  ; later and every Windows 11, so this almost never fires).
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
      MessageBox MB_OK|MB_ICONEXCLAMATION "下载没有成功。将打开微软官网下载页，请手动下载安装“.NET Framework 4.8 运行时”，装好后在地球桌面的“设置 → 硬件监控”里点“修复”。"
      ExecShell "open" "https://dotnet.microsoft.com/zh-cn/download/dotnet-framework/net48"
  dotnet_ok:
!macroend

;  地球桌面输入法 (ime\): a TSF text service. Its DLLs are loaded into every
;  program the user types in, so an upgrade cannot overwrite them; a file in
;  use can still be renamed, so the old copy is moved aside and deleted at
;  the next reboot.
!macro EARTHDESK_IME_MOVE_ASIDE ID NAME
  IfFileExists "$INSTDIR\ime\${NAME}" 0 ime_aside_done_${ID}
    Delete "$INSTDIR\ime\${NAME}.old*"
    StrCpy $R1 0
    ime_aside_loop_${ID}:
      IntOp $R1 $R1 + 1
      IfFileExists "$INSTDIR\ime\${NAME}.old$R1" ime_aside_loop_${ID}
    Rename "$INSTDIR\ime\${NAME}" "$INSTDIR\ime\${NAME}.old$R1"
    Delete /REBOOTOK "$INSTDIR\ime\${NAME}.old$R1"
  ime_aside_done_${ID}:
!macroend

!macro EARTHDESK_IME_STOP
  ; Ask the engine to save the user dictionary and quit, then make sure.
  nsExec::Exec 'taskkill /IM EarthDeskIME.exe'
  Sleep 1000
  nsExec::Exec 'taskkill /F /IM EarthDeskIME.exe'
  !insertmacro EARTHDESK_IME_MOVE_ASIDE tsf64 "EarthDeskTSF.dll"
  !insertmacro EARTHDESK_IME_MOVE_ASIDE tsf32 "EarthDeskTSF32.dll"
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; Upgrading over an older copy: stop the running monitor so its files can
  ; be replaced.
  IfFileExists "$INSTDIR\sensors\setup-sensors.ps1" 0 +2
    nsExec::ExecToLog 'powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "$INSTDIR\sensors\setup-sensors.ps1" stop'
  ; 1.0.0 ran the LibreHardwareMonitor program from a logon task instead
  ; (tray icon, web server on 8085). Remove its task, firewall rule and files.
  IfFileExists "$INSTDIR\lhm\setup-lhm.ps1" 0 +2
    nsExec::ExecToLog 'powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "$INSTDIR\lhm\setup-lhm.ps1" uninstall'
  RMDir /r "$INSTDIR\lhm"
  !insertmacro EARTHDESK_IME_STOP
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro EARTHDESK_CHECK_DOTNET
  DetailPrint "正在配置硬件监控（驱动、后台服务）…"
  nsExec::ExecToLog 'powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "$INSTDIR\sensors\setup-sensors.ps1" install'
  ; Mouse gestures, hotkeys and screenshots need administrator rights to
  ; reach elevated windows. The app starts itself elevated through a
  ; scheduled task ("run with highest privileges"); creating that task needs
  ; the elevation the installer already has, so do it here and the user
  ; never sees a UAC prompt for it. Starting with Windows is kept as it was.
  DetailPrint "正在设置以管理员身份启动…"
  nsExec::ExecToLog '"$INSTDIR\EarthDesk.exe" --setup-task'
  ; The input method: register both DLLs (64- and 32-bit programs), add it to
  ; this user's keyboard list and build the dictionaries now, so it types
  ; the moment the user switches to it. The engine then starts at every
  ; logon (without administrator rights), or on first use.
  DetailPrint "正在安装地球桌面输入法（部署词库约需一分钟）…"
  nsExec::ExecToLog '"$INSTDIR\ime\EarthDeskIME.exe" --register --enable --deploy'
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Run" "EarthDeskIME" '"$INSTDIR\ime\EarthDeskIME.exe"'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  IfFileExists "$INSTDIR\sensors\setup-sensors.ps1" 0 +2
    nsExec::ExecToLog 'powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "$INSTDIR\sensors\setup-sensors.ps1" uninstall'
  IfFileExists "$INSTDIR\lhm\setup-lhm.ps1" 0 +2
    nsExec::ExecToLog 'powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "$INSTDIR\lhm\setup-lhm.ps1" uninstall'
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "EarthDesk"
  nsExec::ExecToLog '"$INSTDIR\EarthDesk.exe" --remove-task'
  IfFileExists "$INSTDIR\ime\EarthDeskIME.exe" 0 +2
    nsExec::ExecToLog '"$INSTDIR\ime\EarthDeskIME.exe" --disable --unregister'
  DeleteRegValue HKLM "Software\Microsoft\Windows\CurrentVersion\Run" "EarthDeskIME"
  !insertmacro EARTHDESK_IME_STOP

!macroend
