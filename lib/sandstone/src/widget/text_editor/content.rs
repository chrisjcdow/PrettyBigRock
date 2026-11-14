// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! [`Content`]: the text a [`TextEditor`](super::TextEditor) shows, kept by the app, and the
//! [`Action`]s that change it (note 0023, section 5; note 0026, section 1).
//!
//! Sandstone's model is that the app owns what matters: a `Content` lives in the app's state,
//! the editor widget only *borrows* it while the view is built, and what the user does comes
//! back to the app as an [`Action`] that the app applies with [`Content::perform`]. So the app
//! sees every change (Notepad shows a "•" in its title while [`Content::is_modified`]), and
//! nothing is copied for each view.
//!
//! **The text is kept as lines**, one `String` each, without the line break. A `Vec` (Rust's
//! growable array) of them makes an edit cost what the lines it touches cost, not what the
//! whole text costs: typing in a 100 000-line text moves no text but the one line's.
//!
//! **Positions** are `(line, column)`, both counted from 0, with the column in *characters*.
//! Rust strings are UTF-8, where one character can take up to four bytes, so a column is never
//! used to slice a string directly: [`byte_at`] turns it into a byte offset on a character
//! boundary first (slicing in the middle of a character panics in Rust).
//!
//! **Undo** records each change as what it removed and what it inserted, where, with the
//! cursor and selection before and after, so undoing puts everything back exactly.

use alloc::string::String;
use alloc::vec::Vec;
use core::cell::Cell;

/// A place in the text: `(line, column)`, both from 0, the column in characters.
pub type Pos = (usize, usize);

/// How many columns a tab stands for: it is drawn as spaces up to the next multiple of this.
pub const TAB_WIDTH: usize = 4;

/// How many steps undo remembers.
const MAX_STEPS: usize = 1000;

/// How the cursor moves (with [`Action::Move`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    /// One character left (to the end of the line above from a line's start).
    Left,
    /// One character right (to the next line from a line's end).
    Right,
    /// To the start of the word on the left (Ctrl+Left).
    WordLeft,
    /// To the start of the next word (Ctrl+Right).
    WordRight,
    /// One line up, keeping the column it wanted; from the first line, to its start.
    Up,
    /// One line down, keeping the column it wanted; from the last line, to its end.
    Down,
    /// To the start of the line (Home).
    LineStart,
    /// To the end of the line (End).
    LineEnd,
    /// To the start of the text (Ctrl+Home).
    TextStart,
    /// To the end of the text (Ctrl+End).
    TextEnd,
    /// This many lines up, keeping the column it wanted (Page Up). It stops at the first line.
    PageUp(usize),
    /// This many lines down (Page Down). It stops at the last line.
    PageDown(usize),
    /// To a position (a click); a position outside the text is brought inside it.
    To(Pos),
}

/// Something the user did to the text: what a [`TextEditor`](super::TextEditor) sends, and
/// what [`Content::perform`] applies. Each moves, extends or replaces the selection as a text
/// editor does: typing replaces the selection, Left with a selection and no Shift goes to its
/// start, and so on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// A character typed. It replaces the selection. Typed in a row, characters make one undo
    /// step. A line break or a control character other than a tab is not inserted this way.
    InsertChar(char),
    /// Text pasted. It replaces the selection, may hold line breaks (`\r\n` and `\r` become
    /// `\n`), keeps tabs, and is one undo step.
    InsertText(String),
    /// Enter: a line break. It replaces the selection, and is an undo step of its own.
    NewLine,
    /// Backspace: the selection, or the character before the cursor (with `word`, the word
    /// before it: Ctrl+Backspace). Backspaces in a row make one undo step.
    Backspace {
        /// Delete a whole word.
        word: bool,
    },
    /// Delete: the selection, or the character after the cursor (with `word`, the word after
    /// it: Ctrl+Delete). Deletes in a row make one undo step.
    Delete {
        /// Delete a whole word.
        word: bool,
    },
    /// Moves the cursor; with `select` the selection is extended to it (Shift held).
    Move {
        /// How.
        motion: Motion,
        /// Extend the selection instead of dropping it.
        select: bool,
    },
    /// Ctrl+A.
    SelectAll,
    /// Selects the word at a position (double click).
    SelectWord(Pos),
    /// Selects the line with this number, with its line break (triple click).
    SelectLine(usize),
    /// Deletes the selection (what Cut does after copying it), as one undo step.
    DeleteSelection,
    /// Undoes the last step.
    Undo,
    /// Does the last undone step again.
    Redo,
    /// Selects from the first position to the second, with the cursor at the second: how the
    /// app selects what [`Content::find`] found.
    Select(Pos, Pos),
}

