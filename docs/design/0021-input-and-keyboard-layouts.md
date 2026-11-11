# 0021: Input: the input service and keyboard layouts

Status: Accepted · 9 November 2026 · Related: DESIGN.md section 8 ("Input"); notes 0011, 0018,
0020, 0022, 0024; milestone M6

## The problem

In M5 the console opens every keyboard itself and turns key codes into characters with a US
layout built into `lib/devices` (note 0018, section 5). M6 needs:
- **keyboard layouts**: US, UK and US-International (with dead keys, so that ê, ë and ô can be
  typed; Chris's answer to Q9), chosen per user;
- **one place that owns the keyboards and pointers**, because the display server (note 0022)
  must be the only program that hears them while the desktop runs (an app must never read the
  keyboard behind the display server's back), and the console must still work in text mode;
- **absolute pointers**: QEMU's tablet, which 1.3 supports (so the mouse doesn't need
  capturing) and touch screens later; M4's virtio-input driver knows only relative mice;
- key repeat that works the same on every keyboard (PS/2 keyboards repeat by themselves,
  virtio ones send repeats only sometimes).

## The design

### 1. The input service

- **`services/input`** is supervised by the root program, like the console (note 0018). It may
  look up `devices`. It finds every device of class `input`, opens it with `pbr.input` (taking
  it over, note 0011), and finds them again when the device manager or a driver restarts (as
  the console's watcher does in M5).
- **It serves `pbr.seat`** (section 3). "Seat" is the usual name for the keyboards and pointers
  one person uses at one screen. It has **one consumer at a time**: taking it over, as
  `pbr.input` does, stops the previous consumer's events. In M6 the consumer is the display
  server, or the console in text mode (note 0024, section 8).
- **It merges** every keyboard into one stream and every pointer into another, keeps each
  device's own state (Shift held on one keyboard doesn't apply to another), applies the
  layout, makes key repeats, and says which device each event came from.
- **What it doesn't do**: decide which window gets an event (the display server), map pointers
  to screens (the display server, which knows where the screens are), or accelerate the pointer
  (the display server, which knows the scale).
- **Who may use it** (the supervisor's lookup table, note 0018): the display server and the
  console may look up `input` (as consumers); the users service may look up `input` too, with a
  badge that can only set the layout (section 5). Nobody else.

### 2. Events

Events come through a shared ring made with `granite::ring`, as `pbr.input`'s do, and a
doorbell notification. Each event is **32 bytes**, little-endian:

| Offset | Size | Field |
|--------|------|-------|
| 0 | 2 | Kind (below) |
| 2 | 2 | The device it came from (the input service's number, see `devices`) |
| 4 | 2 | Code: the key's position code (`pbr.input`'s `KEY_*`), the button, or the axis |
| 6 | 2 | Modifiers held after this event (below) |
| 8 | 4 | Value: for keys 0 released, 1 pressed, 2 repeat; the amount for movement and scrolling; the position for absolute axes |
| 12 | 4 | For keys: the first character typed (a Unicode scalar value), 0 if none. For absolute pointers: the other axis's position |
| 16 | 4 | For keys: a second character, 0 if none (a dead key followed by a letter it doesn't combine with types both) |
| 20 | 4 | Reserved, 0 |
| 24 | 8 | When it happened, nanoseconds on the monotonic clock |

| Kind | Meaning |
|------|---------|
| 0 | **Lost**: the consumer fell behind and `value` events were dropped. A reset follows |
| 1 | **Key** |
| 2 | **Move**: relative, `code` the axis (`AXIS_X`, `AXIS_Y`), `value` in device units |
| 3 | **Button** |
| 4 | **Scroll**: `code` `AXIS_WHEEL` (up is positive) or `AXIS_HWHEEL` (right is positive), `value` in steps |
| 5 | **Position**: absolute, `value` x and the field at 12 y, each 0 to 65535 across the device's whole surface |
| 6 | **Reset**: assume nothing is held. Sent first after `take`, and after a loss, followed by a press for every key and button still held |
| 7 | **Devices changed**: ask `devices` again |

**Modifiers** (bits): 0 Shift, 1 Ctrl, 2 Alt, 3 AltGr (the right Alt key, on layouts that use
it), 4 Meta (the Windows key), 5 Caps Lock on, 6 Num Lock on.

**What a key's characters mean.** The characters are what the key types with Shift, AltGr,
Caps Lock and Num Lock applied, as the layout says. They are given even when Ctrl, Alt or Meta
is held, and then they **don't type**: Ctrl+S carries `s` so that an app's shortcut follows the
letter printed on the key whatever the layout, while a game that wants the key in the same place
on every keyboard uses the position code. A key that types nothing (Shift, F1, an arrow) has no
characters. Dead keys type nothing when pressed; the next key types the combined character.

**Repeat.** The input service drops the drivers' own repeats and makes its own: after 500 ms
held, then 30 a second, as value 2, for keys that type and for Backspace, Delete, Enter, Tab
and the arrows, Home, End, Page Up and Page Down; never for modifiers. One key repeats at a
time, the last one pressed.

### 3. The interface: `pbr.seat`

```
/// Keyboards and pointers, merged, with the layout applied. Served by the input service.
interface pbr.seat version 1;

enum DeviceKind { Keyboard @1, Mouse @2, Tablet @3, Touch @4 }
struct Device { id: u16, kind: DeviceKind, name: string<64>, device: u32 }

/// Become the consumer: the ring of 32-byte events and its doorbell (bit 0). Taking over
/// stops the previous consumer's ring.
@1 fn take() -> (ring: handle<memory>, doorbell: handle<notification>) error SeatError;

/// The devices: the input service's numbers, and the device manager's (`device`).
@2 fn devices() -> (devices: list<Device, 32>) error SeatError;

/// The layout in use, and the ones there are.
@3 fn layouts() -> (current: string<32>, available: list<Layout, 32>) error SeatError;
struct Layout { name: string<32>, title: string<64> }

/// Changes the layout (only through the users service's badge; section 5).
@4 fn set_layout(name: string<32>) -> () error SeatError;

flexible enum SeatError { NotAllowed @1, NoSuchLayout @2, NoRing @3 }
```

### 4. Layouts: `lib/keymap`

A layout is a small text file in the start-up archive, `layouts/<name>.kbd`, in the same
style as manifests (note 0017): one `key value…` per line, `#` for comments, UTF-8, at most
16 KiB.

```
# layouts/us-intl.kbd
title "English (US, international, with dead keys)"
key A a A á Á
key E e E é É
key APOSTROPHE dead:acute dead:diaeresis ' "
key 6 6 dead:circumflex ¼ ^
dead acute a á A Á e é E É i í I Í o ó O Ó u ú U Ú space '
dead circumflex a â A Â e ê E Ê i î I Î o ô O Ô u û U Û space ^
dead diaeresis a ä A Ä e ë E Ë i ï I Ï o ö O Ö u ü U Ü y ÿ space "
```

| Line | Meaning |
|------|---------|
| `title "…"` | The name people see |
| `key CODE NORMAL [SHIFT [ALTGR [ALTGR-SHIFT]]]` | What a key types. `CODE` is `pbr.input`'s name without `KEY_` (`A`, `1`, `APOSTROPHE`, `KP_7`); a value is one character, `space`, `none`, or `dead:NAME` |
| `dead NAME BASE RESULT…` | What a dead key does to the next character; any other character types the accent (the value of `BASE` `space`) and then itself |
| `altgr` | The right Alt key is AltGr on this layout (otherwise it is Alt) |

- **Caps Lock** swaps the normal and shifted characters of a key whose two are a lower-case
  letter and its capital (the rule is in `lib/keymap`, for every letter in Unicode's Latin,
  Greek and Cyrillic blocks). **Num Lock** makes the keypad type digits; off, it is arrows and
  editing keys. Both are kept by the input service, across all keyboards.
- **Keys a layout doesn't mention** type nothing. Control keys (Enter, Tab, Backspace, Escape,
  Delete) are the same in every layout and not written in the files.
- **The parser** refuses the whole file for an unknown key name, a key given twice, a bad
  value, a `dead:` with no `dead` table, or a line too long, and the layout isn't offered; the
  reason is logged. It is host-tested with ordinary and hostile files, like `lib/manifest`.
- **M6's layouts:** `us` (the default), `gb` (UK, with the key between left Shift and Z, and
  `£`, `€` with AltGr), `us-intl` (US with dead keys for the apostrophe, quotation mark, circumflex, grave accent
  and tilde, and AltGr characters,
  covering Afrikaans and the other Western European languages). Adding one is adding a file.
- **New key codes** in `pbr.input` (a compatible addition, note 0008): `KEY_102ND` (the key
  beside left Shift on European keyboards), the keypad (`KEY_KP_0` to `KEY_KP_9`,
  `KEY_KP_PERIOD`, `KEY_KP_PLUS`, `KEY_KP_MINUS`, `KEY_KP_MULTIPLY`, `KEY_KP_DIVIDE`,
  `KEY_KP_ENTER`), `KEY_NUM_LOCK`, `KEY_SCROLL_LOCK`, `KEY_PRINT_SCREEN`, `KEY_PAUSE`,
  `KEY_MENU`; `AXIS_HWHEEL`; `EVENT_POSITION`; and `InputKind`'s `Tablet`. The PS/2 and virtio
  drivers learn them.

### 5. Choosing a layout

- Each user's layout is a setting in their settings file, read by the users service (note
  0024, section 5). When a session starts, and when the user changes it, the users service
  calls `set_layout` through its own badge. With one session at a time in M6, the layout is
  that session's.
