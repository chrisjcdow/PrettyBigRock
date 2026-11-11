// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Tests of the GPU driver (`gpu.rs`) against the model GPU (`model_gpu.rs`), including a model
//! that lies.

use crate::gpu::{
    self, Command, Completion, DeviceError, Error, F_EDID, Gpu, MAX_SCANOUTS, Rect, Response,
    SLOTS, Scanout, error_code,
};
use crate::model::{self, Bar, DMA_BUS, Dev, Lie, Shared};
use crate::model_gpu::GpuLie;
use crate::queue;
use crate::transport::{self, find_layout};
use core::cell::RefCell;
use pbr_devices::regs::{Heap, Registers};
use std::rc::Rc;

const BAR_SIZES: [u64; 6] = [0, 0, 0, 0, model::BAR_SIZE as u64, 0];
/// Where the backing memory is on the bus (apart from the queue's memory).
const BACKING_BUS: u64 = 0x8000_0000;

type TestGpu = Gpu<Bar, Shared>;

fn start_with(bar: Bar, dma: Shared, vector: Option<u16>) -> Result<TestGpu, Error> {
    let (mut cfg, caps) = model::standard_caps();
    let layout = find_layout(&mut cfg, &caps, &BAR_SIZES).map_err(Error::Transport)?;
    Gpu::start(layout, [None, None, None, None, Some(bar), None], dma, DMA_BUS, vector)
}

fn gpu_with(scanouts: u32) -> (Rc<RefCell<Dev>>, TestGpu) {
    let (dev, bar, dma) = model::make_gpu(scanouts, gpu::DMA_NEEDED);
    let g = start_with(bar, dma, Some(0)).expect("starts");
    (dev, g)
}

fn gpu() -> (Rc<RefCell<Dev>>, TestGpu) {
    gpu_with(2)
}

/// Sends a command and takes its completion. The model device does everything while the driver
/// notifies it, so one poll is enough.
fn run(g: &mut TestGpu, cmd: Command) -> Result<Completion, Error> {
    g.submit(7, cmd)?;
    Ok(g.poll()?.expect("the model answered"))
}

fn done(g: &mut TestGpu, cmd: Command) {
    assert_eq!(run(g, cmd).unwrap().result, Ok(Response::Done), "{cmd:?}");
}

fn rect(x: u32, y: u32, width: u32, height: u32) -> Rect {
    Rect { x, y, width, height }
}

/// The pixel at (x, y) of the test picture, as the guest wrote it.
fn pixel(x: u32, y: u32) -> [u8; 4] {
    [x as u8 + 1, y as u8 + 1, 0xA5, 0]
}

/// Memory the device can reach, of `bytes` bytes, holding the test picture `width` pixels wide.
fn backing(dev: &Rc<RefCell<Dev>>, width: u32, height: u32) -> Rc<Heap> {
    let mem = dev.borrow_mut().gpu.as_mut().unwrap().add_region(BACKING_BUS, 8192);
    for y in 0..height {
        for x in 0..width {
            mem.write_bytes(((y * width + x) * 4) as usize, &pixel(x, y));
        }
    }
    mem
}

/// A resource of `w` by `h` with the test picture as backing, shown on scanout 0.
fn shown(dev: &Rc<RefCell<Dev>>, g: &mut TestGpu, w: u32, h: u32) -> Rc<Heap> {
    let mem = backing(dev, w, h);
    done(g, Command::CreateResource { resource: 1, width: w, height: h });
    done(g, Command::AttachBacking { resource: 1, address: BACKING_BUS, len: w * h * 4 });
    done(g, Command::SetScanout { scanout: 0, resource: 1, rect: rect(0, 0, w, h) });
    mem
}

fn picture(dev: &Rc<RefCell<Dev>>, scanout: usize) -> Vec<u8> {
    dev.borrow().gpu.as_ref().unwrap().scanouts[scanout].picture.clone()
}