impl Action {
    /// Whether this action can change the text (a read-only editor sends only those that can't).
    pub fn edits(&self) -> bool {
        !matches!(
            self,
            Action::Move { .. }
                | Action::SelectAll
                | Action::SelectWord(_)
                | Action::SelectLine(_)
                | Action::Select(..)
        )
    }
}

/// The cursor and selection at one moment, kept with each undo step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Where {
    cursor: Pos,
    anchor: Option<Pos>,
}

/// What kind of edit a step is: only steps of the same kind, made one after another with
/// nothing else between, are joined into one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Typing,
    Backspace,
    Delete,
    /// Everything else: never joined.
    Other,
}

/// One undoable change: at `at`, `removed` was taken out and `inserted` put in its place.
#[derive(Clone, Debug)]
struct Step {
    at: Pos,
    removed: String,
    inserted: String,
    before: Where,
    after: Where,
    kind: Kind,
}

/// The text of an editor: lines, a cursor and selection, undo and redo, and whether it has
/// changed since it was saved. See the module's documentation.
#[derive(Clone, Debug)]
pub struct Content {
    /// Never empty: an empty text is one empty line.
    lines: Vec<String>,
    cursor: Pos,
    /// The other end of the selection, when there is one.
    anchor: Option<Pos>,
    /// The column Up and Down try to keep, in characters.
    wanted: usize,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// How many steps were on the undo stack when the text was saved, or `None` if the saved
    /// text can no longer be reached by undoing and redoing.
    saved: Option<usize>,
    /// The kind of the last edit, while nothing but edits of that kind has happened since.
    run: Option<Kind>,
    /// The width of the widest line in columns, if known. A `Cell` can be changed through a
    /// shared reference: [`Content::widest`] fills it in lazily, and edits keep it up to date
    /// where that is cheap (it is worked out again only when the widest line got shorter).
    widest: Cell<Option<usize>>,
}

impl Default for Content {
    fn default() -> Content {
        Content::new()
    }
}

/// The byte offset of character number `col` in `line` (the line's length if `col` is past the
/// end): always on a character boundary, so slicing there is safe.
fn byte_at(line: &str, col: usize) -> usize {
    line.char_indices().nth(col).map_or(line.len(), |(i, _)| i)
}

/// How many characters `line` has.
fn chars_in(line: &str) -> usize {
    line.chars().count()
}

/// The column (counting a tab as the spaces up to the next multiple of [`TAB_WIDTH`]) where
/// character number `col` of `line` starts.
pub(crate) fn display_column(line: &str, col: usize) -> usize {
    let mut at = 0;
    for c in line.chars().take(col) {
        at += cell_width(c, at);
    }
    at
}

/// How many columns `c` takes when it starts at column `at`.
pub(crate) fn cell_width(c: char, at: usize) -> usize {
    if c == '\t' { TAB_WIDTH - at % TAB_WIDTH } else { 1 }
}

/// How many columns `line` is wide.
fn width_of(line: &str) -> usize {
    display_column(line, usize::MAX)
}

/// Where the text `text`, put down at `at`, ends.
fn end_after(at: Pos, text: &str) -> Pos {
    match text.rfind('\n') {
        None => (at.0, at.1 + chars_in(text)),
        Some(i) => (at.0 + text.matches('\n').count(), chars_in(&text[i + 1..])),
    }
}

