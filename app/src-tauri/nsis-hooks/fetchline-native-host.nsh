; Fetchline native-messaging host registration (S13 packaging fix).
;
; The NSIS/MSI installers ship fetchline-host.exe (externalBin) and
; the two host manifests (resources) next to the app binary, but the
; browser only finds the host through HKCU registry keys, so the
; installer must write them — and remove them again on uninstall.
;
; NSIS_HOOK_POSTINSTALL runs after all files are copied, with
; $INSTDIR set:
;   1. Point both shipped manifests at the real install dir. The
;      manifests ship with the default MSI path
;      (C:\Program Files\Fetchline\fetchline-host.exe), which is
;      wrong for the default NSIS install (per-user, under
;      %LOCALAPPDATA%) and for any custom install dir, so the
;      "path" field is rewritten by register-native-host.ps1
;      (shipped as a resource, so it is already in $INSTDIR).
;      PowerShell is used because NSIS has no JSON support; the
;      script writes UTF-8 without a BOM.
;   2. Write the three HKCU native-messaging keys, each pointing
;      at its installed manifest. Chrome and Edge share the
;      Chromium manifest; Firefox takes the separate file with
;      allowed_extensions (BUG-022).
; NSIS_HOOK_PREUNINSTALL runs before any file is removed: delete
; the three keys. The manifests, the .ps1 and fetchline-host.exe
; themselves are removed by the stock uninstaller (resources +
; external binaries).
;
; Note: the keys are HKCU, so they land in the installing user's
; hive — the same convention Tauri itself uses for its InstallDir
; key. A different Windows user on the same machine would need
; their own install (or the key added manually).

!macro NSIS_HOOK_POSTINSTALL
  ExecWait 'powershell.exe -NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File "$INSTDIR\register-native-host.ps1" "$INSTDIR"' $0
  ${If} $0 <> 0
    DetailPrint "Fetchline: could not rewrite host manifest paths (exit code $0); browser capture may not work for a non-default install dir"
  ${EndIf}

  WriteRegStr HKCU "Software\Google\Chrome\NativeMessagingHosts\com.fetchline.host" "" "$INSTDIR\com.fetchline.host.json"
  WriteRegStr HKCU "Software\Microsoft\Edge\NativeMessagingHosts\com.fetchline.host" "" "$INSTDIR\com.fetchline.host.json"
  WriteRegStr HKCU "Software\Mozilla\NativeMessagingHosts\com.fetchline.host" "" "$INSTDIR\com.fetchline.host.firefox.json"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DeleteRegKey HKCU "Software\Google\Chrome\NativeMessagingHosts\com.fetchline.host"
  DeleteRegKey HKCU "Software\Microsoft\Edge\NativeMessagingHosts\com.fetchline.host"
  DeleteRegKey HKCU "Software\Mozilla\NativeMessagingHosts\com.fetchline.host"
!macroend
