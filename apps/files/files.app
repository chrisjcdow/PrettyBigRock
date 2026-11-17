# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Christopher Cameron-Dow
# Files (apps/files; note 0026, section 1): 1.3's file manager. It is trusted, as Chris decided
# (Q16): it sees the session's whole tree, and holds the session's sandbox manager, so that it
# can open a file in another app ("Open with…", note 0025, section 3), which hands that app the
# one file.
title "Files"
budget 8M
trusted
windows
icon files