/// `text` with `\r\n` and `\r` read as `\n`.
fn normalised(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

/// What a character is, for moving by words: 0 for white space, 1 for letters, digits and the
/// underscore, 2 for anything else.
fn class(c: char) -> u8 {
    if c.is_whitespace() {
        0
    } else if c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

/// `c` in lower case when the search ignores case (only when it has one lower-case form).
fn fold(c: char, match_case: bool) -> char {
    if match_case {
        return c;
    }
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(one), None) => one,
        _ => c,
    }
}

impl Content {
    /// An empty text (one empty line), nothing modified.
    pub fn new() -> Content {
        Content {
            lines: alloc::vec![String::new()],
            cursor: (0, 0),
            anchor: None,
            wanted: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            saved: Some(0),
            run: None,
            widest: Cell::new(None),
        }
    }

    /// A text made from `text`, split into lines at `\n` (`\r\n` and `\r` read as `\n`), with
    /// the cursor at the start, nothing modified and nothing to undo. Takes time in proportion
    /// to the text.
    pub fn from_text(text: &str) -> Content {
        let mut c = Content::new();
        c.lines = normalised(text).split('\n').map(String::from).collect();
        c
    }

    /// The whole text, lines joined with `\n`. Takes time in proportion to the text.
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// How many lines there are (at least one).
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Line `n` without its line break; empty for a line that doesn't exist.
    pub fn line(&self, n: usize) -> &str {
        self.lines.get(n).map_or("", String::as_str)
    }

    /// How many characters line `n` has.
    pub fn line_len(&self, n: usize) -> usize {
        chars_in(self.line(n))
    }

    /// The cursor: where typing goes.
    pub fn cursor(&self) -> Pos {
        self.cursor
    }

    /// The selection's other end, which stays where it was while the cursor moves. `None`
    /// when nothing is selected.
    pub fn anchor(&self) -> Option<Pos> {
        self.anchor.filter(|a| *a != self.cursor)
    }

    /// The selection as `(start, end)`, start before end, or `None` when nothing is selected.
    pub fn selection(&self) -> Option<(Pos, Pos)> {
        let a = self.anchor()?;
        Some(if a < self.cursor { (a, self.cursor) } else { (self.cursor, a) })
    }

    /// The selected text (empty when nothing is selected); line breaks are `\n`.
    pub fn selected_text(&self) -> String {
        match self.selection() {
            Some((a, b)) => self.text_between(a, b),
            None => String::new(),
        }
    }

    /// How many characters are selected (a line break counts as one).
    pub fn selection_len(&self) -> usize {
        let Some((a, b)) = self.selection() else {
            return 0;
        };
        if a.0 == b.0 {
            return b.1 - a.1;
        }
        let mut n = self.line_len(a.0) - a.1 + 1;
        for l in a.0 + 1..b.0 {
            n += self.line_len(l) + 1;
        }
        n + b.1
    }

    /// Whether the text has changed since it was made or last saved. Undoing back to the saved
    /// text makes it false again.
    pub fn is_modified(&self) -> bool {
        self.saved != Some(self.undo.len())
    }

    /// Says the text as it is now was saved: [`Content::is_modified`] is false until the next
    /// change. What was typed before is not joined with what is typed after.
    pub fn set_saved(&mut self) {
        self.saved = Some(self.undo.len());
        self.run = None;
    }

    /// Whether there is a step to undo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether there is an undone step to do again.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// How wide the widest line is, in columns (a tab is the spaces up to the next multiple of
    /// [`TAB_WIDTH`]). Usually answered at once; worked out again, in time proportional to the
    /// text, only after the widest line was shortened.
    pub fn widest(&self) -> usize {
        if let Some(w) = self.widest.get() {
            return w;
        }
        let w = self.lines.iter().map(|l| width_of(l)).max().unwrap_or(0);
        self.widest.set(Some(w));
        w
    }

