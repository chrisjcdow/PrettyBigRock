// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! `cargo xtask classic-disk`: makes a RockFS data disk *with PrettyBigRock 1.3 itself*
//! (docs/design/0016-filesystems.md, section 4, layer 4), so PrettyBigRock 2 can be tested on a
//! disk that the classic system really wrote, not one a tool made in its image.
//!
//! It builds the classic system, boots it in QEMU with a blank 16 MiB second disk, accepts its
//! offer to set the disk up, and works the machine the way a person would: it types Terminal
//! commands (nested folders, a long name, `write`, `append`, `cp`, `mv`, `rm`), saves a drawing
//! from Paint (a file of a few hundred kilobytes, written into a fragmented volume so it needs
//! many extents, and so an extent tree), restarting on the way, and shuts down cleanly from
//! the Terminal.
//!
//! The classic system has no serial console, so this can't watch for prompts. It sends keys
//! and mouse movements through QEMU's *monitor* (QEMU's own control console, reached here over
//! a local TCP connection) and waits generously between steps. Instead of trusting the timing,
//! it checks the result: afterwards `tools/rockfs.py` must find the volume shut down cleanly
//! and undamaged, holding exactly the files this script made, with the contents it typed.
//! Screenshots of each step are kept in `build/classic-disk/` for when it goes wrong.
//!
//! The result is `tests/data/classic-1.3.img.gz` and a manifest of every folder and file
//! (`tests/data/classic-1.3.manifest`: `D 0 00000000 /path` or `F size crc32 /path`, the format
//! `rfs_host stress` writes). It's made once and kept in the repository; running this again
//! remakes it (the dates on the disk will differ).

use crate::{Result, status};
use std::fs;
use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// The data disk's size: 16 MiB.
const DATA_BYTES: u64 = 16 << 20;

/// A long file name: 1.3's Terminal takes command lines of under 90 characters, which leaves
/// room for a name of 70.
const LONG_NAME: &str = "A-long-file-name-made-by-PrettyBigRock-1.3-to-see-that-it-survives.txt";

/// The Terminal commands, in order (the rest are made in [`drive`]).
const COMMANDS: &[&str] = &[
    "mkdir Projects",
    "mkdir Projects/Rock",
    "mkdir Projects/Rock/Deep",
    "write Projects/Rock/Deep/notes.txt Written by 1.3 in a nested folder.",
    "write Documents/letter.txt Dear Chris, PrettyBigRock 1.3 wrote this letter.",
    "append Documents/letter.txt Yours, the classic system.",
    "cp Documents/letter.txt Projects/copy.txt",
    "mv Projects/copy.txt Projects/Rock/moved.txt",
    "mkdir Fill",
];

/// What the disk must hold afterwards, besides the drawing and the fill files: (path, contents),
/// with `None` for a folder. `readme.txt`, `shortcuts.txt` and `Documents/todo.txt` are the
/// files 1.3 puts on every new data disk; their contents are 1.3's, so only their presence is
/// checked (`Some("")` below means "a file, any contents").
const EXPECTED: &[(&str, Option<&str>)] = &[
    ("/Documents", None),
    (
        "/Documents/letter.txt",
        Some("Dear Chris, PrettyBigRock 1.3 wrote this letter.\nYours, the classic system.\n"),
    ),
    ("/Documents/todo.txt", Some("")),
    ("/Fill", None),
    ("/Pictures", None),
    ("/Projects", None),
    ("/Projects/Rock", None),
    ("/Projects/Rock/Deep", None),
    ("/Projects/Rock/Deep/notes.txt", Some("Written by 1.3 in a nested folder.\n")),
    (
        "/Projects/Rock/moved.txt",
        Some("Dear Chris, PrettyBigRock 1.3 wrote this letter.\nYours, the classic system.\n"),
    ),
    ("/readme.txt", Some("")),
    ("/shortcuts.txt", Some("")),
];

/// How many small files fill the volume before the drawing is saved; every other one is then
/// removed, so the free space is in one-block holes and the drawing is split across them.
const FILL: usize = 40;

/// QEMU's monitor, over TCP: one command at a time, each answered by the `(qemu)` prompt.
struct Monitor {
    out: TcpStream,
    input: BufReader<TcpStream>,
}