/// What the screen should hold when only `rects` of the test picture have been copied over.
fn expected(w: u32, h: u32, rects: &[Rect]) -> Vec<u8> {
    let mut out = vec![0u8; (w * h * 4) as usize];
    for r in rects {
        for y in r.y..r.y + r.height {
            for x in r.x..r.x + r.width {
                let at = ((y * w + x) * 4) as usize;
                out[at..at + 4].copy_from_slice(&pixel(x, y));
            }
        }
    }
    out
}

fn transfer(g: &mut TestGpu, w: u32, r: Rect) {
    let offset = u64::from((r.y * w + r.x) * 4);
    done(g, Command::Transfer { resource: 1, rect: r, offset });
}

#[test]
fn it_starts_and_says_what_it_is() {
    let (dev, g) = gpu_with(3);
    assert_eq!((g.scanouts(), g.has_edid(), g.capacity(), g.free_slots()), (3, true, SLOTS, SLOTS));
    let d = dev.borrow();
    assert_eq!(d.status, 1 | 2 | 8 | 4);
    assert_eq!(d.drv_features, (1 << 32) | F_EDID);
    assert_eq!((d.q_size, d.q_enable, d.q_vector, d.config_vector), (64, 1, 0, 0));
}

#[test]
fn it_never_accepts_features_it_does_not_use() {
    // 3D (virgl, bit 0), blobs (bit 3) and context types (bit 5), as well as `EDID`.
    let (dev, bar, dma) = model::make_gpu(1, gpu::DMA_NEEDED);
    dev.borrow_mut().dev_features = (1 << 32) | 1 | (1 << 1) | (1 << 2) | (1 << 3) | (1 << 5);
    let g = start_with(bar, dma, None).unwrap();
    assert_eq!(dev.borrow().drv_features, (1 << 32) | F_EDID);
    assert!(g.has_edid());
    // Without `EDID` offered, there is no `GET_EDID`.
    let (dev, bar, dma) = model::make_gpu(1, gpu::DMA_NEEDED);
    dev.borrow_mut().dev_features = 1 << 32;
    let mut g = start_with(bar, dma, None).unwrap();
    assert_eq!(dev.borrow().drv_features, 1 << 32);
    assert!(!g.has_edid());
    assert_eq!(g.submit(1, Command::GetEdid { scanout: 0 }), Err(Error::Unsupported));
    assert!(dev.borrow().gpu.as_ref().unwrap().log.is_empty());
}

#[test]
fn a_device_that_cannot_be_used_is_refused() {
    use transport::Error as T;
    let try_start = |f: &dyn Fn(&mut Dev), vector| {
        let (dev, bar, dma) = model::make_gpu(2, gpu::DMA_NEEDED);
        f(&mut dev.borrow_mut());
        start_with(bar, dma, vector).err()
    };
    assert_eq!(try_start(&|_| {}, Some(0)), None);
    assert_eq!(try_start(&|d| d.dev_features = F_EDID, None), Some(Error::Transport(T::NotModern)));
    assert_eq!(
        try_start(&|d| d.reject_features = true, None),
        Some(Error::Transport(T::FeaturesRejected))
    );
    assert_eq!(try_start(&|d| d.no_vectors = true, Some(0)), Some(Error::Transport(T::NoVector)));
    assert_eq!(try_start(&|d| d.queue_max = 0, None), Some(Error::Transport(T::BadQueue)));
    // The number of scanouts: none, the most, and one too many.
    let scanouts = |n: usize| {
        move |d: &mut Dev| {
            let g = d.gpu.as_mut().unwrap();
            g.scanouts.resize(n, Default::default());
        }
    };
    assert_eq!(try_start(&scanouts(0), None), Some(Error::Scanouts));
    assert_eq!(try_start(&scanouts(MAX_SCANOUTS as usize), None), None);
    assert_eq!(try_start(&scanouts(MAX_SCANOUTS as usize + 1), None), Some(Error::Scanouts));
    assert_eq!(try_start(&scanouts(1000), None), Some(Error::Scanouts));
    // A queue too small to hold a present's commands.
    assert_eq!(try_start(&|d| d.queue_max = 8, None), Some(Error::QueueTooSmall));
    assert_eq!(try_start(&|d| d.queue_max = 15, None), Some(Error::QueueTooSmall));
    assert_eq!(try_start(&|d| d.queue_max = 16, None), None);
    // Too little DMA memory, or none that is aligned.
    let (_, bar, _) = model::make_gpu(1, gpu::DMA_NEEDED);
    let small = Shared(Rc::new(Heap::new(gpu::DMA_NEEDED - 4096)));
    assert_eq!(start_with(bar, small, None).err(), Some(Error::DmaMemory));
}

