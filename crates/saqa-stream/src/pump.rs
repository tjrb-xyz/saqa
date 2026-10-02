//! One thread per link: the device on one side, Roc on the other, a
//! lock-free ring between them (the device's callback must never wait).
//!
//! Send: the device's callback pushes captured frames; the pump writes them
//! to Roc in 10 ms blocks. Receive: the pump reads Roc whenever the ring has
//! room for a block, and the device's callback plays from the ring — so the
//! device's clock paces Roc, and Roc's latency tuner absorbs the difference
//! from the sender's clock.

use crate::audio::{Audio, Fault, RATE};
use crate::{parse_peer, LinkSpec, LinkView};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const BLOCK: usize = (RATE / 100) as usize; // 10 ms

pub struct Stop {
    flag: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Stop {
    pub fn stop_and_wait(mut self) {
        self.flag.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Pushes `samples` whole or not at all: a reader never sees part of a frame.
fn push_all(prod: &mut rtrb::Producer<f32>, samples: &[f32]) -> bool {
    match prod.write_chunk_uninit(samples.len()) {
        Ok(chunk) => {
            chunk.fill_from_iter(samples.iter().copied());
            true
        }
        Err(_) => false,
    }
}

/// Fills `out` with as many whole `n`-channel frames as are ready, then silence.
/// Returns the frames it had to leave silent.
fn pop_frames(cons: &mut rtrb::Consumer<f32>, out: &mut [f32], n: usize) -> usize {
    let take = (cons.slots() / n * n).min(out.len());
    if take > 0 {
        let chunk = cons.read_chunk(take).expect("counted");
        let (a, b) = chunk.as_slices();
        out[..a.len()].copy_from_slice(a);
        out[a.len()..take].copy_from_slice(b);
        chunk.commit_all();
    }
    out[take..].fill(0.0);
    (out.len() - take) / n
}

fn set(view: &Mutex<LinkView>, f: impl FnOnce(&mut LinkView)) {
    f(&mut view.lock().expect("view"));
}

fn fail(view: &Mutex<LinkView>, why: String) {
    set(view, |v| {
        v.state = "failed".into();
        v.detail = Some(why);
    });
}

pub fn start(audio: Arc<dyn Audio>, spec: LinkSpec, view: Arc<Mutex<LinkView>>) -> Stop {
    let flag = Arc::new(AtomicBool::new(false));
    let stop = flag.clone();
    let thread = std::thread::Builder::new()
        .name("saqa-stream".into())
        .spawn(move || {
            let r = match &spec {
                LinkSpec::Send { .. } => send(&*audio, &spec, &view, &stop),
                LinkSpec::Receive { .. } => receive(&*audio, &spec, &view, &stop),
            };
            if let Err(e) = r {
                fail(&view, e);
            }
        })
        .expect("spawn stream link");
    Stop {
        flag,
        thread: Some(thread),
    }
}

/// Where a device's fault lands: the link's loop reads it and fails the link.
fn fault_cell() -> (Arc<OnceLock<String>>, Fault) {
    let cell = Arc::new(OnceLock::new());
    let c = cell.clone();
    (
        cell,
        Box::new(move |why| {
            let _ = c.set(why);
        }),
    )
}

fn went_away(cell: &OnceLock<String>) -> Result<(), String> {
    match cell.get() {
        Some(why) => Err(why.clone()),
        None => Ok(()),
    }
}

fn config(channels: usize, latency_ms: u32) -> saqa_roc::StreamConfig {
    saqa_roc::StreamConfig {
        channels: channels as u32,
        target_latency_ms: latency_ms,
    }
}

fn send(
    audio: &dyn Audio,
    spec: &LinkSpec,
    view: &Mutex<LinkView>,
    stop: &AtomicBool,
) -> Result<(), String> {
    let LinkSpec::Send {
        device,
        channels,
        to,
    } = spec
    else {
        unreachable!()
    };
    let n = channels.len();
    let ctx = saqa_roc::Context::open()?;
    let mut tx = saqa_roc::Sender::open(&ctx, &config(n, 100), &parse_peer(to)?)?;
    // 200 ms of room: a device block is never dropped for want of space.
    let (mut prod, mut cons) = rtrb::RingBuffer::<f32>::new(BLOCK * n * 20);
    let lost = Arc::new(AtomicU64::new(0));
    let l = lost.clone();
    let (gone, fault) = fault_cell();
    let _device = audio.capture(
        device,
        channels,
        Box::new(move |frames| {
            if !push_all(&mut prod, frames) {
                l.fetch_add((frames.len() / n) as u64, Ordering::Relaxed);
            }
        }),
        fault,
    )?;
    set(view, |v| {
        v.state = "running".into();
        v.detail = Some(format!("to {to}"));
    });
    let mut block = vec![0f32; BLOCK * n];
    while !stop.load(Ordering::Relaxed) {
        went_away(&gone)?;
        if cons.slots() >= block.len() {
            pop_frames(&mut cons, &mut block, n);
            tx.write(&block)?;
        } else {
            std::thread::sleep(Duration::from_millis(2));
        }
        let dropouts = lost.load(Ordering::Relaxed);
        set(view, |v| v.dropouts = dropouts);
    }
    Ok(())
}

fn receive(
    audio: &dyn Audio,
    spec: &LinkSpec,
    view: &Mutex<LinkView>,
    stop: &AtomicBool,
) -> Result<(), String> {
    let LinkSpec::Receive {
        device,
        channels,
        port,
        latency_ms,
    } = spec
    else {
        unreachable!()
    };
    let n = channels.len();
    let ctx = saqa_roc::Context::open()?;
    let on = saqa_roc::Address {
        host: "0.0.0.0".into(),
        port: *port,
    };
    let mut rx = saqa_roc::Receiver::bind(&ctx, &config(n, *latency_ms), &on)
        .map_err(|e| format!("{e} (is port {port} free?)"))?;
    // Two blocks ahead of the device: little added latency, no gaps.
    let (mut prod, mut cons) = rtrb::RingBuffer::<f32>::new(BLOCK * n * 3);
    let short = Arc::new(AtomicU64::new(0));
    let s2 = short.clone();
    let (gone, fault) = fault_cell();
    let _device = audio.playback(
        device,
        channels,
        Box::new(move |out| {
            let silent = pop_frames(&mut cons, out, n);
            s2.fetch_add(silent as u64, Ordering::Relaxed);
        }),
        fault,
    )?;
    set(view, |v| {
        v.state = "running".into();
        v.detail = Some(format!("listening on port {port}"));
    });
    let mut block = vec![0f32; BLOCK * n];
    let mut last = Instant::now();
    let mut primed = false;
    while !stop.load(Ordering::Relaxed) {
        went_away(&gone)?;
        if prod.slots() >= block.len() {
            rx.read(&mut block)?;
            push_all(&mut prod, &block); // room was counted above
            primed = true;
        } else {
            std::thread::sleep(Duration::from_millis(2));
        }
        if last.elapsed() >= Duration::from_millis(250) {
            last = Instant::now();
            let m = rx.metrics();
            let dropouts = if primed {
                short.load(Ordering::Relaxed)
            } else {
                0
            };
            set(view, |v| {
                v.connections = m.connections;
                v.e2e_latency_ms = m.latency_ms;
                v.dropouts = dropouts;
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reader running while a block is half-written must never take part
    /// of a frame: that shifted every channel of a 16-channel link by three.
    #[test]
    fn frames_cross_the_ring_whole_or_not_at_all() {
        let n = 16;
        let (mut prod, mut cons) = rtrb::RingBuffer::<f32>::new(n * 8);
        let frame: Vec<f32> = (0..n).map(|c| c as f32).collect();
        let mut out = vec![9.0; n * 3];
        assert_eq!(
            pop_frames(&mut cons, &mut out, n),
            3,
            "nothing ready: silence"
        );
        assert!(out.iter().all(|&s| s == 0.0));
        // A torn write, as a per-sample producer would leave it mid-block.
        for &s in &frame[..3] {
            prod.push(s).unwrap();
        }
        assert_eq!(
            pop_frames(&mut cons, &mut out, n),
            3,
            "a partial frame is not taken"
        );
        for &s in &frame[3..] {
            prod.push(s).unwrap();
        }
        assert!(push_all(&mut prod, &frame));
        assert_eq!(pop_frames(&mut cons, &mut out, n), 1);
        assert_eq!(&out[..n], &frame[..]);
        assert_eq!(&out[n..2 * n], &frame[..]);
        assert!(
            !push_all(&mut prod, &vec![0.0; n * 9]),
            "too big: nothing written"
        );
        assert_eq!(cons.slots(), 0);
    }
}
