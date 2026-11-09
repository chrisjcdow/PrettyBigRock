# 0020: Screens: `pbr.display` version 2, the framebuffer driver and virtio-gpu

Status: Draft · 9 November 2026 · Related: DESIGN.md sections 8 and 11; notes 0010, 0011, 0012,
0018, 0021, 0022; milestone M6

## The problem

M4's `pbr.display` (note 0011, section 8) describes one screen and hands out its memory, and
the console draws straight into it. M6 needs more (DESIGN.md section 8, "several screens from
the start"):
- **several screens**, each with its own size, and appearing and going while the system runs;
- **a device that has no memory to draw in.** virtio-gpu, the QEMU device that can drive two or
  more screens, keeps the picture in the host: the guest draws in its own memory and then sends
  commands saying which parts to copy and show. Later GPU drivers work the same way;
- **screens that aren't 4 bytes a pixel** (M5 left it open: 1.3's VESA modes can be 24-bit, and
  Roxy's may be);
- **knowing when a picture is on the screen**, so the display server (note 0022) can draw the
  next one without tearing and without drawing faster than anyone can see. DESIGN.md asks for
  buffers with fences ("drawing is finished" signals) so that a GPU can take part later without
  a new interface.

This note settles `pbr.display` version 2, the framebuffer driver's changes, the new virtio-gpu
driver, and how the device manager decides which driver gets a screen.

## The design

### 1. The model: screens, buffers and presenting

- **A display device has screens**, numbered from 0 on that device. A screen is a connector
  that may or may not have something plugged in. The framebuffer driver has one; virtio-gpu has
  as many as QEMU was told (`max_outputs`, up to 16).
- **The client draws in buffers the driver makes.** A buffer is a memory object of 32-bit
  pixels (blue in the lowest byte, then green, red and one unused byte: `Bgrx`), made by the
  driver for one screen at one size and shared with the client. The client never touches the
  device's own memory.
- **Presenting** a buffer says which rectangles of it changed. The driver puts those parts on
  the screen: the framebuffer driver copies them into the device's memory, converting the pixel
  format; virtio-gpu sends its transfer and flush commands.
- **When it is done**, the driver signals the client's **done notification** (bit *n* for
  screen *n*). Until then the client must not change that buffer. This is the fence: with a
  GPU later, the same bit says the GPU has finished reading it.
