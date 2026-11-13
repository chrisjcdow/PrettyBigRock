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
use std::path::Path;
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
    pub number: u32,
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
        title: title.to_string(),
        x,
        y,
        width,
        height,
        scale: num(t.get(6)?)?,
        words: t[7..].iter().map(|w| w.to_string()).collect(),
    })
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
        self.qmp.connect()?;
        self.line("input: using QEMU Virtio Keyboard")?;
        self.line("input: using QEMU Virtio Mouse")?;
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
    fn from_start_menu(&mut self, item: &str, times: usize) -> Result<(), String> {
        self.chord(&["meta_l"])?;
        self.lines("desktop: start menu open", times)?;
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
        self.from_start_menu("Shut down", 1)?;
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
        self.from_start_menu("wintest", 2)?;
        self.view("the window from the start menu, focused", |v| {
            v.window("test").is_some_and(|w| w.is("drawn") && w.is("focused"))
        })?;
        self.chord(&["ctrl", "w"])?;
        self.line("wintest test: closing")?;

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
