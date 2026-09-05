@echo off
rem Double-clickable launcher for a live call.
rem
rem The difference from run.bat is what is on the other end. That one replays a
rem capture: a recording of a call somebody else placed, which can be watched
rem and nothing more. This puts a modem of your own on a real line, with the
rem terminal in the window wired to it, so AT commands are answered and ATD
rem dials for real.
rem
rem It will ask which audio devices to use, picking out a virtual cable if it
rem finds one, and offer to start a second modem on the same line so that there
rem is something to dial. Arguments are passed through, so
rem   run-live.bat -Carrier V32
rem works, as do B103 and V22B. Both ends have to agree, and this sets both.
rem
rem Bypasses the execution policy for this one script only, rather than
rem changing the machine's policy.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0run.ps1" -Live %*
if errorlevel 1 pause
