# 0025: The file picker, the clipboard and the permissions viewer

Status: Accepted · 9 November 2026 · Related: DESIGN.md principle 5, sections 7.1 and 7.3; notes
0014, 0017, 0022, 0023, 0024, 0026; milestone M6

## The problem

DESIGN.md section 7.1: "The app asks for 'a file to open'. You choose one, and the app receives
a capability for that file alone. Saving works the same way." Choosing is granting (principle
5), so the file picker is what makes sandboxed apps usable: Notepad, given only
`/Home/Documents`, can still open a letter on a USB stick because the user picked it. M5 did
this through the shell (`run notes FILE`). The clipboard and "Open with…" follow the same rule:
apps never reach each other's data unless the user connects them. And the **permissions
viewer** shows what each running app holds and why, and takes it back.

This note settles the file picker's protocol and behaviour, how its grants are recorded and
revoked, the clipboard as the user meets it (its rules are in note 0022, section 5), "Open
with…" from Files, and the permissions viewer.

## The design

### 1. The file picker

- **`apps/picker`** is a system app, one per session, started by the session's sandbox manager
  itself (as the desktop is) and started again when it ends. Its manifest asks for `trusted`
  (it must show the whole session tree), `windows`, `system` and `hidden`. Apart from the
  session's tree it holds a `pbr.sandbox` capability that may only `grant` (note 0024, section
  9).
- **Apps reach it** through `pbr.picker` at start-up role 1004, if their manifest asks for
  `picker`. The sandbox manager makes the picker's endpoint, gives the picker the receiving end,
  and mints each app's capability with a watched badge carrying the app's number. So the
  picker always knows which app is asking, and knows when that app has gone.

```
/// Choosing files. Served by the session's file picker.
interface pbr.picker version 1;

struct Filter { title: string<64>, extensions: string<128> }   // "Pictures", "bmp"
enum Place { Last @0, Home @1, Documents @2, Pictures @3, Music @4, Videos @5, Downloads @6 }

/// The user chooses an existing file (or a folder, with `folder`). `window` and `serial`
/// are the app's window and the key or button press that asked (note 0022, section 4).
@1 fn open(window: u32, serial: u32, title: string<64>, start: Place, filters: list<Filter, 8>,
           folder: bool, write: bool)
        -> (path: string<1024>, node: handle<pbr.file>) error PickerError;
/// The user chooses where to save, and a name. Answers with the file, made empty if it is
/// new; for an existing one the user has already said to replace it.
@2 fn save(window: u32, serial: u32, title: string<64>, start: Place, name: string<255>,
           filters: list<Filter, 8>)
        -> (path: string<1024>, node: handle<pbr.file>) error PickerError;

flexible enum PickerError {
    /// The user cancelled.
    Cancelled @1,
    /// Not asked from the app's window in answer to the user (below).
    NotAllowed @2,
    /// This app already has a picker open.
    Busy @3,
    /// The sandbox manager couldn't record the grant (too many, or the app has gone).
    Failed @4,
}
```

**Asking.** The picker answers only if `serial` is a key or button press delivered to that
window of that app in the last second (`check_serial`, note 0022), so an app in the background
can't throw pickers at the user. One picker at a time per app.

**What the user sees.** A `System` window over the app's window (`create_over`), with the rock
emblem and the system frame, so it can't be imitated (note 0022, section 6):
- the **title** says who is asking and for what: "Notepad wants to open a file", "Paint wants
  to save a picture", and, when `write` is asked, "Notepad will be able to change the file you
  choose";
- **places** on the left: Home, Documents, Pictures, Music, Videos, Downloads, and for
  administrators the volumes under `/Volumes`;
- the folder's contents as a `List` (name, size, modified), filtered by the chosen filter
  ("All files" is always offered), a path bar, **New folder**, and for saving a **name** field;
- **Open** or **Save**, and **Cancel** (Escape). Saving over an existing file asks "Replace
  "letter.txt"?" first.
