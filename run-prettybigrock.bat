@echo off
rem SPDX-License-Identifier: GPL-3.0-or-later
rem Starts PrettyBigRock 2 in QEMU. Double-click it to start with UEFI on x86-64 and 4 cores, or
rem from a terminal add any of these words, in any order:
rem     bios        start x86-64 the old-fashioned way, through the BIOS
rem     arm         start the ARM64 version instead
rem     two-screens two screens instead of one (QEMU's View menu shows the second); with UEFI
rem     selftest    run the kernel's self-test
rem     a number    how many processor cores (1 to 64; ARM64 can have at most 8 for now)
rem For example:
rem     run-prettybigrock.bat two-screens 8
rem It uses build\release (or build\debug), made by "cargo xtask build", or a downloaded
rem prettybigrock.img put in build\release.
rem Your files can be kept on data.img, next to this file, which is made blank the first time.
rem Until it is set up, they are kept in memory: in Terminal, "volumes" lists it and
rem "setup VOLUME NAME" makes it the system disk, with you as its first user, from the next start.
rem The pointer is a tablet (a virtio one), so the mouse doesn't need capturing. On ARM64 the
rem keyboard is a virtio one too (PrettyBigRock has no USB yet).
rem PrettyBigRock 1.x has its own launcher in the classic folder.
setlocal EnableDelayedExpansion
set "QDIR=C:\Program Files\qemu"
set "MODE=uefi"
set "NAME=prettybigrock.img"
set "CORES=4"
set "SCREENS=1"
for %%A in (%*) do (
  set "WORD=%%~A"
  if /i "!WORD!"=="bios" (
    set "MODE=bios"
  ) else if /i "!WORD!"=="arm" (
    set "MODE=arm"
  ) else if /i "!WORD!"=="two-screens" (
    set "SCREENS=2"
  ) else if /i "!WORD!"=="selftest" (
    set "NAME=prettybigrock-selftest.img"
  ) else (
    set /a "N=!WORD!" 2>nul
    if "!N!"=="!WORD!" (
      set "CORES=!WORD!"
    ) else (
      echo I don't know "!WORD!". Use bios, arm, two-screens, selftest or a number of cores.
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
if "%MODE%"=="bios" if "!SCREENS!"=="2" (
  echo Two screens need UEFI: the BIOS can only start PrettyBigRock on the standard screen.
  pause
  exit /b 1
)

rem The screens, and the pointers that go with them. One screen is the firmware's (QEMU's
rem standard VGA, or ramfb on ARM64) with a tablet. Two screens are a virtio-gpu's two outputs,
rem each with a tablet of its own, as a touch screen is tied to its screen.
if "!SCREENS!"=="2" (
  set "SCREENSET=-vga none -device virtio-gpu-pci,id=gpu,max_outputs=2 -device virtio-tablet-pci,display=gpu,head=0 -device virtio-tablet-pci,display=gpu,head=1"
  set "ARMSCREEN=!SCREENSET!"
) else (
  set "SCREENSET=-device virtio-tablet-pci"
  set "ARMSCREEN=-device ramfb,id=gpu -device virtio-tablet-pci"
)

set "IMG=%~dp0build\release\%NAME%"
if not exist "%IMG%" set "IMG=%~dp0build\debug\%NAME%"
if not exist "%IMG%" (
  echo There's no PrettyBigRock 2 disk image yet. Build one with: cargo xtask build --release
  pause
  exit /b 1
)
set "DATA=%~dp0data.img"
if not exist "%DATA%" "%QDIR%\qemu-img.exe" create -f raw "%DATA%" 256M >nul
set "DATASET=-drive "if=none,id=data,file=%DATA%,format=raw" -device virtio-blk-pci,drive=data"
if "%MODE%"=="arm" goto arm
if "%MODE%"=="bios" goto bios

set "CODE=%QDIR%\share\edk2-x86_64-code.fd"
if not exist "%CODE%" (
  echo QEMU's UEFI firmware wasn't found in %QDIR%\share, so starting with the BIOS instead.
  if "!SCREENS!"=="2" (
    echo Two screens need UEFI, so starting with one.
    set "SCREENSET=-device virtio-tablet-pci"
  )
  goto bios
)
copy /y "%QDIR%\share\edk2-i386-vars.fd" "%TEMP%\prettybigrock-vars.fd" >nul
"%QDIR%\qemu-system-x86_64.exe" -name "PrettyBigRock 2 (x86-64 UEFI)" -machine q35 -m 2048 -smp !CORES! -drive "if=pflash,format=raw,readonly=on,file=%CODE%" -drive "if=pflash,format=raw,file=%TEMP%\prettybigrock-vars.fd" -drive "file=%IMG%,format=raw,snapshot=on" -device virtio-rng-pci !SCREENSET! !DATASET!
goto done

:bios
"%QDIR%\qemu-system-x86_64.exe" -name "PrettyBigRock 2 (x86-64 BIOS)" -machine q35 -m 2048 -smp !CORES! -drive "file=%IMG%,format=raw,snapshot=on" !SCREENSET! !DATASET!
goto done

:arm
"%QDIR%\qemu-system-aarch64.exe" -name "PrettyBigRock 2 (ARM64 UEFI)" -machine virt -cpu cortex-a72 -m 2048 -smp !CORES! -bios "%QDIR%\share\edk2-aarch64-code.fd" !ARMSCREEN! -device qemu-xhci -device usb-kbd -device virtio-keyboard-pci,display=gpu -drive "if=none,id=disk,file=%IMG%,format=raw,snapshot=on" -device virtio-blk-pci,drive=disk -device virtio-rng-pci !DATASET!
goto done

:done
if errorlevel 1 (
  echo.
  echo QEMU stopped with an error, shown above.
  pause
)
