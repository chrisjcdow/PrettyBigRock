# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Christopher Cameron-Dow
# Paint (apps/paint; note 0026, section 1): 1.3's Paint, reading and writing .bmp files with
# lib/bmp. It may read and change Pictures, where it saves "Drawing N.bmp"; anything else the
# user wants it to open or save goes through the file picker, one file at a time. Its own
# folder keeps the last colour chosen.
title "Paint"
budget 12M
windows
picker
files pictures read write
data
opens bmp
icon paint
