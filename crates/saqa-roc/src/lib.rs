//! Roc Toolkit for saqa: real-time audio over the network between
//! machines (and anything else that speaks Roc: roc-send/roc-recv, roc-vad,
//! PipeWire's Roc modules).
//!
//! libroc (0.4, MPL-2.0) is loaded at run time, so saqa builds and runs
//! without it and streaming says what to install when it is missing. It is
//! found at `SAQA_LIBROC`, then in the checkout's `.saqa/lib`, then by the
//! system's library search.
//!
//! A stream uses three consecutive UDP ports from a base port P: audio with
//! Reed-Solomon FEC on P (`rtp+rs8m`), repair packets on P+1 (`rs8m`), and
//! RTCP on P+2. Stereo uses Roc's built-in L16 encoding, so any Roc peer can
//! play it; other channel counts use saqa's multitrack encodings (float32,
//! payload type 100 + channels), which both ends register.

mod ffi;

use ffi::*;
use std::ffi::CString;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

pub const SAMPLE_RATE: u32 = 48_000;
pub const MAX_CHANNELS: u32 = 16;

/// The RTP payload type saqa uses for `channels` float32 tracks.
pub fn multitrack_id(channels: u32) -> i32 {
    100 + channels as i32
}

/// libroc, loaded once.
pub struct Roc {
    f: Fns,
    // Keeps the library mapped while `f` points into it.
    _lib: libloading::Library,
}

fn candidates() -> Vec<PathBuf> {
    let mut c = vec![];
    if let Some(p) = std::env::var_os("SAQA_LIBROC") {
        c.push(PathBuf::from(p));
    }
    let names: &[&str] = if cfg!(target_os = "macos") {
        &["libroc.0.4.dylib", "libroc.0.dylib", "libroc.dylib"]
    } else {
        &["libroc.so.0.4", "libroc.so.0", "libroc.so"]
    };
    if let Ok(exe) = std::env::current_exe() {
        for dir in exe.ancestors().skip(1) {
            if dir.join("scripts").is_dir() {
                for n in names {
                    c.push(dir.join(".saqa/lib").join(n));
                }
                break;
            }
        }
    }
    if cfg!(target_os = "macos") {
        for n in names {
            c.push(PathBuf::from("/opt/homebrew/lib").join(n));
            c.push(PathBuf::from("/usr/local/lib").join(n));
        }
    }
    c.extend(names.iter().map(PathBuf::from));
    c
}

impl Roc {
    /// libroc, or what to do to get it.
    pub fn get() -> Result<Arc<Roc>, String> {
        static ROC: OnceLock<Result<Arc<Roc>, String>> = OnceLock::new();
        ROC.get_or_init(|| {
            let mut tried = vec![];
            for path in candidates() {
                // SAFETY: loading libroc runs only its (static) initializers.
                match unsafe { libloading::Library::new(&path) } {
                    Ok(lib) => {
                        let f = unsafe { Fns::load(&lib) }
                            .map_err(|e| format!("{} is not libroc 0.4: {e}", path.display()))?;
                        unsafe {
                            (f.log_set_level)(
                                std::env::var("SAQA_ROC_LOG")
                                    .ok()
                                    .and_then(|v| v.parse().ok())
                                    .unwrap_or(ROC_LOG_ERROR),
                            )
                        };
                        return Ok(Arc::new(Roc { f, _lib: lib }));
                    }
                    Err(_) => tried.push(path.display().to_string()),
                }
            }
            Err(format!(
                "streaming needs libroc 0.4 (Roc Toolkit): run saqa's `scripts/roc.sh` to build \
                 it, or set SAQA_LIBROC (looked for {})",
                tried.join(", ")
            ))
        })
        .clone()
    }
}

fn check(what: &str, code: i32) -> Result<(), String> {
    if code == 0 {
        Ok(())
    } else {
        Err(format!("roc: {what} failed ({code})"))
    }
}

