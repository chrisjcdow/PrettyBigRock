# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Christopher Cameron-Dow
# The text shell (apps/shell; note 0018, section 6). It is trusted: it sees its session's whole
# tree, and holds the session's sandbox manager and users service. It asks for the terminal.
title "Shell"
budget 4M
trusted
terminal
