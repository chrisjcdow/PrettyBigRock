// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The session's desktop (docs/design/0024-the-session-desktop.md, section 6): 1.3's desktop,
//! as an app. The session's sandbox manager starts it when the session has the screens, and
//! starts it again if it ends.
//!
//! It shows, through the display server (note 0022):
//! - on each screen a `Background` window with the wallpaper, Table Mountain at dusk, and on
//!   the main screen the app icons (a double click launches one);
//! - on the main screen a `Panel` along the bottom: the start button, a button for each window
//!   (click to focus it, again to minimise it), and the clock, in the user's time zone;
//! - the start menu (the start button, the Windows key or Ctrl+Esc), a popup of the panel: the
//!   user's name, every app that has windows and isn't hidden, then Restart and Shut down.
//!
//! What it holds is little (note 0024, section 6): its window capability, with the `Desktop`
//! flag; its own folder, where it keeps `screens.txt`, the screens' arrangement; a `pbr.sandbox`
//! capability that may launch and stop apps and list them; and a `pbr.users` capability that may
//! only ask who the user is, their settings, and to restart or turn off. None of the user's
//! files: launching needs only the sandbox manager.
//!
//! The drawing and the arrangement file are `lib/desktop`, built and tested on the host; this is
//! the program around them. Each line it logs starts `desktop: `; the desktop boot's test runner
//! reads some of them (note 0026, section 6).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use granite::endpoint::Endpoint;
use granite::time;
use pbr_desktop::icons::Icon;
use pbr_desktop::menu::{Choice, Item, StartMenu};
use pbr_desktop::screens::Arrangement;
use pbr_desktop::shortcuts::{self, Shortcut};
use pbr_desktop::taskbar::{self, Part, TaskButton, Taskbar};
use pbr_desktop::{clock, wallpaper};
use pbr_draw::Damage;
use pbr_files::{Files, READ};
use pbr_interfaces::pbr_sandbox::{self as sandbox, Available};
use pbr_interfaces::pbr_users as users;
use pbr_interfaces::pbr_window::{ScreenPlace, WindowInfo};
use pbr_window::{Connection, Event, PointerEvent, PointerKind, Win, WindowFlags, WindowKind};

granite::main!(main);

/// Where the sandbox manager gives it `pbr.sandbox` and `pbr.users` (note 0024, section 10).
const SANDBOX_ROLE: u32 = 1000;
const USERS_ROLE: u32 = 1001;
/// This app's name, for its own folder.
const NAME: &str = "desktop";
/// What `act` does to a window (`lib/compositor`'s `act`).
const ACT_FOCUS: u32 = 1;
const ACT_MINIMISE: u32 = 3;
/// A window menu choice (note 0022, section 10): the permissions viewer, or stopping the app.
const CHOICE_PERMISSIONS: u32 = 1;
const CHOICE_STOP: u32 = 2;
/// The edge a panel goes along, as `create`'s `at` gives it (`lib/compositor`: 1 top, 2
/// bottom, 3 left, 4 right).
const BOTTOM: i32 = 2;
/// The clock's timer.
const CLOCK: u32 = 1;
/// Two clicks on the same icon within this long are a double click (nanoseconds).
const DOUBLE_CLICK: u64 = 500_000_000;
/// The start menu's heading: the theme's heading style, which sandstone's dialogs use too, so
/// the two always match (note 0023; Chris is still to choose it).
const HEADING: pbr_draw::TextStyle = pbr_sandstone::theme::HEADING;
/// The left button.
const LEFT: u16 = 1;

/// Logs a line with the program's prefix.
macro_rules! say {
    ($($arg:tt)*) => {
        granite::println!("desktop: {}", format_args!($($arg)*))
    };
}

/// A screen's wallpaper window.
struct Background {
    screen: u32,
    win: Win,
}

/// The start menu, while it is open.
struct Menu {
    win: Win,
    hovered: Option<Choice>,
}

