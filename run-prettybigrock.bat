@echo off
rem Boots PrettyBigRock in QEMU. Double-click to run.
rem Your files are kept on data.img, next to this file. It is made the first time you run this.
set "QDIR=C:\Program Files\qemu"
set "QEMU=%QDIR%\qemu-system-i386.exe"
if not exist "%QEMU%" set "QEMU=%QDIR%\qemu-system-x86_64.exe"
if not exist "%QEMU%" (
  echo QEMU was not found in %QDIR%
  pause
  exit /b 1
)
set "DATA=%~dp0data.img"
if not exist "%DATA%" "%QDIR%\qemu-img.exe" create -f raw "%DATA%" 64M >nul
start "" "%QEMU%" -name PrettyBigRock -m 128 -drive "file=%~dp0build\prettybigrock.img,format=raw,index=0,snapshot=on" -drive "file=%DATA%,format=raw,index=1"
