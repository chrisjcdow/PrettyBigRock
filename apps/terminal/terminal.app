# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Christopher Cameron-Dow
# Terminal (apps/terminal; note 0026, section 1): the shell in a window, as 1.3's Terminal was.
# It is trusted, as the text shell is: it sees the session's whole tree and holds the session's
# sandbox manager and users service. System information answers `mem`, `uptime`, `disk` and
# `fetch`; its own folder keeps the command history.
title "Terminal"
budget 8M
trusted
windows
sysinfo
data
icon terminal
