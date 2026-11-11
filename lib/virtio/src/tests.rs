// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

use crate::blk::{self, Block, Done, Error, Request};
use crate::input::{self as vinput, Kind, Tablet, check_range, scale};
use crate::model::{self, Bar, DMA_BUS, Dev, InputDev, Lie, Shared};
use crate::queue::{self, Buffer, Queue};
use crate::transport::{self, Window, find_layout};
use core::cell::RefCell;
use pbr_devices::regs::{Heap, Registers};
use std::rc::Rc;

const SECTORS: u64 = 1024;
const BAR_SIZES: [u64; 6] = [0, 0, 0, 0, model::BAR_SIZE as u64, 0];

type Disk = Block<Bar, Shared>;

fn start_with(
    dev: &Rc<RefCell<Dev>>,
    bar: Bar,
    dma: Shared,
    vector: Option<u16>,
) -> Result<Disk, Error> {
    let _ = dev;
    let (mut cfg, caps) = model::standard_caps();
    let layout = find_layout(&mut cfg, &caps, &BAR_SIZES).map_err(Error::Transport)?;
    Block::start(layout, [None, None, None, None, Some(bar), None], dma, DMA_BUS, vector)
}

fn disk() -> (Rc<RefCell<Dev>>, Disk) {
    let (dev, bar, dma) = model::make(SECTORS, blk::DMA_NEEDED);
    let d = start_with(&dev, bar, dma, Some(0)).expect("starts");
    (dev, d)
}

fn run(d: &mut Disk, r: Request<'_>) -> Result<Done, Error> {
    d.submit(r)?;
    // The model device completes during the notification, so one poll is enough.
    d.poll()?.ok_or(Error::Idle)
}