    /// Looks for `query` forward from `from` (a match may start there), going round to the
    /// start of the text if there is none after it. Answers where the match starts and ends
    /// (the second is just past its last character), ready for [`Action::Select`]. A query
    /// may hold line breaks (`\n`). `match_case: false` ignores the difference between
    /// capitals and small letters. `None` for an empty query or no match.
    pub fn find(&self, query: &str, from: Pos, match_case: bool) -> Option<(Pos, Pos)> {
        if query.is_empty() {
            return None;
        }
        let q: Vec<Vec<char>> = normalised(query)
            .split('\n')
            .map(|s| s.chars().map(|c| fold(c, match_case)).collect())
            .collect();
        let from = self.clamp(from);
        let n = self.lines.len();
        // The first line from the start column on; the others whole; then the first line
        // again, up to its start column: that is the going round.
        for step in 0..=n {
            let l = (from.0 + step) % n;
            let (lo, hi) = match step {
                0 => (from.1, usize::MAX),
                s if s == n => (0, from.1),
                _ => (0, usize::MAX),
            };
            if let Some(found) = self.find_in_line(l, &q, match_case, lo, hi) {
                return Some(found);
            }
        }
        None
    }

    /// A match of `q` (the query's lines, folded) starting on line `l` in a column from `lo` up
    /// to but not including `hi`.
    fn find_in_line(
        &self,
        l: usize,
        q: &[Vec<char>],
        match_case: bool,
        lo: usize,
        hi: usize,
    ) -> Option<(Pos, Pos)> {
        let last = q.len() - 1;
        if l + last >= self.lines.len() {
            return None;
        }
        let chars: Vec<char> = self.lines[l].chars().map(|c| fold(c, match_case)).collect();
        let first = &q[0];
        let latest = chars.len().checked_sub(first.len())?;
        // With more query lines than one, the first part must end the line.
        let earliest = if last == 0 { 0 } else { latest };
        for c in earliest.max(lo)..=latest {
            if c >= hi {
                break;
            }
            if chars[c..c + first.len()] != first[..] {
                continue;
            }
            if last == 0 {
                return Some(((l, c), (l, c + first.len())));
            }
            let middle = (1..last).all(|i| {
                let mut got = self.lines[l + i].chars().map(|c| fold(c, match_case));
                q[i].iter().all(|w| got.next() == Some(*w)) && got.next().is_none()
            });
            let mut got = self.lines[l + last].chars().map(|c| fold(c, match_case));
            if middle && q[last].iter().all(|w| got.next() == Some(*w)) {
                return Some(((l, c), (l + last, q[last].len())));
            }
        }
        None
    }

    /// Does what the user did.
    pub fn perform(&mut self, action: Action) {
        match action {
            Action::InsertChar(c) => self.insert_char(c),
            Action::InsertText(text) => {
                let text = normalised(&text);
                if !text.is_empty() {
                    self.replace_selection(&text, Kind::Other);
                }
            }
            Action::NewLine => self.replace_selection("\n", Kind::Other),
            Action::Backspace { word } => self.backspace(word),
            Action::Delete { word } => self.delete(word),
            Action::DeleteSelection => {
                if let Some((a, b)) = self.selection() {
                    self.edit(a, b, "", Kind::Other);
                }
            }
            Action::Undo => self.undo_one(),
            Action::Redo => self.redo_one(),
            Action::Move { motion, select } => {
                self.run = None;
                self.motion(motion, select);
            }
            Action::SelectAll => {
                self.run = None;
                let end = self.end();
                self.anchor = Some((0, 0));
                self.cursor = end;
                self.wanted = end.1;
            }
            Action::SelectWord(at) => {
                self.run = None;
                self.select_word(at);
            }
            Action::SelectLine(l) => {
                self.run = None;
                let l = l.min(self.lines.len() - 1);
                self.anchor = Some((l, 0));
                self.cursor =
                    if l + 1 < self.lines.len() { (l + 1, 0) } else { (l, self.line_len(l)) };
                self.wanted = self.cursor.1;
            }
            Action::Select(a, b) => {
                self.run = None;
                self.anchor = Some(self.clamp(a));
                self.cursor = self.clamp(b);
                self.wanted = self.cursor.1;
            }
        }
    }