/// Where a stream goes (sender) or arrives (receiver): a host and base port.
#[derive(Debug, Clone, PartialEq)]
pub struct Address {
    pub host: String,
    pub port: u16,
}

impl Address {
    /// The three endpoint URIs: audio + FEC, repair, control.
    fn uris(&self) -> [(i32, String); 3] {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host) // IPv6
        } else {
            self.host.clone()
        };
        [
            (
                ROC_INTERFACE_AUDIO_SOURCE,
                format!("rtp+rs8m://{host}:{}", self.port),
            ),
            (
                ROC_INTERFACE_AUDIO_REPAIR,
                format!("rs8m://{host}:{}", self.port + 1),
            ),
            (
                ROC_INTERFACE_AUDIO_CONTROL,
                format!("rtcp://{host}:{}", self.port + 2),
            ),
        ]
    }
}

struct Endpoint<'a> {
    roc: &'a Roc,
    ptr: *mut RocEndpoint,
}

impl<'a> Endpoint<'a> {
    fn new(roc: &'a Roc, uri: &str) -> Result<Self, String> {
        let mut ptr = std::ptr::null_mut();
        check("endpoint", unsafe { (roc.f.endpoint_allocate)(&mut ptr) })?;
        let e = Endpoint { roc, ptr };
        let c = CString::new(uri).map_err(|_| "bad address".to_string())?;
        check(&format!("address {uri}"), unsafe {
            (roc.f.endpoint_set_uri)(e.ptr, c.as_ptr())
        })?;
        Ok(e)
    }
}

impl Drop for Endpoint<'_> {
    fn drop(&mut self) {
        unsafe { (self.roc.f.endpoint_deallocate)(self.ptr) };
    }
}

/// A Roc context: network threads and the encodings both ends agree on.
pub struct Context {
    roc: Arc<Roc>,
    ptr: *mut RocContext,
}

// libroc contexts, senders and receivers may be used from any thread.
unsafe impl Send for Context {}
unsafe impl Sync for Context {}

impl Context {
    pub fn open() -> Result<Arc<Context>, String> {
        let roc = Roc::get()?;
        let config = RocContextConfig::default();
        let mut ptr = std::ptr::null_mut();
        check("context", unsafe {
            (roc.f.context_open)(&config, &mut ptr)
        })?;
        let ctx = Context { roc, ptr };
        // saqa's multitrack encodings, 1…16 float32 tracks at 48 kHz.
        for ch in 1..=MAX_CHANNELS {
            let enc = RocMediaEncoding::multitrack(SAMPLE_RATE, ch);
            check("register encoding", unsafe {
                (ctx.roc.f.context_register_encoding)(ctx.ptr, multitrack_id(ch), &enc)
            })?;
        }
        Ok(Arc::new(ctx))
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        unsafe { (self.roc.f.context_close)(self.ptr) };
    }
}

/// How a stream is carried.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamConfig {
    pub channels: u32,
    /// What the receiver keeps buffered against network jitter.
    pub target_latency_ms: u32,
}

impl StreamConfig {
    fn frame_encoding(&self) -> RocMediaEncoding {
        if self.channels == 2 {
            RocMediaEncoding::stereo(SAMPLE_RATE)
        } else {
            RocMediaEncoding::multitrack(SAMPLE_RATE, self.channels)
        }
    }

    fn validate(&self) -> Result<(), String> {
        if !(1..=MAX_CHANNELS).contains(&self.channels) {
            return Err(format!("a stream carries 1–{MAX_CHANNELS} channels"));
        }
        if !(10..=2000).contains(&self.target_latency_ms) {
            return Err("latency: 10–2000 ms".into());
        }
        Ok(())
    }
}

/// Audio bytes per packet: one UDP datagram on an ordinary network (1500
/// bytes, less IP, UDP, RTP and FEC headers), so nothing is fragmented.
pub const PACKET_PAYLOAD: u32 = 1200;

