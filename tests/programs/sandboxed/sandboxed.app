# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Christopher Cameron-Dow
# The sandbox self-test's app (tests/programs/sandboxed; note 0018, section 7, test 6).
# It asks for three places and for trust: the self-test's rules decide what it gets.
title "Sandbox test"
budget 4M
files documents read write
files pictures read
files music read
trusted
