@echo off
rem SPDX-License-Identifier: GPL-3.0-or-later
rem Starts PrettyBigRock 2 in QEMU. Double-click it to start with UEFI on x86-64 and 4 cores, or
rem from a terminal add any of these words, in any order:
rem     bios        start x86-64 the old-fashioned way, through the BIOS
rem     arm         start the ARM64 version instead
rem     selftest    run the kernel's self-test
rem     a number    how many processor cores (1 to 64; ARM64 can have at most 8 for now)
rem For example:
rem     run-prettybigrock.bat bios selftest 8
rem It uses build\release (or build\debug), made by "cargo xtask build".
rem PrettyBigRock 1.x has its own launcher in the classic folder.
setlocal EnableDelayedExpansion
set "QDIR=C:\Program Files\qemu"
set "MODE=uefi"
set "NAME=prettybigrock.img"
set "CORES=4"
for %%A in (%*) do (
  set "WORD=%%~A"
  if /i "!WORD!"=="bios" (
    set "MODE=bios"
  ) else if /i "!WORD!"=="arm" (
    set "MODE=arm"
  ) else if /i "!WORD!"=="selftest" (
    set "NAME=prettybigrock-selftest.img"
  ) else (
    set /a "N=!WORD!" 2>nul
    if "!N!"=="!WORD!" (
      set "CORES=!WORD!"
    ) else (
      echo I don't know "!WORD!". Use bios, arm, selftest or a number of cores.
      pause
      exit /b 1
    )
  )
)
if !CORES! LSS 1 set "CORES=1"
if !CORES! GTR 64 set "CORES=64"
if "%MODE%"=="arm" if !CORES! GTR 8 (
  echo ARM64 PrettyBigRock can use at most 8 cores for now, so using 8.
  set "CORES=8"
)

set "IMG=%~dp0build\release\%NAME%"
if not exist "%IMG%" set "IMG=%~dp0build\debug\%NAME%"
if not exist "%IMG%" (
  echo There's no PrettyBigRock 2 disk image yet. Build one with: cargo xtask build --release
  pause
  exit /b 1
)
if "%MODE%"=="arm" goto arm
if "%MODE%"=="bios" goto bios

set "CODE=%QDIR%\share\edk2-x86_64-code.fd"
if not exist "%CODE%" (
  echo QEMU's UEFI firmware wasn't found in %QDIR%\share, so starting with the BIOS instead.
  goto bios
)
copy /y "%QDIR%\share\edk2-i386-vars.fd" "%TEMP%\prettybigrock-vars.fd" >nul
"%QDIR%\qemu-system-x86_64.exe" -name "PrettyBigRock 2 (x86-64 UEFI)" -machine q35 -m 512 -smp !CORES! -drive "if=pflash,format=raw,readonly=on,file=%CODE%" -drive "if=pflash,format=raw,file=%TEMP%\prettybigrock-vars.fd" -drive "file=%IMG%,format=raw,snapshot=on" -device virtio-rng-pci
goto done

:bios
"%QDIR%\qemu-system-x86_64.exe" -name "PrettyBigRock 2 (x86-64 BIOS)" -machine q35 -m 512 -smp !CORES! -drive "file=%IMG%,format=raw,snapshot=on"
goto done

:arm
"%QDIR%\qemu-system-aarch64.exe" -name "PrettyBigRock 2 (ARM64 UEFI)" -machine virt -cpu cortex-a72 -m 512 -smp !CORES! -bios "%QDIR%\share\edk2-aarch64-code.fd" -device ramfb -device qemu-xhci -device usb-kbd -drive "if=none,id=disk,file=%IMG%,format=raw,snapshot=on" -device virtio-blk-pci,drive=disk -device virtio-rng-pci
goto done

:done
if errorlevel 1 (
  echo.
  echo QEMU stopped with an error, shown above.
  pause
)
