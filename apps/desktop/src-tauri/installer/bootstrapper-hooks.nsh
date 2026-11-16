; Additive extension. The existing asset/data cleanup hooks are unchanged.
!include "${__FILEDIR__}\hooks.nsh"
!ifndef DEEPAGENT_OPTIONS_HELPER
  !define DEEPAGENT_OPTIONS_HELPER "${__FILEDIR__}\..\binaries\deepagent-install-options.exe"
!endif

; Keep this helper outside Tauri's resource file list: it must survive until
; un.onUninstSuccess, after the regular files have been removed.
!macro NSIS_HOOK_POSTINSTALL
  SetOutPath "$INSTDIR"
  File "/oname=deepagent-install-options.exe" "${DEEPAGENT_OPTIONS_HELPER}"
!macroend

Function un.onUninstSuccess
  ; NSIS upgrades uninstall the old package with /UPDATE. Preserve preferences
  ; on that path; only an explicit uninstall reverses owned HKCU changes.
  ClearErrors
  ${GetOptions} $CMDLINE "/UPDATE" $0
  ${If} ${Errors}
    ExecWait '"$INSTDIR\deepagent-install-options.exe" uninstall "$INSTDIR"' $0
    ${If} $0 != 0
      MessageBox MB_OK|MB_ICONEXCLAMATION "DeepAgent Studio was removed, but its optional PATH/startup settings could not be cleaned. The ownership record was kept for repair."
      Return
    ${EndIf}
  ${EndIf}
  Delete "$INSTDIR\deepagent-install-options.exe"
  RMDir "$INSTDIR"
FunctionEnd