impl Monitor {
    fn connect(port: u16, qemu: &mut Child) -> Result<Monitor> {
        let end = Instant::now() + Duration::from_secs(20);
        let stream = loop {
            match TcpStream::connect(("127.0.0.1", port)) {
                Ok(s) => break s,
                Err(e) => {
                    if Instant::now() > end || qemu.try_wait().ok().flatten().is_some() {
                        return Err(format!("couldn't reach QEMU's monitor: {e}"));
                    }
                    sleep(Duration::from_millis(200));
                }
            }
        };
        stream.set_read_timeout(Some(Duration::from_secs(30))).map_err(|e| e.to_string())?;
        let input = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
        let mut m = Monitor { out: stream, input };
        m.prompt()?;
        Ok(m)
    }

    /// Reads up to and including the next `(qemu) ` prompt.
    fn prompt(&mut self) -> Result {
        let mut seen = Vec::new();
        let mut byte = [0u8; 1];
        while !seen.ends_with(b"(qemu) ") {
            self.input.read_exact(&mut byte).map_err(|e| format!("QEMU's monitor: {e}"))?;
            seen.push(byte[0]);
        }
        Ok(())
    }

    fn send(&mut self, command: &str) -> Result {
        writeln!(self.out, "{command}").map_err(|e| format!("QEMU's monitor: {e}"))?;
        self.prompt()
    }

    /// Presses one key (a QEMU key name such as `a`, `shift-a`, `ret` or `ctrl-esc`).
    fn key(&mut self, name: &str) -> Result {
        self.send(&format!("sendkey {name}"))?;
        // 1.3 reads the keyboard in its main loop; give it time to keep up.
        sleep(Duration::from_millis(40));
        Ok(())
    }

    /// Types `text` (the characters on a US keyboard that the commands above use).
    fn type_text(&mut self, text: &str) -> Result {
        for c in text.chars() {
            let name = match c {
                'a'..='z' | '0'..='9' => c.to_string(),
                'A'..='Z' => format!("shift-{}", c.to_ascii_lowercase()),
                ' ' => "spc".into(),
                '.' => "dot".into(),
                ',' => "comma".into(),
                '/' => "slash".into(),
                '-' => "minus".into(),
                '_' => "shift-minus".into(),
                _ => return Err(format!("classic-disk can't type {c:?}")),
            };
            self.key(&name)?;
        }
        Ok(())
    }

    /// Types a command and Enter, and waits for it to finish.
    fn command(&mut self, line: &str) -> Result {
        // 1.3's input line holds 90 bytes, but in practice the last few of a line that long
        // are lost; staying well inside it is safe.
        if line.len() > 80 {
            return Err(format!("too long for 1.3's Terminal: {line}"));
        }
        self.type_text(line)?;
        self.key("ret")?;
        sleep(Duration::from_millis(1500));
        Ok(())
    }

    /// Moves the mouse by (dx, dy), in steps 1.3's PS/2 driver takes without accelerating.
    fn mouse(&mut self, dx: i32, dy: i32) -> Result {
        let steps = (dx.abs().max(dy.abs()) / 5).max(1);
        for i in 0..steps {
            let x = dx * (i + 1) / steps - dx * i / steps;
            let y = dy * (i + 1) / steps - dy * i / steps;
            self.send(&format!("mouse_move {x} {y}"))?;
            sleep(Duration::from_millis(15));
        }
        Ok(())
    }

    /// Saves a screenshot, for looking at when something went wrong.
    fn screenshot(&mut self, dir: &Path, name: &str) -> Result {
        self.send(&format!("screendump {}", dir.join(format!("{name}.ppm")).display()))
    }
}

/// A free TCP port on this machine for the monitor.
fn free_port() -> Result<u16> {
    let l = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    Ok(l.local_addr().map_err(|e| e.to_string())?.port())
}

fn wait(seconds: u64) {
    sleep(Duration::from_secs(seconds));
}

