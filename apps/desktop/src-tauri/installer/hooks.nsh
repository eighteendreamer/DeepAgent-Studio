; NSIS hooks for DeepAgent Studio's installer.
;
; The data root lives in the user's folder (`~/.deepagent`), outside the install
; directory, so the installer's own file list cannot clean it. The ledger written
; at install time is the only accurate record of what this program put there, so
; the uninstaller asks this program's own command-line binary to reverse it.
;
; Only plain NSIS instructions are used (no LogicLib, no custom Var): this file has
; to compile inside Tauri's generated script without adding dependencies to it.

!macro NSIS_HOOK_PREUNINSTALL
  ; The CLI ships as a sidecar, so its installed file name may or may not carry
  ; the target-triple suffix. Discover it instead of hardcoding one spelling.
  StrCpy $9 ""
  FindFirst $R0 $R1 "$INSTDIR\deepagent*.exe"
  deepagent_find_loop:
    StrCmp $R1 "" deepagent_found
    StrCpy $9 "$INSTDIR\$R1"
    FindNext $R0 $R1
    Goto deepagent_find_loop
  deepagent_found:
    FindClose $R0

  StrCmp $9 "" deepagent_no_cli
    DetailPrint "Removing the assets this program installed (skills, plugins, runtimes, caches)."
    ExecWait '"$9" uninstall --all --yes' $8
    DetailPrint "Asset cleanup finished (exit code $8)."
    MessageBox MB_YESNO|MB_ICONEXCLAMATION "Also delete sessions, settings and downloaded files stored under .deepagent in your user folder? This cannot be undone." IDNO deepagent_no_purge
      DetailPrint "Purging the session database and the managed file tree."
      ExecWait '"$9" uninstall --all --purge --yes' $8
      DetailPrint "Purge finished (exit code $8)."
    deepagent_no_purge:
    Goto deepagent_done
  deepagent_no_cli:
    DetailPrint "No DeepAgent command-line binary in the install directory; ~/.deepagent was left untouched."
  deepagent_done:
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; Everything under $INSTDIR is already gone here. The user folder is reported,
  ; never deleted, so an interrupted uninstall cannot take session history with it.
  DetailPrint "Install directory removed. ~/.deepagent was handled by the cleanup above."
!macroend