- The layout changes between two events, never in the middle of a dead key (a dead key waiting
  is dropped).
- In text mode with no session yet, and when the setting names a layout that isn't there, the
  layout is `us`.

### 6. The console

- **The console no longer reads keyboards.** It looks up `input`, takes the seat, and uses the
  characters in key events; `lib/terminal`'s decoder and the US table in `lib/devices` go
  (the table moves into `layouts/us.kbd`, and a test checks the two give the same characters).
- It moves to `pbr.display` version 2 (note 0020): a buffer per screen, presenting the changed
  rows; its 4-bytes-a-pixel limit goes, since drivers convert.
- If the input service restarts, the console's watch on it fires (note 0019) and it takes the
  new one, as it already does with the display driver.

### 7. Absolute pointers in virtio-input

The virtio driver recognises a device with `ABS_X` and `ABS_Y` as a **tablet**, reads each axis'
range from the device's configuration, and posts positions scaled to 0 to 65535. (A touch
screen is a tablet that reports touching as the left button; several fingers wait for touch
support proper.) QEMU's `virtio-tablet-pci` can be tied to one screen of a display device
(`display=` and `head=`), which is how DESIGN.md's "a touch screen is tied to its own screen"
is tested: the display server learns which screen a tablet belongs to from its settings (note
0022, section 4).