- It starts in the `start` place, or (`Last`) where this app's last pick was, which the picker
  keeps in its own folder per app.

**What the app gets.**
1. The picker opens the chosen file or folder in its own tree with the rights to give: `Read`,
   plus `Write` if asked (`open` with `write`, or `save`). For `save` of a new name, it creates
   the file first.
2. It calls `pbr.sandbox.grant(app, path, node, rights)`. The sandbox manager reduces the
   capability to those rights, keeps it as the parent, records a grant with reason `Chosen`, and
   answers with a capability derived from it for the app (note 0024, section 9).
3. The picker answers the app with that capability and the path, which is the session path, the
   same the user saw. `sandstone::picker` mounts it in the app's tree there (`Files::mount`),
   so the app opens it by its path like any other file.

So a chosen file is exactly like one chosen at launch in M5: it is in the permissions viewer
with "You chose it", and `revoke` takes it back, with everything the app opened through it.
A file the app could reach anyway (inside a place its manifest gives it) is answered as is,
without a new grant, as `choose` does (note 0017, Changes).

**When the app goes**, its badge's gone notice closes any picker window it had open.

### 2. The clipboard, as the user meets it

The rules are in note 0022, section 5; what they mean in the apps:
- **Notepad**: Cut, Copy and Paste in its Edit menu and with Ctrl+X, Ctrl+C and Ctrl+V.
- **Terminal**: Copy and Paste in its right-click menu, and Ctrl+Shift+C and Ctrl+Shift+V
  (Ctrl+C is the shell's to keep).
- **Paint**: Copy picture, and Paste as a new picture.
- **Text fields** everywhere (sandstone): the same keys as Notepad.
- Paste is greyed out when the clipboard has nothing the app can use.
- An app pastes only when the user presses the keys or chooses Paste, and copies only while it
  has the focus. Nothing else can read the clipboard.

### 3. "Open with…" from Files

- Files is trusted (Chris's answer to Q16), so it can open any file in the session's tree and
  launch apps through `pbr.sandbox`, as the shell does with `run`.
- **Double-clicking a file** opens it in the app whose manifest lists its extension in `opens`
  (note 0024, section 3; Notepad `txt md`, Paint `bmp`); **Open with…** in its right-click menu
  lists every app that has `opens`, and any app with `windows`.
- It does what `run` does: `prepare(app, path)`, `choose(launch, path, node)` with the file
  opened read and write if the user may write it, then `start`. The app gets that file alone,
  recorded as `Chosen`, and its path as its argument.

### 4. The permissions viewer

- **`apps/permissions`** is a system app with `windows`, `sandbox view` (note 0024) and `system`
  (its window is a `System` window, so an app can't fake one). Launched from the start menu
  ("Permissions") it opens on the list of running apps; from a window's menu ("What can this app
  reach?") the desktop launches it with that app's number as its argument and it opens on that
  app.
- **For each running app** it shows: its title and icon, whether it is trusted, when it
  started; **what it holds**, each with why:

  | Shown | From |
  |-------|------|
  | "All your files (trusted)" | a `Trusted` grant |
  | "/Home/Documents: read and change, asked for in its manifest" | a `Manifest` grant |
  | "… allowed by your rules" | a `Rule` grant |
  | "/Home/Documents/letter.txt: read and change, you chose it" | a `Chosen` grant |
  | "Its own folder", "Windows", "The file picker", "System information", "The terminal" | `AppServices` (note 0024, section 9) |

- **Take back** on a file or folder grant calls `revoke`: the app loses it, and everything it
  opened through it, at once. **Stop app** calls `stop`. Services (windows, the picker) can't be
  taken back one by one in M6: stopping the app is the way.
- It refreshes when an app starts or ends, and after each change it makes.
- The shell's `apps` and `grants` stay, as the text version.

### 5. Testing

- **On the host**: the picker's logic as a library (choosing, filters, places for an
  administrator and a standard user, replacing, the rights for each request, the last place per
  app) against an in-memory tree; the permissions viewer's descriptions of each grant reason.
- **In QEMU, the self-test boot**: `grant` through a picker badge records a `Chosen` grant at the
  right path with the right rights, a revoke takes it and what was opened through it; an app
  that isn't the picker can't `grant`; a file the app gets anyway isn't granted twice.
- **In QEMU, the desktop boot** (note 0026): Notepad's Ctrl+S opens the picker, the runner types
  a name and presses Enter, and the file is in Documents; Notepad's Open picks a file on the 1.3
  disk; replacing asks first; the picker refuses a request with no recent serial and a second
  while one is open (`tests/programs/windowprobe`); the permissions viewer shows exactly each
  running app's grants (checked against `grants` and against what the app finds from inside, as
  M5's test 6 does); Take back and Stop work; copy in Notepad and paste in Terminal; a paste
  without a key press is refused.

## Alternatives considered

- **The picker inside the sandbox manager.** It would put a whole windowed program (lists,
  text, layout) into the one program that decides every app's grants, which should stay small.
- **The picker inside each app** (as most systems' open dialogs are, run in the app's process).
  The app would then need to see every folder in order to show them, which is exactly what the
  sandbox prevents.
- **Asking permission instead** ("Notepad wants to open your Documents. Allow?"). Prompt
  fatigue (DESIGN.md section 7.3): picking the file is already the user saying yes.
- **The app passing a path for the picker to open** without showing anything. That would let
  any app reach any file by name.
- **Returning a file name and letting the app open it.** The app can't open anything outside its
  tree; the capability is the permission.

## Open questions

- Remembering a chosen file across restarts of the app ("recent files"), which needs a stored
  grant the rules can express.
- Drag and drop between apps, with the clipboard's rules.
- Taking back services (a window, the picker) without stopping the app.
- Previews of pictures in the picker.

## Changes
- 21 November 2026 (M6 stage 10b, the capability review): checked the picker, `grant`, Open with… and the permissions
  viewer. `grant` is the picker badge's alone, only for an app whose manifest's `picker` was
  granted, with `Read` and at most `Write`, never more than the capability passed has; the
  kept capability is limited and the app's is derived from it, so `revoke` takes it and what
  was opened through it. The picker checks the serial as a request arrives, keeps one request
  per app, and a gone notice drops that app's waiting and open pickers. Capabilities attached
  to a request that takes none are closed by `granite` (`Incoming`'s `Drop`), so an app can't
  fill the picker's table that way. Fixed: a `grant` already covered by what the app holds was
  answered even if the app ended while it was being opened; now it is refused. **For Chris**:
  a chosen file's capability is derived from the chooser's (the picker's, or Files' for Open
  with…). So taking back the picker's (or Files') `Trusted` grant in the viewer also takes
  back, without saying so, every file chosen through it by every app; and what an app opens
  through a chosen folder counts against the chooser's share of records too (note 0026,
  section 4), so a few apps given folders could use up the picker's 16384 on a volume until
  it is started again. The sandbox manager could instead derive the kept capability from its
  own session tree, but that changes section 1's "keeps it as the parent".
- 17 November 2026, built (M6 stage 9b): section 2's clipboard in Notepad and Terminal,
  section 3's Open with…, and section 5's Notepad and Terminal tests. What building settled:
  - **`pbr.sandbox` gains `@12 openers()`**: the apps with windows that aren't hidden, each
    with the kinds of file its manifest `opens` (`Available` couldn't grow without a new
    version). Trusted callers may ask it.
  - **Opening from Files** is `run`'s three steps: Files opens the file read and write if the
    user may change it (read only otherwise), `prepare(app, path)`, `choose(launch, path,
    node)`, `start`, and `cancel` if any step fails. The app gets that one file (nothing more if
    its own grants already reach it) and the path as its argument. A file no app `opens` goes
    to Notepad, as 1.3 sent every file that wasn't a picture there. Open with… lists the apps
    that open the file's kind first, then every other app with windows.
  - **Notepad asks the picker first, then "Save changes?"** for Open: the question's answer is
    a press in the question, and an untitled text would need a second picker for Save. New and
    Close ask first. A Save in the question that needs the picker uses the question's press
    (note 0022, Changes: `check_serial` counts an app's own dialogs).
  - **Terminal**: Copy and Paste in a right-click menu and Ctrl+Shift+C and Ctrl+Shift+V; a
    paste goes into the line being typed, its line breaks turned into spaces, so a paste never
    runs a command.
- 14 November 2026, built (M6 stage 8): sections 1, 4 and 5 except what needs the apps of
  stage 9 (Notepad's and Terminal's parts of the tests; section 2's clipboard in the apps and
  section 3's "Open with…" come with them). What building settled:
  - **The picker** (`apps/picker`) is a service and a window. A thread of its own receives
    `pbr.picker` requests and hands them to the window's event loop through one of its own
    notification bits, so neither waits for the other; replies are kept until the user has
    chosen. A request's serial is checked (`check_serial`) as it arrives, while the press is
    less than a second old. **One picker per app** (`Busy`); other apps' requests wait their
    turn rather than opening a second window at once. The window is `create_over` the app's.
  - **The title** is "APP wants to WHAT": APP is the app's manifest title, which the picker
    asks the sandbox manager for (`details`: the badge carries only the number), and WHAT the
    app's own `title` argument ("open a picture"), or "open a file", "open a folder", "save a
    file". With `write` a line above says "Notepad will be able to change the file you choose".
  - **The window** is a sandstone app over `lib/browse`'s `Folders` trait, tested on the host
    against a tree in memory: places (an administrator's volumes after the six), the folder's
    contents with hidden files left out, folders first and sorted naturally, sortable by name,
    size and modified; the filters as toggle buttons with "All files" last; Up, the path bar,
    New folder; for saving, a name field (the filter's first extension is added to a name
    without one) and "Replace?" before saving over a file. The Modified column is in UTC: the
    picker holds no `pbr.users` to read the time zone from.
  - **What the app gets**: the picker opens (or, for a new name, creates) the choice in its
    tree with `Read`, plus `Write` if asked or saving, and calls `grant`; a file it made for a
    grant that was refused is removed again. Each app's last folder is kept, by the app's
    name, in `/Home/Apps/picker/last.txt`.
  - **`sandstone::picker`**: `FilePicker` holds the app's picker capability, asks on a helper
    thread (taking the capability with it) and answers through one of the event loop's bits;
    `FilePicker::mount` puts the answer in the app's tree at the path the user saw.
  - **The permissions viewer** (`apps/permissions`) holds no files and a `sandbox view`
    capability; its window is a `System` window. It looks again once a second, which is how
    it follows apps starting and ending. Grants are described as section 4's table says, an
    app's own folder as "Its own folder (/Home/Apps/APP)", and its services as "Windows", "The
    file picker", "System information", "The terminal", "Your account and settings" and
    "Launching and managing apps".
  - **Tests**: the host tests of section 5 (picker and viewer, with pictures at 1x and 2x).
    The self-test: `grant` through a picker capability (`services/root/src/pickerparts.rs`).
    The desktop boot drives the picker from the gallery (Notepad is stage 9): Save as, saving
    again over the same name, opening what was saved, and `letter.txt` on 1.3's disk; the
    viewer, from the gallery's window menu, shows what the sandbox manager says (the root
    program logs it), Take back and Stop; `windowprobe` is refused the picker without a press,
    for a window it hasn't, and a second while one is open.
- 9 November 2026: first draft, for discussion with Chris; accepted by Chris the same day
  (he left the walk-through for later and asked for stage 0 to start).