#[test]
fn a_smaller_queue_means_fewer_commands_at_once() {
    let (dev, bar, dma) = model::make_gpu(1, gpu::DMA_NEEDED);
    dev.borrow_mut().queue_max = 20; // rounds down to 16 entries: 8 commands
    let mut g = start_with(bar, dma, None).unwrap();
    assert_eq!((dev.borrow().q_size, g.capacity()), (16, 8));
    for i in 0..8 {
        g.submit(i, Command::GetDisplayInfo).unwrap();
    }
    assert_eq!(g.submit(9, Command::GetDisplayInfo), Err(Error::PoolFull));
    assert_eq!((g.free_slots(), g.in_flight()), (0, 8));
    assert!(g.poll().unwrap().is_some());
    g.submit(9, Command::GetDisplayInfo).unwrap();
}

#[test]
fn display_information_and_edids_are_read() {
    let (dev, mut g) = gpu_with(3);
    {
        let mut d = dev.borrow_mut();
        let m = d.gpu.as_mut().unwrap();
        m.scanouts[1].enabled = false;
        m.scanouts[2].width = 1920;
        m.scanouts[2].height = 1080;
        m.scanouts[2].edid = (0..128u8).collect();
    }
    let c = run(&mut g, Command::GetDisplayInfo).unwrap();
    assert_eq!(c.token, 7);
    let Ok(Response::DisplayInfo(info)) = c.result else { panic!("{c:?}") };
    let want = |w, h, enabled| Scanout { rect: rect(0, 0, w, h), enabled };
    assert_eq!(info, vec![want(1024, 768, true), want(1024, 768, false), want(1920, 1080, true)]);
    let c = run(&mut g, Command::GetEdid { scanout: 2 }).unwrap();
    assert_eq!(c.result, Ok(Response::Edid((0..128u8).collect())));
    // A scanout with no EDID gets an empty answer; one that doesn't exist is refused by the
    // driver before the device sees it.
    assert_eq!(
        run(&mut g, Command::GetEdid { scanout: 0 }).unwrap().result,
        Ok(Response::Edid(vec![]))
    );
    let sent = dev.borrow().gpu.as_ref().unwrap().log.len();
    assert_eq!(g.submit(1, Command::GetEdid { scanout: 3 }), Err(Error::BadArgument));
    assert_eq!(dev.borrow().gpu.as_ref().unwrap().log.len(), sent);
}