### 8. Restarts

| What ended | What happens |
|------------|--------------|
| A keyboard or mouse driver | The input service finds it again (as the console does in M5), and sends a reset to its consumer, so no key is left held |
| The input service | Restarted by the root program. The consumer's watch fires (note 0019); it looks up `input` again and takes the seat |
| The consumer | Nothing: the next `take` gets the stream |

### 9. Testing

- **On the host**: `lib/keymap`, every line kind and every refusal; each layout's file against
  a table of what each key types with every modifier (the US one also against M5's table);
  dead keys (each table, an uncombinable letter, a dead key twice, space); Caps Lock and Num
  Lock; the merge and repeat logic with a fake clock (repeat timing, the last key repeating,
  modifiers not repeating, a key released during a loss); events encoded and decoded.
- **In QEMU** (the self-test boot, which runs in text mode): M5's shell test (test 8) passes with
  the console reading through the input service; the runner types a line with the `gb` and
  `us-intl` layouts set (through `pbr.users.debug`, note 0024) and checks the characters,
  including `£`, `ê` and `ë`; the virtio keyboard driver is crashed while a key is held and
  nothing stays held afterwards; the input service is stopped and the console takes the new one
  and keeps working; a tablet's positions arrive scaled; only the display server and the console
  can look up `input` as consumers, and the users service's badge can't take the seat.

## Alternatives considered

- **Layouts in every app** (as in M5's console). Every app would need every layout and the
  user's choice, and shortcuts would differ between apps.
- **Layouts in the display server.** The console needs them too, in text mode, without the
  display server.
- **Apps opening input devices.** The display server must decide who hears what; an app that
  could read the keyboard directly could read passwords typed into other windows.
- **Layouts compiled into the code** (a Rust table per layout). A text file can be read and
  checked by anyone, and adding a layout needs no rebuild of anything else.
- **XKB's format**, which Linux uses. Very large and complicated for what M6 needs; our format
  can be generated from it later if many layouts are wanted.

## Open questions

- Input methods for scripts that need them (Chinese, Japanese), and composing with a compose key.
- Several touches at once, gestures, pens and tablets with pressure.
- Per-keyboard layouts, and different layouts for different sessions once several people can be
  logged in.
- Changing the repeat delay and rate (a setting).
- Keyboard lights (Caps Lock, Num Lock).

## Changes
- 11 November 2026, built ahead of the rest (M6 stage 2): section 7, tablets in the virtio
  driver. In `pbr.input`'s 16-byte events (the drivers' rings) a position can't have the
  second field section 2's table gives the input service's events, so `EVENT_POSITION`
  carries both there: x in the low 16 bits of `value`, y in the high 16. The driver posts one
  position for each report from the device, and posts it before a button or wheel event of
  the same report, so a click arrives after the place it happened at. A tablet whose axis
  range is empty or backwards isn't started.
- 9 November 2026: first draft, for discussion with Chris; accepted by Chris the same day
  (he left the walk-through for later and asked for stage 0 to start).
