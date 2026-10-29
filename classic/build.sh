#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Christopher Cameron-Dow
# Builds PrettyBigRock into build/prettybigrock.img (+ .vdi / .vmdk if qemu-img is available)
set -e
cd "$(dirname "$0")"
mkdir -p build
STAGE2_SECTORS=4
CFLAGS="-m32 -march=i686 -ffreestanding -fno-pie -fno-pic -fno-stack-protector -nostdlib -O2 -Wall -Wextra \
 -mno-sse -mno-sse2 -mno-mmx -fno-tree-loop-distribute-patterns -fno-asynchronous-unwind-tables -Ikernel"

python3 tools/mkassets.py build/assets.c
nasm -f elf32 kernel/entry.asm -o build/entry.o
OBJS="build/entry.o"
for f in kernel/*.c build/assets.c; do
  o=build/$(basename "$f" .c).o
  gcc $CFLAGS -c "$f" -o "$o"
  OBJS="$OBJS $o"
done
ld -m elf_i386 --no-warn-rwx-segments -z noexecstack -T kernel/linker.ld -o build/kernel.elf $OBJS "$(gcc -m32 -print-libgcc-file-name)"
objcopy -O binary build/kernel.elf build/kernel.bin

KSIZE=$(stat -c %s build/kernel.bin)
KSECT=$(( (KSIZE + 511) / 512 ))
END=$(nm build/kernel.elf | awk '/ __bss_end$/ {print $1}')
echo "kernel: $KSIZE bytes ($KSECT sectors), image ends at 0x$END"
[ $((0x$END)) -lt $((0x9F000)) ] || { echo "kernel too big for low memory"; exit 1; }

nasm -f bin -DSTAGE2_SECTORS=$STAGE2_SECTORS boot/stage2.asm -o build/stage2.bin
nasm -f bin -DSTAGE2_SECTORS=$STAGE2_SECTORS -DKERNEL_SECTORS=$KSECT boot/mbr.asm -o build/mbr.bin

# 4 MiB raw disk
dd if=/dev/zero of=build/prettybigrock.img bs=1M count=4 status=none
dd if=build/mbr.bin of=build/prettybigrock.img conv=notrunc status=none
dd if=build/stage2.bin of=build/prettybigrock.img bs=512 seek=1 conv=notrunc status=none
dd if=build/kernel.bin of=build/prettybigrock.img bs=512 seek=$((1 + STAGE2_SECTORS)) conv=notrunc status=none

if command -v qemu-img >/dev/null; then
  qemu-img convert -f raw -O vdi build/prettybigrock.img build/prettybigrock.vdi
  qemu-img convert -f raw -O vmdk build/prettybigrock.img build/prettybigrock.vmdk
fi
echo "done: build/prettybigrock.img"
