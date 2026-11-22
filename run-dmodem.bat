@echo off
rem A call to somebody else's modem without leaving the machine.
rem
rem D-Modem (github.com/D-Modem/D-Modem, AGPL, built separately in
rem ..\D-Modem) is a softmodem written by other people from the same
rem Recommendations. This starts its dmodemd answering SIP calls on port 5070,
rem a terminal on its end, and then this modem's window with the scopes open,
rem recording, and already dialling it. Three windows: the far modem's log,
rem the far terminal, and BinModem.
rem
rem   run-dmodem.bat          a V.90 call
rem   run-dmodem.bat V34      or V92, V32B, V22B ...
rem   run-dmodem.bat FAX      a test page sent to it as an ordinary group 3 fax
rem   run-dmodem.bat FAX34    the same page as Super G3, over V.34
rem
rem A fax has no window: the page is one the program draws itself, and the
rem transcript scrolls in this console instead. Its recording is
rem captures\dmodem-fax.wav, and what D-Modem received is beside it.
rem
rem The recording is kept in captures\ when the BinModem window is closed (or
rem its Record key pressed): two channels, what arrived and what was sent. It
rem plays in anything, and replays through  binmodem captures\live-N.wav.
rem Close the other two windows afterwards.
setlocal
set MS=%1
if "%MS%"=="" set MS=V90
set HERE=%~dp0
set DMODEMD=%HERE%..\D-Modem\build-msvc\dmodemd\dmodemd.exe
if not exist "%DMODEMD%" (
    echo dmodemd.exe is not at %DMODEMD%
    echo Build D-Modem first: cmake -B build-msvc -G Ninja, from a VS prompt.
    pause
    exit /b 1
)
rem The built exe in dist\ first: it is the one being tested, and target\
rem can hold a build of whatever branch this checkout was last on.
set BINMODEM=%HERE%dist\binmodem.exe
if not exist "%BINMODEM%" set BINMODEM=%HERE%target\release\binmodem.exe

if not exist "%HERE%captures" mkdir "%HERE%captures"
rem dmodemd keeps its settings under LOCALAPPDATA; give it a corner of its own.
rem DMODEM_REC has it write its own end of the line, captures\dmodem-line.rx
rem and .tx; tools\dmodem\dmodem_rec_to_wav.py turns the pair into a WAV.
start "D-Modem: the far modem" cmd /k "set LOCALAPPDATA=%HERE%tools\dmodem\state&& set DMODEM_REC=%HERE%captures\dmodem-line&& set SIP_LOGIN=anonymous&& set SIP_PORT=5070&& set SIP_T38=no&& set SIP_LOG_LEVEL=3&& "%DMODEMD%" -L -d1 --tcp 127.0.0.1:2323"
if /i "%MS%"=="FAX" (
    start "D-Modem: the receiving fax" cmd /k python "%HERE%tools\dmodem\dmodem_fax.py" 2323 1,5,0,2,0,0,0,0 "%HERE%captures\dmodem-fax" 180
) else if /i "%MS%"=="FAX34" (
    start "D-Modem: the receiving fax" cmd /k python "%HERE%tools\dmodem\dmodem_fax.py" 2323 1,13,0,2,0,2,0,0 "%HERE%captures\dmodem-fax" 180
) else (
    start "D-Modem: its terminal" cmd /k python "%HERE%tools\dmodem\dmodem_dte.py" 2323 %MS% "%HERE%captures\dmodem-terminal.log"
)
timeout /t 4 /nobreak >nul

set BINMODEM_SIP_FILE=%HERE%tools\dmodem\sip.txt
if /i "%MS%"=="FAX" goto fax
if /i "%MS%"=="FAX34" goto fax
if /i "%MS%"=="V92" set BINMODEM_V92_PCM_UP=1
rem D-Modem reads 6.4.2's classes as printed; see precoder::pair_first_index_offset.
if /i "%MS%"=="V92" set BINMODEM_V92_PAIR_OFFSET=0
cd /d "%HERE%"
"%BINMODEM%" --sip dmodem --scopes --record --type "AT+MS=%MS%" --type "ATD sip:modem@127.0.0.1:5070"
goto :eof

:fax
set KIND=g3
if /i "%MS%"=="FAX34" set KIND=v34
if not exist "%HERE%captures" mkdir "%HERE%captures"
"%HERE%target\release\modem-sip.exe" --accounts "%BINMODEM_SIP_FILE%" --account dmodem --fax %KIND% --record "%HERE%captures\dmodem-fax.wav" --type "AT+FCLASS=1" --type "ATD sip:fax@127.0.0.1:5070" --seconds 170
echo.
echo The call is in captures\dmodem-fax.wav: left is what arrived, right what was sent.
pause