/// How long each packet is for `channels` channels: Roc's 5 ms when it fits
/// (stereo L16 does), shorter when float32 multitrack would not.
pub fn packet_length_ns(channels: u32) -> u64 {
    if channels == 2 {
        return 5_000_000;
    }
    let frames = (PACKET_PAYLOAD / (channels * 4)).min(SAMPLE_RATE / 200);
    frames as u64 * 1_000_000_000 / SAMPLE_RATE as u64
}

/// Sends interleaved float32 frames to one receiver.
pub struct Sender {
    ctx: Arc<Context>,
    ptr: *mut RocSender,
    channels: usize,
}

unsafe impl Send for Sender {}

impl Sender {
    pub fn open(ctx: &Arc<Context>, config: &StreamConfig, to: &Address) -> Result<Sender, String> {
        config.validate()?;
        let f = &ctx.roc.f;
        let mut c = RocSenderConfig::zeroed();
        c.frame_encoding = config.frame_encoding();
        c.packet_encoding = if config.channels == 2 {
            ROC_PACKET_ENCODING_AVP_L16_STEREO
        } else {
            multitrack_id(config.channels)
        };
        c.packet_length = packet_length_ns(config.channels);
        c.fec_encoding = ROC_FEC_ENCODING_RS8M;
        // Paced by the audio device it reads (saqa writes as it captures).
        c.clock_source = ROC_CLOCK_SOURCE_EXTERNAL;
        let mut ptr = std::ptr::null_mut();
        check("sender", unsafe { (f.sender_open)(ctx.ptr, &c, &mut ptr) })?;
        let s = Sender {
            ctx: ctx.clone(),
            ptr,
            channels: config.channels as usize,
        };
        for (iface, uri) in to.uris() {
            let e = Endpoint::new(&ctx.roc, &uri)?;
            check(&format!("connect {uri}"), unsafe {
                (f.sender_connect)(s.ptr, ROC_SLOT_DEFAULT, iface, e.ptr)
            })?;
        }
        Ok(s)
    }

    /// Interleaved samples, a whole number of frames.
    pub fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        debug_assert_eq!(samples.len() % self.channels, 0);
        let frame = RocFrame {
            samples: samples.as_ptr() as *mut _,
            samples_size: std::mem::size_of_val(samples),
        };
        check("send", unsafe {
            (self.ctx.roc.f.sender_write)(self.ptr, &frame)
        })
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        unsafe { (self.ctx.roc.f.sender_close)(self.ptr) };
    }
}

/// What a receiver reports.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReceiverMetrics {
    /// Senders currently streaming to it.
    pub connections: u32,
    /// End-to-end latency of the first, when known.
    pub latency_ms: Option<f64>,
}

/// Receives from any number of senders on one address, mixed.
pub struct Receiver {
    ctx: Arc<Context>,
    ptr: *mut RocReceiver,
    channels: usize,
}

unsafe impl Send for Receiver {}

impl Receiver {
    pub fn bind(
        ctx: &Arc<Context>,
        config: &StreamConfig,
        on: &Address,
    ) -> Result<Receiver, String> {
        config.validate()?;
        let f = &ctx.roc.f;
        let mut c = RocReceiverConfig::zeroed();
        c.frame_encoding = config.frame_encoding();
        // Paced by the audio device it feeds.
        c.clock_source = ROC_CLOCK_SOURCE_EXTERNAL;
        c.target_latency = config.target_latency_ms as u64 * 1_000_000;
        let mut ptr = std::ptr::null_mut();
        check("receiver", unsafe {
            (f.receiver_open)(ctx.ptr, &c, &mut ptr)
        })?;
        let r = Receiver {
            ctx: ctx.clone(),
            ptr,
            channels: config.channels as usize,
        };
        for (iface, uri) in on.uris() {
            let e = Endpoint::new(&ctx.roc, &uri)?;
            check(&format!("listen on {uri}"), unsafe {
                (f.receiver_bind)(r.ptr, ROC_SLOT_DEFAULT, iface, e.ptr)
            })?;
        }
        Ok(r)
    }