struct Desktop {
    conn: Connection,
    sandbox: Option<sandbox::Client>,
    users: Option<users::Client>,
    files: Files,
    /// The user's display name, for the start menu.
    name: String,
    /// Minutes east of UTC.
    timezone: i32,
    /// The apps the start menu and the icons list.
    apps: Vec<Available>,
    /// The screens, as the display server last said.
    screens: Vec<ScreenPlace>,
    backgrounds: Vec<Background>,
    /// The taskbar, and the screen it is on.
    panel: Option<(u32, Win)>,
    menu: Option<Menu>,
    /// The windows for the taskbar, and each app's icon by its number.
    windows: Vec<WindowInfo>,
    icons: Vec<(u32, Icon)>,
    /// The selected icon, and the last click on one: (which, when).
    selected: Option<usize>,
    last_click: Option<(usize, u64)>,
}

fn main(mut start: granite::Start) -> i32 {
    let conn = match Connection::from_start(&mut start) {
        Ok(c) => c,
        Err(e) => {
            say!("can't reach the display server: {e:?}");
            return 2;
        }
    };
    let sandbox = start.take(SANDBOX_ROLE).map(|h| {
        let mut c = sandbox::Client::new(Endpoint(h));
        c.set_timeout_ms(10_000);
        c
    });
    let users = start.take(USERS_ROLE).map(|h| {
        let mut c = users::Client::new(Endpoint(h));
        c.set_timeout_ms(60_000);
        c
    });
    let files = Files::from_start(&mut start);
    let mut d = Desktop {
        conn,
        sandbox,
        users,
        files,
        name: String::new(),
        timezone: 0,
        apps: Vec::new(),
        screens: Vec::new(),
        backgrounds: Vec::new(),
        panel: None,
        menu: None,
        windows: Vec::new(),
        icons: Vec::new(),
        selected: None,
        last_click: None,
    };
    d.who();
    d.find_apps();
    d.arrange();
    d.make_windows();
    d.tick();
    say!("ready, {} apps, {} screens.", d.apps.len(), d.screens.len());
    let mut code = 0;
    loop {
        let Some(event) = d.conn.next_event(time::FOREVER) else { continue };
        if !d.handle(event, &mut code) {
            break;
        }
    }
    code
}

impl Desktop {
    /// Who the user is, and their time zone.
    fn who(&mut self) {
        let Some(u) = &self.users else { return };
        match u.whoami() {
            Ok(user) => self.name = user.display,
            Err(e) => say!("whoami: {e}"),
        }
        if let Ok(list) = u.settings()
            && let Some(tz) = list.iter().find(|s| s.key == "timezone")
        {
            self.timezone = pbr_manifest::settings::timezone_minutes(&tz.value).unwrap_or(0);
        }
    }

    /// The apps the start menu lists: those that have windows and aren't hidden.
    fn find_apps(&mut self) {
        let Some(s) = &self.sandbox else { return };
        match s.apps() {
            Ok(list) => self.apps = list.into_iter().filter(|a| a.windows && !a.hidden).collect(),
            Err(e) => say!("listing the apps: {e}"),
        }
    }

    // ---- The screens ---------------------------------------------------------------------

    /// Asks where the screens are, and arranges them as `screens.txt` says, if it says
    /// anything about them.
    fn arrange(&mut self) {
        self.screens = match self.conn.client().screens() {
            Ok(s) => s,
            Err(e) => {
                say!("asking for the screens: {e}");
                return;
            }
        };
        for s in &self.screens {
            say!("screen {} is \"{}\".", s.screen, s.identity);
        }
        let Some(saved) = self.saved_arrangement() else { return };
        let Some(new) = saved.apply(&self.screens) else { return };
        match self.conn.client().arrange(&new) {
            Ok(()) => {
                say!("arranged {} screens from screens.txt.", new.len());
                if let Ok(s) = self.conn.client().screens() {
                    self.screens = s;
                }
            }
            Err(e) => say!("the display server refused screens.txt's arrangement: {e}"),
        }
    }

