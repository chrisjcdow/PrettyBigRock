// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The interpreter for ACPI Machine Language, AML (docs/design/0028-acpi.md, section 2).
//!
//! **ACPI** is how a PC's firmware describes the machine to the operating system. Some of its
//! tables are fixed records, which the kernel reads itself; but the DSDT and SSDTs are
//! programs, in a bytecode called **AML**. Loading them builds a tree of named objects, the
//! **namespace** (`\_SB.PCI0.LPCB.UAR2` is a serial port under the chipset's LPC bridge under
//! the first PCI host bridge), and questions are asked by running small methods in it: a
//! device's `_CRS` returns the ports and interrupts it uses, a bridge's `_PRT` says how its
//! interrupt pins are wired, `\_S5` says how to turn the machine off.
//!
//! The bytecode is the firmware's, and it runs with the power to touch hardware, but to this
//! library it is **untrusted input**: whatever the bytes say, the interpreter never panics,
//! never indexes outside a slice, and never runs for ever. Every length is checked, recursion
//! is limited, and each evaluation has a budget of operations, time and memory ([`limits`]).
//!
//! The library knows nothing about PrettyBigRock. Everything it needs from the machine
//! (reading a register, sleeping, the time) comes through the [`Host`] trait, which the ACPI
//! service implements over its device access and the tests implement over a fake machine. So
//! it is built and tested on the host computer as well as in the system.
//!
//! The parts:
//! - `code.rs`: reading the bytecode (package lengths, names);
//! - `namespace.rs`: the tree of named objects;
//! - `value.rs`: the data AML computes with, and converting between kinds;
//! - `exec.rs` and `ops.rs`: running it;
//! - `field.rs`: operation regions and fields, through which AML reads and writes hardware;
//! - `skip.rs`: stepping over bytecode without running it;
//! - `resource.rs`: decoding the resource templates `_CRS` returns;
//! - `device.rs`: starting the namespace up, listing devices, PCI interrupt routing, `\_S5`.
//!
//! `#![no_std]` means the library uses only Rust's `core` and `alloc` libraries, not the
//! standard library (which needs an operating system underneath), so it builds for the ACPI
//! service as well as for the host. Its tests use `std`.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod code;
mod device;
mod exec;
mod field;
mod namespace;
mod op;
mod ops;
pub mod resource;
mod skip;
mod value;

#[cfg(test)]
mod tests;

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

pub use code::AmlName;
pub use device::{DeviceInfo, InitReport, LoadReport, Route, SleepType};
pub use namespace::NodeId;
pub use value::{Reference, Value};

/// The limits that keep bad bytecode from hanging or exhausting the service (note 0028,
/// section 2).
pub mod limits {
    /// How deep method calls may nest.
    pub const CALL_DEPTH: u32 = 64;
    /// How deep expressions may nest, counting every method's together. It keeps the
    /// interpreter's own recursion within the service's stack.
    pub const NESTING: u32 = 1024;
    /// Operations in one evaluation (a question asked, and every method it calls).
    pub const OPERATIONS: u64 = 10_000_000;
    /// Nanoseconds one evaluation may take by the host's clock: 30 seconds.
    pub const TIME: u64 = 30_000_000_000;
    /// Bytes of strings, buffers and packages one evaluation may create, and loading the
    /// tables may create.
    pub const MEMORY: usize = 16 << 20;
    /// The longest string or buffer, and the most elements in a package.
    pub const OBJECT: usize = 1 << 20;
    /// Named objects in the namespace.
    pub const NODES: usize = 200_000;
}

/// An access the host refused (the kernel or the service's own rules said no). The
/// interpreter carries on as Windows would: a read gives 0 and a write does nothing (note
/// 0028, section 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refused;

/// Where a PCI function is: the address its configuration space is reached by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PciAddress {
    /// The PCI segment (`_SEG`), 0 on most machines.
    pub segment: u16,
    /// The bus.
    pub bus: u8,
    /// The device on the bus, 0 to 31.
    pub device: u8,
    /// The function of the device, 0 to 7.
    pub function: u8,
}

