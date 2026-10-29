@echo off
rem SPDX-License-Identifier: GPL-3.0-or-later
rem Starts PrettyBigRock 2 in QEMU. Double-click it to start with UEFI on x86-64, or from a
rem terminal run one of:
rem     run-prettybigrock.bat          x86-64, UEFI
rem     run-prettybigrock.bat bios     x86-64, BIOS
rem     run-prettybigrock.bat arm      ARM64, UEFI
rem It uses build\release\prettybigrock.img (or build\debug\...), made by "cargo xtask build".
rem PrettyBigRock 1.x has its own launcher in the classic folder.
setlocal
set "QDIR=C:\Program Files\qemu"
set "IMG=%~dp0build\release\prettybigrock.img"
if not exist "%IMG%" set "IMG=%~dp0build\debug\prettybigrock.img"
if not exist "%IMG%" (
  echo There's no PrettyBigRock 2 disk image yet. Build one with: cargo xtask build --release
  pause
  exit /b 1
)
if /i "%~1"=="arm" goto arm
if /i "%~1"=="bios" goto bios

set "CODE=%QDIR%\share\edk2-x86_64-code.fd"
if not exist "%CODE%" (
  echo QEMU's UEFI firmware wasn't found in %QDIR%\share, so starting with the BIOS instead.
  goto bios
)
copy /y "%QDIR%\share\edk2-i386-vars.fd" "%TEMP%\prettybigrock-vars.fd" >nul
start "" "%QDIR%\qemu-system-x86_64.exe" -name "PrettyBigRock 2 (x86-64, UEFI)" -machine q35 -m 512 -drive "if=pflash,format=raw,readonly=on,file=%CODE%" -drive "if=pflash,format=raw,file=%TEMP%\prettybigrock-vars.fd" -drive "file=%IMG%,format=raw,snapshot=on" -device virtio-rng-pci
exit /b 0

:bios
start "" "%QDIR%\qemu-system-x86_64.exe" -name "PrettyBigRock 2 (x86-64, BIOS)" -machine q35 -m 512 -drive "file=%IMG%,format=raw,snapshot=on"
exit /b 0

:arm
start "" "%QDIR%\qemu-system-aarch64.exe" -name "PrettyBigRock 2 (ARM64, UEFI)" -machine virt -cpu cortex-a72 -m 512 -bios "%QDIR%\share\edk2-aarch64-code.fd" -device ramfb -device qemu-xhci -device usb-kbd -drive "if=none,id=disk,file=%IMG%,format=raw,snapshot=on" -device virtio-blk-pci,drive=disk -device virtio-rng-pci
exit /b 0
