//! Where a link's audio comes from and goes to: a real device through cpal,
//! or (in tests) memory. Samples cross as interleaved f32 at 48 kHz, only
//! the link's channels, in the link's order.

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const RATE: u32 = saqa_roc::SAMPLE_RATE;

/// Called on the device's own thread with each captured block.
pub type Sink = Box<dyn FnMut(&[f32]) + Send>;
/// Called on the device's own thread to fill each block to play.
pub type Source = Box<dyn FnMut(&mut [f32]) + Send>;
/// Keeps a device open; dropping it closes the device. It stays on the
/// thread that opened it (cpal streams are not always `Send`).
pub type Open = Box<dyn Any>;
/// Called, off the audio callback, when an open device stops for good: the
/// loopback was removed, or the device was unplugged. Its link then fails
/// and says why, instead of reading `running` while nothing plays.
pub type Fault = Box<dyn FnMut(String) + Send>;

pub trait Audio: Send + Sync {
    /// Captures `channels` of `device` into `sink`.
    fn capture(
        &self,
        device: &str,
        channels: &[u32],
        sink: Sink,
        fault: Fault,
    ) -> Result<Open, String>;
    /// Plays `source` into `channels` of `device` (its other channels get silence).
    fn playback(
        &self,
        device: &str,
        channels: &[u32],
        source: Source,
        fault: Fault,
    ) -> Result<Open, String>;
}

/// Picks `channels` out of interleaved `frame`s of `width` channels.
fn pick(data: &[f32], width: usize, channels: &[usize], out: &mut Vec<f32>) {
    out.clear();
    for frame in data.chunks_exact(width) {
        out.extend(channels.iter().map(|&c| frame[c]));
    }
}

/// Spreads `n`-channel frames of `src` into `channels` of `width`-wide `data`.
fn spread(src: &[f32], channels: &[usize], width: usize, data: &mut [f32]) {
    data.fill(0.0);
    for (frame, s) in data
        .chunks_exact_mut(width)
        .zip(src.chunks_exact(channels.len()))
    {
        for (&c, &v) in channels.iter().zip(s) {
            frame[c] = v;
        }
    }
}

/// Devices, through cpal: CoreAudio on macOS, ALSA on Linux.
#[derive(Default)]
pub struct CpalAudio;

fn find(input: bool, name: &str) -> Result<cpal::Device, String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let devices = host.devices().map_err(|e| format!("cpal: {e}"))?;
    for d in devices {
        let by_name = d.description().is_ok_and(|x| x.name() == name);
        let by_id = d.id().is_ok_and(|x| x.id() == name);
        let usable = if input {
            d.supported_input_configs()
                .is_ok_and(|mut c| c.next().is_some())
        } else {
            d.supported_output_configs()
                .is_ok_and(|mut c| c.next().is_some())
        };
        if (by_name || by_id) && usable {
            return Ok(d);
        }
    }
    Err(format!(
        "no {} device '{name}'",
        if input { "input" } else { "output" }
    ))
}

/// The device's widest 48 kHz configuration: its channel count and sample format.
fn config(d: &cpal::Device, input: bool) -> Result<(u16, cpal::SampleFormat), String> {
    use cpal::traits::DeviceTrait;
    let configs: Vec<cpal::SupportedStreamConfigRange> = if input {
        d.supported_input_configs()
            .map_err(|e| e.to_string())?
            .collect()
    } else {
        d.supported_output_configs()
            .map_err(|e| e.to_string())?
            .collect()
    };
    // f32 where offered (no conversion), else the widest integer format.
    let rank = |f: cpal::SampleFormat| match f {
        cpal::SampleFormat::F32 => 0,
        cpal::SampleFormat::I32 => 1,
        cpal::SampleFormat::I16 => 2,
        _ => 9,
    };
    configs
        .iter()
        .filter(|c| c.min_sample_rate() <= RATE && RATE <= c.max_sample_rate())
        .filter(|c| rank(c.sample_format()) < 9)
        .max_by_key(|c| (c.channels(), std::cmp::Reverse(rank(c.sample_format()))))
        .map(|c| (c.channels(), c.sample_format()))
        .ok_or_else(|| "it does not run at 48 kHz".into())
}