    /// `screens.txt` from its own folder, if it is there and can be read.
    fn saved_arrangement(&self) -> Option<Arrangement> {
        let path = format!("{}/screens.txt", Files::data_folder(NAME));
        let f = self.files.open(&path, READ).ok()?;
        let size = f.info().ok()?.size;
        if size > pbr_desktop::screens::MAX_SIZE as u64 {
            say!("screens.txt is too big; ignored.");
            return None;
        }
        let text = f.read_all().ok()?;
        match Arrangement::parse(&text) {
            Ok(a) => Some(a),
            Err((e, line)) => {
                say!("screens.txt is ignored: {e:?} on line {line}.");
                None
            }
        }
    }

    /// The screens in the space (not mirroring another), and the main one.
    fn shown(&self) -> impl Iterator<Item = &ScreenPlace> {
        self.screens.iter().filter(|s| s.mirrors == 0)
    }

    fn main_screen(&self) -> Option<&ScreenPlace> {
        self.shown().find(|s| s.main).or_else(|| self.shown().next())
    }

    /// Makes a wallpaper for each screen that hasn't one and the taskbar on the main screen,
    /// and closes those whose screen has gone (or now mirrors another).
    fn make_windows(&mut self) {
        let shown: Vec<(u32, u32, u32)> =
            self.shown().map(|s| (s.screen, s.size.width, s.size.height)).collect();
        let gone: Vec<Win> = self
            .backgrounds
            .iter()
            .filter(|b| !shown.iter().any(|(n, ..)| *n == b.screen))
            .map(|b| b.win)
            .collect();
        for w in gone {
            let _ = self.conn.destroy(w);
            self.backgrounds.retain(|b| b.win != w);
        }
        for (screen, w, h) in shown {
            if self.backgrounds.iter().any(|b| b.screen == screen) {
                continue;
            }
            let at = (screen as i32, 0);
            match self.conn.create_child(
                WindowKind::Background,
                Win(0),
                "Desktop",
                (w.max(1), h.max(1)),
                WindowFlags::default(),
                at,
                0,
            ) {
                Ok(win) => self.backgrounds.push(Background { screen, win }),
                Err(e) => say!("a wallpaper for screen {screen}: {e:?}"),
            }
        }
        // The taskbar belongs on the main screen.
        let main = self.main_screen().map(|s| (s.screen, s.size.width));
        if let Some((screen, _)) = self.panel
            && main.is_none_or(|(m, _)| m != screen)
        {
            self.close_menu();
            if let Some((_, w)) = self.panel.take() {
                let _ = self.conn.destroy(w);
            }
        }
        if self.panel.is_none()
            && let Some((screen, width)) = main
        {
            match self.conn.create_child(
                WindowKind::Panel,
                Win(0),
                "Taskbar",
                (width.max(1), taskbar::HEIGHT as u32),
                WindowFlags::default(),
                (screen as i32, BOTTOM),
                0,
            ) {
                Ok(win) => self.panel = Some((screen, win)),
                Err(e) => say!("the taskbar: {e:?}"),
            }
        }
    }

    // ---- Drawing -------------------------------------------------------------------------

    /// The usable height of the main screen, above the taskbar.
    fn usable_height(&self) -> i32 {
        self.main_screen().map_or(0, |s| s.size.height as i32 - taskbar::HEIGHT)
    }

    fn draw_background(&mut self, win: Win) {
        let Some(screen) = self.backgrounds.iter().find(|b| b.win == win).map(|b| b.screen) else {
            return;
        };
        let main = self.main_screen().map(|s| s.screen) == Some(screen);
        let height = self.usable_height();
        let labels: Vec<(Icon, String)> = if main {
            self.apps.iter().map(|a| (Icon::from_name(&a.icon), a.title.clone())).collect()
        } else {
            Vec::new()
        };
        let selected = self.selected;
        let r = self.conn.draw(win, |canvas| {
            let scale = canvas.scale();
            let (pixels, w, h, stride) = canvas.whole();
            wallpaper::draw(pixels, w, h, stride, scale);
            let list: Vec<Shortcut<'_>> =
                labels.iter().map(|(icon, label)| Shortcut { icon: *icon, label }).collect();
            shortcuts::draw(canvas, &list, selected, height);
            Damage::new()
        });
        if let Err(e) = r {
            say!("drawing a wallpaper: {e:?}");
        }
    }

