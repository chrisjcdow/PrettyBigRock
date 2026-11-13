# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Christopher Cameron-Dow
# The session's desktop (apps/desktop; note 0024, section 6). It is not trusted: it sees none of
# the user's files, only its own folder (where it keeps screens.txt). It launches and stops apps
# through the sandbox manager, and asks the users service only who the user is, their settings,
# and to restart or turn off.
title "Desktop"
budget 8M
windows
desktop
data
hidden