    // ---- Positions. ----

    /// The end of the text.
    fn end(&self) -> Pos {
        let l = self.lines.len() - 1;
        (l, self.line_len(l))
    }

    /// `p` brought inside the text.
    fn clamp(&self, p: Pos) -> Pos {
        let l = p.0.min(self.lines.len() - 1);
        (l, p.1.min(self.line_len(l)))
    }

    /// The text from `a` to `b` (`a` not after `b`, both inside the text).
    fn text_between(&self, a: Pos, b: Pos) -> String {
        let from = byte_at(&self.lines[a.0], a.1);
        if a.0 == b.0 {
            let to = byte_at(&self.lines[a.0], b.1);
            return String::from(&self.lines[a.0][from..to]);
        }
        let mut out = String::from(&self.lines[a.0][from..]);
        for line in &self.lines[a.0 + 1..b.0] {
            out.push('\n');
            out.push_str(line);
        }
        out.push('\n');
        let to = byte_at(&self.lines[b.0], b.1);
        out.push_str(&self.lines[b.0][..to]);
        out
    }

    fn here(&self) -> Where {
        Where { cursor: self.cursor, anchor: self.anchor() }
    }

    // ---- Editing. ----

    /// Puts `text` in place of what is between `start` and `end`, keeping the widest line's
    /// width up to date. This is the only place the lines are changed.
    fn replace_range(&mut self, start: Pos, end: Pos, text: &str) {
        let known = self.widest.get();
        let old_widest = known
            .map(|_| self.lines[start.0..=end.0].iter().map(|l| width_of(l)).max().unwrap_or(0));
        let head_end = byte_at(&self.lines[start.0], start.1);
        let tail_start = byte_at(&self.lines[end.0], end.1);
        let tail = String::from(&self.lines[end.0][tail_start..]);
        let mut head = core::mem::take(&mut self.lines[start.0]);
        head.truncate(head_end);
        let mut parts = text.split('\n');
        head.push_str(parts.next().unwrap_or(""));
        let mut fresh: Vec<String> = alloc::vec![head];
        fresh.extend(parts.map(String::from));
        if let Some(last) = fresh.last_mut() {
            last.push_str(&tail);
        }
        let new_widest = fresh.iter().map(|l| width_of(l)).max().unwrap_or(0);
        self.lines.splice(start.0..=end.0, fresh);
        if let (Some(known), Some(old)) = (known, old_widest) {
            self.widest.set(if old < known {
                Some(known.max(new_widest))
            } else if new_widest >= known {
                Some(new_widest)
            } else {
                None
            });
        }
    }

    /// Replaces what is between `start` and `end` with `text`, leaving the cursor after it,
    /// and records the step.
    fn edit(&mut self, start: Pos, end: Pos, text: &str, kind: Kind) {
        let removed = self.text_between(start, end);
        if removed.is_empty() && text.is_empty() {
            return;
        }
        let before = self.here();
        self.replace_range(start, end, text);
        self.cursor = end_after(start, text);
        self.anchor = None;
        self.wanted = self.cursor.1;
        let after = self.here();
        self.record(Step { at: start, removed, inserted: String::from(text), before, after, kind });
    }

    /// Replaces the selection (or nothing, at the cursor) with `text`.
    fn replace_selection(&mut self, text: &str, kind: Kind) {
        let (a, b) = self.selection().unwrap_or((self.cursor, self.cursor));
        self.edit(a, b, text, kind);
    }

    fn insert_char(&mut self, c: char) {
        match c {
            '\n' | '\r' => self.replace_selection("\n", Kind::Other),
            c if c.is_control() && c != '\t' => {}
            c => {
                let mut buf = [0u8; 4];
                self.replace_selection(c.encode_utf8(&mut buf), Kind::Typing);
            }
        }
    }