fn stream_config(width: u16) -> cpal::StreamConfig {
    cpal::StreamConfig {
        channels: width,
        sample_rate: RATE,
        buffer_size: cpal::BufferSize::Default,
    }
}

/// The error callback: says what happened, and reports a device gone for good.
fn on_error(what: &'static str, device: &str, mut fault: Fault) -> impl FnMut(cpal::Error) {
    let device = device.to_string();
    move |e| {
        eprintln!("saqad: stream {what}: {e}");
        use cpal::ErrorKind::*;
        if matches!(
            e.kind(),
            DeviceNotAvailable | StreamInvalidated | HostUnavailable
        ) {
            fault(format!("{device}: the device went away ({e})"));
        }
    }
}

pub(crate) fn wide_enough(
    channels: &[u32],
    width: u16,
    device: &str,
) -> Result<Vec<usize>, String> {
    match channels.iter().find(|&&c| c >= width as u32) {
        Some(c) => Err(format!(
            "{device} has no channel {} ({width} channels)",
            c + 1
        )),
        None => Ok(channels.iter().map(|&c| c as usize).collect()),
    }
}

macro_rules! input_as {
    ($d:expr, $cfg:expr, $t:ty, $width:expr, $picked:expr, $sink:expr, $err:expr) => {{
        let (width, picked, mut sink) = ($width, $picked, $sink);
        let mut floats = Vec::new();
        let mut out = Vec::new();
        $d.build_input_stream::<$t, _, _>(
            $cfg,
            move |data: &[$t], _: &cpal::InputCallbackInfo| {
                use cpal::Sample;
                floats.clear();
                floats.extend(data.iter().map(|s| s.to_sample::<f32>()));
                pick(&floats, width, &picked, &mut out);
                sink(&out);
            },
            $err,
            None,
        )
    }};
}

macro_rules! output_as {
    ($d:expr, $cfg:expr, $t:ty, $width:expr, $picked:expr, $source:expr, $err:expr) => {{
        let (width, picked, mut source) = ($width, $picked, $source);
        let mut src = Vec::new();
        let mut floats = Vec::new();
        $d.build_output_stream::<$t, _, _>(
            $cfg,
            move |data: &mut [$t], _: &cpal::OutputCallbackInfo| {
                use cpal::Sample;
                let frames = data.len() / width;
                src.resize(frames * picked.len(), 0.0);
                source(&mut src);
                floats.resize(data.len(), 0.0);
                spread(&src, &picked, width, &mut floats);
                for (d, f) in data.iter_mut().zip(&floats) {
                    *d = <$t>::from_sample(*f);
                }
            },
            $err,
            None,
        )
    }};
}

impl Audio for CpalAudio {
    fn capture(
        &self,
        device: &str,
        channels: &[u32],
        sink: Sink,
        fault: Fault,
    ) -> Result<Open, String> {
        use cpal::traits::{DeviceTrait, StreamTrait};
        let d = find(true, device)?;
        let (width, format) = config(&d, true).map_err(|e| format!("{device}: {e}"))?;
        let picked = wide_enough(channels, width, device)?;
        let cfg = stream_config(width);
        let w = width as usize;
        let err = on_error("capture", device, fault);
        let stream = match format {
            cpal::SampleFormat::F32 => input_as!(d, cfg, f32, w, picked, sink, err),
            cpal::SampleFormat::I32 => input_as!(d, cfg, i32, w, picked, sink, err),
            _ => input_as!(d, cfg, i16, w, picked, sink, err),
        }
        .map_err(|e| format!("{device}: {e}"))?;
        stream.play().map_err(|e| format!("{device}: {e}"))?;
        Ok(Box::new(stream))
    }

