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

;  地球桌面输入法 (ime\): its DLLs are loaded into every program the user
;  types in (and the engine's into EarthDeskIME.exe), and Windows' crash
;  reporter or a virus scanner may hold them open too, so an upgrade cannot
;  simply overwrite them. A file in use can still be renamed: every .dll and
;  .exe in ime\ is moved aside (name.old1, .old2 ...) and deleted now if
;  possible, otherwise later (see above). The new files go in cleanly and no
;  restart is ever needed: programs opened from now on load the new ones.
!macro EARTHDESK_IME_MOVE_ASIDE_ALL ID
  Delete "$INSTDIR\ime\*.old*"
  FindFirst $R2 $R3 "$INSTDIR\ime\*.*"
  ime_all_loop_${ID}:
    StrCmp $R3 "" ime_all_done_${ID}
    StrCpy $R4 $R3 "" -4
    StrCmp $R4 ".dll" ime_all_move_${ID}
    StrCmp $R4 ".exe" ime_all_move_${ID} ime_all_next_${ID}
    ime_all_move_${ID}:
      StrCpy $R1 0
      ime_all_n_${ID}:
        IntOp $R1 $R1 + 1
        IfFileExists "$INSTDIR\ime\$R3.old$R1" ime_all_n_${ID}
      Rename "$INSTDIR\ime\$R3" "$INSTDIR\ime\$R3.old$R1"
      ; No /REBOOTOK: a file still in use just stays (and never asks for a
      ; restart); EarthDesk deletes leftovers when it next starts, and the
      ; next installer tries again.
      Delete "$INSTDIR\ime\$R3.old$R1"
    ime_all_next_${ID}:
    FindNext $R2 $R3
    Goto ime_all_loop_${ID}
  ime_all_done_${ID}:
  FindClose $R2
!macroend

!macro EARTHDESK_IME_STOP ID
  ; Ask the engine to save the user dictionary and quit, then make sure.
  nsExec::Exec 'taskkill /IM EarthDeskIME.exe'
  Sleep 1000
  nsExec::Exec 'taskkill /F /IM EarthDeskIME.exe'
  Sleep 300
  !insertmacro EARTHDESK_IME_MOVE_ASIDE_ALL ${ID}
!macroend

; The finish page's "create a desktop shortcut" box starts unticked (Tauri's
; template ticks it). The template reads this define after including us.
!define MUI_FINISHPAGE_SHOWREADME_NOTCHECKED

; Always install clean: whatever version is there, its own uninstaller runs
; first (silently; user data under %APPDATA% stays: the uninstaller only
; deletes it when asked, which a silent run never does). Tauri's page before
; this only offers it; the choice made there no longer matters.
!macro EARTHDESK_UNINSTALL_OLD
  ReadRegStr $R5 SHCTX "${MANUPRODUCTKEY}" ""
  StrCmp $R5 "" 0 +2
    StrCpy $R5 $INSTDIR
  IfFileExists "$R5\${MAINBINARYNAME}.exe" 0 old_done
  IfFileExists "$R5\uninstall.exe" 0 old_done
    DetailPrint "正在卸载旧版本…"
    nsExec::Exec 'taskkill /F /IM ${MAINBINARYNAME}.exe'
    Sleep 500
    ; _?= keeps the uninstaller in place, so ExecWait really waits for it.
    ExecWait '"$R5\uninstall.exe" /S _?=$R5' $R6
    Delete "$R5\uninstall.exe"
    DetailPrint "旧版本已卸载（返回 $R6）"
  old_done:
!macroend

; The same, already when the welcome page is left: then Tauri's "an older
; version is installed, uninstall it first?" page that follows finds nothing
; installed and skips itself (it would otherwise ask, and its uninstall
; shows the old uninstaller's own dialogs). This file is included before the
; template names its registry keys, so they are spelt out here: publisher
; "EarthDesk", product "地球桌面", perMachine (HKLM).
!define MUI_PAGE_CUSTOMFUNCTION_LEAVE EarthDeskUninstallOldEarly
Function EarthDeskUninstallOldEarly
  ReadRegStr $R5 HKLM "Software\EarthDesk\地球桌面" ""
  StrCmp $R5 "" early_done
  IfFileExists "$R5\uninstall.exe" 0 early_done
    nsExec::Exec 'taskkill /F /IM EarthDesk.exe'
    Sleep 500
    ExecWait '"$R5\uninstall.exe" /S _?=$R5' $R6
    Delete "$R5\uninstall.exe"
  early_done:
FunctionEnd

!macro NSIS_HOOK_PREINSTALL
  !insertmacro EARTHDESK_UNINSTALL_OLD
  ; Upgrading over an older copy: stop the running monitor so its files can
  ; be replaced.
  IfFileExists "$INSTDIR\sensors\setup-sensors.ps1" 0 +2
    nsExec::ExecToLog 'powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "$INSTDIR\sensors\setup-sensors.ps1" stop'
  ; 1.0.0 ran the LibreHardwareMonitor program from a logon task instead
  ; (tray icon, web server on 8085). Remove its task, firewall rule and files.
  IfFileExists "$INSTDIR\lhm\setup-lhm.ps1" 0 +2
    nsExec::ExecToLog 'powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "$INSTDIR\lhm\setup-lhm.ps1" uninstall'
  RMDir /r "$INSTDIR\lhm"
  !insertmacro EARTHDESK_IME_STOP inst
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro EARTHDESK_CHECK_DOTNET
  DetailPrint "正在配置硬件监控（驱动、后台服务）…"
  nsExec::ExecToLog 'powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "$INSTDIR\sensors\setup-sensors.ps1" install'
  ; Mouse gestures, hotkeys and screenshots need administrator rights to
  ; reach elevated windows. The app starts itself elevated through a
  ; scheduled task ("run with highest privileges"); creating that task needs
  ; the elevation the installer already has, so do it here and the user
  ; never sees a UAC prompt for it. Every install starts with Windows (the
  ; settings page can turn it off).
  DetailPrint "正在设置以管理员身份启动…"
  nsExec::ExecToLog '"$INSTDIR\EarthDesk.exe" --setup-task --autostart'
  ; The input method: register both DLLs (64- and 32-bit programs), add it to
  ; this user's keyboard list and build the dictionaries now, so it types
  ; the moment the user switches to it. The engine then starts at every
  ; logon (without administrator rights), or on first use.
  DetailPrint "正在安装地球桌面输入法（部署词库约需一分钟）…"
  nsExec::ExecToLog '"$INSTDIR\ime\EarthDeskIME.exe" --register --enable --deploy'
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Run" "EarthDeskIME" '"$INSTDIR\ime\EarthDeskIME.exe"'
  ; Start the new engine right away, with the user's ordinary rights (Explorer
  ; starts it, not this elevated installer), so programs that are already
  ; open keep typing without a restart.
  Exec '"$WINDIR\explorer.exe" "$INSTDIR\ime\EarthDeskIME.exe"'
  ; The Start menu's search box (and the Start menu) keep the old DLL loaded
  ; until they restart; since 1.2.8 the DLL draws the candidate window there
  ; itself. Windows starts them again on their next use.
  nsExec::Exec 'taskkill /F /IM SearchHost.exe'
  nsExec::Exec 'taskkill /F /IM SearchApp.exe'
  nsExec::Exec 'taskkill /F /IM StartMenuExperienceHost.exe'
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
  !insertmacro EARTHDESK_IME_STOP uninst

!macroend
