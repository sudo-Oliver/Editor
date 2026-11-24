@echo off
rem The editor command: opens a folder or file in Editor, as `editor .` on the Mac.
start "" "%~dp0..\editor.exe" %*
