# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Christopher Cameron-Dow
# The permissions viewer (apps/permissions; note 0025, section 4): what each running app can
# reach and why, with Take back and Stop. It holds no files; its pbr.sandbox capability may
# only look, take back and stop ("sandbox view"), and its window is a system window.
title "Permissions"
budget 4M
windows
system
sandbox view
icon about