#[test]
fn the_happy_path_puts_exactly_the_transferred_pixels_on_the_screen() {
    let (dev, mut g) = gpu();
    let mem = shown(&dev, &mut g, 8, 4);
    assert_eq!(picture(&dev, 0), expected(8, 4, &[]), "nothing yet: the resource is blank");
    // One rectangle, transferred and flushed (with a fence).
    let r1 = rect(2, 1, 3, 2);
    transfer(&mut g, 8, r1);
    assert_eq!(picture(&dev, 0), expected(8, 4, &[]), "a transfer alone shows nothing");
    g.submit(40, Command::Flush { resource: 1, rect: r1, fenced: true }).unwrap();
    let c = g.poll().unwrap().unwrap();
    assert_eq!((c.token, c.result), (40, Ok(Response::Done)));
    assert_eq!(picture(&dev, 0), expected(8, 4, &[r1]));
    assert_eq!(dev.borrow().gpu.as_ref().unwrap().fences, vec![1]);
    // The guest draws somewhere else; flushing without transferring changes nothing.
    mem.write_bytes(0, &[9, 9, 9, 9]);
    done(&mut g, Command::Flush { resource: 1, rect: rect(0, 0, 8, 4), fenced: false });
    assert_eq!(picture(&dev, 0), expected(8, 4, &[r1]));
    // A second rectangle that touches the first's edge, and a flush of both at once.
    let r2 = rect(5, 0, 3, 4);
    transfer(&mut g, 8, r2);
    g.submit(41, Command::Flush { resource: 1, rect: rect(2, 0, 6, 4), fenced: true }).unwrap();
    assert_eq!(g.poll().unwrap().unwrap().token, 41);
    assert_eq!(picture(&dev, 0), expected(8, 4, &[r1, r2]));
    assert_eq!(dev.borrow().gpu.as_ref().unwrap().fences, vec![1, 2], "each fence is new");
    // The other scanout was never touched.
    assert!(picture(&dev, 1).is_empty());
    assert_eq!(dev.borrow().gpu.as_ref().unwrap().scanouts[1].flushes, 0);
    // Everything, including the pixel the guest changed (which was in the first column of the
    // picture, outside both rectangles until now).
    let all = rect(0, 0, 8, 4);
    transfer(&mut g, 8, all);
    done(&mut g, Command::Flush { resource: 1, rect: all, fenced: false });
    let mut whole = expected(8, 4, &[all]);
    whole[0..4].copy_from_slice(&[9, 9, 9, 9]);
    assert_eq!(picture(&dev, 0), whole);
}

#[test]
fn a_scanout_shows_a_resource_until_it_is_told_otherwise() {
    let (dev, mut g) = gpu();
    shown(&dev, &mut g, 8, 4);
    let all = rect(0, 0, 8, 4);
    transfer(&mut g, 8, all);
    done(&mut g, Command::Flush { resource: 1, rect: all, fenced: false });
    let first = picture(&dev, 0);
    assert_eq!(first, expected(8, 4, &[all]));
    // Freeing the resource the scanout shows blanks the screen: this is why the driver keeps
    // a shown resource alive until the scanout has another.
    done(&mut g, Command::Unref { resource: 1 });
    assert!(picture(&dev, 0).is_empty());
    // Showing another resource, then turning the scanout off.
    done(&mut g, Command::CreateResource { resource: 2, width: 8, height: 4 });
    done(&mut g, Command::SetScanout { scanout: 0, resource: 2, rect: all });
    assert_eq!(picture(&dev, 0), expected(8, 4, &[]));
    done(&mut g, Command::SetScanout { scanout: 0, resource: 0, rect: all });
    assert!(picture(&dev, 0).is_empty());
    // Detaching backing leaves the resource's pixels where they were.
    done(&mut g, Command::CreateResource { resource: 3, width: 8, height: 4 });
    backing(&dev, 8, 4);
    done(&mut g, Command::AttachBacking { resource: 3, address: BACKING_BUS, len: 128 });
    done(&mut g, Command::SetScanout { scanout: 1, resource: 3, rect: all });
    let offset = 0;
    done(&mut g, Command::Transfer { resource: 3, rect: all, offset });
    done(&mut g, Command::DetachBacking { resource: 3 });
    done(&mut g, Command::Flush { resource: 3, rect: all, fenced: false });
    assert_eq!(picture(&dev, 1), expected(8, 4, &[all]));
    let c = run(&mut g, Command::Transfer { resource: 3, rect: all, offset }).unwrap();
    assert_eq!(c.result, Err(DeviceError(error_code::UNSPEC)), "no backing to copy from");
}

