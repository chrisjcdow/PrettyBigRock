/* SPDX-License-Identifier: GPL-3.0-or-later
 * Copyright (C) 2026 Christopher Cameron-Dow */
/* PrettyBigRock - a tiny RAM filesystem (flat, no directories) */
#include "kernel.h"

typedef struct { int used, size; char name[FS_NAME_LEN]; char *data; } file_t;
static file_t files[FS_MAX_FILES];
int fs_generation;

static void seed(const char *name, const char *text) {
    int i = fs_create(name);
    if (i >= 0) fs_write(i, text, (int)strlen(text));
}

void fs_init(void) {
    for (int i = 0; i < FS_MAX_FILES; i++) files[i].data = kmalloc(FS_MAX_SIZE + 1);
    seed("readme.txt",
         "Welcome to PrettyBigRock!\n"
         "\n"
         "PrettyBigRock is a small hobby operating system written from\n"
         "scratch: its own boot loader, kernel, drivers, font,\n"
         "window manager and apps.\n"
         "\n"
         "Things to try:\n"
         "  - Double-click the icons on the desktop.\n"
         "  - Drag windows around by their title bars.\n"
         "  - Open the Terminal and type 'help'.\n"
         "  - Write something in Notepad and press Ctrl+S.\n"
         "  - Beat your high score in Snake.\n"
         "\n"
         "Files live in memory, so they are gone after a restart.\n");
    seed("shortcuts.txt",
         "Keyboard shortcuts\n"
         "------------------\n"
         "Windows key or Ctrl+Esc   open the start menu\n"
         "Ctrl+W                    close the active window\n"
         "Ctrl+S (Notepad)          save the file\n"
         "Up / Down (Terminal)      command history\n"
         "Arrows or WASD (Snake)    steer, P to pause\n");
    seed("todo.txt",
         "- water the proteas\n"
         "- write a real filesystem driver\n"
         "- teach the kernel to multitask\n");
}

int fs_valid(int i) { return i >= 0 && i < FS_MAX_FILES && files[i].used; }

int fs_find(const char *name) {
    for (int i = 0; i < FS_MAX_FILES; i++)
        if (files[i].used && !strcmp(files[i].name, name)) return i;
    return -1;
}

int fs_create(const char *name) {
    if (!name[0] || strlen(name) >= FS_NAME_LEN) return -1;
    int e = fs_find(name);
    if (e >= 0) return e;
    for (int i = 0; i < FS_MAX_FILES; i++) {
        if (files[i].used) continue;
        files[i].used = 1;
        files[i].size = 0;
        files[i].data[0] = 0;
        strlcpy0(files[i].name, name, FS_NAME_LEN);
        fs_generation++;
        return i;
    }
    return -1;
}

int fs_write(int i, const char *data, int len) {
    if (!fs_valid(i)) return -1;
    if (len > FS_MAX_SIZE) len = FS_MAX_SIZE;
    memcpy(files[i].data, data, len);
    files[i].data[len] = 0;
    files[i].size = len;
    fs_generation++;
    return len;
}

int fs_append(int i, const char *data, int len) {
    if (!fs_valid(i)) return -1;
    if (files[i].size + len > FS_MAX_SIZE) len = FS_MAX_SIZE - files[i].size;
    memcpy(files[i].data + files[i].size, data, len);
    files[i].size += len;
    files[i].data[files[i].size] = 0;
    fs_generation++;
    return len;
}

int fs_delete(int i) {
    if (!fs_valid(i)) return -1;
    files[i].used = 0;
    fs_generation++;
    return 0;
}

const char *fs_name(int i) { return files[i].name; }
const char *fs_data(int i) { return files[i].data; }
int fs_size(int i) { return files[i].size; }

int fs_count(void) {
    int n = 0;
    for (int i = 0; i < FS_MAX_FILES; i++) n += files[i].used;
    return n;
}
