# 0019: Kernel interface 1.3: turning the machine off, and noticing a service has gone

Status: Draft · 9 November 2026 · Related: DESIGN.md sections 6.1 and 8; notes 0006, 0007,
0008, 0013, 0018, 0022, 0024; milestone M6

## The problem

M6 needs two things from the kernel that interface version 1.2 can't do:

1. **Turning the machine off and restarting it.** PrettyBigRock 1.3 does both, and M6 must do
   everything 1.3 does (Chris's answer to question Q11 of the M6 plan). On ARM64 both go
   through **PSCI**, the firmware interface ARM machines use for power (the kernel already uses
   it to start the other cores), and PSCI can only be called from the kernel's privilege level.
   On x86-64, restarting can be done several ways, the last resort of which (a deliberate
   "triple fault") only the kernel can do. Turning an x86 PC off needs a value from the ACPI
   tables' bytecode, which is a job for a program (note 0024).
2. **Noticing that a service has gone without calling it.** After a service restarts, its old
   clients' capabilities fail with `PEER_GONE`, but only when they next call (note 0018,
   section 2). The users service learns of it by checking every 500 ms, which M5 left as an
   open question. In M6 every app holds a window from the display server (note 0022); if the
   display server restarts, every app must notice and reconnect, or end, at once, not at its
   next call (an app waiting for input makes no calls) and not by polling.

## The design

All of this is **interface version 1.3**, a compatible minor version (note 0007, section 7):
programs built for 1.0 to 1.2 run unchanged.

### 1. The machine capability and `machine_power`

- **A new object type, 12: Machine.** There is one, made by the kernel at start. It has one
  type-specific right, **`POWER`** (bit 8): turning the machine off or restarting it.
- **The root program gets it at start-up role 13**, with all rights. It passes it only to the
  power service (note 0024, section 7). Nothing else ever holds it.
- **New call 100, `machine_power(machine h, action)`**, needing `POWER`:

  | Action | Meaning |
  |--------|---------|
  | 1 | Turn the machine off |
  | 2 | Restart it |

  Any other action is `INVALID_ARGUMENT`. On success the call doesn't return.
