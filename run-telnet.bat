@echo off
rem Double-clickable launcher for the terminal on its own.
rem
rem No modem, no line, no audio: a socket to a bulletin board, and the same
rem terminal a call would feed. It exists because "the board looked wrong" has
rem two possible causes over a call -- a byte the line dropped, or an escape
rem sequence the terminal does not implement -- and this removes the first one.
rem Whatever still looks wrong here is ours.
rem
rem A host may be given, or chosen in the window:
rem   run-telnet.bat
rem   run-telnet.bat vert.synchro.net
rem
rem Bypasses the execution policy for this one script only, rather than
rem changing the machine's policy.
if "%~1"=="" (
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0run.ps1" -TelnetOnly
) else (
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0run.ps1" -Telnet %*
)
if errorlevel 1 pause
