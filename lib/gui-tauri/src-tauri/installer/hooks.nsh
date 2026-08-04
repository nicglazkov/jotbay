; Put the `jotbay` command on PATH, and take it off again on uninstall.
;
; The install directory holds both jotbay-gui.exe and jotbay.exe, so somebody
; who installs the app and later wants a terminal has one already; without this
; they would have to run install.ps1 as well and end up with two copies.
;
; The edit is delegated to PowerShell rather than done with ReadRegStr and
; WriteRegExpandStr. NSIS strings are capped (1024 characters in the standard
; build), and a PATH longer than the cap comes back silently truncated —
; writing that value back would delete the tail of the user's PATH. That is not
; a risk worth taking for a convenience feature.
;
; HKCU only: installMode is currentUser, so there is no elevation to spend and
; nothing here touches the machine-wide environment.

; Escaping note, learned the hard way: inside an NSIS single-quoted string a
; literal apostrophe is written $\' — NOT '' as in SQL or PowerShell. The first
; shipped version used '' and NSIS passed both characters through, so
; PowerShell received `TrimEnd('';'')`: an empty string, a statement-ending
; semicolon, and a parse error ("Missing ')' in method call", exit code 1).
; The hook failed gracefully - PATH was left untouched rather than damaged -
; but the CLI never went on PATH. Caught on the first real Windows install.
!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Adding $INSTDIR to PATH"
  nsExec::ExecToLog 'powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$$p = [Environment]::GetEnvironmentVariable($\'Path$\', $\'User$\'); if ($$p -eq $$null) { $$p = $\'$\' }; if (($$p -split $\';$\') -notcontains $\'$INSTDIR$\') { [Environment]::SetEnvironmentVariable($\'Path$\', ($$p.TrimEnd($\';$\') + $\';$INSTDIR$\').TrimStart($\';$\'), $\'User$\') }"'
  Pop $0
  ${If} $0 != 0
    DetailPrint "Could not update PATH (code $0). Run install.ps1 for the command line tool."
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  DetailPrint "Removing $INSTDIR from PATH"
  nsExec::ExecToLog 'powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$$p = [Environment]::GetEnvironmentVariable($\'Path$\', $\'User$\'); if ($$p -ne $$null) { [Environment]::SetEnvironmentVariable($\'Path$\', (($$p -split $\';$\' | Where-Object { $$_ -ne $\'$INSTDIR$\' -and $$_ -ne $\'$\' }) -join $\';$\'), $\'User$\') }"'
  Pop $0
!macroend
