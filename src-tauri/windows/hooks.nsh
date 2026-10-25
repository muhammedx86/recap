; Recap installer hooks (Tauri NSIS).

!macro NSIS_HOOK_PREINSTALL
  ; Bundled games are replaced on every install, not merged: detectors a newer version
  ; removed would otherwise linger and show up as "needs an image". The player's own
  ; edits live in %APPDATA%\com.recap.app\games and are untouched.
  RMDir /r "$INSTDIR\games"
!macroend