    fn playback(
        &self,
        device: &str,
        channels: &[u32],
        source: Source,
        fault: Fault,
    ) -> Result<Open, String> {
        use cpal::traits::{DeviceTrait, StreamTrait};
        let d = find(false, device)?;
        let (width, format) = config(&d, false).map_err(|e| format!("{device}: {e}"))?;
        let picked = wide_enough(channels, width, device)?;
        let cfg = stream_config(width);
        let w = width as usize;
        let err = on_error("playback", device, fault);
        let stream = match format {
            cpal::SampleFormat::F32 => output_as!(d, cfg, f32, w, picked, source, err),
            cpal::SampleFormat::I32 => output_as!(d, cfg, i32, w, picked, source, err),
            _ => output_as!(d, cfg, i16, w, picked, source, err),
        }
        .map_err(|e| format!("{device}: {e}"))?;
        stream.play().map_err(|e| format!("{device}: {e}"))?;
        Ok(Box::new(stream))
    }
}

/// Devices made of memory, clocked by a thread: what tests stream through.
/// A memory device has 16 channels unless it is given a width; capturing
/// gives channel c the constant level `levels[c]`, and playback keeps the
/// last frame each device played. A device can be removed while it runs,
/// as the engine removes a loopback.
#[derive(Default, Clone)]
pub struct MemoryAudio {
    pub levels: Vec<f32>,
    played: Arc<Mutex<BTreeMap<String, Vec<f32>>>>,
    widths: Arc<Mutex<BTreeMap<String, u16>>>,
    gone: Arc<Mutex<BTreeSet<String>>>,
}

pub const MEMORY_WIDTH: usize = 16;

impl MemoryAudio {
    pub fn with_levels(levels: Vec<f32>) -> Self {
        MemoryAudio {
            levels,
            ..Self::default()
        }
    }

    /// The same devices, with `device` `n` channels wide.
    pub fn with_width(self, device: &str, n: u16) -> Self {
        self.set_width(device, n);
        self
    }

    /// Makes `device` `n` channels wide (16 unless set).
    pub fn set_width(&self, device: &str, n: u16) {
        self.widths
            .lock()
            .expect("widths")
            .insert(device.to_string(), n);
    }

    /// How many channels `device` has.
    pub fn width(&self, device: &str) -> usize {
        self.widths
            .lock()
            .expect("widths")
            .get(device)
            .map_or(MEMORY_WIDTH, |&n| n as usize)
    }

    /// Takes `device` away: it no longer opens, and a link using it hears
    /// that it went away.
    pub fn remove(&self, device: &str) {
        self.gone.lock().expect("gone").insert(device.to_string());
    }

    fn there(&self, device: &str, dir: &str) -> Result<(), String> {
        if device.contains("missing") || self.gone.lock().expect("gone").contains(device) {
            return Err(format!("no {dir} device '{device}'"));
        }
        Ok(())
    }

    /// The last frame `device` played, as wide as the device.
    pub fn played(&self, device: &str) -> Option<Vec<f32>> {
        self.played.lock().expect("played").get(device).cloned()
    }
}

struct Ticker(Arc<std::sync::atomic::AtomicBool>);

impl Drop for Ticker {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Calls `f` every 10 ms (480 frames), paced like a device clock, until the
/// device is dropped, or until it is removed: then `fault` hears it once.
fn tick(
    gone: Arc<Mutex<BTreeSet<String>>>,
    device: String,
    mut fault: Fault,
    mut f: impl FnMut(usize) + Send + 'static,
) -> Ticker {
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let s = stop.clone();
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        let mut n = 0u64;
        while !s.load(std::sync::atomic::Ordering::Relaxed) {
            if gone.lock().expect("gone").contains(&device) {
                fault(format!("{device}: the device went away"));
                return;
            }
            f(480);
            n += 1;
            let due = start + Duration::from_millis(10 * n);
            if let Some(w) = due.checked_duration_since(std::time::Instant::now()) {
                std::thread::sleep(w);
            }
        }
    });
    Ticker(stop)
}