/// Works the classic system: everything a person would do to make the disk.
fn drive(m: &mut Monitor, shots: &Path) -> Result {
    // The desktop comes up with the offer to set up the blank disk; Enter is "Set it up", and
    // Enter again closes the message that says it's ready.
    wait(25);
    m.screenshot(shots, "01-offer")?;
    m.key("ret")?;
    wait(8);
    m.screenshot(shots, "02-ready")?;
    m.key("ret")?;
    wait(2);

    // The start menu (Ctrl+Esc), its first item (Terminal).
    m.key("ctrl-esc")?;
    wait(1);
    m.key("down")?;
    m.key("ret")?;
    wait(3);
    for c in COMMANDS {
        m.command(c)?;
    }
    let long = format!("write {LONG_NAME} ok");
    m.command(&long)?;
    for i in 0..FILL {
        m.command(&format!("write Fill/f{i:02} {i:02} {}", "fill ".repeat(10)))?;
    }
    for i in (0..FILL).step_by(2) {
        m.command(&format!("rm Fill/f{i:02}"))?;
    }
    m.screenshot(shots, "03-terminal")?;

    // 1.3 allocates "next fit", carrying on from the last block it allocated, so the holes
    // just made would only be reached once the volume wrapped round. It starts again from the
    // beginning after a restart: restarting (which shuts the disk down cleanly first) puts the
    // drawing into the holes.
    m.command("reboot")?;
    wait(30);
    m.screenshot(shots, "04-restarted")?;

    // Paint is the start menu's fifth item. Draw a few strokes around the middle of the screen
    // (the canvas), so the picture isn't one colour and a misplaced block would show in its
    // checksum, then save it as Pictures/rock.bmp (Ctrl+S; clear the suggested name; type).
    m.key("ctrl-esc")?;
    wait(1);
    for _ in 0..5 {
        m.key("down")?;
    }
    m.key("ret")?;
    wait(3);
    for (dx, dy) in [(-120, -60), (90, 40), (-30, 70), (140, -100)] {
        m.mouse(dx / 2, dy / 2)?;
        m.send("mouse_button 1")?;
        m.mouse(dx, dy)?;
        m.send("mouse_button 0")?;
    }
    m.screenshot(shots, "05-paint")?;
    m.key("ctrl-s")?;
    wait(2);
    for _ in 0..20 {
        m.key("backspace")?;
    }
    m.type_text("rock.bmp")?;
    m.key("ret")?;
    wait(6);
    m.screenshot(shots, "06-saved")?;
    m.key("ctrl-w")?;
    wait(2);

    // A Terminal again (the restart closed the first): copy the drawing (a second large file, made by `cp`), then shut
    // down, which marks the volume as shut down cleanly and powers QEMU off.
    m.key("ctrl-esc")?;
    wait(1);
    m.key("down")?;
    m.key("ret")?;
    wait(3);
    m.command("cp Pictures/rock.bmp Projects/rock-copy.bmp")?;
    wait(4);
    m.screenshot(shots, "07-copied")?;
    m.type_text("shutdown")?;
    m.key("ret")
}

/// Checks the disk with `tools/rockfs.py` and writes the manifest: the volume must be clean
/// and undamaged, and hold exactly what [`drive`] made.
fn verify(root: &Path, image: &Path, manifest: &Path) -> Result {
    let mut expected = String::new();
    for (path, contents) in EXPECTED {
        match contents {
            None => expected.push_str(&format!("D {path}\n")),
            Some("") => expected.push_str(&format!("F {path}\n")),
            Some(text) => expected.push_str(&format!("T {path}\t{}\n", text.escape_default())),
        }
    }
    expected.push_str(&format!("T /{LONG_NAME}\tok\\n\n"));
    for i in (1..FILL).step_by(2) {
        expected.push_str(&format!("T /Fill/f{i:02}\t{i:02} {}\\n\n", "fill ".repeat(10)));
    }
    let script = r#"
import sys, zlib
sys.path.insert(0, sys.argv[1])
import rockfs
image, manifest, expected = sys.argv[2], sys.argv[3], sys.argv[4]
vol = rockfs.Volume(image, writable=False)
assert vol.sb['state'] == 0, 'the volume was not shut down cleanly'
problems = vol.check()
assert not problems, problems[:5]
found = {}
def walk(path, n):
    for _, _, inum, etype, name in vol.dir_entries(n):
        p = path.rstrip('/') + '/' + name
        if etype == rockfs.T_DIR:
            found[p] = None
            walk(p, inum)
        else:
            found[p] = vol.read_file(vol.read_inode(inum))
walk('/', vol.resolve('/'))
want = {}
for line in open(expected, encoding='utf-8'):
    kind, rest = line.rstrip('\n').split(' ', 1)
    if kind == 'T':
        path, text = rest.split('\t', 1)
        want[path] = text.encode('utf-8').decode('unicode_escape').encode('utf-8')
    else:
        want[rest] = None if kind == 'D' else ''
for big in ('/Pictures/rock.bmp', '/Projects/rock-copy.bmp'):
    want[big] = 'bmp'
missing = sorted(set(want) - set(found))
extra = sorted(set(found) - set(want))
assert not missing and not extra, 'missing %s; unexpected %s' % (missing, extra)
for p, w in want.items():
    got = found[p]
    if isinstance(w, bytes):
        assert got == w, '%s holds %r' % (p, got)
bmp = found['/Pictures/rock.bmp']
assert bmp[:2] == b'BM' and len(bmp) > 100000, 'rock.bmp is not a picture'
assert found['/Projects/rock-copy.bmp'] == bmp, 'the copy of rock.bmp differs'
assert len(set(bmp[54:])) > 1, 'nothing was drawn in rock.bmp'
ino = vol.read_inode(vol.resolve('/Pictures/rock.bmp'))
extents, tree = vol.load_extents(ino)  # the extents, and the tree's own blocks
assert len(extents) >= 10, 'rock.bmp is in %d pieces: the volume was not fragmented' % len(extents)
with open(manifest, 'w', encoding='utf-8') as f:
    for p in sorted(found):
        d = found[p]
        if d is None:
            f.write('D 0 00000000 %s\n' % p)
        else:
            f.write('F %d %08x %s\n' % (len(d), zlib.crc32(d), p))
assert tree, 'rock.bmp has no extent tree'
print('classic-disk: %d folders and files; rock.bmp is %d bytes in %d extents (tree depth %d)'
      % (len(found), len(bmp), len(extents), ino.depth))
vol.close(clean=False)
"#;
    let dir = image.parent().ok_or("no folder")?;
    let exp = dir.join("expected.txt");
    fs::write(&exp, expected).map_err(|e| e.to_string())?;
    let check = dir.join("check.py");
    fs::write(&check, script).map_err(|e| e.to_string())?;
    status(
        Command::new("python3")
            .arg(&check)
            .arg(root.join("tools"))
            .arg(image)
            .arg(manifest)
            .arg(&exp),
    )
}

