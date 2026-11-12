// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// A text terminal on the screen (docs/design/0018-supervision-shell-and-m5-tests.md, section
/// 5). Served by the console. The users service gives it to the first administrator's session,
/// whose sandbox manager passes it to apps granted `terminal` (note 0017, section 5): the shell.
///
/// Text is UTF-8. The console draws every character the system font has (ASCII, the accented
/// Latin letters and some punctuation; `lib/font`) and `\n`; anything else is drawn as `?`. If the console is restarted, calls fail with PEER_GONE: the app ends, and its
/// session starts it again with the new console's terminal.
interface pbr.terminal version 1;

/// Shows `text` after what is already there, wrapping long lines and scrolling at the bottom.
@1 fn write(text: string<4096>) -> () error TerminalError;

/// Shows `prompt`, then waits for the user to type a line and press Enter. The user can edit it
/// (backspace, delete, left and right, home and end) and go back through earlier lines (up and
/// down). The answer is the line without its end-of-line.
@2 fn read_line(prompt: string<256>) -> (line: string<4096>) error TerminalError;

/// The size of the screen in characters.
@3 fn size() -> (columns: u32, rows: u32) error TerminalError;

/// Empties the screen and puts the cursor at the top left.
@4 fn clear() -> () error TerminalError;

/// What can go wrong.
flexible enum TerminalError {
    /// Another `read_line` came while this one waited, and took over from it. (The terminal
    /// has one user, which asks for a line at a time; a second means the first caller has
    /// probably gone, which the console can't otherwise tell.)
    Busy @1,
    /// The console has nothing to get keys from (it has not been given the seat, note 0021).
    Closed @2,
}