#[test]
fn every_command_can_be_refused_and_the_device_stays_usable() {
    let (dev, mut g) = gpu();
    shown(&dev, &mut g, 8, 4);
    let all = rect(0, 0, 8, 4);
    let cases = [
        (0x0100, Command::GetDisplayInfo),
        (0x010A, Command::GetEdid { scanout: 0 }),
        (0x0101, Command::CreateResource { resource: 5, width: 4, height: 4 }),
        (0x0102, Command::Unref { resource: 1 }),
        (0x0103, Command::SetScanout { scanout: 0, resource: 1, rect: all }),
        (0x0104, Command::Flush { resource: 1, rect: all, fenced: true }),
        (0x0105, Command::Transfer { resource: 1, rect: all, offset: 0 }),
        (0x0106, Command::AttachBacking { resource: 1, address: BACKING_BUS, len: 128 }),
        (0x0107, Command::DetachBacking { resource: 1 }),
    ];
    for (n, (kind, cmd)) in cases.into_iter().enumerate() {
        for code in [error_code::UNSPEC, error_code::OUT_OF_MEMORY, error_code::INVALID_PARAMETER] {
            dev.borrow_mut().gpu.as_mut().unwrap().refuse = vec![(kind, code)];
            let c = run(&mut g, cmd).unwrap();
            assert_eq!(c.result, Err(DeviceError(code)), "{cmd:?}");
            assert_eq!(c.token, 7);
            assert!(!g.is_broken(), "{cmd:?}");
        }
        dev.borrow_mut().gpu.as_mut().unwrap().refuse.clear();
        // Nothing happened to the resource the refused commands were about, and an ordinary
        // command still works.
        assert!(dev.borrow().gpu.as_ref().unwrap().resources.contains_key(&1), "case {n}");
        assert!(run(&mut g, Command::GetDisplayInfo).unwrap().result.is_ok());
    }
}

#[test]
fn the_devices_own_errors_come_back_as_errors() {
    let (dev, mut g) = gpu();
    let all = rect(0, 0, 8, 4);
    let code = |g: &mut TestGpu, cmd| match run(g, cmd).unwrap().result {
        Err(DeviceError(c)) => c,
        other => panic!("{cmd:?} gave {other:?}"),
    };
    // Things that don't exist.
    assert_eq!(code(&mut g, Command::Unref { resource: 9 }), error_code::INVALID_RESOURCE_ID);
    assert_eq!(
        code(&mut g, Command::DetachBacking { resource: 9 }),
        error_code::INVALID_RESOURCE_ID
    );
    assert_eq!(
        code(&mut g, Command::SetScanout { scanout: 0, resource: 9, rect: all }),
        error_code::INVALID_RESOURCE_ID
    );
    assert_eq!(
        code(&mut g, Command::Flush { resource: 9, rect: all, fenced: true }),
        error_code::INVALID_RESOURCE_ID
    );
    assert_eq!(
        code(&mut g, Command::Transfer { resource: 9, rect: all, offset: 0 }),
        error_code::INVALID_RESOURCE_ID
    );
    // A resource that exists twice, and a rectangle off the resource.
    done(&mut g, Command::CreateResource { resource: 1, width: 8, height: 4 });
    assert_eq!(
        code(&mut g, Command::CreateResource { resource: 1, width: 8, height: 4 }),
        error_code::INVALID_RESOURCE_ID
    );
    assert_eq!(
        code(&mut g, Command::SetScanout { scanout: 0, resource: 1, rect: rect(4, 0, 8, 4) }),
        error_code::INVALID_PARAMETER
    );
    assert_eq!(
        code(&mut g, Command::Flush { resource: 1, rect: rect(0, 0, 9, 4), fenced: false }),
        error_code::INVALID_PARAMETER
    );
    // Backing: outside any memory the device can reach, and twice.
    backing(&dev, 8, 4);
    assert_eq!(
        code(&mut g, Command::AttachBacking { resource: 1, address: 0x1000, len: 128 }),
        error_code::UNSPEC
    );
    assert_eq!(
        code(
            &mut g,
            Command::AttachBacking { resource: 1, address: BACKING_BUS + 8192 - 64, len: 128 }
        ),
        error_code::UNSPEC
    );
    done(&mut g, Command::AttachBacking { resource: 1, address: BACKING_BUS, len: 128 });
    assert_eq!(
        code(&mut g, Command::AttachBacking { resource: 1, address: BACKING_BUS, len: 128 }),
        error_code::UNSPEC
    );
    // A transfer from an offset that runs off the end of the backing changes nothing.
    assert_eq!(
        code(&mut g, Command::Transfer { resource: 1, rect: all, offset: 4 }),
        error_code::INVALID_PARAMETER
    );
    assert!(dev.borrow().gpu.as_ref().unwrap().resources[&1].pixels.iter().all(|b| *b == 0));
    assert!(!g.is_broken());
}