    fn backspace(&mut self, word: bool) {
        if let Some((a, b)) = self.selection() {
            return self.edit(a, b, "", Kind::Other);
        }
        let from = if word { self.word_left(self.cursor) } else { self.char_left(self.cursor) };
        self.edit(from, self.cursor, "", Kind::Backspace);
    }

    fn delete(&mut self, word: bool) {
        if let Some((a, b)) = self.selection() {
            return self.edit(a, b, "", Kind::Other);
        }
        let to = if word { self.word_right(self.cursor) } else { self.char_right(self.cursor) };
        self.edit(self.cursor, to, "", Kind::Delete);
    }

    /// Writes the step down, joining it to the last one when they are of the same kind and
    /// follow on from each other.
    fn record(&mut self, step: Step) {
        self.redo.clear();
        if self.saved.is_some_and(|s| s > self.undo.len()) {
            self.saved = None;
        }
        // Not joined to a step the text was saved after: undo must be able to come back to the
        // saved text.
        let joinable = step.kind != Kind::Other
            && self.run == Some(step.kind)
            && self.saved != Some(self.undo.len())
            && step.before.anchor.is_none();
        if joinable && let Some(last) = self.undo.last_mut() {
            let joined = match step.kind {
                Kind::Typing => {
                    let fits = step.removed.is_empty() && last.after.cursor == step.at;
                    if fits {
                        last.inserted.push_str(&step.inserted);
                    }
                    fits
                }
                Kind::Backspace => {
                    let fits = last.inserted.is_empty()
                        && step.inserted.is_empty()
                        && end_after(step.at, &step.removed) == last.at;
                    if fits {
                        last.removed.insert_str(0, &step.removed);
                        last.at = step.at;
                    }
                    fits
                }
                Kind::Delete => {
                    let fits =
                        last.inserted.is_empty() && step.inserted.is_empty() && last.at == step.at;
                    if fits {
                        last.removed.push_str(&step.removed);
                    }
                    fits
                }
                Kind::Other => false,
            };
            if joined {
                last.after = step.after;
                return;
            }
        }
        self.run = (step.kind != Kind::Other).then_some(step.kind);
        self.undo.push(step);
        if self.undo.len() > MAX_STEPS {
            self.undo.remove(0);
            // The saved text was before the step that was just forgotten: out of reach.
            self.saved = self.saved.and_then(|s| s.checked_sub(1));
        }
    }

    fn undo_one(&mut self) {
        self.run = None;
        let Some(step) = self.undo.pop() else {
            return;
        };
        let end = end_after(step.at, &step.inserted);
        self.replace_range(step.at, end, &step.removed);
        self.cursor = step.before.cursor;
        self.anchor = step.before.anchor;
        self.wanted = self.cursor.1;
        self.redo.push(step);
    }

    fn redo_one(&mut self) {
        self.run = None;
        let Some(step) = self.redo.pop() else {
            return;
        };
        let end = end_after(step.at, &step.removed);
        self.replace_range(step.at, end, &step.inserted);
        self.cursor = step.after.cursor;
        self.anchor = step.after.anchor;
        self.wanted = self.cursor.1;
        self.undo.push(step);
    }

    // ---- Moving. ----

