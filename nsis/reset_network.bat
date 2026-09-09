@echo off
setlocal EnableExtensions
title Network Reset
cd /d "%~dp0"

rem ============================================================
rem  reset_network.bat
rem
rem  Runs the following with Administrator privileges:
rem    ipconfig /release
rem    ipconfig /flushdns
rem    ipconfig /renew
rem    netsh int ip reset
rem    netsh winsock reset
rem
rem  Notes:
rem    * ipconfig /release + /renew will briefly drop your
rem      network connection while a new DHCP lease is obtained.
rem    * netsh int ip reset / netsh winsock reset restore the
rem      TCP/IP stack and Winsock catalog to defaults.
rem    * A REBOOT is strongly recommended afterwards so the
rem      netsh resets fully take effect.
rem    * Self-elevates via UAC when not already Administrator.
rem    * Pass "/nopause" to skip the prompt at the end (handy
rem      when launched from an installer).
rem    * Output is logged to reset_network.log next to this
rem      script, or %TEMP%\reset_network.log if that folder
rem      is not writable.
rem
rem  Exit code: 0 if every command returned 0, 1 otherwise.
rem
rem  Source: https://forum.gamer.com.tw/C.php?bsn=7650&snA=1023817
rem ============================================================

set "NOPAUSE=0"
if /i "%~1"=="/nopause" set "NOPAUSE=1"

rem --- Elevate if not already Administrator ---
net session >nul 2>&1
if not "%errorlevel%"=="0" (
    echo This script requires Administrator privileges.
    echo Requesting elevation via UAC...
    powershell -NoProfile -ExecutionPolicy Bypass -Command "$p = Start-Process -FilePath '%~f0' -Verb RunAs -Wait -PassThru; exit $p.ExitCode"
    exit /b %errorlevel%
)

rem --- Pick a writable log location (script dir first) ---
set "LOG=%~dp0reset_network.log"
>nul 2>&1 echo.>>"%LOG%"
if not exist "%LOG%" set "LOG=%TEMP%\reset_network.log"

echo ================================================
echo  Network reset started: %date% %time%
echo  Log: %LOG%
echo ================================================
echo.

set "FAILED=0"

call :run ipconfig /release
call :run ipconfig /flushdns
call :run ipconfig /renew
call :run netsh int ip reset
call :run netsh winsock reset

echo.
echo ================================================
if "%FAILED%"=="1" (
    echo  One or more commands reported an error.
    echo  Check the log for details.
) else (
    echo  All commands completed successfully.
)
echo  A REBOOT is strongly recommended for the
echo  netsh ip/winsock resets to fully take effect.
echo ================================================
echo.

if "%NOPAUSE%"=="1" exit /b %FAILED%
pause
exit /b %FAILED%

rem ------------------------------------------------------------
rem  Run a command, echo + log its exit code.
rem  Usage: call :run command [args...]
rem ------------------------------------------------------------
:run
echo.
echo [CMD] %*
>>"%LOG%" echo.
>>"%LOG%" echo [CMD] %*
%*
set "EC=%errorlevel%"
echo [EXIT] %EC%
>>"%LOG%" echo [EXIT] %EC%
if not "%EC%"=="0" set "FAILED=1"
goto :eof
