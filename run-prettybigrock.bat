@echo off
rem Boots PrettyBigRock in QEMU. Double-click to run.
set "QEMU=C:\Program Files\qemu\qemu-system-i386.exe"
if not exist "%QEMU%" set "QEMU=C:\Program Files\qemu\qemu-system-x86_64.exe"
if not exist "%QEMU%" (
  echo QEMU was not found in C:\Program Files\qemu
  pause
  exit /b 1
)
start "" "%QEMU%" -name PrettyBigRock -m 128 -drive "file=%~dp0build\prettybigrock.img,format=raw,snapshot=on"
