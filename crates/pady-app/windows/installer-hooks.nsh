; Pady NSIS installer hooks (bundle > windows > nsis > installerHooks).
;
; Tauri's template already removes the "Pady" autostart entry on uninstall
; (and keeps it during upgrades). It also deletes the bundle-id data folders
; when "Delete the application data" is ticked; Pady's settings live in
; %APPDATA%\Pady instead, shared with pady-cli, so remove that folder too.

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    RmDir /r "$APPDATA\Pady"
  ${EndIf}
!macroend
