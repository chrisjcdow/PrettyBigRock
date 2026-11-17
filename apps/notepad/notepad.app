# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Christopher Cameron-Dow
# Notepad (apps/notepad; note 0026, section 1): 1.3's text editor. It may read and change
# Documents; anything else the user wants it to open or save goes through the file picker, one
# file at a time, or is handed to it by Files' "Open with…" and Terminal's `edit`.
title "Notepad"
budget 8M
windows
picker
files documents read write
opens txt md
icon notepad