- **One client at a time** may present (`take`, as `pbr.input`'s `open`, note 0011): taking
  over stops the previous client's buffers being shown, so a client that crashed can't hold the
  screens. In M6 the client is the display server, or the console in text mode (note 0024).

Why buffers belong to the driver: virtio-gpu reads them by DMA, so they must be the driver's
DMA memory, which belongs to the device manager (note 0011, section 6) and so stays safe if the
driver or the device manager dies. For the framebuffer driver they are ordinary memory, but the
client can't tell the difference.

### 2. The interface: `pbr.display` version 2

```
/// Screens. Served by display drivers.
interface pbr.display version 2;

struct Rect { x: u32, y: u32, width: u32, height: u32 }

struct Mode {
    width: u32,
    height: u32,
    /// Refreshes a second, times 1000; 0 if unknown.
    refresh_mhz: u32,
}

struct ScreenInfo {
    /// 0 up, on this device; the same for as long as the device is there.
    screen: u32,
    /// A name for the connector, the same at every start ("Virtual-1", "Firmware").
    connector: string<32>,
    /// From the screen's EDID (its manufacturer and serial number) where it has one, so that a
    /// remembered arrangement finds the same screen again; empty if not.
    identity: string<64>,
    /// Something is plugged in.
    connected: bool,
    /// Its picture's size in millimetres, 0 if unknown (for the scale factor, note 0022).
    width_mm: u32,
    height_mm: u32,
    /// The mode it is showing; all 0 if it is off.
    current: Mode,
    /// The mode it prefers (the first of `modes`), and others it can show.
    modes: list<Mode, 16>,
}

/// The screens, and a number that changes whenever anything in them does.
@1 fn screens() -> (screens: list<ScreenInfo, 16>, generation: u32) error DisplayError;

/// Become the client that presents. `done` gets bit n when screen n's last present is on the
/// screen, and bit 63 when `screens()` would answer differently. Taking over ends the previous
/// client's buffers.
@2 fn take() -> (done: handle<notification>) error DisplayError;

/// Shows `mode` on a screen (a mode from its list, or any size the device can show), or
/// turns it off (width 0). Buffers made for another size stop being shown.
@3 fn set_mode(screen: u32, mode: Mode) -> () error DisplayError;

/// A buffer for a screen at its current size, to map read and write. `stride` is the bytes
/// from one row to the next.
@4 fn create_buffer(screen: u32) -> (buffer: u32, memory: handle<memory>, stride: u32)
        error DisplayError;
@5 fn destroy_buffer(buffer: u32) -> () error DisplayError;

/// Shows a buffer on its screen, copying the parts in `changed` (all of it if empty). Answers
/// at once; `done` says when it is on the screen.
@6 fn present(buffer: u32, changed: list<Rect, 64>) -> () error DisplayError;

flexible enum DisplayError {
    NoScreen @1, NotConnected @2, BadMode @3, NoMemory @4, NoSuchBuffer @5,
    /// Another client has taken over, or this one never took.
    NotTaken @6,
    /// The previous present on that screen isn't done.
    Busy @7,
    BadArgument @8,
}
```

(The largest lists that fit are settled by the interface compiler when the file is written,
and recorded here.)

- **Version 2 replaces version 1.** Only the console used version 1; it moves to version 2
  (note 0021, section 6). Interfaces are versioned so that this is a deliberate step: version 1
  is retired, not kept beside it.
- Only `screens` may be asked by a client that hasn't taken over; everything else answers
  `NotTaken`. A client's buffers are destroyed when another takes over or when its capability
  goes (a watched badge, note 0013).
- **Buffers are limited by the driver's DMA allowance** (`NoMemory` past it), so a client can't
  make a driver use more than its manifest says.
- A present while the previous one on that screen isn't done answers `Busy`: there is never
  more than one picture waiting per screen, which is what keeps drawing in step with the
  screen.
- **Rotation is the client's.** The display server draws rotated pictures itself (note 0022);
  drivers only ever show buffers as they are. Hardware rotation can be added as a new field
  later without a new version.
- **The cursor is drawn into the picture** in M6. A hardware cursor (virtio-gpu has one) is a
  later addition.

### 3. The framebuffer driver

- One screen, `Firmware`, connected, with the one mode the firmware set (it can't change it);
  `set_mode` answers `BadMode` for anything else. Size in millimetres unknown.
- **Pixel formats it can show**: 32-bit `Bgrx` and `Rgbx`, and **24-bit** `Bgr` and `Rgb`
  (three bytes a pixel), converted while copying. The boot information (note 0003) already says
  which; the device record's format field gains the two 24-bit values (3 and 4).
- Buffers are ordinary memory in its budget; its manifest's budget becomes 40 MiB (two buffers
  of 2560 × 1600).
- It maps the device's memory write-combining (as the console did) and copies only the changed
  rectangles. Done is signalled as soon as the copy is finished.

### 4. The virtio-gpu driver

`drivers/virtio-gpu`, on `lib/virtio` like `virtio-blk` and `virtio-input`:

```
# drivers/virtio-gpu.drv
class display
match pci 1af4:1050
budget 4M
dma 48M
irqs 1
```

- **2D only** (no 3D, `VIRGL`, which needs a GPU interface of its own). It uses the control
  queue; the cursor queue waits for the hardware cursor.
- **Screens** from `GET_DISPLAY_INFO`; with the `EDID` feature, `GET_EDID` gives each one's
  size in millimetres, modes and identity. The connector names are `Virtual-1` upwards.
- **A buffer is a 2D resource** (`RESOURCE_CREATE_2D`, format `B8G8R8X8_UNORM`) with its
  pixels in the driver's DMA memory (`RESOURCE_ATTACH_BACKING`). Presenting is
  `SET_SCANOUT` (when the screen shows a different buffer from last time), then
  `TRANSFER_TO_HOST_2D` and `RESOURCE_FLUSH` for the changed rectangles, the last with a
  fence; done is signalled when the fence comes back.
- **Screens appearing and going**: the device's `EVENT_DISPLAY` interrupt; the driver reads
  the screens again, and signals bit 63.
- **The DMA allowance.** 48 MiB covers two screens of 1920 × 1200 with two buffers each (35 MiB)
  and the queues. The 4 MiB limit on a driver's DMA memory (note 0011, section 5) becomes
  64 MiB.

### 5. Who gets the screen

- **The kernel draws its messages** on the firmware's screen until a display driver takes it
  (note 0010). Every driver whose manifest says `class display` is given the kernel log with
  `CONTROL` (role 1050, as the framebuffer driver is now), and calls `console_screen_release`
  on its first `take`.
- **One device, one driver.** The firmware's screen may be drawn by a PCI device that has a
  driver of its own (a `virtio-vga`, or a real graphics card later). The device manager starts
  the framebuffer driver only if the firmware's screen memory is not inside a BAR of a PCI
  device that has a display driver; otherwise that driver has it, and the firmware screen is
  not listed. (QEMU's firmware draws on a `virtio-gpu-pci` device through commands, without
  memory to hand over, so with one the boot information has no framebuffer and the kernel's
  messages appear only on the serial port until the driver starts. That is fine and is tested.)
- **Several devices** each have their own driver and their own screens; the display server
  (note 0022) asks the device manager for every `display` device and puts all their screens on
  one desktop.

### 6. Crashes and restarts

As for every driver (note 0012): the device manager restarts it; the client's capability
fails with `PEER_GONE`, and its receiver watch fires (note 0019); the client asks the device
manager for the device again, takes it, makes new buffers and draws everything again. The old
buffers' memory stays safe until the device is stopped (note 0018, section 4).

### 7. Testing

- **On the host**: the framebuffer driver's copying and conversion, every format, rectangles
  at the edges and outside the buffer, and hostile sizes, as a library (`lib/devices`' style:
  the logic host-tested, the driver the glue); the virtio-gpu command encoding and the screens'
  and EDID's decoding, against `lib/virtio`'s model device extended with a 2D GPU, including a
  hostile device (screens past the maximum, EDID that lies, fences that never come back).
- **In QEMU** (the device self-test, all four configurations; which configuration has which
  device is note 0026, section 6). In self-test boots the root program runs this before it
  starts the console, so the two don't take the screens from each other. Every screen is listed with the right size; a known pattern
  (stripes and a rectangle at each corner) is presented on every screen, and the runner takes
  a screenshot of each screen through QEMU's control socket (`screendump` with `head`) and
  compares it with the pattern; only the changed rectangles change; a second client's `take`
  ends the first one's buffers; past the DMA allowance `create_buffer` is refused; a present
  before done answers `Busy`; the driver is crashed and restarted (note 0012's tools) and the
  pattern is presented again.
- **A screen going and coming.** If QEMU can unplug a virtio-gpu screen while running without
  a window (checked first, in stage 2), the runner does it. If not, the driver has a
  self-test-only operation, `pbr.display.debug.pretend` (like `pbr.devices.debug`), that makes
  it report a screen as unplugged and plugged in again, through the same path as the real
  event; the report to Chris will say which.

## Alternatives considered

- **Keeping version 1 and adding to it.** Its `framebuffer()` hands the device's memory to the
  client, which virtio-gpu doesn't have, and clients would have to support two ways of drawing.
- **The client making its own buffers.** virtio-gpu needs them as DMA memory, which only the
  device manager makes, and a client could revoke a buffer while the device reads it (note
  0006).
- **A buffer per screen, drawn in place.** Tearing (half old, half new pictures), and no way to
  know when it's safe to draw again.
- **The driver keeping time (a vertical blank interrupt).** virtio-gpu has none; a fence per
  present works for every device, and the display server paces itself from them.
- **One driver for virtio-gpu and the framebuffer** (the firmware screen as a special case of
  virtio-gpu). They have nothing in common below the interface.

## Open questions

- A hardware cursor (virtio-gpu's cursor queue; later GPUs).
- Hardware rotation and scaling, other pixel formats, higher depths (10-bit colour).
- 3D and GPU drawing: a separate interface, with the display server's buffers drawn by a GPU
  and fenced the same way (DESIGN.md section 8, "graphics acceleration").
- Variable refresh rates, and screens faster than 60 Hz.
- Brightness for laptop screens (M11).

## Changes
- 9 November 2026: first draft, for discussion with Chris.