/// What the interpreter needs from the machine. Widths are in bits: 8, 16, 32 or 64 (64 only
/// for memory; the interpreter splits wider I/O and PCI accesses itself).
///
/// A *trait* in Rust is a set of functions a type promises to provide, like an interface in
/// other languages; the interpreter is generic over it, so the service and the tests each
/// give it their own machine.
pub trait Host {
    /// Reads system memory at a physical address.
    fn read_memory(&mut self, address: u64, width: u8) -> core::result::Result<u64, Refused>;
    /// Writes system memory at a physical address.
    fn write_memory(
        &mut self,
        address: u64,
        width: u8,
        value: u64,
    ) -> core::result::Result<(), Refused>;
    /// Reads an I/O port (x86-64 only; elsewhere refused).
    fn read_io(&mut self, port: u16, width: u8) -> core::result::Result<u64, Refused>;
    /// Writes an I/O port.
    fn write_io(&mut self, port: u16, width: u8, value: u64) -> core::result::Result<(), Refused>;
    /// Reads a PCI function's configuration space.
    fn read_pci(
        &mut self,
        at: PciAddress,
        offset: u16,
        width: u8,
    ) -> core::result::Result<u64, Refused>;
    /// Writes a PCI function's configuration space.
    fn write_pci(
        &mut self,
        at: PciAddress,
        offset: u16,
        width: u8,
        value: u64,
    ) -> core::result::Result<(), Refused>;
    /// A clock that only goes forward, in nanoseconds.
    fn now(&mut self) -> u64;
    /// Waits about `ms` milliseconds, letting other programs run (`Sleep`).
    fn sleep(&mut self, ms: u64);
    /// Waits about `us` microseconds without giving up the processor (`Stall`).
    fn stall(&mut self, us: u64);
    /// Tries once to take the global lock the firmware shares with its hidden handler (note
    /// 0028, section 4); true if taken. The default, for a machine without one, always is.
    fn acquire_global_lock(&mut self) -> bool {
        true
    }
    /// Lets the global lock go.
    fn release_global_lock(&mut self) {}
    /// A message for the log. `verbose` ones are only wanted when asked for (`acpi=verbose`).
    fn log(&mut self, verbose: bool, message: fmt::Arguments);
    /// A `Notify` the firmware made: M7 logs it and does nothing more (note 0028, section 5).
    fn notify(&mut self, path: &str, value: u64) {
        self.log(false, format_args!("Notify({path}, {value:#x}) ignored"));
    }
}