fn read(d: &mut Disk, sector: u64, count: u32) -> Result<Vec<u8>, Error> {
    match run(d, Request::Read { sector, count })? {
        Done::Read { bytes } => {
            let mut v = vec![0; bytes];
            d.data(&mut v);
            Ok(v)
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_layout_is_found_and_checked() {
    let (mut cfg, caps) = model::standard_caps();
    let l = find_layout(&mut cfg, &caps, &BAR_SIZES).unwrap();
    assert_eq!(l.common, Window { bar: 4, offset: 0, length: 56 });
    assert_eq!((l.notify_multiplier, l.notify.offset), (4, 0x300));
    assert_eq!(l.device, Some(Window { bar: 4, offset: 0x200, length: 16 }));
    // The BAR wasn't given to us.
    assert_eq!(find_layout(&mut cfg, &caps, &[0; 6]), Err(transport::Error::BadStructure));
    // The window runs off the end of the BAR.
    assert_eq!(
        find_layout(&mut cfg, &caps, &[0, 0, 0, 0, 0x2FF, 0]),
        Err(transport::Error::BadStructure)
    );
    // A missing structure.
    assert_eq!(find_layout(&mut cfg, &caps[1..], &BAR_SIZES), Err(transport::Error::Missing));
    // An offset that wraps round, a BAR that doesn't exist, a common structure that's too short,
    // an offset that isn't aligned.
    for (field, value, want) in [
        (2usize, u32::MAX - 3, transport::Error::BadStructure),
        (1, 9, transport::Error::BadStructure),
        (3, 20, transport::Error::BadStructure),
        (2, 2, transport::Error::BadStructure),
    ] {
        let (mut c, caps) = model::standard_caps();
        c.0[0x40 / 4 + field] = value;
        assert_eq!(find_layout(&mut c, &caps, &BAR_SIZES), Err(want), "field {field}");
    }
    // A notify capability without room for the multiplier.
    let (mut c, caps) = model::standard_caps();
    c.0[0x54 / 4] = 9 | (16 << 16) | (2 << 24);
    assert_eq!(find_layout(&mut c, &caps, &BAR_SIZES), Err(transport::Error::BadStructure));
    // A device with no device-specific configuration is fine to find.
    let (mut c, caps) = model::standard_caps();
    assert!(find_layout(&mut c, &caps[..3], &BAR_SIZES).unwrap().device.is_none());
}

#[test]
fn it_starts_and_says_what_it_is() {
    let (dev, d) = disk();
    assert_eq!((d.sectors(), d.read_only(), d.has_flush()), (SECTORS, false, true));
    let dev = dev.borrow();
    assert_eq!(dev.status, 1 | 2 | 8 | 4);
    assert_eq!(dev.drv_features, (1 << 32) | (1 << 9), "VERSION_1 and the flush it offered");
    assert_eq!((dev.q_size, dev.q_enable, dev.q_vector), (64, 1, 0));
}

#[test]
fn it_reads_and_writes() {
    let (dev, mut d) = disk();
    let expect =
        |s: u64, n: usize| dev.borrow().disk[s as usize * 512..s as usize * 512 + n * 512].to_vec();
    assert_eq!(read(&mut d, 0, 1).unwrap(), expect(0, 1));
    assert_eq!(read(&mut d, 5, 3).unwrap(), expect(5, 3));
    assert_eq!(read(&mut d, SECTORS - 128, 128).unwrap(), expect(SECTORS - 128, 128));
    assert_eq!(read(&mut d, SECTORS - 1, 1).unwrap(), expect(SECTORS - 1, 1));
    let data: Vec<u8> = (0..1024u32).map(|i| (i * 7) as u8).collect();
    assert_eq!(run(&mut d, Request::Write { sector: 10, data: &data }), Ok(Done::Written));
    assert_eq!(read(&mut d, 10, 2).unwrap(), data);
    assert_eq!(&dev.borrow().disk[10 * 512..12 * 512], &data[..]);
    assert_eq!(run(&mut d, Request::Flush), Ok(Done::Flushed));
    assert_eq!(dev.borrow().flushes, 1);
}

#[test]
fn bad_requests_are_refused_before_the_device_sees_them() {
    let (dev, mut d) = disk();
    let before = dev.borrow().requests;
    let sector = vec![0u8; 512];
    let ok = [
        (Request::Read { sector: 0, count: 0 }, Error::BadArgument),
        (Request::Read { sector: 0, count: 129 }, Error::BadArgument),
        (Request::Read { sector: SECTORS, count: 1 }, Error::OutOfRange),
        (Request::Read { sector: SECTORS - 1, count: 2 }, Error::OutOfRange),
        (Request::Read { sector: u64::MAX, count: 2 }, Error::OutOfRange),
        (Request::Write { sector: 0, data: &[] }, Error::BadArgument),
        (Request::Write { sector: 0, data: &sector[..100] }, Error::BadArgument),
        (Request::Write { sector: SECTORS, data: &sector }, Error::OutOfRange),
    ];
    for (r, e) in ok {
        assert_eq!(d.submit(r), Err(e), "{r:?}");
    }
    let big = vec![0u8; 513 * 128];
    assert_eq!(d.submit(Request::Write { sector: 0, data: &big }), Err(Error::BadArgument));
    assert_eq!(dev.borrow().requests, before);
    assert!(!d.busy());
    // One at a time.
    d.submit(Request::Read { sector: 0, count: 1 }).unwrap();
    assert_eq!(d.submit(Request::Read { sector: 0, count: 1 }), Err(Error::Busy));
    assert!(d.poll().unwrap().is_some());
    assert_eq!(d.poll(), Err(Error::Idle));
}

#[test]
fn read_only_and_no_flush() {
    let (dev, bar, dma) = model::make(SECTORS, blk::DMA_NEEDED);
    dev.borrow_mut().dev_features = (1 << 32) | blk::F_RO;
    let mut d = start_with(&dev, bar, dma, None).unwrap();
    assert!(d.read_only() && !d.has_flush());
    assert_eq!(d.submit(Request::Write { sector: 0, data: &[0; 512] }), Err(Error::ReadOnly));
    assert_eq!(d.submit(Request::Flush), Err(Error::BadArgument));
    assert!(read(&mut d, 0, 1).is_ok());
    assert_eq!(dev.borrow().q_vector, 0xFFFF, "no interrupts: the queue is polled");
}

#[test]
fn a_device_that_cannot_be_used_is_refused() {
    let try_start = |f: &dyn Fn(&mut Dev), vector| {
        let (dev, bar, dma) = model::make(SECTORS, blk::DMA_NEEDED);
        f(&mut dev.borrow_mut());
        start_with(&dev, bar, dma, vector).err()
    };
    use transport::Error as T;
    assert_eq!(try_start(&|_| {}, Some(0)), None);
    assert_eq!(try_start(&|d| d.dev_features = 1 << 9, None), Some(Error::Transport(T::NotModern)));
    assert_eq!(
        try_start(&|d| d.reject_features = true, None),
        Some(Error::Transport(T::FeaturesRejected))
    );
    assert_eq!(
        try_start(
            &|d| {
                d.hang_on_reset = true;
                d.status = 4
            },
            None
        ),
        Some(Error::Transport(T::ResetTimeout))
    );
    assert_eq!(try_start(&|d| d.no_vectors = true, Some(0)), Some(Error::Transport(T::NoVector)));
    assert_eq!(try_start(&|d| d.queue_max = 0, None), Some(Error::Transport(T::BadQueue)));
    assert_eq!(try_start(&|d| d.capacity = 0, None), Some(Error::Capacity));
    assert_eq!(try_start(&|d| d.capacity = u64::MAX, None), Some(Error::Capacity));
    assert_eq!(try_start(&|d| d.capacity = 1 << 62, None), Some(Error::Capacity));
    // Queue sizes: a device that allows fewer than we'd like, and one with an odd maximum.
    let (dev, bar, dma) = model::make(SECTORS, blk::DMA_NEEDED);
    dev.borrow_mut().queue_max = 100;
    let mut d = start_with(&dev, bar, dma, None).unwrap();
    assert_eq!(dev.borrow().q_size, 64);
    assert!(read(&mut d, 0, 1).is_ok());
    let (dev, bar, dma) = model::make(SECTORS, blk::DMA_NEEDED);
    dev.borrow_mut().queue_max = 8;
    let mut d = start_with(&dev, bar, dma, None).unwrap();
    assert_eq!(dev.borrow().q_size, 8);
    assert!(read(&mut d, 0, 1).is_ok());
    // Too little DMA memory.
    let (dev, bar, _) = model::make(SECTORS, blk::DMA_NEEDED);
    let small = Shared(Rc::new(Heap::new(blk::DMA_NEEDED - 4096)));
    assert_eq!(start_with(&dev, bar, small, None).err(), Some(Error::DmaMemory));
}

#[test]
fn every_lie_is_caught_and_the_queue_stays_broken_until_a_restart() {
    let cases = [
        (Lie::LongLength, Error::Queue(queue::Error::BadLength)),
        (Lie::BadId, Error::Queue(queue::Error::NotInFlight)),
        (Lie::Phantom, Error::Queue(queue::Error::TooManyUsed)),
        (Lie::Twice, Error::Queue(queue::Error::TooManyUsed)),
        (Lie::Stranger, Error::Queue(queue::Error::NotInFlight)),
        (Lie::IndexJump, Error::Queue(queue::Error::TooManyUsed)),
        (Lie::BadStatus, Error::BadCompletion),
        (Lie::ShortRead, Error::BadCompletion),
    ];
    for (lie, want) in cases {
        let (dev, bar, dma) = model::make(SECTORS, blk::DMA_NEEDED);
        let mut d = start_with(&dev, bar, dma, None).unwrap();
        dev.borrow_mut().lie = lie;
        let got = run(&mut d, Request::Read { sector: 3, count: 2 });
        // Phantom reports the never-made request first; IndexJump reports the true completion
        // first and is caught on the next look.
        let got = match (lie, got) {
            (Lie::IndexJump, Ok(_)) => run(&mut d, Request::Read { sector: 3, count: 2 }),
            (_, g) => g,
        };
        assert_eq!(got, Err(want), "{lie:?}");
        if matches!(want, Error::Queue(_)) {
            assert_eq!(
                d.submit(Request::Read { sector: 0, count: 1 }),
                Err(Error::Queue(queue::Error::Broken)),
                "{lie:?}"
            );
        }
    }
}

#[test]
fn a_restart_recovers() {
    let (dev, bar, dma) = model::make(SECTORS, blk::DMA_NEEDED);
    let bar = Rc::new(bar);
    let _ = bar;
    let mut d = start_with(
        &dev,
        Bar { dev: dev.clone(), plain: RefCell::new(vec![0; model::BAR_SIZE]) },
        dma,
        None,
    )
    .unwrap();
    dev.borrow_mut().lie = Lie::BadId;
    assert!(run(&mut d, Request::Read { sector: 0, count: 1 }).is_err());
    // A new driver instance starts from reset, over the same DMA memory.
    dev.borrow_mut().lie = Lie::None;
    let dma = Shared(dev.borrow().dma.clone());
    let mut d = start_with(
        &dev,
        Bar { dev: dev.clone(), plain: RefCell::new(vec![0; model::BAR_SIZE]) },
        dma,
        None,
    )
    .unwrap();
    assert!(read(&mut d, 0, 4).is_ok());
}

#[test]
fn indices_wrap_round() {
    let (dev, mut d) = disk();
    for i in 0..70_000u32 {
        // Flushes are the smallest requests; mix a few reads in so descriptors of all sorts cycle.
        let r = if i % 1000 == 0 {
            Request::Read { sector: u64::from(i % 1000), count: 1 }
        } else {
            Request::Flush
        };
        assert!(run(&mut d, r).is_ok(), "request {i}");
    }
    assert!(
        dev.borrow().used_idx < 5000 + 64,
        "the index went all the way round: {}",
        dev.borrow().used_idx
    );
}

#[test]
fn garbage_in_the_rings_is_survived() {
    let mut rng = 0x1234_5678_9abc_def0u64;
    let mut next = move || {
        rng ^= rng >> 12;
        rng ^= rng << 25;
        rng ^= rng >> 27;
        rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    for _ in 0..500 {
        let (dev, bar, dma) = model::make(SECTORS, blk::DMA_NEEDED);
        let mut d = start_with(&dev, bar, dma, None).unwrap();
        d.submit(Request::Read { sector: 0, count: 2 }).unwrap();
        // The device scribbles over the used ring and the descriptor area after the request.
        let (used, mem) = (dev.borrow().q_addr[2] - DMA_BUS, dev.borrow().dma.clone());
        for _ in 0..1 + next() % 6 {
            let at = used as usize + (next() % 500) as usize;
            mem.write8(at, next() as u8);
        }
        if let Ok(Some(Done::Read { bytes })) = d.poll() {
            assert_eq!(bytes, 1024);
        }
        // Whatever happened, a new request is answered or refused, never a panic.
        let _ = d.submit(Request::Read { sector: 1, count: 1 });
        let _ = d.poll();
    }
}

#[test]
fn the_queue_checks_what_it_is_asked() {
    let mem = Heap::new(1 << 16);
    assert!(Queue::new(&mem, 0, DMA_BUS, 3).is_none(), "not a power of two");
    assert!(Queue::new(&mem, 0, DMA_BUS, 0).is_none());
    assert!(Queue::new(&mem, 8, DMA_BUS, 8).is_none(), "misaligned");
    assert!(Queue::new(&mem, 1 << 16, DMA_BUS, 8).is_none(), "outside the memory");
    assert!(Queue::new(&mem, usize::MAX - 8, DMA_BUS, 8).is_none(), "wraps round");
    assert!(Queue::new(&Heap::new(100), 0, DMA_BUS, 8).is_none(), "too small");
    let layout = queue::Layout::new(8).unwrap();
    assert_eq!((layout.desc, layout.avail, layout.used), (0, 128, 128 + 4 + 16 + 2 + 2));
    let mut q = Queue::new(&mem, 0, DMA_BUS, 4).unwrap();
    let b = |len| Buffer { bus: 0x1000, len, device_writes: true };
    assert_eq!(q.push(&mem, &[]), Err(queue::Error::BadRequest));
    assert_eq!(q.push(&mem, &[b(0)]), Err(queue::Error::BadRequest));
    assert_eq!(
        q.push(&mem, &[b(1); 5]),
        Err(queue::Error::BadRequest),
        "more buffers than entries"
    );
    let h1 = q.push(&mem, &[b(16), b(16), b(16)]).unwrap();
    assert_eq!(q.push(&mem, &[b(1), b(1)]), Err(queue::Error::Full));
    let h2 = q.push(&mem, &[b(10)]).unwrap();
    assert_ne!(h1, h2);
    assert_eq!(q.in_flight(), 2);
    assert_eq!(q.pop(&mem), Ok(None));
    // The device completes the second, claiming exactly what it can hold.
    mem.write32(q.addresses().2 as usize - DMA_BUS as usize + 4, u32::from(h2));
    mem.write32(q.addresses().2 as usize - DMA_BUS as usize + 8, 10);
    mem.write16(q.addresses().2 as usize - DMA_BUS as usize + 2, 1);
    assert_eq!(q.pop(&mem), Ok(Some(queue::Completed { head: h2, written: 10 })));
    assert_eq!(q.in_flight(), 1);
    assert_eq!(q.pop(&mem), Ok(None));
    // Two in flight, and the device completes the same one twice: the second is a lie.
    let h3 = q.push(&mem, &[b(4)]).unwrap();
    let used = q.addresses().2 as usize - DMA_BUS as usize;
    for (i, id) in [h3, h3].into_iter().enumerate() {
        mem.write32(used + 4 + 8 * (1 + i), u32::from(id));
        mem.write32(used + 8 + 8 * (1 + i), 4);
    }
    mem.write16(used + 2, 3);
    assert_eq!(q.pop(&mem), Ok(Some(queue::Completed { head: h3, written: 4 })));
    // (The first request is still in flight, so one more completion is not too many; but it is
    // for a buffer that has already been completed.)
    assert_eq!(q.pop(&mem), Err(queue::Error::NotInFlight));
    assert!(q.is_broken());
    assert_eq!(q.push(&mem, &[b(1)]), Err(queue::Error::Broken));
    // With nothing else in flight, the repeat is more completions than requests.
    let mut q = Queue::new(&mem, 0, DMA_BUS, 4).unwrap();
    let (a, c) = (q.push(&mem, &[b(4)]).unwrap(), q.push(&mem, &[b(4)]).unwrap());
    for (i, id) in [a, a].into_iter().enumerate() {
        mem.write32(used + 4 + 8 * i, u32::from(id));
        mem.write32(used + 8 + 8 * i, 4);
    }
    mem.write16(used + 2, 2);
    assert!(q.pop(&mem).unwrap().is_some());
    assert_eq!(q.pop(&mem), Err(queue::Error::NotInFlight));
    let _ = c;
}

// ---- Input devices ----

type Pad = vinput::Input<Bar, Shared>;

/// Starts the driver against a model input device that says it is `personality`.
fn start_pad(personality: InputDev) -> Result<Pad, vinput::Error> {
    let (dev, bar, dma) = model::make(0, vinput::DMA_NEEDED);
    dev.borrow_mut().input = Some(personality);
    let (mut cfg, caps) = model::input_caps();
    let layout = find_layout(&mut cfg, &caps, &BAR_SIZES).map_err(vinput::Error::Transport)?;
    Pad::start(layout, [None, None, None, None, Some(bar), None], dma, DMA_BUS, Some(0))
}

const AXIS_RANGE: (i32, i32) = (0, 32767);

#[test]
fn a_keyboard_and_a_mouse_are_what_they_were() {
    let kb = start_pad(InputDev::keyboard()).expect("starts");
    assert_eq!(kb.kind(), Kind::Keyboard);
    assert_eq!((kb.name(), kb.abs_ranges()), ("QEMU Virtio Keyboard", None));
    let mouse = start_pad(InputDev::mouse()).expect("starts");
    assert_eq!((mouse.kind(), mouse.abs_ranges()), (Kind::Mouse, None));
    // Something with neither keys, movement nor axes is still refused.
    let nothing = InputDev { name: "nothing".into(), ..Default::default() };
    assert_eq!(start_pad(nothing).err(), Some(vinput::Error::Unsupported));
    // Only one of the two absolute axes is not a tablet: these are left as what they are.
    let one_axis = InputDev { abs_bits: vec![0b01], ..InputDev::mouse() };
    assert_eq!(start_pad(one_axis).map(|p| p.kind()).ok(), Some(Kind::Mouse));
    let only_y = InputDev { abs_bits: vec![0b10], ..InputDev::keyboard() };
    assert_eq!(start_pad(only_y).map(|p| p.kind()).ok(), Some(Kind::Keyboard));
}

#[test]
fn a_tablet_is_recognised_with_its_ranges() {
    // QEMU's tablet has buttons and a wheel too, and is still a tablet.
    let t = start_pad(InputDev::tablet(AXIS_RANGE, (-100, 4000))).expect("starts");
    assert_eq!(t.kind(), Kind::Tablet);
    assert_eq!(t.name(), "QEMU Virtio Tablet");
    assert_eq!(t.abs_ranges(), Some([(0, 32767), (-100, 4000)]));
    // A tablet that is also a keyboard (it has the letter A) is still a tablet.
    let mut both = InputDev::tablet(AXIS_RANGE, AXIS_RANGE);
    both.key_bits = InputDev::keyboard().key_bits;
    assert_eq!(start_pad(both).map(|p| p.kind()).ok(), Some(Kind::Tablet));
    // The widest possible ranges are fine.
    let wide = InputDev::tablet((i32::MIN, i32::MAX), (i32::MIN, i32::MIN + 1));
    assert_eq!(
        start_pad(wide).map(|p| p.abs_ranges()).ok(),
        Some(Some([(i32::MIN, i32::MAX), (i32::MIN, i32::MIN + 1)]))
    );
}

#[test]
fn a_tablet_that_lies_about_its_ranges_is_refused() {
    let refused = |t: InputDev| assert_eq!(start_pad(t).err(), Some(vinput::Error::BadRange));
    // Empty, backwards (as signed numbers: 0xFFFF_FFFF is -1), on either axis.
    refused(InputDev::tablet((5, 5), AXIS_RANGE));
    refused(InputDev::tablet(AXIS_RANGE, (9, 3)));
    refused(InputDev::tablet((0, -1), AXIS_RANGE));
    refused(InputDev::tablet((i32::MAX, i32::MIN), AXIS_RANGE));
    // An answer too short to hold a range, or no answer, for either axis.
    for axis in 0..2 {
        for len in [0, 1, 7] {
            let mut t = InputDev::tablet(AXIS_RANGE, AXIS_RANGE);
            t.abs_info[axis].truncate(len);
            refused(t);
        }
    }
    // Exactly 8 bytes is enough: the rest is not used.
    let mut t = InputDev::tablet(AXIS_RANGE, AXIS_RANGE);
    t.abs_info.iter_mut().for_each(|a| a.truncate(8));
    assert_eq!(start_pad(t).map(|p| p.kind()).ok(), Some(Kind::Tablet));
    // A configuration too short to hold the answer at all is a transport error, not a panic.
    let (dev, bar, dma) = model::make(0, vinput::DMA_NEEDED);
    dev.borrow_mut().input = Some(InputDev::tablet(AXIS_RANGE, AXIS_RANGE));
    let (mut cfg, caps) = model::standard_caps(); // 16 bytes
    let layout = find_layout(&mut cfg, &caps, &BAR_SIZES).unwrap();
    let r = Pad::start(layout, [None, None, None, None, Some(bar), None], dma, DMA_BUS, Some(0));
    assert_eq!(r.err(), Some(vinput::Error::Transport(transport::Error::OutOfRange)));
}

#[test]
fn the_range_check_is_exact() {
    assert_eq!(check_range(20, 0, 100), Some((0, 100)));
    assert_eq!(check_range(8, 0, 100), Some((0, 100)));
    assert_eq!(check_range(7, 0, 100), None);
    assert_eq!(check_range(0, 0, 100), None);
    assert_eq!(check_range(20, 100, 100), None);
    assert_eq!(check_range(20, 100, 99), None);
    assert_eq!(check_range(20, 0, u32::MAX), None, "the largest is -1, below 0");
    assert_eq!(check_range(20, u32::MAX, 0), Some((-1, 0)));
    assert_eq!(check_range(u8::MAX, i32::MIN as u32, i32::MAX as u32), Some((i32::MIN, i32::MAX)));
}

#[test]
fn scaling_covers_the_whole_axis() {
    // The ends, and outside them.
    assert_eq!(scale(0, 0, 32767), 0);
    assert_eq!(scale(32767, 0, 32767), 65535);
    assert_eq!(scale(-5, 0, 32767), 0);
    assert_eq!(scale(40000, 0, 32767), 65535);
    assert_eq!(scale(i32::MIN, 0, 32767), 0);
    assert_eq!(scale(i32::MAX, 0, 32767), 65535);
    // The middle, and rounding down.
    assert_eq!(scale(50, 0, 100), 32767); // 32767.5
    assert_eq!(scale(1, 0, 100), 655); // 655.35
    assert_eq!(scale(99, 0, 100), 64879); // 64879.65
    assert_eq!(scale(1, 0, 65535), 1);
    assert_eq!(scale(65534, 0, 65535), 65534);
    // A range that does not start at 0.
    assert_eq!(scale(-100, -100, 100), 0);
    assert_eq!(scale(0, -100, 100), 32767);
    assert_eq!(scale(100, -100, 100), 65535);
    // A range with two values.
    assert_eq!((scale(0, 0, 1), scale(1, 0, 1)), (0, 65535));
    // The most extreme ranges: no overflow.
    assert_eq!(scale(i32::MIN, i32::MIN, i32::MAX), 0);
    assert_eq!(scale(i32::MAX, i32::MIN, i32::MAX), 65535);
    assert_eq!(scale(-1, i32::MIN, i32::MAX), 32767);
    assert_eq!(scale(0, i32::MIN, i32::MAX), 32767);
    assert_eq!(scale(i32::MAX - 1, i32::MIN, i32::MAX), 65534);
    assert_eq!(scale(i32::MIN + 1, i32::MIN, i32::MAX), 0);
    assert_eq!(scale(0, i32::MIN, 0), 65535);
    assert_eq!(scale(-1, i32::MIN, 0), 65534);
    // An impossible range gives 0 instead of dividing by zero.
    assert_eq!(scale(5, 5, 5), 0);
    assert_eq!(scale(5, 9, 3), 0);
    // It never goes backwards as the value rises.
    let mut last = 0;
    for v in (-1000..=1000).step_by(7) {
        let now = scale(v, -900, 700);
        assert!(now >= last, "{v}");
        last = now;
    }
}

#[test]
fn a_tablet_gives_one_position_per_report() {
    let mut t = Tablet::new([(0, 100), (0, 200)]);
    assert_eq!(t.take(), None, "nothing yet");
    // Only one axis seen: no position, rather than a made-up other coordinate.
    t.abs(0, 100);
    assert_eq!(t.take(), None);
    t.abs(1, 100);
    assert_eq!(t.take(), Some((65535, 32767)));
    assert_eq!(t.take(), None, "already given");
    // One axis moved: the other keeps its latest value.
    t.abs(0, 0);
    assert_eq!(t.take(), Some((0, 32767)));
    // Other axes (pressure, say) are ignored.
    t.abs(2, 5);
    t.abs(0x18, 5);
    assert_eq!(t.take(), None);
    // Several changes in one report give only the last.
    t.abs(0, 10);
    t.abs(0, 20);
    t.abs(1, 200);
    assert_eq!(t.take(), Some((13107, 65535)));
    // Values outside the range are held at its ends.
    t.abs(0, -50);
    t.abs(1, 9999);
    assert_eq!(t.take(), Some((0, 65535)));
}
