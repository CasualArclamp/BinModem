@echo off
rem Double-clickable launcher for the BinModem scope.
rem Bypasses the execution policy for this one script only, rather than
rem changing the machine's policy.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0run.ps1" %*
if errorlevel 1 pause