impl Audio for MemoryAudio {
    fn capture(
        &self,
        device: &str,
        channels: &[u32],
        mut sink: Sink,
        fault: Fault,
    ) -> Result<Open, String> {
        self.there(device, "input")?;
        let width = self.width(device);
        let picked = wide_enough(channels, width as u16, device)?;
        let frame: Vec<f32> = (0..width)
            .map(|c| self.levels.get(c).copied().unwrap_or(0.0))
            .collect();
        let mut out = Vec::new();
        let gone = self.gone.clone();
        Ok(Box::new(tick(
            gone,
            device.to_string(),
            fault,
            move |frames| {
                let data: Vec<f32> = frame.iter().copied().cycle().take(frames * width).collect();
                pick(&data, width, &picked, &mut out);
                sink(&out);
            },
        )))
    }

    fn playback(
        &self,
        device: &str,
        channels: &[u32],
        mut source: Source,
        fault: Fault,
    ) -> Result<Open, String> {
        self.there(device, "output")?;
        let width = self.width(device);
        let picked = wide_enough(channels, width as u16, device)?;
        let played = self.played.clone();
        let name = device.to_string();
        let mut src = Vec::new();
        let mut data = Vec::new();
        Ok(Box::new(tick(
            self.gone.clone(),
            device.to_string(),
            fault,
            move |frames| {
                src.resize(frames * picked.len(), 0.0);
                source(&mut src);
                data.resize(frames * width, 0.0);
                spread(&src, &picked, width, &mut data);
                let last = data[(frames - 1) * width..].to_vec();
                played.lock().expect("played").insert(name.clone(), last);
            },
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_are_picked_and_spread_in_the_links_order() {
        let data = [0.0, 1.0, 2.0, 3.0, 10.0, 11.0, 12.0, 13.0]; // 2 frames × 4
        let mut out = Vec::new();
        pick(&data, 4, &[3, 1], &mut out);
        assert_eq!(out, [3.0, 1.0, 13.0, 11.0]);
        let mut wide = [9.0; 8];
        spread(&out, &[2, 0], 4, &mut wide);
        assert_eq!(wide, [1.0, 0.0, 3.0, 0.0, 11.0, 0.0, 13.0, 0.0]);
        assert!(wide_enough(&[0, 16], 16, "x")
            .unwrap_err()
            .contains("no channel 17"));
    }

    fn quiet() -> Fault {
        Box::new(|_| {})
    }

    #[test]
    fn memory_devices_are_as_wide_as_they_are_told() {
        let audio = MemoryAudio::default().with_width("dsper system 2ch", 2);
        assert_eq!(audio.width("dsper system 2ch"), 2);
        assert_eq!(audio.width("dsper stream 16ch"), MEMORY_WIDTH);
        let refused = audio
            .playback("dsper system 2ch", &[2], Box::new(|_| {}), quiet())
            .expect_err("a 2-channel device has no third channel");
        assert_eq!(refused, "dsper system 2ch has no channel 3 (2 channels)");

        let open = audio
            .playback(
                "dsper system 2ch",
                &[1, 0],
                Box::new(|out| {
                    for f in out.chunks_exact_mut(2) {
                        f.copy_from_slice(&[0.25, 0.5]);
                    }
                }),
                quiet(),
            )
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while audio.played("dsper system 2ch").is_none() {
            assert!(std::time::Instant::now() < deadline, "nothing played");
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(open);
        assert_eq!(
            audio.played("dsper system 2ch").unwrap(),
            [0.5, 0.25],
            "two channels wide, in the link's order"
        );

        let (tx, rx) = std::sync::mpsc::channel();
        let _open = audio
            .capture(
                "dsper system 2ch",
                &[0],
                Box::new(|_| {}),
                Box::new(move |why| {
                    let _ = tx.send(why);
                }),
            )
            .unwrap();
        audio.remove("dsper system 2ch");
        let why = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(why, "dsper system 2ch: the device went away");
        assert!(audio
            .playback("dsper system 2ch", &[0], Box::new(|_| {}), quiet())
            .err()
            .unwrap()
            .contains("no output device"));
    }
}