/// `cargo xtask classic-disk`.
pub fn run(root: &Path) -> Result {
    let classic = root.join("classic");
    status(Command::new("sh").current_dir(&classic).arg("build.sh"))?;
    let work = root.join("build").join("classic-disk");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
    let image = work.join("classic-1.3.img");
    fs::File::create(&image)
        .and_then(|f| f.set_len(DATA_BYTES))
        .map_err(|e| format!("{}: {e}", image.display()))?;

    let port = free_port()?;
    let boot = classic.join("build").join("prettybigrock.img");
    // The same machine `run-prettybigrock.bat` starts, without a window; the boot disk is a
    // snapshot, so only the data disk changes.
    let mut qemu = Command::new("qemu-system-i386")
        .args(["-m", "128", "-display", "none"])
        // Without VMware's mouse device, QEMU's mouse movements go to the PS/2 mouse, as
        // relative movements; with it, they're taken as positions on the screen.
        .args(["-machine", "pc,vmport=off"])
        .arg("-drive")
        .arg(format!("file={},format=raw,index=0,snapshot=on", boot.display()))
        .arg("-drive")
        .arg(format!("file={},format=raw,index=1", image.display()))
        .arg("-monitor")
        .arg(format!("tcp:127.0.0.1:{port},server,nowait"))
        .spawn()
        .map_err(|e| format!("couldn't start qemu-system-i386: {e}"))?;
    let driven = Monitor::connect(port, &mut qemu).and_then(|mut m| drive(&mut m, &work));
    // Shutting down powers QEMU off; give it a minute.
    let end = Instant::now() + Duration::from_secs(60);
    let exited = loop {
        if qemu.try_wait().ok().flatten().is_some() {
            break true;
        }
        if Instant::now() > end {
            let _ = qemu.kill();
            let _ = qemu.wait();
            break false;
        }
        sleep(Duration::from_millis(500));
    };
    driven?;
    if !exited {
        return Err(format!(
            "the classic system didn't power off; screenshots are in {}",
            work.display()
        ));
    }

    let data = root.join("tests").join("data");
    fs::create_dir_all(&data).map_err(|e| e.to_string())?;
    let manifest = data.join("classic-1.3.manifest");
    verify(root, &image, &manifest)
        .map_err(|e| format!("{e}\nscreenshots are in {}", work.display()))?;
    // `-n` leaves the time and name out of the gzip header, so the same disk compresses to
    // the same bytes.
    let gz = Command::new("gzip")
        .args(["-9", "-n", "-c"])
        .arg(&image)
        .output()
        .map_err(|e| format!("couldn't run gzip: {e}"))?;
    if !gz.status.success() {
        return Err("gzip failed".into());
    }
    let out = data.join("classic-1.3.img.gz");
    fs::write(&out, &gz.stdout).map_err(|e| format!("{}: {e}", out.display()))?;
    println!(
        "classic-disk: wrote {} ({} bytes) and {}",
        out.display(),
        gz.stdout.len(),
        manifest.display()
    );
    Ok(())
}
