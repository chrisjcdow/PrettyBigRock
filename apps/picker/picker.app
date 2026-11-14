# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Christopher Cameron-Dow
# The file picker (apps/picker; note 0025, section 1). Started by the session's sandbox manager
# itself, never from the start menu. Trusted, so that it can show the user every folder; its
# windows are system windows, so no app can draw a fake one; its own folder keeps where each
# app's last pick was. Its pbr.sandbox capability may only grant what the user chose.
title "File picker"
budget 8M
trusted
windows
system
data
hidden
