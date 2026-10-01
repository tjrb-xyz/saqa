//! Where a link's audio comes from and goes to: a real device through cpal,
//! or (in tests) memory. Samples cross as interleaved f32 at 48 kHz, only
//! the link's channels, in the link's order.

use std::any::Any;
use std::collections::BTreeMap;
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

pub trait Audio: Send + Sync {
    /// Captures `channels` of `device` into `sink`.
    fn capture(&self, device: &str, channels: &[u32], sink: Sink) -> Result<Open, String>;
    /// Plays `source` into `channels` of `device` (its other channels get silence).
    fn playback(&self, device: &str, channels: &[u32], source: Source) -> Result<Open, String>;
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

fn wide_enough(channels: &[u32], width: u16, device: &str) -> Result<Vec<usize>, String> {
    match channels.iter().find(|&&c| c >= width as u32) {
        Some(c) => Err(format!(
            "{device} has no channel {} ({width} channels)",
            c + 1
        )),
        None => Ok(channels.iter().map(|&c| c as usize).collect()),
    }
}

macro_rules! input_as {
    ($d:expr, $cfg:expr, $t:ty, $width:expr, $picked:expr, $sink:expr) => {{
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
            |e| eprintln!("saqad: stream capture: {e}"),
            None,
        )
    }};
}

macro_rules! output_as {
    ($d:expr, $cfg:expr, $t:ty, $width:expr, $picked:expr, $source:expr) => {{
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
            |e| eprintln!("saqad: stream playback: {e}"),
            None,
        )
    }};
}

impl Audio for CpalAudio {
    fn capture(&self, device: &str, channels: &[u32], sink: Sink) -> Result<Open, String> {
        use cpal::traits::{DeviceTrait, StreamTrait};
        let d = find(true, device)?;
        let (width, format) = config(&d, true).map_err(|e| format!("{device}: {e}"))?;
        let picked = wide_enough(channels, width, device)?;
        let cfg = stream_config(width);
        let w = width as usize;
        let stream = match format {
            cpal::SampleFormat::F32 => input_as!(d, cfg, f32, w, picked, sink),
            cpal::SampleFormat::I32 => input_as!(d, cfg, i32, w, picked, sink),
            _ => input_as!(d, cfg, i16, w, picked, sink),
        }
        .map_err(|e| format!("{device}: {e}"))?;
        stream.play().map_err(|e| format!("{device}: {e}"))?;
        Ok(Box::new(stream))
    }

    fn playback(&self, device: &str, channels: &[u32], source: Source) -> Result<Open, String> {
        use cpal::traits::{DeviceTrait, StreamTrait};
        let d = find(false, device)?;
        let (width, format) = config(&d, false).map_err(|e| format!("{device}: {e}"))?;
        let picked = wide_enough(channels, width, device)?;
        let cfg = stream_config(width);
        let w = width as usize;
        let stream = match format {
            cpal::SampleFormat::F32 => output_as!(d, cfg, f32, w, picked, source),
            cpal::SampleFormat::I32 => output_as!(d, cfg, i32, w, picked, source),
            _ => output_as!(d, cfg, i16, w, picked, source),
        }
        .map_err(|e| format!("{device}: {e}"))?;
        stream.play().map_err(|e| format!("{device}: {e}"))?;
        Ok(Box::new(stream))
    }
}

/// Devices made of memory, clocked by a thread: what tests stream through.
/// A memory device has 16 channels; capturing gives channel c the constant
/// level `levels[c]`, and playback keeps the last frame each channel played.
#[derive(Default, Clone)]
pub struct MemoryAudio {
    pub levels: Vec<f32>,
    played: Arc<Mutex<BTreeMap<String, Vec<f32>>>>,
}

pub const MEMORY_WIDTH: usize = 16;

impl MemoryAudio {
    pub fn with_levels(levels: Vec<f32>) -> Self {
        MemoryAudio {
            levels,
            ..Self::default()
        }
    }

    /// The last frame `device` played, all 16 channels.
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

/// Calls `f` every 10 ms (480 frames), paced like a device clock.
fn tick(mut f: impl FnMut(usize) + Send + 'static) -> Ticker {
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let s = stop.clone();
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        let mut n = 0u64;
        while !s.load(std::sync::atomic::Ordering::Relaxed) {
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
    fn capture(&self, device: &str, channels: &[u32], mut sink: Sink) -> Result<Open, String> {
        if device.contains("missing") {
            return Err(format!("no input device '{device}'"));
        }
        let picked = wide_enough(channels, MEMORY_WIDTH as u16, device)?;
        let frame: Vec<f32> = (0..MEMORY_WIDTH)
            .map(|c| self.levels.get(c).copied().unwrap_or(0.0))
            .collect();
        let mut out = Vec::new();
        Ok(Box::new(tick(move |frames| {
            let data: Vec<f32> = frame
                .iter()
                .copied()
                .cycle()
                .take(frames * MEMORY_WIDTH)
                .collect();
            pick(&data, MEMORY_WIDTH, &picked, &mut out);
            sink(&out);
        })))
    }

    fn playback(&self, device: &str, channels: &[u32], mut source: Source) -> Result<Open, String> {
        if device.contains("missing") {
            return Err(format!("no output device '{device}'"));
        }
        let picked = wide_enough(channels, MEMORY_WIDTH as u16, device)?;
        let played = self.played.clone();
        let name = device.to_string();
        let mut src = Vec::new();
        let mut data = Vec::new();
        Ok(Box::new(tick(move |frames| {
            src.resize(frames * picked.len(), 0.0);
            source(&mut src);
            data.resize(frames * MEMORY_WIDTH, 0.0);
            spread(&src, &picked, MEMORY_WIDTH, &mut data);
            let last = data[(frames - 1) * MEMORY_WIDTH..].to_vec();
            played.lock().expect("played").insert(name.clone(), last);
        })))
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
}
