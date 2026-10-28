@echo off
rem Boots FynbosOS in QEMU. Double-click to run.
set "QEMU=C:\Program Files\qemu\qemu-system-i386.exe"
if not exist "%QEMU%" set "QEMU=C:\Program Files\qemu\qemu-system-x86_64.exe"
if not exist "%QEMU%" (
  echo QEMU was not found in C:\Program Files\qemu
  pause
  exit /b 1
)
start "" "%QEMU%" -name FynbosOS -m 128 -drive "file=%~dp0build\fynbos.img,format=raw,snapshot=on"