#[test]
fn commands_with_impossible_numbers_are_refused_before_the_device_sees_them() {
    let (dev, mut g) = gpu();
    let sent = |dev: &Rc<RefCell<Dev>>| dev.borrow().gpu.as_ref().unwrap().log.len();
    let ok = rect(0, 0, 8, 4);
    let max = gpu::MAX_SCANOUTS;
    let bad = [
        Command::GetEdid { scanout: 2 },
        Command::GetEdid { scanout: u32::MAX },
        Command::CreateResource { resource: 0, width: 8, height: 4 },
        Command::CreateResource { resource: 1, width: 0, height: 4 },
        Command::CreateResource { resource: 1, width: 8, height: 0 },
        Command::CreateResource { resource: 1, width: 16385, height: 4 },
        Command::CreateResource { resource: 1, width: 4, height: u32::MAX },
        Command::Unref { resource: 0 },
        Command::SetScanout { scanout: 2, resource: 1, rect: ok },
        Command::SetScanout { scanout: max, resource: 0, rect: ok },
        Command::SetScanout { scanout: 0, resource: 1, rect: rect(0, 0, 0, 4) },
        Command::SetScanout { scanout: 0, resource: 1, rect: rect(0, 0, 8, 0) },
        Command::SetScanout { scanout: 0, resource: 1, rect: rect(u32::MAX, 0, 8, 4) },
        Command::Flush { resource: 0, rect: ok, fenced: false },
        Command::Flush { resource: 1, rect: rect(0, 0, 0, 0), fenced: true },
        Command::Flush { resource: 1, rect: rect(1, 1, u32::MAX, 1), fenced: true },
        Command::Transfer { resource: 0, rect: ok, offset: 0 },
        Command::Transfer { resource: 1, rect: rect(0, u32::MAX, 1, 1), offset: 0 },
        Command::AttachBacking { resource: 0, address: BACKING_BUS, len: 128 },
        Command::AttachBacking { resource: 1, address: BACKING_BUS, len: 0 },
        Command::AttachBacking { resource: 1, address: u64::MAX - 10, len: 128 },
        Command::DetachBacking { resource: 0 },
    ];
    for cmd in bad {
        assert_eq!(g.submit(1, cmd), Err(Error::BadArgument), "{cmd:?}");
    }
    assert_eq!(sent(&dev), 0);
    assert_eq!(g.free_slots(), SLOTS, "a refused command takes no slot");
    // Turning a scanout off doesn't need a rectangle, and the driver sends none.
    let c = run(&mut g, Command::SetScanout { scanout: 1, resource: 0, rect: rect(5, 5, 0, 0) });
    assert_eq!(c.unwrap().result, Ok(Response::Done));
}

#[test]
fn a_slot_is_free_again_when_its_command_is_taken() {
    let (_dev, mut g) = gpu();
    // The model finishes everything at once, so SLOTS commands fill the pool.
    for t in 0..SLOTS as u64 {
        g.submit(t, Command::GetDisplayInfo).unwrap();
    }
    assert_eq!((g.free_slots(), g.in_flight()), (0, SLOTS));
    assert_eq!(g.submit(99, Command::GetDisplayInfo), Err(Error::PoolFull));
    // They complete in order, each under its own token.
    for t in 0..SLOTS as u64 {
        assert_eq!(g.poll().unwrap().unwrap().token, t);
        assert_eq!(g.free_slots(), t as usize + 1);
    }
    assert_eq!(g.poll(), Ok(None));
    g.submit(100, Command::GetDisplayInfo).unwrap();
    assert_eq!(g.poll().unwrap().unwrap().token, 100);
}