    /// Moves the cursor to `to`, extending the selection or dropping it. `keep_wanted` leaves
    /// the column Up and Down aim for as it was.
    fn go(&mut self, to: Pos, select: bool, keep_wanted: bool) {
        if select {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = to;
        if self.anchor == Some(to) {
            self.anchor = None;
        }
        if !keep_wanted {
            self.wanted = to.1;
        }
    }

    fn char_left(&self, p: Pos) -> Pos {
        if p.1 > 0 {
            (p.0, p.1 - 1)
        } else if p.0 > 0 {
            (p.0 - 1, self.line_len(p.0 - 1))
        } else {
            p
        }
    }

    fn char_right(&self, p: Pos) -> Pos {
        if p.1 < self.line_len(p.0) {
            (p.0, p.1 + 1)
        } else if p.0 + 1 < self.lines.len() {
            (p.0 + 1, 0)
        } else {
            p
        }
    }

    /// The start of the word before `p`: past white space, then past the letters (or the
    /// punctuation) before it; from a line's start, the end of the line above.
    fn word_left(&self, p: Pos) -> Pos {
        if p.1 == 0 {
            return self.char_left(p);
        }
        let chars: Vec<char> = self.line(p.0).chars().collect();
        let mut i = p.1.min(chars.len());
        while i > 0 && class(chars[i - 1]) == 0 {
            i -= 1;
        }
        if i > 0 {
            let kind = class(chars[i - 1]);
            while i > 0 && class(chars[i - 1]) == kind {
                i -= 1;
            }
        }
        (p.0, i)
    }

    /// The start of the word after `p`: past the rest of this one, then past white space; from
    /// a line's end, the start of the line below.
    fn word_right(&self, p: Pos) -> Pos {
        let chars: Vec<char> = self.line(p.0).chars().collect();
        if p.1 >= chars.len() {
            return self.char_right(p);
        }
        let mut i = p.1;
        let kind = class(chars[i]);
        if kind != 0 {
            while i < chars.len() && class(chars[i]) == kind {
                i += 1;
            }
        }
        while i < chars.len() && class(chars[i]) == 0 {
            i += 1;
        }
        (p.0, i)
    }

    /// `lines` lines down (negative: up) from the cursor, in the column it wanted.
    fn vertical(&self, lines: isize) -> Pos {
        let l = self.cursor.0.saturating_add_signed(lines).min(self.lines.len() - 1);
        (l, self.wanted.min(self.line_len(l)))
    }

    fn motion(&mut self, motion: Motion, select: bool) {
        let at = self.cursor;
        let (to, keep) = match motion {
            // Without Shift, a selection is dropped and the cursor goes to its near end.
            Motion::Left => match (select, self.selection()) {
                (false, Some((start, _))) => (start, false),
                _ => (self.char_left(at), false),
            },
            Motion::Right => match (select, self.selection()) {
                (false, Some((_, end))) => (end, false),
                _ => (self.char_right(at), false),
            },
            Motion::WordLeft => (self.word_left(at), false),
            Motion::WordRight => (self.word_right(at), false),
            Motion::Up if at.0 == 0 => ((0, 0), false),
            Motion::Up => (self.vertical(-1), true),
            Motion::Down if at.0 + 1 == self.lines.len() => (self.end(), false),
            Motion::Down => (self.vertical(1), true),
            Motion::LineStart => ((at.0, 0), false),
            Motion::LineEnd => ((at.0, self.line_len(at.0)), false),
            Motion::TextStart => ((0, 0), false),
            Motion::TextEnd => (self.end(), false),
            Motion::PageUp(n) => (self.vertical(-(n.min(isize::MAX as usize) as isize)), true),
            Motion::PageDown(n) => (self.vertical(n.min(isize::MAX as usize) as isize), true),
            Motion::To(p) => (self.clamp(p), false),
        };
        self.go(to, select, keep);
    }

    fn select_word(&mut self, at: Pos) {
        let at = self.clamp(at);
        let chars: Vec<char> = self.line(at.0).chars().collect();
        if chars.is_empty() {
            return self.go(at, false, false);
        }
        // At the end of a line, the word before the cursor.
        let i = at.1.min(chars.len() - 1);
        let kind = class(chars[i]);
        let mut start = i;
        while start > 0 && class(chars[start - 1]) == kind {
            start -= 1;
        }
        let mut end = i + 1;
        while end < chars.len() && class(chars[end]) == kind {
            end += 1;
        }
        self.anchor = Some((at.0, start));
        self.cursor = (at.0, end);
        self.wanted = end;
    }
}
