// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The desktop boot (docs/design/0026-apps-m6-tests-and-stages.md, section 6): the machine is
//! started with `desktoptest` on the kernel's command line, the root program starts the display
//! server and windowed test programs (`services/root/src/desktoptest.rs`), and this runner is
//! the person at the machine. It moves the pointer, clicks and types through QEMU's control
//! socket, plugs the second screen out and in, takes screenshots, and checks what happened by
//! what the display server and the test programs write to the serial port:
//!
//! - the display server's **description** of its screens (note 0022, section 13): after each
//!   change a block of lines, `view N` to `view N done`, one per screen and per window, then
//!   the clipboard and the pointer. [`View`] reads the last whole block.
//! - the test programs' lines: `wintest NAME: key 8 'h'` and the like (`tests/programs/wintest`).
//!
//! The steps are the stage-5 part of section 7's table: windows drawn where they open; the
//! focus, keys only to the focused window, the pointer only to the one under it; the clipboard
//! between two apps; a window dragged onto a 2x screen redrawn at 2x; a screen unplugged moving
//! its windows; mirroring and the start menu key; a title bar's close button; a hung app marked
//! not responding while the others carry on; a hostile app refused everything; Ctrl+W; and the
//! display server crashed and started again.

use crate::pictures;
use crate::qemu::{self, Config, Qmp, Screens};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

/// How long each step may take to show what it should.
const STEP: Duration = Duration::from_secs(120);
/// How long the colours on a screen may take to be right after the description says so.
const PICTURE: Duration = Duration::from_secs(20);

/// One screen as the description gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenLine {
    pub number: u32,
    pub x: i32,
    pub y: i32,
    /// Logical pixels.
    pub width: i32,
    pub height: i32,
    pub scale: u32,
    pub mirrors: u32,
}

/// One window as the description gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowLine {
    /// The screen holding most of it (0 for none).
    pub screen: u32,
    /// The window's number, which its app knows it by (each app counts its own).
    pub number: u32,
    /// The app's number.
    pub app: u32,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale: u32,
    /// The words after the scale: `drawn`, `focused`, `not-responding`...
    pub words: Vec<String>,
}

impl WindowLine {
    fn is(&self, word: &str) -> bool {
        self.words.iter().any(|w| w == word)
    }

    /// The middle of its content.
    fn centre(&self) -> (i32, i32) {
        (self.x + self.width / 2, self.y + self.height / 2)
    }
}

/// The window menu, as the description gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuLine {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    /// Its rows' words, top to bottom.
    pub rows: Vec<String>,
}

impl MenuLine {
    /// The middle of the row that says `row`: rows are 22 pixels high, from 2 below the top
    /// (`lib/compositor`'s `MENU_ROW`).
    fn row(&self, row: &str) -> Option<(i32, i32)> {
        let i = self.rows.iter().position(|r| r == row)? as i32;
        Some((self.x + self.width / 2, self.y + 2 + i * 22 + 11))
    }
}

/// The last whole description.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub screens: Vec<ScreenLine>,
    pub windows: Vec<WindowLine>,
    pub pointer: Option<(i32, i32)>,
    pub menu: Option<MenuLine>,
}

impl View {
    /// The window whose title bar says `name`: as the whole title, the manifest's part (the
    /// app's own part follows after a dash), or the app's own part (`Test window — Alpha` is
    /// found as `Alpha`).
    fn window(&self, name: &str) -> Option<&WindowLine> {
        self.windows.iter().find(|w| {
            // (The display server adds this to a hung app's title.)
            let title = w.title.strip_suffix(" (not responding)").unwrap_or(&w.title);
            title == name
                || title.strip_prefix(name).is_some_and(|r| r.starts_with(" \u{2014}"))
                || title.strip_suffix(name).is_some_and(|r| r.ends_with("\u{2014} "))
        })
    }

    fn screen(&self, number: u32) -> Option<&ScreenLine> {
        self.screens.iter().find(|s| s.number == number)
    }
}

/// A point in the content of the window titled `title` that no window above it covers (the
/// description lists windows from the bottom up), with room for their frames.
fn visible_point(v: &View, title: &str) -> Option<(i32, i32)> {
    let at = v.windows.iter().position(|w| v.window(title) == Some(w))?;
    let w = &v.windows[at];
    let above = &v.windows[at + 1..];
    let covered = |x: i32, y: i32| {
        above.iter().any(|o| {
            x >= o.x - 8 && x < o.x + o.width + 8 && y >= o.y - 32 && y < o.y + o.height + 8
        })
    };
    for fy in [2, 1, 3] {
        for fx in [2, 1, 3] {
            let (x, y) = (w.x + w.width * fx / 4, w.y + w.height * fy / 4);
            if !covered(x, y) {
                return Some((x, y));
            }
        }
    }
    None
}

/// What the display server writes in front of each description line (the kernel puts the
/// program's name in front of each line it logs).
const PREFIX: &str = "[display] ";

fn num<T: std::str::FromStr>(s: &str) -> Option<T> {
    s.parse().ok()
}

fn pair(s: &str, sep: char) -> Option<(i32, i32)> {
    let (a, b) = s.split_once(sep)?;
    Some((num(a)?, num(b)?))
}

/// Reads the last description in `text` that has its `done` line.
pub fn last_view(text: &str) -> Option<View> {
    let lines: Vec<&str> =
        text.lines().filter_map(|l| l.trim_end_matches('\r').strip_prefix(PREFIX)).collect();
    let end = lines.iter().rposition(|l| l.starts_with("view ") && l.ends_with(" done"))?;
    let n = lines[end].strip_prefix("view ")?.strip_suffix(" done")?;
    let start = lines[..end].iter().rposition(|l| *l == format!("view {n}"))?;
    let mut view = View::default();
    for l in &lines[start + 1..end] {
        if let Some(rest) = l.strip_prefix("pointer at ") {
            view.pointer = pair(rest.split_whitespace().next()?, ',');
        } else if l.starts_with("menu for window ") {
            view.menu = menu_line(l);
        } else if l.starts_with("screen ") && l.contains(" window ") {
            view.windows.push(window_line(l)?);
        } else if l.starts_with("screen ") {
            view.screens.push(screen_line(l)?);
        }
    }
    Some(view)
}

/// `menu for window W of app A at X,Y size WxH: Row|Row|...`
fn menu_line(l: &str) -> Option<MenuLine> {
    let (head, rows) = l.split_once(": ")?;
    let words: Vec<&str> = head.split_whitespace().collect();
    let at = words.iter().position(|w| *w == "at")?;
    let (x, y) = pair(words.get(at + 1)?, ',')?;
    let (width, _) = pair(words.get(at + 3)?, 'x')?;
    Some(MenuLine { x, y, width, rows: rows.split('|').map(String::from).collect() })
}

/// `screen 1 "Virtual-1" 1280x800 at 0,0 scale 100 [rotation R] [mirrors M] [main]`
fn screen_line(l: &str) -> Option<ScreenLine> {
    let mut words = l.split_whitespace();
    words.next();
    let number = num(words.next()?)?;
    // The connector's name is in quotes and has no spaces.
    words.next();
    let (width, height) = pair(words.next()?, 'x')?;
    words.next();
    let (x, y) = pair(words.next()?, ',')?;
    let rest: Vec<&str> = words.collect();
    let after = |w: &str| rest.iter().position(|x| *x == w).and_then(|i| rest.get(i + 1));
    Some(ScreenLine {
        number,
        x,
        y,
        width,
        height,
        scale: num(after("scale")?)?,
        mirrors: after("mirrors").and_then(|m| num(m)).unwrap_or(0),
    })
}

/// `screen S window W "TITLE" app A at X,Y size WxH scale S words...`
fn window_line(l: &str) -> Option<WindowLine> {
    let (head, rest) = l.split_once(" \"")?;
    let (title, tail) = rest.rsplit_once("\" app ")?;
    let h: Vec<&str> = head.split_whitespace().collect();
    let t: Vec<&str> = tail.split_whitespace().collect();
    // t: A at X,Y size WxH scale S words...
    let (x, y) = pair(t.get(2)?, ',')?;
    let (width, height) = pair(t.get(4)?, 'x')?;
    Some(WindowLine {
        screen: num(h.get(1)?)?,
        number: num(h.get(3)?)?,
        app: num(t.first()?)?,
        title: title.to_string(),
        x,
        y,
        width,
        height,
        scale: num(t.get(6)?)?,
        words: t[7..].iter().map(|w| w.to_string()).collect(),
    })
}

/// One widget of the gallery, as `tests/programs/gallery` describes it in the log (sandstone's
/// `Description::line`): `gallery: widget W ROLE "LABEL" at X,Y size WxH [id KEY] [focused]
/// [disabled] [checked|unchecked] [value "VALUE"]`, positions in window W's logical pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WidgetLine {
    /// The gallery's own number for the window (0 is its main window).
    pub window: u32,
    pub role: String,
    pub label: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub key: Option<String>,
    pub focused: bool,
    pub checked: Option<bool>,
    pub value: String,
}