#[test]
fn indices_and_fences_wrap_round() {
    let (dev, mut g) = gpu();
    for i in 0..70_000u32 {
        // Fenced flushes of a resource that doesn't exist: errors, with fences, in every slot.
        let cmd = Command::Flush { resource: 1, rect: rect(0, 0, 1, 1), fenced: i % 2 == 0 };
        let c = run(&mut g, cmd).unwrap();
        assert_eq!(c.result, Err(DeviceError(error_code::INVALID_RESOURCE_ID)), "command {i}");
    }
    assert_eq!(dev.borrow().gpu.as_ref().unwrap().fences.len(), 35_000);
    assert!(dev.borrow().used_idx < 5000 + 64, "the index went all the way round");
}

/// A lie for each command type, and how the driver says it noticed.
#[test]
fn every_lie_in_a_response_is_caught_and_the_device_stays_broken() {
    let all = rect(0, 0, 8, 4);
    let fenced = Command::Flush { resource: 1, rect: all, fenced: true };
    let cases: [(GpuLie, Command); 9] = [
        (GpuLie::ShortResponse, Command::GetDisplayInfo),
        (GpuLie::ShortResponse, Command::Unref { resource: 1 }),
        (GpuLie::WrongType, Command::GetDisplayInfo),
        (GpuLie::WrongType, Command::DetachBacking { resource: 1 }),
        (GpuLie::UnknownType, Command::Unref { resource: 1 }),
        (GpuLie::NoFence, fenced),
        (GpuLie::WrongFence, fenced),
        (GpuLie::ShortInfo, Command::GetDisplayInfo),
        (GpuLie::HugeEdid, Command::GetEdid { scanout: 0 }),
    ];
    let more = [(GpuLie::EdidPastResponse, Command::GetEdid { scanout: 0 })];
    for (lie, cmd) in cases.into_iter().chain(more) {
        let (dev, mut g) = gpu();
        shown(&dev, &mut g, 8, 4);
        dev.borrow_mut().gpu.as_mut().unwrap().lie = lie;
        g.submit(1, cmd).unwrap();
        assert_eq!(g.poll(), Err(Error::BadCompletion), "{lie:?} on {cmd:?}");
        assert!(g.is_broken());
        // Broken for good: nothing more is sent or taken.
        let sent = dev.borrow().gpu.as_ref().unwrap().log.len();
        assert_eq!(g.submit(2, Command::GetDisplayInfo), Err(Error::Broken));
        assert_eq!(g.poll(), Err(Error::Broken));
        assert_eq!(dev.borrow().gpu.as_ref().unwrap().log.len(), sent);
    }
}

#[test]
fn a_lie_about_one_command_leaves_the_others_alone() {
    let (dev, mut g) = gpu();
    dev.borrow_mut().gpu.as_mut().unwrap().lie = GpuLie::WrongType;
    dev.borrow_mut().gpu.as_mut().unwrap().lie_only = Some(0x0102);
    assert!(run(&mut g, Command::GetDisplayInfo).unwrap().result.is_ok());
    assert_eq!(run(&mut g, Command::Unref { resource: 1 }), Err(Error::BadCompletion));
}

#[test]
fn a_device_that_writes_no_answer_is_not_given_the_last_ones() {
    let (dev, mut g) = gpu();
    // A real answer leaves "success" in the slot's response buffer...
    done(&mut g, Command::CreateResource { resource: 1, width: 4, height: 4 });
    // ...and a device that says it answered without writing anything must not be believed on
    // the strength of what was left there.
    dev.borrow_mut().gpu.as_mut().unwrap().lie = GpuLie::Silent;
    g.submit(1, Command::Unref { resource: 1 }).unwrap();
    assert_eq!(g.poll(), Err(Error::BadCompletion));
}

