@echo off
rem Build the one file there is to hand to somebody: dist\binmodem.exe.
rem
rem Everything is in it -- the scope, a modem, the telnet terminal, the
rem answering board, and the capture it opens with -- and nothing has to be
rem installed beside it.
rem
rem Bypasses the execution policy for this one script only, rather than
rem changing the machine's policy.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0dist.ps1"
if errorlevel 1 pause