    /// The taskbar's buttons, for the windows there are: (icon, title, the window).
    fn buttons(&self) -> Vec<(Icon, String, WindowInfo)> {
        self.windows
            .iter()
            .map(|w| {
                let icon =
                    self.icons.iter().find(|(a, _)| *a == w.app).map_or(Icon::App, |(_, i)| *i);
                (icon, w.title.clone(), w.clone())
            })
            .collect()
    }

    fn taskbar_width(&self) -> i32 {
        let screen = self.panel.map(|(s, _)| s);
        self.screens.iter().find(|s| Some(s.screen) == screen).map_or(0, |s| s.size.width as i32)
    }

    fn draw_panel(&mut self) {
        let Some((_, win)) = self.panel else { return };
        let (time, date) = clock::local(time::wall_seconds(), self.timezone);
        let width = self.taskbar_width();
        let start_open = self.menu.is_some();
        let made = self.buttons();
        let buttons: Vec<TaskButton<'_>> = made
            .iter()
            .map(|(icon, title, w)| TaskButton {
                icon: *icon,
                title,
                focused: w.focused,
                minimised: w.minimised,
                attention: w.attention,
                not_responding: w.not_responding,
            })
            .collect();
        let bar = Taskbar { width, start_open, buttons: &buttons, time: &time, date: &date };
        let r = self.conn.draw(win, |canvas| {
            bar.draw(canvas);
            Damage::new()
        });
        if let Err(e) = r {
            say!("drawing the taskbar: {e:?}");
        }
    }

    fn menu_items(&self) -> Vec<(Icon, String)> {
        self.apps.iter().map(|a| (Icon::from_name(&a.icon), a.title.clone())).collect()
    }

    fn draw_menu(&mut self) {
        let Some(m) = &self.menu else { return };
        let (win, hovered) = (m.win, m.hovered);
        let made = self.menu_items();
        let items: Vec<Item<'_>> =
            made.iter().map(|(icon, label)| Item { icon: *icon, label }).collect();
        let menu = StartMenu { heading: &self.name, style: HEADING, apps: &items, hovered };
        let r = self.conn.draw(win, |canvas| {
            menu.draw(canvas);
            Damage::new()
        });
        if let Err(e) = r {
            say!("drawing the start menu: {e:?}");
        }
    }

    /// Redraws the clock, and sets the timer for the next minute.
    fn tick(&mut self) {
        self.draw_panel();
        let next = clock::until_next_minute(time::wall_seconds());
        self.conn.set_timer(CLOCK, time::after_ms(next * 1000));
    }

    // ---- The start menu ------------------------------------------------------------------

    /// Opens the start menu over the taskbar, with the serial of the key or click that asked
    /// for it (a popup needs one), or closes it if it is open.
    fn toggle_menu(&mut self, serial: u32) {
        if self.menu.is_some() {
            self.close_menu();
            return;
        }
        let Some((_, panel)) = self.panel else { return };
        let made = self.menu_items();
        let items: Vec<Item<'_>> =
            made.iter().map(|(icon, label)| Item { icon: *icon, label }).collect();
        let menu = StartMenu { heading: &self.name, style: HEADING, apps: &items, hovered: None };
        let (w, h) = menu.size();
        // 1.3 put it 4 pixels from the left, just above the taskbar.
        let at = (4, -h);
        match self.conn.create_child(
            WindowKind::Popup,
            panel,
            "Start",
            (w as u32, h as u32),
            WindowFlags::default(),
            at,
            serial,
        ) {
            Ok(win) => {
                self.menu = Some(Menu { win, hovered: None });
                say!("start menu open, {} apps.", self.apps.len());
                self.say_menu_places(&menu, at);
                self.draw_panel();
            }
            Err(e) => say!("the start menu: {e:?}"),
        }
    }