#[test]
fn every_lie_about_the_queue_is_caught() {
    let cases = [
        (Lie::LongLength, queue::Error::BadLength),
        (Lie::BadId, queue::Error::NotInFlight),
        // A completion for a command that was never sent...
        (Lie::Phantom, queue::Error::TooManyUsed),
        (Lie::Stranger, queue::Error::NotInFlight),
        // ...or sent twice, or too many at once.
        (Lie::Twice, queue::Error::TooManyUsed),
        (Lie::IndexJump, queue::Error::TooManyUsed),
    ];
    for (lie, want) in cases {
        let (dev, mut g) = gpu();
        dev.borrow_mut().lie = lie;
        g.submit(1, Command::GetDisplayInfo).unwrap();
        let mut got = g.poll();
        // These two report the true completion first and are caught on the next look.
        if matches!(lie, Lie::IndexJump) && got.is_ok() {
            got = g.poll();
        }
        assert_eq!(got, Err(Error::Queue(want)), "{lie:?}");
        assert!(g.is_broken());
        assert_eq!(g.submit(2, Command::GetDisplayInfo), Err(Error::Broken));
    }
}

#[test]
fn events_are_read_once_and_cleared() {
    let (dev, g) = gpu();
    assert_eq!(g.take_events(), Ok(0));
    dev.borrow_mut().gpu.as_mut().unwrap().events_read = gpu::EVENT_DISPLAY;
    assert_eq!(g.take_events(), Ok(gpu::EVENT_DISPLAY));
    assert_eq!(dev.borrow().gpu.as_ref().unwrap().events_read, 0, "cleared by writing the bits");
    assert_eq!(g.take_events(), Ok(0));
    // Bits the driver doesn't know are read and cleared too, so that they don't keep it busy.
    dev.borrow_mut().gpu.as_mut().unwrap().events_read = 0x8000_0001;
    assert_eq!(g.take_events(), Ok(0x8000_0001));
    assert_eq!(g.take_events(), Ok(0));
}

#[test]
fn a_restart_recovers() {
    let (dev, bar, dma) = model::make_gpu(2, gpu::DMA_NEEDED);
    let mut g = start_with(bar, dma, None).unwrap();
    dev.borrow_mut().lie = Lie::BadId;
    g.submit(1, Command::GetDisplayInfo).unwrap();
    assert!(g.poll().is_err());
    g.stop();
    assert_eq!(g.submit(2, Command::GetDisplayInfo), Err(Error::Broken));
    // A new driver instance starts from reset, over the same DMA memory.
    dev.borrow_mut().lie = Lie::None;
    let bar = Bar { dev: dev.clone(), plain: RefCell::new(vec![0; model::BAR_SIZE]) };
    let dma = Shared(dev.borrow().dma.clone());
    let mut g = start_with(bar, dma, None).unwrap();
    assert!(run(&mut g, Command::GetDisplayInfo).unwrap().result.is_ok());
}

#[test]
fn garbage_in_the_rings_and_the_responses_is_survived() {
    let mut rng = 0x1234_5678_9abc_def0u64;
    let mut next = move || {
        rng ^= rng >> 12;
        rng ^= rng << 25;
        rng ^= rng >> 27;
        rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    for round in 0..600 {
        let (dev, mut g) = gpu();
        g.submit(1, Command::GetEdid { scanout: 1 }).unwrap();
        // The device scribbles over the used ring and the response after the command.
        let (used, mem) = (dev.borrow().q_addr[2] - DMA_BUS, dev.borrow().dma.clone());
        for _ in 0..1 + next() % 6 {
            let at = if round % 2 == 0 {
                used as usize + (next() % 500) as usize
            } else {
                4096 + 64 + (next() % 100) as usize
            };
            mem.write8(at, next() as u8);
        }
        let _ = g.poll();
        // Whatever happened, a new command is sent or refused, and polling answers or errs.
        let _ = g.submit(2, Command::GetDisplayInfo);
        let _ = g.poll();
        let _ = g.take_events();
    }
}