- **The kernel saves nothing.** Whoever calls it must have saved everything first (the users
  service's `shutdown`, note 0017). The kernel writes `Turning off` or `Restarting` to its log,
  stops the other cores (an interrupt to each, after which it waits at most 100 ms), and then:

  | | Turn off | Restart |
  |--|---------|---------|
  | ARM64 | PSCI `SYSTEM_OFF` (function `0x8400_0008`), by `HVC` or `SMC` as the ACPI tables say (note 0005) | PSCI `SYSTEM_RESET` (`0x8400_0009`) |
  | x86-64 | `NOT_SUPPORTED`: done by the power service (note 0024) until the ACPI interpreter (M7) | In order, each tried if the one before didn't work within 100 ms: the FADT's reset register (ACPI 2.0 and later, when its flag says it is supported); port `0xCF9` (writing 2, then 6); the keyboard controller's reset line (`0xFE` to port `0x64`), if the FADT says there is one; a triple fault (an empty interrupt table, then a breakpoint) |

  Every refusal is decided before anything is stopped: `NOT_SUPPORTED` for turning off on
  x86-64, and for both actions on an ARM64 machine whose ACPI tables say it has no PSCI. Once
  the cores are stopped there is no way back: if PSCI returns instead of acting, or the last
  x86 method fails, the kernel stops with a message on the serial port, as it does for a fault
  of its own.
- **Why a capability, not a right on the kernel log or the process.** The kernel log's
  `CONTROL` already goes to the display driver (note 0010), which must not be able to turn the
  machine off, and principle 1 says there are no global powers. A kernel object of its own
  makes "who can turn the machine off" a question with one answer: whoever holds it.

### 2. Watching for an endpoint's receiver to go

- **New call 59, `endpoint_watch_receiver(endpoint h, notification h, bits)`**, needing `SEND`
  on the endpoint and `SIGNAL` on the notification: when no capability with `RECEIVE` to that
  endpoint is left anywhere, signal `bits` on the notification. If there is already none, the
  bits are signalled at once.
- **It fires once.** An endpoint that has lost its last receiver can never get one back (a
  receiving capability can only be copied from another one), so there is nothing more to say.
- **The watch belongs to the capability it was set through**, not to the endpoint. Closing,
  revoking or moving that capability removes its watch; copies of it don't have one. Setting a
  watch through a capability that already has one replaces it. So a client watches the service
  through exactly the capability it uses, and many clients can each watch the same endpoint.
- **Memory.** A watch is charged to the process that set it, as a notification is (note 0006,
  section 9), and the charge is returned when it fires or is removed.
- **Order.** A call waiting on the endpoint when the last receiver goes fails with `PEER_GONE`
  (as before) and the watch fires; which the client sees first isn't defined.
- **Using it.** A service that is also a client binds the notification to its own endpoint
  (`endpoint_bind`, call 56), so the news arrives as notification bits in its receive loop; an
  app waits on the notification beside its other events (note 0023, `lib/window`'s event loop).
  The users service replaces its 500 ms checks of the storage service and the console with it;
  apps use it for the display server.
- Existing calls are unchanged. `endpoint_watch` (call 57) is still the receiver's signal that
  its last *sender* has gone.

### 3. In `granite`

- `granite::machine::power(&Machine, Power::Off | Power::Restart) -> Result<Never, Error>`,
  with `Machine` a handle type of its own, given only by `start::machine()` (role 13).
- `Endpoint::watch_receiver(&Notification, bits) -> Result<(), Error>`.
- `handle::Type::Machine` for `handle_info`.

### 4. Numbers

| No. | Call | Arguments | Results | Needs |
|-----|------|-----------|---------|-------|
| 59 | `endpoint_watch_receiver` | endpoint h, notification h, bits | | `SEND`; `SIGNAL` on the notification |
| 100 | `machine_power` | machine h, action | (doesn't return) | `POWER` |

Also: object type 12 (Machine) and its right `POWER` (bit 8); the root program's start-up role
13. No new error codes.

### 5. Testing

- **In the program self-test** (all four configurations): `machine_power` through a copy
  without `POWER`, through a capability of another type, and with action 0 or 3 is refused, and
  turning off on x86-64 answers `NOT_SUPPORTED` with every core running afterwards (the
  self-test's thread check). `endpoint_watch_receiver`: fires when the last receiver closes its
  capability, when it is revoked, when its process ends, and at once on an endpoint with none;
  doesn't fire while one copy of the receiving capability is left; is removed with the client's
  capability (closed, revoked, moved), and its charge returned; replacing a watch; many clients
  watching one endpoint.
- **Turning off and restarting really happening** can't be checked from inside: the desktop
  boot (note 0026) ends by turning off, and one boot per configuration restarts, with QEMU told
  `-no-reboot` so that it exits instead of restarting. The runner tells the two apart by QEMU's
  `SHUTDOWN` event, whose reason is `guest-shutdown` for turning off and `guest-reset` for a
  restart.

## Alternatives considered

- **Power in a program on every architecture.** ARM's PSCI is reached by `HVC` or `SMC`, which
  programs can't use; it has to be the kernel there. On x86-64 the kernel offers restarting so
  that the triple fault is there as a last resort, and so that "restart" means one call on both.
- **Turning an x86 PC off in the kernel**, with 1.3's byte scan of the DSDT for `\_S5`. That
  scan is a guess at the ACPI bytecode, not a reading of it; it belongs in a replaceable program
  until M7 brings a real interpreter, and the kernel stays out of ACPI's bytecode for good.
- **Polling, as the users service does now.** Every client of every service would wake every
  few hundred milliseconds for nothing, and still be late.
- **A gone notice on the client's own endpoint** (like note 0013's receive kind 3). Apps don't
  all have endpoints, and a notification can be bound to an endpoint anyway, so a notification
  serves both.
- **A watch on the endpoint rather than on the capability.** Anyone holding a copy could then
  remove another program's watch, and nothing would remove a watch when its client dropped the
  capability.

## Open questions

- Sleep and waking (laptops, M11).
- The kernel stopping the other cores before turning off assumes nothing on them must finish
  first; it is the caller's job to save, but a device in the middle of a write (DMA) is stopped
  only by the power going.

## Changes
- 9 November 2026: first draft, for discussion with Chris.