    /// Says where each choice of the start menu is on the screens (the middle of its row), for
    /// the desktop boot's test runner, which clicks them.
    fn say_menu_places(&self, menu: &StartMenu<'_>, at: (i32, i32)) {
        let Some(main) = self.main_screen() else { return };
        let (w, h) = menu.size();
        let top = main.y + main.size.height as i32 - taskbar::HEIGHT + at.1;
        let x = main.x + at.0 + w * 2 / 3;
        let mut y = 0;
        while y < h {
            let Some(choice) = menu.choice_at(w * 2 / 3, y) else {
                y += 1;
                continue;
            };
            let first = y;
            while y < h && menu.choice_at(w * 2 / 3, y) == Some(choice) {
                y += 1;
            }
            let what = match choice {
                Choice::App(i) => self.apps.get(i).map_or("?", |a| a.app.as_str()),
                Choice::Restart => "Restart",
                Choice::ShutDown => "Shut down",
            };
            say!("menu {what} at {x},{}", top + (first + y) / 2);
        }
    }

    fn close_menu(&mut self) {
        if let Some(m) = self.menu.take() {
            let _ = self.conn.destroy(m.win);
            say!("start menu closed.");
            self.draw_panel();
        }
    }

    fn choose(&mut self, choice: Choice) {
        self.close_menu();
        match choice {
            Choice::App(i) => {
                if let Some(app) = self.apps.get(i).map(|a| a.app.clone()) {
                    self.launch(&app);
                }
            }
            Choice::Restart => self.power(true),
            Choice::ShutDown => self.power(false),
        }
    }

    // ---- What it asks of others --------------------------------------------------------

    /// Launches an app through the sandbox manager, which tells the display server that the
    /// user launched it (so its first window takes the focus).
    fn launch(&mut self, app: &str) {
        let Some(s) = &self.sandbox else { return };
        say!("launching {app}.");
        let started = s.prepare(app, "").map_err(|e| format!("{e}")).and_then(|launch| {
            s.start(launch).map_err(|e| {
                let _ = s.cancel(launch);
                format!("{e}")
            })
        });
        match started {
            Ok(r) => say!("launched {app} as app {}.", r.app),
            Err(e) => say!("can't launch {app}: {e}"),
        }
    }

    fn power(&mut self, restart: bool) {
        let Some(u) = &self.users else { return };
        say!("{}.", if restart { "restarting" } else { "shutting down" });
        // On success this never answers: everything has stopped.
        let r = if restart { u.restart() } else { u.power_off() };
        if let Err(e) = r {
            say!("couldn't {}: {e}", if restart { "restart" } else { "shut down" });
        }
    }

    /// The window list changed: the taskbar again, and each app's icon.
    fn windows_changed(&mut self) {
        match self.conn.client().windows() {
            Ok(list) => self.windows = list,
            Err(e) => say!("asking for the windows: {e}"),
        }
        let unknown = self.windows.iter().any(|w| !self.icons.iter().any(|(a, _)| *a == w.app));
        if unknown
            && let Some(s) = &self.sandbox
            && let Ok(running) = s.list()
        {
            self.icons = running
                .iter()
                .map(|r| {
                    let icon = self
                        .apps
                        .iter()
                        .find(|a| a.app == r.app)
                        .map_or(Icon::App, |a| Icon::from_name(&a.icon));
                    (r.id, icon)
                })
                .collect();
        }
        self.draw_panel();
    }

    /// A choice from a window's menu, which the display server leaves to the desktop.
    fn window_menu(&mut self, window: u64, choice: u32) {
        let Some(app) = self.windows.iter().find(|w| w.window == window).map(|w| w.app) else {
            return;
        };
        match choice {
            CHOICE_STOP => {
                say!("stopping app {app}.");
                if let Some(s) = &self.sandbox
                    && let Err(e) = s.stop(app)
                {
                    say!("can't stop app {app}: {e}");
                }
            }
            CHOICE_PERMISSIONS => {
                // The permissions viewer comes with note 0025 (M6 stage 8).
                say!("what can app {app} reach? (the permissions viewer isn't there yet)");
            }
            _ => {}
        }
    }

    // ---- Input ---------------------------------------------------------------------------