/// A sandstone program's last whole description (the gallery's, the file picker's, the
/// permissions viewer's): its windows (its own number and the display server's), their
/// widgets, and any other lines the program adds (what the picker's lists hold, what the
/// viewer shows an app holds).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GalleryView {
    pub windows: Vec<(u32, u32)>,
    pub widgets: Vec<WidgetLine>,
    pub other: Vec<String>,
}

impl GalleryView {
    /// The widget with id `name`, or else the one labelled `name` (not a plain label), or else a
    /// label saying it, or else the first of that role (`status bar`).
    fn widget(&self, name: &str) -> Option<&WidgetLine> {
        let w = &self.widgets;
        w.iter()
            .find(|w| w.key.as_deref() == Some(name))
            .or_else(|| w.iter().find(|w| w.label == name && w.role != "label"))
            .or_else(|| w.iter().find(|w| w.label == name))
            .or_else(|| w.iter().find(|w| w.role == name && w.window == 0))
    }

    /// The focused widget of the main window.
    fn focused(&self) -> Option<&WidgetLine> {
        self.widgets.iter().find(|w| w.window == 0 && w.focused)
    }

    /// The display server's number for the gallery's window `window`.
    fn display(&self, window: u32) -> Option<u32> {
        self.windows.iter().find(|(g, _)| *g == window).map(|(_, d)| *d)
    }

    /// The rest of the other line that starts with `start`.
    fn other(&self, start: &str) -> Option<&str> {
        self.other.iter().find_map(|l| l.strip_prefix(start))
    }

    /// The names a picker line lists (`folder PATH: a|b|c`, `places a|b`), if `start` is there.
    fn names(&self, start: &str) -> Option<Vec<String>> {
        let rest = self.other(start)?;
        Some(if rest.is_empty() { Vec::new() } else { rest.split('|').map(String::from).collect() })
    }
}

/// Undoes Rust's `escape_debug` for the characters the gallery's labels and values may hold.
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The text between a quote at the start of `s` and the next quote that isn't escaped, and
/// what follows it.
fn quoted(s: &str) -> Option<(String, &str)> {
    let s = s.strip_prefix('"')?;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        match c {
            '\\' if !escaped => escaped = true,
            '"' if !escaped => return Some((unescape(&s[..i]), &s[i + 1..])),
            _ => escaped = false,
        }
    }
    None
}

fn widget_line(rest: &str) -> Option<WidgetLine> {
    let (window, rest) = rest.split_once(' ')?;
    let q = rest.find('"')?;
    let role = rest[..q].trim().to_string();
    let (label, tail) = quoted(&rest[q..])?;
    let (tail, value) = match tail.find(" value \"") {
        Some(at) => (&tail[..at], quoted(&tail[at + 7..]).map(|(v, _)| v).unwrap_or_default()),
        None => (tail, String::new()),
    };
    let t: Vec<&str> = tail.split_whitespace().collect();
    // t: at X,Y size WxH words...
    let (x, y) = pair(t.get(1)?, ',')?;
    let (width, height) = pair(t.get(3)?, 'x')?;
    let words = &t[4..];
    let key =
        words.iter().position(|w| *w == "id").and_then(|i| words.get(i + 1)).map(|k| k.to_string());
    let checked = if words.contains(&"checked") {
        Some(true)
    } else if words.contains(&"unchecked") {
        Some(false)
    } else {
        None
    };
    Some(WidgetLine {
        window: num(window)?,
        role,
        label,
        x,
        y,
        width,
        height,
        key,
        focused: words.contains(&"focused"),
        checked,
        value,
    })
}

/// Reads the last whole description the gallery wrote: `gallery: view N` to `gallery: view N
/// done`.
pub fn last_gallery_view(text: &str) -> Option<GalleryView> {
    last_program_view(text, "gallery")
}

/// Reads the last whole description `program` wrote: `[program] program: view N` to `...
/// view N done`.
pub fn last_program_view(text: &str, program: &str) -> Option<GalleryView> {
    let prefix = format!("[{program}] {program}: ");
    let lines: Vec<&str> =
        text.lines().filter_map(|l| l.split_once(prefix.as_str()).map(|(_, r)| r)).collect();
    let end = lines.iter().rposition(|l| l.starts_with("view ") && l.ends_with(" done"))?;
    let n = lines[end].strip_prefix("view ")?.strip_suffix(" done")?;
    let start = lines[..end].iter().rposition(|l| *l == format!("view {n}"))?;
    let mut view = GalleryView::default();
    for l in &lines[start + 1..end] {
        if let Some(rest) = l.strip_prefix("window ") {
            let (g, d) = rest.split_once(" is ")?;
            view.windows.push((num(g)?, num(d)?));
        } else if let Some(rest) = l.strip_prefix("widget ") {
            view.widgets.push(widget_line(rest)?);
        } else {
            view.other.push(l.to_string());
        }
    }
    Some(view)
}

/// A running desktop boot.
struct Run<'a> {
    cfg: &'a Config,
    image: &'a Path,
    log: std::path::PathBuf,
    child: Child,
    qmp: Qmp,
    vnc: Option<crate::vnc::Vnc>,
    started: Instant,
    limit: Duration,
}

/// Which of the desktop boot's three boots (note 0024, section 11).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Boot {
    /// On fresh disks: the desktop, its windows and menu, ending in falling back to the shell,
    /// which restarts the machine.
    First,
    /// The same disks again: what was saved is there; the start menu shuts the machine down.
    Second,
    /// From the `text desktoptest` image: the shell, which turns the machine off.
    Text,
}

/// Boots `cfg` three times (note 0024, section 11): from `image` (made with `desktoptest` on
/// its command line) on fresh disks, then on the same disks again, then from `text_image`
/// (`text desktoptest`). Returns what the machines wrote to their serial ports.
pub fn test(
    cfg: &Config,
    image: &Path,
    text_image: &Path,
    work: &Path,
    limit: Duration,
) -> Result<String, String> {
    let mut all = String::new();
    for (boot, img, name) in [
        (Boot::First, image, "desktop"),
        (Boot::Second, image, "desktop-again"),
        (Boot::Text, text_image, "desktop-text"),
    ] {
        eprintln!("  ({name})");
        let mut run = Run::start(cfg, img, work, limit, boot, name)?;
        let result = run.steps(boot);
        let text = run.text();
        run.stop();
        all.push_str(&text);
        if let Err(e) = result {
            return Err(format!("{name}: {e}\n--- serial output ---\n{text}"));
        }
    }
    Ok(all)
}

impl<'a> Run<'a> {
    /// Starts QEMU for one boot, on fresh disks unless it is the second.
    fn start(
        cfg: &'a Config,
        image: &'a Path,
        work: &Path,
        limit: Duration,
        boot: Boot,
        name: &str,
    ) -> Result<Run<'a>, String> {
        let tag = qemu::tag(cfg);
        let log = work.join(format!("{name}-{tag}.txt"));
        let _ = fs::remove_file(&log);
        let mut cmd = qemu::command_on(cfg, image, work, true, boot != Boot::Second)?;
        cmd.args(["-display", "none", "-monitor", "none"]);
        let two = cfg.screens == Screens::TwoVirtioGpu;
        let vnc_path = work.join(format!("vnc-desktop-{tag}.sock"));
        let _ = fs::remove_file(&vnc_path);
        if two {
            cmd.arg("-vnc").arg(format!("unix:{},display=gpu2,head=1", vnc_path.display()));
        }
        cmd.arg("-serial").arg(format!("file:{}", log.display()));
        if cfg.arch == "x86_64" {
            cmd.args(["-serial", "null"]);
        }
        let qmp_path = work.join(format!("qmp-desktop-{tag}.sock"));
        let _ = fs::remove_file(&qmp_path);
        cmd.arg("-qmp").arg(format!("unix:{},server=on,wait=off", qmp_path.display()));
        cmd.stdout(Stdio::null()).stderr(Stdio::piped());
        let child = cmd.spawn().map_err(|e| format!("couldn't start QEMU: {e}"))?;
        let mut run = Run {
            cfg,
            image,
            log,
            child,
            qmp: Qmp::new(qmp_path, qemu::input_screen(cfg, true)),
            vnc: None,
            started: Instant::now(),
            limit,
        };
        if two {
            match qemu::plug(&vnc_path) {
                Ok(v) => run.vnc = Some(v),
                Err(e) => {
                    run.stop();
                    return Err(format!("plugging in the second screen: {e}"));
                }
            }
        }
        Ok(run)
    }
}