    /// Fills `samples` (interleaved); silence while nothing arrives.
    pub fn read(&mut self, samples: &mut [f32]) -> Result<(), String> {
        debug_assert_eq!(samples.len() % self.channels, 0);
        let mut frame = RocFrame {
            samples: samples.as_mut_ptr() as *mut _,
            samples_size: std::mem::size_of_val(samples),
        };
        check("receive", unsafe {
            (self.ctx.roc.f.receiver_read)(self.ptr, &mut frame)
        })
    }

    pub fn metrics(&self) -> ReceiverMetrics {
        let mut slot = RocReceiverMetrics::default();
        let mut conns = [RocConnectionMetrics::default(); 4];
        let mut n = conns.len();
        let r = unsafe {
            (self.ctx.roc.f.receiver_query)(
                self.ptr,
                ROC_SLOT_DEFAULT,
                &mut slot,
                conns.as_mut_ptr(),
                &mut n,
            )
        };
        if r != 0 {
            return ReceiverMetrics::default();
        }
        ReceiverMetrics {
            connections: slot.connection_count,
            latency_ms: (n > 0 && conns[0].e2e_latency > 0)
                .then(|| conns[0].e2e_latency as f64 / 1e6),
        }
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        unsafe { (self.ctx.roc.f.receiver_close)(self.ptr) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    /// The structs match libroc 0.4's headers (x86_64 and arm64, LP64).
    #[test]
    fn struct_layouts_match_the_c_headers() {
        assert_eq!(size_of::<RocMediaEncoding>(), 16);
        assert_eq!(size_of::<RocContextConfig>(), 8);
        assert_eq!(offset_of!(RocSenderConfig, packet_encoding), 16);
        assert_eq!(offset_of!(RocSenderConfig, packet_length), 24);
        assert_eq!(offset_of!(RocSenderConfig, fec_encoding), 36);
        assert_eq!(offset_of!(RocSenderConfig, target_latency), 72);
        assert_eq!(size_of::<RocSenderConfig>(), 88);
        assert_eq!(offset_of!(RocReceiverConfig, target_latency), 40);
        assert_eq!(size_of::<RocReceiverConfig>(), 72);
        assert_eq!(size_of::<RocFrame>(), 16);
    }

    #[test]
    fn addresses_become_the_three_roc_endpoints() {
        let a = Address {
            host: "10.0.0.5".into(),
            port: 10001,
        };
        let u: Vec<String> = a.uris().into_iter().map(|(_, u)| u).collect();
        assert_eq!(
            u,
            [
                "rtp+rs8m://10.0.0.5:10001",
                "rs8m://10.0.0.5:10002",
                "rtcp://10.0.0.5:10003"
            ]
        );
        let v6 = Address {
            host: "::1".into(),
            port: 20000,
        };
        assert_eq!(v6.uris()[0].1, "rtp+rs8m://[::1]:20000");
    }

    #[test]
    fn multitrack_packets_fit_one_datagram() {
        for ch in 1..=MAX_CHANNELS {
            let frames = packet_length_ns(ch) * SAMPLE_RATE as u64 / 1_000_000_000;
            let bytes = frames * if ch == 2 { 2 * 2 } else { ch as u64 * 4 };
            assert!(
                frames >= 1 && bytes <= PACKET_PAYLOAD as u64,
                "{ch}ch: {frames} frames, {bytes} B"
            );
        }
        assert_eq!(packet_length_ns(2), 5_000_000, "stereo keeps Roc's default");
    }

    #[test]
    fn stream_configs_are_bounded() {
        let ok = StreamConfig {
            channels: 16,
            target_latency_ms: 100,
        };
        assert!(ok.validate().is_ok());
        assert!(StreamConfig {
            channels: 17,
            ..ok.clone()
        }
        .validate()
        .is_err());
        assert!(StreamConfig {
            channels: 0,
            ..ok.clone()
        }
        .validate()
        .is_err());
        assert!(StreamConfig {
            target_latency_ms: 5,
            ..ok
        }
        .validate()
        .is_err());
    }
}