    fn pointer(&mut self, p: PointerEvent) {
        let pressed = matches!(p.what, PointerKind::Button { button: LEFT, pressed: true });
        if let Some(m) = &self.menu
            && p.window == m.win
        {
            let made = self.menu_items();
            let items: Vec<Item<'_>> =
                made.iter().map(|(icon, label)| Item { icon: *icon, label }).collect();
            let menu =
                StartMenu { heading: &self.name, style: HEADING, apps: &items, hovered: None };
            let at = menu.choice_at(p.x, p.y);
            match p.what {
                PointerKind::Moved if at != m.hovered => {
                    if let Some(m) = &mut self.menu {
                        m.hovered = at;
                    }
                    self.draw_menu();
                }
                PointerKind::Left => {
                    if let Some(m) = &mut self.menu {
                        m.hovered = None;
                    }
                    self.draw_menu();
                }
                _ if pressed => {
                    if let Some(c) = at {
                        self.choose(c);
                    }
                }
                _ => {}
            }
            return;
        }
        if !pressed {
            return;
        }
        if let Some((_, panel)) = self.panel
            && p.window == panel
        {
            let made = self.buttons();
            let buttons: Vec<TaskButton<'_>> = made
                .iter()
                .map(|(icon, title, w)| TaskButton {
                    icon: *icon,
                    title,
                    focused: w.focused,
                    minimised: w.minimised,
                    attention: w.attention,
                    not_responding: w.not_responding,
                })
                .collect();
            let bar = Taskbar {
                width: self.taskbar_width(),
                start_open: self.menu.is_some(),
                buttons: &buttons,
                time: "",
                date: "",
            };
            match bar.part_at(p.x, p.y) {
                Some(Part::Start) => self.toggle_menu(p.serial),
                Some(Part::Button(i)) => {
                    let Some(w) = self.windows.get(i) else { return };
                    // A click on the focused window's button minimises it; on any other, it
                    // comes forward.
                    let action = if w.focused && !w.minimised { ACT_MINIMISE } else { ACT_FOCUS };
                    if let Err(e) = self.conn.client().act(w.window, action) {
                        say!("acting on a window: {e}");
                    }
                }
                _ => {}
            }
            return;
        }
        // The wallpaper of the main screen: the icons.
        let main = self.main_screen().map(|s| s.screen);
        let on_main = self.backgrounds.iter().any(|b| b.win == p.window && Some(b.screen) == main);
        self.close_menu();
        if !on_main {
            return;
        }
        let hit = shortcuts::at(self.apps.len(), self.usable_height(), p.x, p.y);
        let now = time::now();
        let double = hit.is_some()
            && self.last_click.is_some_and(|(i, t)| Some(i) == hit && now - t < DOUBLE_CLICK);
        self.last_click = hit.map(|i| (i, now));
        if self.selected != hit {
            self.selected = hit;
            let win = p.window;
            self.draw_background(win);
        }
        if double && let Some(app) = hit.and_then(|i| self.apps.get(i)).map(|a| a.app.clone()) {
            self.last_click = None;
            self.launch(&app);
        }
    }

    /// Deals with one event. Returns false when the desktop should end, with `code` its status.
    fn handle(&mut self, event: Event, code: &mut i32) -> bool {
        match event {
            Event::Configure { window, .. } => {
                if self.backgrounds.iter().any(|b| b.win == window) {
                    self.draw_background(window);
                } else if self.panel.is_some_and(|(_, w)| w == window) {
                    self.draw_panel();
                } else if self.menu.as_ref().is_some_and(|m| m.win == window) {
                    self.draw_menu();
                }
            }
            // A popup closed by the display server (a click elsewhere): it is the app's to
            // destroy.
            Event::CloseRequested { window } => {
                if self.menu.as_ref().is_some_and(|m| m.win == window) {
                    self.close_menu();
                }
            }
            Event::Pointer(p) => self.pointer(p),
            Event::StartMenu { serial } => self.toggle_menu(serial),
            Event::Windows => self.windows_changed(),
            Event::WindowMenu { window, choice } => self.window_menu(window, choice),
            Event::Screens => {
                self.close_menu();
                self.arrange();
                self.make_windows();
                // Sizes may have changed: everything is drawn again as it is configured.
            }
            Event::Timer(CLOCK) => self.tick(),
            Event::Disconnected => {
                say!("the display server has gone.");
                *code = 3;
                return false;
            }
            _ => {}
        }
        true
    }
}