/// Why an evaluation, or loading a table, failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The bytecode doesn't follow AML's grammar: cut short, an unknown opcode, a length
    /// running past its end.
    Malformed(&'static str),
    /// A name the bytecode used isn't in the namespace.
    NotFound(String),
    /// A name being created is already there.
    AlreadyExists(String),
    /// An object of the wrong kind for what was done with it.
    WrongType(&'static str),
    /// An index past the end of a buffer, string or package.
    OutOfRange,
    /// Division by zero.
    DivideByZero,
    /// One of the [`limits`]: which one.
    Limit(&'static str),
    /// The firmware asked for something M7 doesn't do (note 0028, section 9).
    NotSupported(&'static str),
    /// The firmware's `Fatal` statement, with its type, code and argument.
    Fatal(u8, u32, u64),
    /// Releasing a mutex that wasn't held, and the like.
    BadState(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Malformed(what) => write!(f, "malformed bytecode: {what}"),
            Error::NotFound(name) => write!(f, "{name} not found"),
            Error::AlreadyExists(name) => write!(f, "{name} already exists"),
            Error::WrongType(what) => write!(f, "wrong kind of object: {what}"),
            Error::OutOfRange => write!(f, "index out of range"),
            Error::DivideByZero => write!(f, "division by zero"),
            Error::Limit(which) => write!(f, "limit reached: {which}"),
            Error::NotSupported(what) => write!(f, "not supported: {what}"),
            Error::Fatal(t, c, a) => write!(f, "Fatal({t:#x}, {c:#x}, {a:#x})"),
            Error::BadState(what) => write!(f, "{what}"),
        }
    }
}

/// `Result` with this library's error. `Result<T, E>` is Rust's way of returning either a
/// value (`Ok`) or an error (`Err`); `?` after a call returns early with the error.
pub type Result<T> = core::result::Result<T, Error>;

/// The budget of the evaluation under way.
#[derive(Default)]
pub(crate) struct Run {
    /// Operations so far.
    pub ops: u64,
    /// When the evaluation must end, by [`Host::now`].
    pub deadline: u64,
    /// How deep the interpreter's recursion is.
    pub nesting: u32,
    /// How deep method calls are.
    pub calls: u32,
    /// Bytes of objects created.
    pub allocated: usize,
    /// How many times the global lock is held (`Acquire(\_GL)` may nest).
    pub global_lock: u32,
    /// Mutexes held, to let go of when the evaluation ends.
    pub mutexes: Vec<NodeId>,
}

/// The interpreter: the namespace, the host it reaches the machine through, and the state of
/// the evaluation under way. `H` is the host's type: the interpreter is *generic*, so each
/// host gets its own copy of the code, with no cost for the indirection.
pub struct Aml<H: Host> {
    pub(crate) ns: namespace::Namespace,
    pub(crate) host: H,
    /// Integers are 64 bits wide, or 32 if the DSDT's revision is below 2.
    pub(crate) wide: bool,
    pub(crate) run: Run,
    /// Regions already reported: refused, of a kind M7 doesn't reach, or (verbose) first used.
    pub(crate) reported: BTreeSet<(NodeId, u8)>,
    /// Whether every region's first access is logged (`acpi=verbose`).
    pub verbose: bool,
    /// Operations one evaluation may run: [`limits::OPERATIONS`], lower in tests that run
    /// thousands of broken tables.
    pub operations: u64,
}

impl<H: Host> Aml<H> {
    /// An empty namespace with the names the specification predefines (`\_SB`, `\_GPE`,
    /// `\_OSI` and the rest), using `host` to reach the machine.
    pub fn new(host: H) -> Self {
        let mut aml = Aml {
            ns: namespace::Namespace::new(),
            host,
            wide: true,
            run: Run::default(),
            reported: BTreeSet::new(),
            verbose: false,
            operations: limits::OPERATIONS,
        };
        aml.predefine();
        aml
    }

    /// The host, to look at what the interpreter did to it (the tests' fake machine).
    pub fn host(&mut self) -> &mut H {
        &mut self.host
    }

    /// True if integers are 64 bits wide.
    pub fn wide_integers(&self) -> bool {
        self.wide
    }

    /// How many named objects the namespace holds.
    pub fn objects(&self) -> usize {
        self.ns.len()
    }

    /// Evaluates the object at `path` (absolute, such as `\_SB.PCI0._CRS`) with `args`: runs
    /// it if it's a method, or reads it if it's data. A method's result is a [`Value`], or
    /// [`Value::Uninitialized`] if it returned nothing.
    pub fn evaluate(&mut self, path: &str, args: &[Value]) -> Result<Value> {
        let name = AmlName::parse(path).ok_or(Error::Malformed("not a name"))?;
        let root = self.ns.root();
        let node = self.ns.lookup(root, &name).ok_or_else(|| Error::NotFound(path.into()))?;
        self.evaluate_node(node, args)
    }

    /// The same for a node already found.
    pub fn evaluate_node(&mut self, node: NodeId, args: &[Value]) -> Result<Value> {
        self.begin();
        let result = self.call_or_read(node, args).and_then(|v| self.resolved(v));
        self.end();
        result
    }

    /// Finds `path` relative to `scope` (absolute if `scope` is `None`), without the search
    /// rules: the node exactly there.
    pub fn find(&self, scope: Option<NodeId>, path: &str) -> Option<NodeId> {
        let name = AmlName::parse(path)?;
        self.ns.lookup_exact(scope.unwrap_or(self.ns.root()), &name)
    }

    /// A node's full name, such as `\_SB.PCI0`.
    pub fn path(&self, node: NodeId) -> String {
        self.ns.path(node)
    }

    /// Starts an evaluation's budget.
    pub(crate) fn begin(&mut self) {
        let now = self.host.now();
        self.run = Run { deadline: now.saturating_add(limits::TIME), ..Run::default() };
    }

    /// Ends one: lets go of whatever the firmware's methods left held.
    pub(crate) fn end(&mut self) {
        for m in core::mem::take(&mut self.run.mutexes) {
            if let Some(namespace::Object::Mutex(state)) = self.ns.object_mut(m) {
                state.depth = 0;
            }
        }
        if self.run.global_lock > 0 {
            self.host.release_global_lock();
            self.run.global_lock = 0;
        }
    }
}
