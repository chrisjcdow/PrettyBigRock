# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Christopher Cameron-Dow
# The Welcome window (apps/welcome; note 0024, section 6a): where the user's files are kept,
# and a user of their own. It holds no files. Its pbr.users capability ("users welcome") may
# only ask who the user is, list and set up disks, add the user logged in at start, say the
# window was answered and restart; system information says which disk is the system volume.
title "Welcome"
budget 4M
windows
system
sysinfo
users welcome
icon about