impl Run<'_> {
    fn text(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default().replace('\r', "")
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// The registers of every core, with the kernel's functions named: for a boot that hung.
    fn registers(&mut self) -> String {
        let regs = self.qmp.registers(self.cfg.cores);
        qemu::name_addresses(regs, self.cfg, self.image)
    }

    /// Waits until `ready` says the serial output shows what it should. Fails at once if the
    /// test failed, the kernel stopped or QEMU ended, and after `STEP` (or the whole boot's
    /// limit) if it never shows.
    fn wait(&mut self, what: &str, ready: impl Fn(&str) -> bool) -> Result<String, String> {
        let end = Instant::now() + STEP;
        loop {
            let text = self.text();
            if ready(&text) {
                return Ok(text);
            }
            if let Some(at) = text.find("Desktop test FAILED") {
                let line = text[at..].lines().next().unwrap_or_default();
                return Err(format!("waiting for {what}: {line}"));
            }
            if text.contains("\nException on core") || text.contains("The kernel stopped because") {
                std::thread::sleep(Duration::from_secs(3));
                let regs = self.registers();
                return Err(format!("waiting for {what}: the kernel stopped\n{regs}"));
            }
            if let Ok(Some(st)) = self.child.try_wait() {
                return Err(format!("waiting for {what}: QEMU stopped ({st})"));
            }
            if Instant::now() > end || self.started.elapsed() > self.limit {
                let regs = self.registers();
                let _ = fs::write(self.log.with_extension("registers.txt"), &regs);
                return Err(format!("waiting for {what}: it didn't happen\n{regs}"));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Waits for a line containing `line`.
    fn line(&mut self, line: &str) -> Result<String, String> {
        eprintln!("  (waiting for: {line})");
        self.wait(line, |t| t.contains(line))
    }

    /// Waits for `line` to have been written at least `n` times.
    fn lines(&mut self, line: &str, n: usize) -> Result<(), String> {
        eprintln!("  (waiting for: {line} x{n})");
        self.wait(line, |t| t.matches(line).count() >= n).map(|_| ())
    }

    /// Waits for a description that `ok` accepts, and returns it.
    fn view(&mut self, what: &str, ok: impl Fn(&View) -> bool) -> Result<View, String> {
        eprintln!("  (waiting for the screens to show: {what})");
        let text = self.wait(what, |t| last_view(t).is_some_and(|v| ok(&v)))?;
        last_view(&text).ok_or_else(|| "no description".into())
    }

    /// Moves the pointer to (`x`, `y`) in the space with the mouse, from where the
    /// description says it is, and waits for it to get there.
    fn move_to(&mut self, x: i32, y: i32) -> Result<View, String> {
        let at = last_view(&self.text()).and_then(|v| v.pointer).unwrap_or((0, 0));
        let rel = |axis: &str, v: i32| {
            format!(r#"{{"type":"rel","data":{{"axis":"{axis}","value":{v}}}}}"#)
        };
        let (dx, dy) = (x - at.0, y - at.1);
        if (dx, dy) != (0, 0) {
            self.qmp.send(true, &[rel("x", dx), rel("y", dy)])?;
        }
        self.view(&format!("the pointer at {x},{y}"), |v| v.pointer == Some((x, y)))
    }

    fn button(&mut self, down: bool) -> Result<(), String> {
        let b = format!(r#"{{"type":"btn","data":{{"button":"left","down":{down}}}}}"#);
        self.qmp.send(true, &[b])?;
        std::thread::sleep(Duration::from_millis(150));
        Ok(())
    }

    fn click_at(&mut self, x: i32, y: i32) -> Result<(), String> {
        self.move_to(x, y)?;
        self.button(true)?;
        self.button(false)
    }

    /// Presses keys in order and lets them go the other way round (QEMU's names for them).
    fn chord(&mut self, keys: &[&str]) -> Result<(), String> {
        let key = |name: &str, down: bool| {
            format!(
                r#"{{"type":"key","data":{{"down":{down},"key":{{"type":"qcode","data":"{name}"}}}}}}"#
            )
        };
        for k in keys {
            self.qmp.send(true, &[key(k, true)])?;
            std::thread::sleep(Duration::from_millis(40));
        }
        for k in keys.iter().rev() {
            self.qmp.send(true, &[key(k, false)])?;
            std::thread::sleep(Duration::from_millis(40));
        }
        Ok(())
    }

    /// Clicks with the tablet tied to the first screen, at (`x`, `y`) on it (logical pixels;
    /// `screen` is that screen's line).
    fn tablet_click(&mut self, screen: &ScreenLine, x: i32, y: i32) -> Result<(), String> {
        let target = match self.cfg.screens {
            Screens::TwoVirtioGpu => r#""device":"gpu2","head":0,"#,
            _ => "",
        };
        // QEMU's tablets measure 0 to 32767 across the screen; aim at the middle of the
        // logical pixel.
        let at = |v: i32, len: i32| {
            ((2 * i64::from(v) + 1) * 32767 / (2 * i64::from(len.max(1)))) as u32
        };
        let abs = |axis: &str, v: u32| {
            format!(r#"{{"type":"abs","data":{{"axis":"{axis}","value":{v}}}}}"#)
        };
        let button =
            |down: bool| format!(r#"{{"type":"btn","data":{{"button":"left","down":{down}}}}}"#);
        let (ax, ay) = (at(x - screen.x, screen.width), at(y - screen.y, screen.height));
        for events in [vec![abs("x", ax), abs("y", ay)], vec![button(true)], vec![button(false)]] {
            self.qmp.command(&format!(
                r#"{{"execute":"input-send-event","arguments":{{{target}"events":[{}]}}}}"#,
                events.join(",")
            ))?;
            std::thread::sleep(Duration::from_millis(150));
        }
        Ok(())
    }

    /// QEMU's name and head for screen `number` of the description.
    fn qemu_screen(&self, view: &View, number: u32) -> Result<(&'static str, u32), String> {
        match self.cfg.screens {
            Screens::TwoVirtioGpu => {
                // The screens are numbered in device order; the first is head 0. A screen
                // plugged in again gets a new number, but is still the second.
                let i = view.screens.iter().position(|s| s.number == number).ok_or("no screen")?;
                Ok(("gpu2", i as u32))
            }
            _ => Ok(("gpu", 0)),
        }
    }

    /// Waits until the screen showing point (`x`, `y`) of the space has `colour` there.
    fn colour_at(&mut self, view: &View, x: i32, y: i32, colour: u32) -> Result<(), String> {
        let s = view
            .screens
            .iter()
            .find(|s| {
                s.mirrors == 0 && x >= s.x && y >= s.y && x < s.x + s.width && y < s.y + s.height
            })
            .ok_or(format!("no screen shows {x},{y}"))?
            .clone();
        let (device, head) = self.qemu_screen(view, s.number)?;
        let factor = (s.scale / 100).max(1) as i32;
        let (px, py) = ((x - s.x) * factor, (y - s.y) * factor);
        let file = self.log.with_extension(format!("{device}-{head}.ppm"));
        let end = Instant::now() + PICTURE;
        loop {
            let _ = fs::remove_file(&file);
            self.qmp.screendump(device, head, &file)?;
            let bytes = fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let shot = pictures::read_ppm(&bytes)?;
            let got = shot.pixels.get((py as u32 * shot.width + px as u32) as usize).copied();
            if got == Some(colour) {
                let _ = fs::remove_file(&file);
                return Ok(());
            }
            if Instant::now() > end {
                return Err(format!(
                    "screen {} at {px},{py} is {got:06x?}, not {colour:06x} (screenshot kept: {})",
                    s.number,
                    file.display()
                ));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// Plugs the second screen in (`true`) or out.
    fn plug(&mut self, on: bool) -> Result<(), String> {
        let (w, h) = if on { qemu::SECOND_SCREEN } else { (0, 0) };
        self.vnc.as_mut().ok_or("no second screen")?.set_size(w, h)
    }

    fn steps(&mut self, boot: Boot) -> Result<(), String> {
        match boot {
            Boot::First => self.first(),
            Boot::Second => self.second(),
            Boot::Text => self.text_mode(),
        }
    }

    /// Waits for the input devices, and the desktop drawn (with the screens arranged as its
    /// `screens.txt` says, on two screens): QEMU drops input sent to a device whose driver
    /// isn't listening yet.
    fn desktop_ready(&mut self, times: usize) -> Result<(), String> {
        self.line("input: using QEMU Virtio Keyboard")?;
        self.line("input: using QEMU Virtio Mouse")?;
        // (Only now: QEMU makes its control socket a moment after it starts.)
        self.qmp.connect()?;
        match self.cfg.screens {
            Screens::TwoVirtioGpu => self.lines("display: 2 tablets.", times)?,
            Screens::FirmwareWithTablet => self.lines("display: 1 tablets.", times)?,
            Screens::Firmware => {}
        }
        self.lines("desktop: ready", times)?;
        if self.cfg.screens == Screens::TwoVirtioGpu {
            self.lines("desktop: arranged 2 screens from screens.txt", times)?;
            self.view("the second screen at 2x", |v| v.screens.iter().any(|s| s.scale == 200))?;
        }
        Ok(())
    }

    /// Keeps a screenshot of every screen beside the log, as `NAME-<what>-<screen>.ppm`: what
    /// the desktop looks like, for people (they aren't compared with anything).
    fn keep_screens(&mut self, what: &str) -> Result<(), String> {
        std::thread::sleep(Duration::from_secs(2));
        let heads = if self.cfg.screens == Screens::TwoVirtioGpu { 2 } else { 1 };
        let device = if self.cfg.screens == Screens::TwoVirtioGpu { "gpu2" } else { "gpu" };
        for head in 0..heads {
            let file = self.log.with_extension(format!("{what}-{head}.ppm"));
            self.qmp.screendump(device, head, &file)?;
        }
        Ok(())
    }

    /// Opens the start menu with the Windows key and clicks `item` in it (the desktop says
    /// where each is).
    fn choose_from_start_menu(&mut self, item: &str, times: usize) -> Result<(), String> {
        // The desktop opens the menu as a popup, which needs the key's serial within a second;
        // a desktop still busy redrawing its wallpapers (the screens changed just before)
        // can miss that, and the menu doesn't open. A person presses the key again, and so
        // does the runner, up to three times.
        let mut tries = 0;
        loop {
            self.chord(&["meta_l"])?;
            tries += 1;
            let end = Instant::now() + Duration::from_secs(20);
            while self.text().matches("desktop: start menu open").count() < times
                && Instant::now() < end
            {
                std::thread::sleep(Duration::from_millis(200));
            }
            if self.text().matches("desktop: start menu open").count() >= times || tries == 3 {
                break;
            }
            eprintln!("  (the start menu didn't open: pressing the key again)");
        }
        self.lines("desktop: start menu open", times)?;
        // The desktop says where each choice is after it says the menu is open, Shut down
        // last: wait for all of them.
        self.lines("desktop: menu Shut down at ", times)?;
        if times == 2 {
            self.keep_screens("start-menu")?;
        }
        let text = self.text();
        let want = format!("desktop: menu {item} at ");
        let at = text
            .lines()
            .rev()
            .find_map(|l| l.split_once(want.as_str()).map(|(_, r)| r.trim().to_string()))
            .ok_or(format!("the start menu has no {item}"))?;
        let (x, y) = pair(&at, ',').ok_or(format!("the start menu's {item} at {at}"))?;
        self.click_at(x, y)
    }

    /// Waits for QEMU to stop by itself, for `reason` (`guest-reset`: told not to restart, it
    /// stops; `guest-shutdown`: turned off).
    fn stopped(&mut self, reason: &str) -> Result<(), String> {
        let end = Instant::now() + Duration::from_secs(120);
        while !matches!(self.child.try_wait(), Ok(Some(_))) {
            if Instant::now() > end {
                return Err(format!("the machine didn't stop ({reason}) in 2 minutes"));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        match self.qmp.shutdown_reason().as_deref() {
            Some(r) if r == reason => Ok(()),
            other => Err(format!("QEMU stopped for the reason {other:?}, not {reason}")),
        }
    }

    /// Waits for the shell's prompt (the console mirrors what it shows to the serial port),
    /// then types `line` into it.
    fn shell(&mut self, line: &str) -> Result<(), String> {
        self.line("[console] [console:read] ")?;
        self.qmp.connect()?;
        std::thread::sleep(Duration::from_millis(500));
        self.qmp.type_line(line)
    }

    /// The second boot, on the same disks: the note saved in the first is found, the screens are
    /// where `screens.txt` says, and the start menu's Shut down turns the machine off.
    fn second(&mut self) -> Result<(), String> {
        self.desktop_ready(1)?;
        self.line("Desktop test: after restarting")?;
        self.line("Desktop test passed")?;
        self.choose_from_start_menu("Shut down", 1)?;
        self.line("users: chris asked to turn the computer off")?;
        self.stopped("guest-shutdown")
    }

    /// The text-mode boot: the console instead of the display server, and the shell on it.
    fn text_mode(&mut self) -> Result<(), String> {
        self.line("Desktop test: text mode")?;
        self.shell("poweroff")?;
        self.line("users: chris asked to turn the computer off")?;
        self.stopped("guest-shutdown")
    }

    /// Waits for a description from the gallery that `ok` accepts, and returns it.
    fn gallery_view(
        &mut self,
        what: &str,
        ok: impl Fn(&GalleryView) -> bool,
    ) -> Result<GalleryView, String> {
        eprintln!("  (waiting for the gallery to show: {what})");
        let text = self.wait(what, |t| last_gallery_view(t).is_some_and(|v| ok(&v)))?;
        last_gallery_view(&text).ok_or_else(|| "no description from the gallery".into())
    }

    /// The display server's line for the gallery's window `window` (`main` is the line of its
    /// main window: windows are numbered by each app, so the app's number tells them apart).
    fn gallery_window(
        v: &View,
        g: &GalleryView,
        main: &WindowLine,
        window: u32,
    ) -> Option<WindowLine> {
        let number = g.display(window)?;
        v.windows.iter().find(|w| w.app == main.app && w.number == number).cloned()
    }

    /// Clicks the middle of the gallery's widget `name` (an id or a label), in whichever of its
    /// windows it is.
    fn click_widget(&mut self, name: &str) -> Result<(), String> {
        let g = self.gallery_view(&format!("a widget {name}"), |g| g.widget(name).is_some())?;
        let w = g.widget(name).cloned().ok_or(format!("no widget {name}"))?;
        let v = self.view("the gallery", |v| v.window("Gallery").is_some())?;
        let main = v.window("Gallery").cloned().ok_or("no Gallery")?;
        let win = Self::gallery_window(&v, &g, &main, w.window)
            .ok_or(format!("the display server doesn't show the gallery's window {}", w.window))?;
        self.click_at(win.x + w.x + w.width / 2, win.y + w.y + w.height / 2)
    }

    /// Turns the wheel `steps` towards the user where the pointer is.
    fn wheel(&mut self, steps: u32) -> Result<(), String> {
        for _ in 0..steps {
            for down in [true, false] {
                let b =
                    format!(r#"{{"type":"btn","data":{{"button":"wheel-down","down":{down}}}}}"#);
                self.qmp.send(true, &[b])?;
                std::thread::sleep(Duration::from_millis(60));
            }
        }
        Ok(())
    }

    /// Compares what the screen shows of window `w` with the reference picture `name`
    /// (`tests/pictures/sandstone/NAME.png`, drawn on the host by the gallery's own tests),
    /// pixel for pixel, over the part of the window that is on its screen. The screenshot is
    /// kept beside the log as a PNG either way, for people to look at.
    fn compare_window(&mut self, v: &View, w: &WindowLine, name: &str) -> Result<(), String> {
        let s = v.screen(w.screen).cloned().ok_or(format!("no screen holds {}", w.title))?;
        let (device, head) = self.qemu_screen(v, s.number)?;
        let f = (s.scale / 100).max(1) as i32;
        let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../tests/pictures/sandstone/{name}.png"));
        let want =
            pbr_testpng::decode(&fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?)?;
        let shot_file = self.log.with_extension(format!("{name}.ppm"));
        let kept = self.log.with_extension(format!("{name}.png"));
        let end = Instant::now() + PICTURE;
        loop {
            let _ = fs::remove_file(&shot_file);
            self.qmp.screendump(device, head, &shot_file)?;
            let bytes =
                fs::read(&shot_file).map_err(|e| format!("{}: {e}", shot_file.display()))?;
            let shot = pictures::read_ppm(&bytes)?;
            let _ = fs::remove_file(&shot_file);
            // The window's content in the screenshot's pixels, and the part of it on the screen.
            let (ox, oy) = ((w.x - s.x) * f, (w.y - s.y) * f);
            let (cw, ch) = (
                (w.width * f).min(shot.width as i32 - ox),
                (w.height * f).min(shot.height as i32 - oy),
            );
            if ox < 0 || oy < 0 || cw <= 0 || ch <= 0 || want.width as i32 != w.width * f {
                return Err(format!(
                    "{} isn't where a picture of it can be taken ({w:?})",
                    w.title
                ));
            }
            let mut got =
                pbr_testpng::Picture { width: cw as u32, height: ch as u32, pixels: Vec::new() };
            let mut differ = None;
            for y in 0..ch {
                for x in 0..cw {
                    let p = shot.pixels[((oy + y) as u32 * shot.width + (ox + x) as u32) as usize]
                        | 0xFF00_0000;
                    let r = want.pixels[(y as u32 * want.width + x as u32) as usize] | 0xFF00_0000;
                    if p != r && differ.is_none() {
                        differ = Some((x, y, p, r));
                    }
                    got.pixels.push(p);
                }
            }
            if let Ok(png) = pbr_testpng::encode(&got) {
                let _ = fs::write(&kept, png);
            }
            match differ {
                None => return Ok(()),
                Some((x, y, p, r)) if Instant::now() > end => {
                    return Err(format!(
                        "the gallery on screen differs from {name}.png at {x},{y}: {:06x}, not {:06x} \
                         (the screenshot is kept: {})",
                        p & 0xFF_FFFF,
                        r & 0xFF_FFFF,
                        kept.display()
                    ));
                }
                Some(_) => std::thread::sleep(Duration::from_millis(500)),
            }
        }
    }

    /// Waits for a description from `program` (`picker`, `permissions`) that `ok` accepts.
    fn program_view(
        &mut self,
        program: &str,
        what: &str,
        ok: impl Fn(&GalleryView) -> bool,
    ) -> Result<GalleryView, String> {
        eprintln!("  (waiting for the {program} to show: {what})");
        let text = self.wait(what, |t| last_program_view(t, program).is_some_and(|v| ok(&v)))?;
        last_program_view(&text, program)
            .ok_or_else(|| format!("no description from the {program}"))
    }

    /// The display server's line for `program`'s window `window`, given the line of its main
    /// window (titled `title`).
    fn program_window(
        &mut self,
        g: &GalleryView,
        title: &str,
        window: u32,
    ) -> Result<(View, WindowLine), String> {
        let number = g.display(window).ok_or(format!("{title} has no window {window}"))?;
        let v = self.view(&format!("{title}'s window {window}"), |v| {
            v.window(title).is_some_and(|main| {
                v.windows.iter().any(|w| w.app == main.app && w.number == number)
            })
        })?;
        let main = v.window(title).cloned().ok_or(format!("no {title}"))?;
        let w = v
            .windows
            .iter()
            .find(|w| w.app == main.app && w.number == number)
            .cloned()
            .ok_or(format!("no window {number} of {title}"))?;
        Ok((v, w))
    }

    /// Clicks the middle of `program`'s widget `name`, in whichever of its windows it is; its
    /// main window is titled `title`.
    fn click_program_widget(
        &mut self,
        program: &str,
        title: &str,
        name: &str,
    ) -> Result<(), String> {
        let g =
            self.program_view(program, &format!("a widget {name}"), |g| g.widget(name).is_some())?;
        let w = g.widget(name).cloned().ok_or(format!("no widget {name}"))?;
        let (_, win) = self.program_window(&g, title, w.window)?;
        self.click_at(win.x + w.x + w.width / 2, win.y + w.y + w.height / 2)
    }

    /// Clicks row `row` of `program`'s list `list` (in its main window, titled `title`): one
    /// pixel of edge, the heading's 21, then rows of 20 (sandstone's `List`).
    fn click_row(
        &mut self,
        program: &str,
        title: &str,
        list: &str,
        row: usize,
    ) -> Result<(), String> {
        let g =
            self.program_view(program, &format!("the list {list}"), |g| g.widget(list).is_some())?;
        let l = g.widget(list).cloned().ok_or(format!("no list {list}"))?;
        let (_, win) = self.program_window(&g, title, l.window)?;
        let y = l.y + 1 + 21 + 20 * row as i32 + 10;
        if y > l.y + l.height - 4 {
            return Err(format!("row {row} of {list} is out of sight"));
        }
        self.click_at(win.x + l.x + 40, win.y + y)
    }

    /// Keeps a picture of what the screen shows of window `w` beside the log, as
    /// `NAME.png`: for people to look at (it isn't compared with anything).
    fn keep_window(&mut self, v: &View, w: &WindowLine, name: &str) -> Result<(), String> {
        let s = v.screen(w.screen).cloned().ok_or(format!("no screen holds {}", w.title))?;
        let (device, head) = self.qemu_screen(v, s.number)?;
        let f = (s.scale / 100).max(1) as i32;
        // Give the window a moment to finish drawing.
        std::thread::sleep(Duration::from_secs(2));
        let shot_file = self.log.with_extension(format!("{name}.ppm"));
        self.qmp.screendump(device, head, &shot_file)?;
        let bytes = fs::read(&shot_file).map_err(|e| format!("{}: {e}", shot_file.display()))?;
        let _ = fs::remove_file(&shot_file);
        let shot = pictures::read_ppm(&bytes)?;
        // The window with its frame: the title bar (27) above, and a few pixels round it.
        let (ox, oy) = (((w.x - s.x - 4) * f).max(0), ((w.y - s.y - 31) * f).max(0));
        let cw = ((w.width + 8) * f).min(shot.width as i32 - ox);
        let ch = ((w.height + 35) * f).min(shot.height as i32 - oy);
        if cw <= 0 || ch <= 0 {
            return Err(format!("{} isn't on its screen ({w:?})", w.title));
        }
        let mut got =
            pbr_testpng::Picture { width: cw as u32, height: ch as u32, pixels: Vec::new() };
        for y in 0..ch {
            for x in 0..cw {
                got.pixels.push(
                    shot.pixels[((oy + y) as u32 * shot.width + (ox + x) as u32) as usize]
                        | 0xFF00_0000,
                );
            }
        }
        let png = pbr_testpng::encode(&got)?;
        fs::write(self.log.with_extension(format!("{name}.png")), png).map_err(|e| e.to_string())
    }

    /// Drags window `w` by its title bar so that its content's corner is at (`x`, `y`).
    fn drag_window(&mut self, w: &WindowLine, x: i32, y: i32) -> Result<(), String> {
        self.move_to(w.x + 60, w.y - 13)?;
        self.button(true)?;
        self.move_to(x + 60, y - 13)?;
        self.button(false)
    }

    /// Types `text` (lower-case letters and digits) one key at a time.
    fn type_text(&mut self, text: &str) -> Result<(), String> {
        for c in text.chars() {
            self.chord(&[&c.to_string()])?;
        }
        Ok(())
    }

    /// How many times `line` has been written so far.
    fn count(&self, line: &str) -> usize {
        self.text().matches(line).count()
    }

    /// Gives the gallery the keyboard, with a click on its status bar.
    fn focus_gallery(&mut self) -> Result<(), String> {
        self.click_widget("status bar")?;
        self.view("the gallery focused", |v| v.window("Gallery").is_some_and(|w| w.is("focused")))
            .map(|_| ())
    }

    /// The gallery asks the picker (Ctrl+S, Ctrl+O): waits for its window, over the gallery's.
    fn ask_picker(
        &mut self,
        keys: &[&str],
        what: &str,
    ) -> Result<(GalleryView, WindowLine), String> {
        let opened = self.count("picker: open for app");
        self.focus_gallery()?;
        self.chord(keys)?;
        self.lines("picker: open for app", opened + 1)?;
        let g = self.program_view("picker", what, |g| {
            g.windows.len() == 1 && g.other("places ").is_some()
        })?;
        let title = "File picker";
        let v = self.view("the picker's window", |v| {
            v.window(title).is_some_and(|w| w.is("drawn") && w.is("system"))
        })?;
        let w = v.window(title).cloned().ok_or("no picker window")?;
        Ok((g, w))
    }

    /// Saves the gallery's text through the picker as `name` (`.txt` is added): typed over the
    /// name offered, then Enter. With `replace`, the file is there already, and the picker asks
    /// first.
    fn save_as(
        &mut self,
        name: &str,
        replace: bool,
        pictures: bool,
        first: &ScreenLine,
    ) -> Result<(), String> {
        let given = self.count(&format!("was given /Home/Documents/{name}.txt"));
        let saved = self.count(&format!("to /Home/Documents/{name}.txt"));
        let (_, w) = self.ask_picker(&["ctrl", "s"], "saving")?;
        self.program_view("picker", "the name field focused", |g| {
            g.widget("name").is_some_and(|w| w.focused && w.value == "Gallery.txt")
        })?;
        if pictures {
            // For people: the picker as it opened, at 1x and, dragged there, at 2x. The pointer
            // is moved off it first, so that it hovers nothing.
            self.move_to(first.x + first.width - 8, first.y + 8)?;
            let v = self.view("the picker", |v| v.window("File picker").is_some())?;
            self.keep_window(&v, &w, "picker-1x")?;
            if self.cfg.screens == Screens::TwoVirtioGpu {
                let second =
                    v.screens.iter().find(|s| s.number != 1).cloned().ok_or("no screen 2")?;
                self.drag_window(&w, second.x + 20, second.y + 40)?;
                self.move_to(first.x + first.width - 8, first.y + 8)?;
                let v = self.view("the picker at 2x", |v| {
                    v.window("File picker")
                        .is_some_and(|p| p.scale == 200 && p.screen == second.number)
                })?;
                let w2 = v.window("File picker").cloned().ok_or("no picker")?;
                self.keep_window(&v, &w2, "picker-2x")?;
                self.drag_window(&w2, w.x, w.y)?;
                self.view("the picker back", |v| {
                    v.window("File picker").is_some_and(|p| p.scale == 100)
                })?;
            }
        }
        // Select what is offered, type over it, Enter.
        self.chord(&["ctrl", "a"])?;
        self.type_text(name)?;
        self.program_view("picker", "the name typed", |g| {
            g.widget("name").is_some_and(|w| w.value == name)
        })?;
        self.chord(&["ret"])?;
        if replace {
            self.program_view("picker", "Replace?", |g| {
                g.widget("Replace").is_some_and(|w| w.window != 0)
            })?;
            self.chord(&["ret"])?;
        }
        self.lines(&format!("was given /Home/Documents/{name}.txt"), given + 1)?;
        self.lines(&format!("to /Home/Documents/{name}.txt"), saved + 1)
    }

    /// In the open picker, opens folder or file `name` of the folder `folder` shown: a click on
    /// its row, then Enter.
    fn pick_row(&mut self, folder: &str, name: &str) -> Result<(), String> {
        let start = format!("folder {folder}: ");
        let g = self
            .program_view("picker", &format!("{folder} listed"), |g| g.other(&start).is_some())?;
        let names = g.names(&start).unwrap_or_default();
        let row = names
            .iter()
            .position(|n| n == name)
            .ok_or(format!("no {name} in {folder}: {names:?}"))?;
        self.click_row("picker", "File picker", "files", row)?;
        self.program_view("picker", &format!("{name} picked"), |g| {
            g.widget("files").is_some_and(|w| w.value.starts_with(&format!("{name}|")))
        })?;
        self.chord(&["ret"])
    }

    /// The file picker (note 0025, section 5), from the gallery: Save as (with pictures of the
    /// picker), saving again over the same name (it asks first), opening what was saved, and
    /// opening a file on 1.3's disk (an administrator has every volume among the places).
    fn picker(&mut self, first: &ScreenLine) -> Result<(), String> {
        self.save_as("saved", false, true, first)?;
        self.save_as("saved", true, false, first)?;
        // Open: Documents is where it saved last, and the file is there.
        let read = self.count("gallery: read ");
        self.ask_picker(&["ctrl", "o"], "opening")?;
        self.pick_row("/Home/Documents", "saved.txt")?;
        self.lines("gallery: read ", read + 1)?;
        self.line("gallery: read 5 bytes from /Home/Documents/saved.txt")?;
        // 1.3's disk: the place named after its volume, Data.
        let (g, _) = self.ask_picker(&["ctrl", "o"], "opening again")?;
        let places = g.names("places ").unwrap_or_default();
        let row =
            places.iter().position(|p| p == "Data").ok_or(format!("no Data among {places:?}"))?;
        self.click_row("picker", "File picker", "places", row)?;
        self.pick_row("/Volumes/Data", "Documents")?;
        self.pick_row("/Volumes/Data/Documents", "letter.txt")?;
        self.line("gallery: read 76 bytes from /Volumes/Data/Documents/letter.txt")?;
        Ok(())
    }

    /// The permissions viewer (note 0025, sections 4 and 5), from the gallery's window menu:
    /// what it shows the gallery holds is what the sandbox manager says; Take back takes the
    /// file the gallery saved; Stop stops the gallery.
    fn permissions(&mut self, first: &ScreenLine) -> Result<(), String> {
        let v = self.view("the gallery", |v| v.window("Gallery").is_some())?;
        let g = v.window("Gallery").cloned().ok_or("no Gallery")?;
        let app = g.app;
        self.move_to(g.x + 40, g.y - 13)?;
        let right =
            |down: bool| format!(r#"{{"type":"btn","data":{{"button":"right","down":{down}}}}}"#);
        self.qmp.send(true, &[right(true)])?;
        std::thread::sleep(Duration::from_millis(150));
        self.qmp.send(true, &[right(false)])?;
        let reach = "What can this app reach?";
        let v = self.view("the gallery's window menu", |v| {
            v.menu.as_ref().is_some_and(|m| m.rows.iter().any(|r| r == reach))
        })?;
        let (mx, my) = v.menu.as_ref().and_then(|m| m.row(reach)).ok_or("no row for the viewer")?;
        self.click_at(mx, my)?;
        self.line(&format!("permissions: open on app {app}"))?;
        // What it shows is what the sandbox manager says (the root program logs each change).
        let holds = format!("app {app} holds ");
        let text = self.line(&format!("Desktop test: app {app} grants: "))?;
        let grants_line = text
            .lines()
            .rev()
            .find_map(|l| {
                l.split_once(&format!("Desktop test: app {app} grants: "))
                    .map(|(_, r)| r.to_string())
            })
            .unwrap_or_default();
        let wanted: Vec<(String, bool)> = grants_line
            .split("; ")
            .filter(|g| !g.is_empty())
            .filter_map(|g| {
                let mut w = g.rsplitn(3, ' ');
                let (_, rights, path) = (w.next()?, w.next()?, w.next()?);
                Some((path.to_string(), rights == "rw"))
            })
            .collect();
        let pv = self.program_view("permissions", "what the gallery holds", |p| {
            p.other.iter().filter(|l| l.starts_with(&holds)).count() == wanted.len() + 2
        })?;
        let shown: Vec<&str> =
            pv.other.iter().filter_map(|l| l.strip_prefix(holds.as_str())).collect();
        for (path, rw) in &wanted {
            let words = if *rw { "read and change" } else { "read" };
            let line = format!("{path}: {words}, you chose it");
            if !shown.contains(&line.as_str()) {
                return Err(format!("the viewer doesn't show \"{line}\": it shows {shown:?}"));
            }
        }
        for service in ["Windows", "The file picker"] {
            if !shown.contains(&service) {
                return Err(format!("the viewer doesn't show {service}: it shows {shown:?}"));
            }
        }
        // For people: the viewer at 1x and, dragged there, at 2x.
        let v =
            self.view("the viewer", |v| v.window("Permissions").is_some_and(|w| w.is("system")))?;
        let w = v.window("Permissions").cloned().ok_or("no viewer")?;
        self.move_to(first.x + first.width - 8, first.y + 8)?;
        self.keep_window(&v, &w, "permissions-1x")?;
        if self.cfg.screens == Screens::TwoVirtioGpu {
            let second = v.screens.iter().find(|s| s.number != 1).cloned().ok_or("no screen 2")?;
            self.drag_window(&w, second.x + 20, second.y + 40)?;
            self.move_to(first.x + first.width - 8, first.y + 8)?;
            let v = self.view("the viewer at 2x", |v| {
                v.window("Permissions").is_some_and(|p| p.scale == 200 && p.screen == second.number)
            })?;
            let w2 = v.window("Permissions").cloned().ok_or("no viewer")?;
            self.keep_window(&v, &w2, "permissions-2x")?;
            self.drag_window(&w2, w.x, w.y)?;
            self.view("the viewer back", |v| {
                v.window("Permissions").is_some_and(|p| p.scale == 100)
            })?;
        }
        // Take back the file it saved: it goes from the sandbox manager's record.
        let saved = "/Home/Documents/saved.txt: read and change, you chose it";
        let row = shown.iter().position(|l| *l == saved).ok_or("no row for the saved file")?;
        self.click_row("permissions", "Permissions", "held", row)?;
        self.click_program_widget("permissions", "Permissions", "takeback")?;
        self.line("permissions: took /Home/Documents/saved.txt back from gallery")?;
        self.line("took /Home/Documents/saved.txt back from gallery.")?;
        self.wait("the grant gone from the sandbox manager's record", |t| {
            t.lines()
                .rev()
                .find_map(|l| {
                    l.split_once(&format!("Desktop test: app {app} grants: "))
                        .map(|(_, r)| r.to_string())
                })
                .is_some_and(|r| !r.contains("/Home/Documents/saved.txt"))
        })?;
        // Stop the gallery.
        self.click_program_widget("permissions", "Permissions", "stop")?;
        self.line("permissions: stopped gallery")?;
        self.line("sandbox (chris): stopped gallery.")?;
        self.click_program_widget("permissions", "Permissions", "close")?;
        self.line("permissions: closing")?;
        Ok(())
    }

    /// The gallery (note 0023, section 6; `tests/programs/gallery`): every sandstone widget in
    /// one window, beside a test window `Other` for the clipboard. Its look is compared with the
    /// pictures its host tests drew, at 1x and (on two screens) at 2x; then it is driven: a
    /// click, typing, Tab, a menu, a dialog, the clipboard both ways, the wheel; then both
    /// windows are closed.
    fn gallery(&mut self, first: ScreenLine) -> Result<(), String> {
        let two = self.cfg.screens == Screens::TwoVirtioGpu;
        self.line("Desktop test: the gallery")?;
        let v = self.view("the gallery and Other drawn", |v| {
            v.window("Gallery").is_some_and(|w| w.is("drawn"))
                && v.window("Other").is_some_and(|w| w.is("drawn"))
        })?;
        self.gallery_view("its widgets", |g| g.widget("field").is_some())?;
        // Other opened over the gallery's corner: it is dragged by its title bar to the bottom
        // right of the first screen, clear of the gallery, for the clipboard later.
        let o = v.window("Other").cloned().ok_or("no Other")?;
        let (tx, ty) =
            (first.x + first.width - o.width - 20, first.y + first.height - o.height - 70);
        self.move_to(o.x + 60, o.y - 13)?;
        self.button(true)?;
        self.move_to(tx + 60, ty - 13)?;
        self.button(false)?;
        let v = self
            .view("Other moved", |v| v.window("Other").is_some_and(|w| (w.x, w.y) == (tx, ty)))?;
        let g = v.window("Gallery").cloned().ok_or("no Gallery")?;

        // As it opened, on top and with the keyboard (Other, launched after it, opened over its
        // corner and took the keyboard: a click on its status bar, which Other never covers,
        // gives both back without focusing a widget), and the pointer out of the way (it is
        // drawn on the screen, and would hover a widget).
        self.click_widget("status bar")?;
        self.view("the gallery focused", |v| v.window("Gallery").is_some_and(|w| w.is("focused")))?;
        self.move_to(first.x + first.width - 8, first.y + 8)?;
        let v = self.view("the gallery", |v| v.window("Gallery").is_some())?;
        self.compare_window(&v, &g, "gallery-1x")?;

        // On the 2x screen: dragged by its title bar, it is told the new scale and draws itself
        // twice as fine.
        if two {
            let second =
                v.screens.iter().find(|s| s.number != 1).cloned().ok_or("no second screen")?;
            self.move_to(g.x + 60, g.y - 13)?;
            self.button(true)?;
            self.move_to(second.x + 60, second.y + 14)?;
            self.button(false)?;
            self.line("gallery: configure 560x380 scale 200")?;
            self.move_to(first.x + first.width - 8, first.y + 8)?;
            let v = self.view("the gallery on the 2x screen", |v| {
                v.window("Gallery").is_some_and(|w| w.scale == 200 && w.screen == second.number)
            })?;
            let g2 = v.window("Gallery").cloned().ok_or("no Gallery")?;
            self.compare_window(&v, &g2, "gallery-2x")?;
            // And back where it was.
            self.move_to(g2.x + 60, g2.y - 13)?;
            self.button(true)?;
            self.move_to(g.x + 60, g.y - 13)?;
            self.button(false)?;
            self.lines("gallery: configure 560x380 scale 100", 2)?;
        }

        // A click.
        self.click_widget("click")?;
        self.line("gallery: message Clicked")?;
        // Typing into the text field.
        self.click_widget("field")?;
        for k in ["h", "e", "l", "l", "o"] {
            self.chord(&[k])?;
        }
        self.gallery_view("hello typed", |g| {
            g.widget("field").is_some_and(|w| w.value == "hello" && w.focused)
        })?;
        // Tab and Shift+Tab move the focus in order; Space ticks the check box.
        self.chord(&["tab"])?;
        self.gallery_view("the check box focused", |g| {
            g.focused().is_some_and(|w| w.key.as_deref() == Some("wrap"))
        })?;
        self.chord(&["tab"])?;
        self.gallery_view("the toggle focused", |g| {
            g.focused().is_some_and(|w| w.key.as_deref() == Some("pen"))
        })?;
        self.chord(&["shift", "tab"])?;
        self.gallery_view("the check box focused again", |g| {
            g.focused().is_some_and(|w| w.key.as_deref() == Some("wrap"))
        })?;
        self.chord(&["spc"])?;
        self.line("gallery: message Wrap(true)")?;

        // A menu: F10 opens File, Right moves to Edit, and Copy (clicked) copies the field.
        self.chord(&["f10"])?;
        self.gallery_view("the File menu", |g| g.widget("New").is_some_and(|w| w.window != 0))?;
        self.chord(&["right"])?;
        self.gallery_view("the Edit menu", |g| g.widget("Copy").is_some_and(|w| w.window != 0))?;
        self.click_widget("Copy")?;
        self.line("gallery: message Copy")?;
        self.view("the clipboard holding text", |v| {
            v.windows.iter().all(|w| !w.words.iter().any(|x| x == "popup"))
        })?;

        // The clipboard, to the other window and back.
        let v = self.view("Other", |v| v.window("Other").is_some())?;
        let o = v.window("Other").cloned().ok_or("no Other")?;
        self.click_at(o.x + 10, o.y + o.height - 10)?;
        self.view("Other focused", |v| v.window("Other").is_some_and(|w| w.is("focused")))?;
        self.chord(&["ctrl", "v"])?;
        self.line("wintest Other: pasted hello")?;
        self.chord(&["ctrl", "c"])?;
        self.line("wintest Other: copied")?;
        self.click_widget("field")?;
        self.chord(&["ctrl", "a"])?;
        self.chord(&["ctrl", "v"])?;
        self.gallery_view("Other pasted", |g| {
            g.widget("field").is_some_and(|w| w.value == "Other")
        })?;

        // A dialog: a question, answered with a click.
        self.click_widget("ask")?;
        self.gallery_view("the question", |g| g.widget("Yes").is_some_and(|w| w.window != 0))?;
        self.click_widget("Yes")?;
        self.line("gallery: message Answer(true)")?;

        // The wheel over the scrolled column.
        let gv = self.gallery_view("the scroll area", |g| g.widget("scroll").is_some())?;
        let sc = gv.widget("scroll").cloned().ok_or("no scroll area")?;
        let v = self.view("the gallery", |v| v.window("Gallery").is_some())?;
        let g = v.window("Gallery").cloned().ok_or("no Gallery")?;
        self.move_to(g.x + sc.x + sc.width / 2, g.y + sc.y + sc.height / 2)?;
        self.wheel(3)?;
        self.gallery_view("scrolled", |g| g.widget("scroll").is_some_and(|w| w.value != "0,0"))?;

        // Keep a picture of it as the user left it.
        self.keep_screens("gallery")?;

        // ---- The file picker and the permissions viewer (note 0025). ----
        self.picker(&first)?;
        self.permissions(&first)?;

        // The gallery was stopped from the permissions viewer; Other is closed with Ctrl+W.
        let v = self.view("Other", |v| v.window("Other").is_some())?;
        let o = v.window("Other").cloned().ok_or("no Other")?;
        self.click_at(o.x + 10, o.y + o.height - 10)?;
        self.view("Other focused", |v| v.window("Other").is_some_and(|w| w.is("focused")))?;
        self.chord(&["ctrl", "w"])?;
        self.line("wintest Other: closing")?;
        Ok(())
    }

    /// The first boot: the desktop, two windows the runner drives, a hung and a hostile app,
    /// the display server's crash, the start menu, and falling back to the shell.
    fn first(&mut self) -> Result<(), String> {
        let two = self.cfg.screens == Screens::TwoVirtioGpu;
        let tablet = self.cfg.screens != Screens::Firmware;

        // ---- Step 1: two windows, which the runner drives. ----
        self.desktop_ready(1)?;
        self.keep_screens("desktop")?;
        self.line("Desktop test: drive the windows")?;
        // (Beta, opened last, is on top. Whether it also has the focus depends on how soon
        // after its launch it opened, which a slow emulated machine can stretch past the two
        // seconds the rule allows; the host tests check that rule.)
        let v = self.view("Alpha and Beta drawn", |v| {
            let drawn = |t: &str| v.window(t).is_some_and(|w| w.is("drawn"));
            drawn("Alpha") && drawn("Beta") && (!two || v.screens.iter().any(|s| s.scale == 200))
        })?;
        let alpha = v.window("Alpha").cloned().ok_or("no Alpha")?;
        let beta = v.window("Beta").cloned().ok_or("no Beta")?;
        // Windows open where they should: cascading on the main screen, Beta after Alpha.
        if (beta.x - alpha.x, beta.y - alpha.y) != (24, 24) || alpha.screen != 1 {
            return Err(format!("Alpha and Beta opened at {alpha:?} and {beta:?}"));
        }
        // And are drawn: Alpha's corner (which Beta doesn't cover) and Beta's middle.
        self.colour_at(&v, alpha.x + 5, alpha.y + 5, 0xc04040)?;
        self.colour_at(&v, beta.centre().0, beta.centre().1, 0x4060c0)?;

        // Clicking Alpha gives it the focus; keys then go to it alone.
        if tablet {
            let first = v.screen(1).cloned().ok_or("no screen 1")?;
            self.tablet_click(&first, alpha.x + 5, alpha.y + 5)?;
        } else {
            self.click_at(alpha.x + 5, alpha.y + 5)?;
        }
        self.line("wintest Alpha: button 1 pressed")?;
        self.view("Alpha focused", |v| v.window("Alpha").is_some_and(|w| w.is("focused")))?;
        self.chord(&["h"])?;
        self.chord(&["i"])?;
        self.line("wintest Alpha: key 9 'i'")?;
        let text = self.line("wintest Alpha: key 8 'h'")?;
        if text.contains("wintest Beta: key") {
            return Err("Beta got keys typed into Alpha".into());
        }

        // The clipboard: Ctrl+C in Alpha, Ctrl+V in Beta.
        self.chord(&["ctrl", "c"])?;
        self.line("wintest Alpha: copied")?;
        // Alpha is on top now, over Beta's middle: Beta's bottom right corner is uncovered.
        self.click_at(beta.x + beta.width - 10, beta.y + beta.height - 10)?;
        self.view("Beta focused", |v| v.window("Beta").is_some_and(|w| w.is("focused")))?;
        self.chord(&["ctrl", "v"])?;
        self.line("wintest Beta: pasted Alpha")?;

        // The pointer goes to the window under it: out of Beta onto the desktop.
        let first = self
            .view("a screen", |v| v.screen(1).is_some())?
            .screen(1)
            .cloned()
            .ok_or("no screen 1")?;
        self.move_to(first.x + first.width - 8, first.y + first.height - 8)?;
        self.line("wintest Beta: pointer out")?;

        // Beta, dragged by its title bar onto the 2x screen, is told the new scale and
        // redraws sharply; with the screen unplugged it comes back to the first screen.
        if two {
            let v = self.view("two screens", |v| v.screens.len() == 2)?;
            let second = v.screens.iter().find(|s| s.number != 1).cloned().ok_or("no screen 2")?;
            self.move_to(beta.x + 60, beta.y - 13)?;
            self.button(true)?;
            self.move_to(second.x + 150, second.y + 120)?;
            self.button(false)?;
            self.line("wintest Beta: configure 300x200 scale 200")?;
            let v = self.view("Beta on the 2x screen", |v| {
                v.window("Beta").is_some_and(|w| w.screen == second.number && w.scale == 200)
            })?;
            let b = v.window("Beta").cloned().ok_or("no Beta")?;
            self.colour_at(&v, b.centre().0, b.centre().1, 0x4060c0)?;
            self.plug(false)?;
            let v = self.view("Beta back on screen 1", |v| {
                v.screens.len() == 1
                    && v.window("Beta").is_some_and(|w| w.screen == 1 && w.scale == 100)
            })?;
            let b = v.window("Beta").cloned().ok_or("no Beta")?;
            self.lines("wintest Beta: configure 300x200 scale 100", 1)?;
            self.colour_at(&v, b.centre().0, b.centre().1, 0x4060c0)?;
            self.plug(true)?;
            // It comes back where it was, 2x, right of the first.
            self.view("the second screen back", |v| {
                v.screens.len() == 2
                    && v.screens
                        .iter()
                        .any(|s| s.number != 1 && s.scale == 200 && s.x == first.width)
            })?;
        }

        // The start menu key goes to the desktop, not to the app with the focus: it opens the
        // start menu, and closes it again.
        self.chord(&["meta_l"])?;
        self.line("desktop: start menu open")?;
        self.chord(&["meta_l"])?;
        self.line("desktop: start menu closed")?;
        if self.text().contains("wintest Beta: key 82")
            || self.text().contains("wintest Beta: key 83")
        {
            return Err("the start menu key reached Beta".into());
        }

        // Beta out of Alpha's way, then Alpha closed with its title bar's close button.
        let v = self.view("Alpha and Beta", |v| {
            v.window("Alpha").is_some() && v.window("Beta").is_some()
        })?;
        let (a, b) = (
            v.window("Alpha").cloned().ok_or("no Alpha")?,
            v.window("Beta").cloned().ok_or("no Beta")?,
        );
        self.move_to(b.x + 60, b.y - 13)?;
        self.button(true)?;
        // (Above the taskbar.)
        let (tx, ty) = (first.x + first.width - 300 - 20, first.y + first.height - 200 - 70);
        self.move_to(tx + 60, ty)?;
        self.button(false)?;
        self.view("Beta moved", |v| v.window("Beta").is_some_and(|w| w.x == tx && w.y == ty + 13))?;
        // The close button: 22 pixels in from the frame's right edge, 5 down from its top
        // (`lib/compositor/src/decor.rs`), 16 across.
        self.click_at(a.x + a.width + 1 - 22 + 8, a.y - 27 + 5 + 8)?;
        self.line("wintest Alpha: closing")?;

        // ---- Step 2: a hung app, and a hostile one. ----
        self.line("Desktop test: the hung window")?;
        self.view("Hung not responding", |v| {
            v.window("Hung").is_some_and(|w| w.is("not-responding"))
        })?;
        // The hostile app tries the file picker (note 0025): asks without a press, then, once
        // clicked, a second while the first is open; the first is cancelled with Escape.
        self.line("windowprobe: click me")?;
        let v = self.view("the probe's window", |v| v.window("probe").is_some())?;
        let (px, py) = visible_point(&v, "probe").ok_or("the probe's window is covered")?;
        let opened = self.count("picker: open for app");
        self.click_at(px, py)?;
        self.lines("picker: open for app", opened + 1)?;
        self.line("windowprobe: refused: a second picker while one is open (Busy)")?;
        self.view("the picker over the probe", |v| {
            v.window("File picker").is_some_and(|w| w.is("drawn") && w.is("focused"))
        })?;
        self.chord(&["esc"])?;
        self.line("windowprobe: the first picker was cancelled")?;

        // The rest carry on: Beta takes a click and keys.
        let v = self.view("Beta", |v| v.window("Beta").is_some())?;
        let b = v.window("Beta").cloned().ok_or("no Beta")?;
        self.click_at(b.centre().0, b.centre().1)?;
        self.view("Beta focused", |v| v.window("Beta").is_some_and(|w| w.is("focused")))?;
        self.chord(&["o"])?;
        self.chord(&["k"])?;
        self.line("wintest Beta: key 11 'k'")?;
        self.line("windowprobe: all")?;
        // Ctrl+W closes the focused window.
        self.chord(&["ctrl", "w"])?;
        self.line("wintest Beta: closing")?;
        // The hung app is stopped from its window menu (a right click on its title bar): the
        // display server leaves Stop to the desktop, which asks the sandbox manager.
        let v = self.view("Hung", |v| v.window("Hung").is_some())?;
        let h = v.window("Hung").cloned().ok_or("no Hung")?;
        self.move_to(h.x + 40, h.y - 13)?;
        let right =
            |down: bool| format!(r#"{{"type":"btn","data":{{"button":"right","down":{down}}}}}"#);
        self.qmp.send(true, &[right(true)])?;
        std::thread::sleep(Duration::from_millis(150));
        self.qmp.send(true, &[right(false)])?;
        let v = self.view("Hung's window menu with Stop", |v| {
            v.menu.as_ref().is_some_and(|m| m.rows.iter().any(|r| r == "Stop"))
        })?;
        let (sx, sy) = v.menu.as_ref().and_then(|m| m.row("Stop")).ok_or("no Stop")?;
        self.click_at(sx, sy)?;
        self.line("desktop: stopping app")?;

        // ---- Step 3: the display server crashed and started again, and the desktop. ----
        self.line("Desktop test: after the restart")?;
        self.desktop_ready(2)?;
        self.view("Again drawn and focused", |v| {
            v.window("Again").is_some_and(|w| w.is("drawn") && w.is("focused"))
        })?;
        // The guest may finish the test and restart the machine before the keys are let go,
        // and QEMU then closes its control socket: the next lines are what count.
        self.chord(&["ctrl", "w"])?;
        self.line("wintest Again: closing")?;

        // ---- Step 4: the start menu launches the test window, focused. ----
        self.line("Desktop test: the start menu")?;
        self.choose_from_start_menu("wintest", 2)?;
        self.view("the window from the start menu, focused", |v| {
            v.window("test").is_some_and(|w| w.is("drawn") && w.is("focused"))
        })?;
        self.chord(&["ctrl", "w"])?;
        self.line("wintest test: closing")?;

        // ---- Step 4b: the widget gallery (note 0023, section 6). ----
        self.gallery(first)?;

        // ---- Step 5: falling back to the shell, which restarts the machine. ----
        self.line("Desktop test: falling back")?;
        self.line("Desktop test passed")?;
        self.shell("reboot")?;
        self.line("users: chris asked to restart")?;
        self.stopped("guest-reset")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_the_gallerys_description() {
        let text = "[gallery] gallery: view 1\n[gallery] gallery: window 0 is 7\n\
            [gallery] gallery: widget 0 text field \"Type \\\"here\\\"\" at 8,40 size 200x24 id field focused value \"a \\\"b\\\"\"\n\
            [gallery] gallery: widget 0 check box \"Wrap\" at 8,70 size 60x24 id wrap unchecked\n\
            [gallery] gallery: view 1 done\n[gallery] gallery: view 2\n";
        let g = last_gallery_view(text).expect("a view");
        assert_eq!(g.windows, vec![(0, 7)]);
        let f = g.widget("field").unwrap();
        assert_eq!(
            (f.role.as_str(), f.label.as_str(), f.value.as_str()),
            ("text field", "Type \"here\"", "a \"b\"")
        );
        assert_eq!((f.x, f.y, f.width, f.height, f.focused), (8, 40, 200, 24, true));
        assert_eq!(g.widget("Wrap").unwrap().checked, Some(false));
        assert_eq!(g.focused().unwrap().key.as_deref(), Some("field"));
    }

    #[test]
    fn reading_the_last_whole_description() {
        let text = "\
[display] view 1
[display] screen 1 \"Virtual-1\" 1280x800 at 0,0 scale 100 main
[display] view 1 done
[wintest] wintest Alpha: window 1
[display] view 2
[display] screen 1 \"Virtual-1\" 1280x800 at 0,0 scale 100 main
[display] screen 3 \"Virtual-2\" 512x384 at 1280,0 scale 200 mirrors 1
[display] screen 1 window 1 \"Alpha \u{2014} a \"b\" c\" app 2 at 33,51 size 300x200 scale 100 drawn focused
[display] clipboard empty
[display] pointer at 5,6 screen 1
[display] view 2 done
[display] view 3
[display] screen 1 \"Virtual-1\" 1280x800 at 0,0 scale 100 main
";
        let v = last_view(text).unwrap();
        assert_eq!(v.screens.len(), 2);
        assert_eq!(
            v.screens[1],
            ScreenLine {
                number: 3,
                x: 1280,
                y: 0,
                width: 512,
                height: 384,
                scale: 200,
                mirrors: 1
            }
        );
        let w = &v.windows[0];
        assert_eq!(w.title, "Alpha \u{2014} a \"b\" c");
        assert_eq!(
            (w.screen, w.number, w.x, w.y, w.width, w.height, w.scale),
            (1, 1, 33, 51, 300, 200, 100)
        );
        assert!(w.is("drawn") && w.is("focused") && !w.is("minimised"));
        assert_eq!(v.pointer, Some((5, 6)));
    }
}
