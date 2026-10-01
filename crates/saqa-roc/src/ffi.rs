//! libroc 0.4's C API, as much of it as saqa uses. Layouts follow
//! `roc/config.h`, `frame.h` and `metrics.h` of v0.4.0 (C enums are `int`);
//! `struct_layouts_match_the_c_headers` checks the offsets.

#![allow(dead_code)]

use std::os::raw::{c_char, c_int, c_void};

pub enum RocContext {}
pub enum RocSender {}
pub enum RocReceiver {}
pub enum RocEndpoint {}

pub const ROC_SLOT_DEFAULT: u64 = 0;

pub const ROC_INTERFACE_AUDIO_SOURCE: c_int = 11;
pub const ROC_INTERFACE_AUDIO_REPAIR: c_int = 12;
pub const ROC_INTERFACE_AUDIO_CONTROL: c_int = 13;

pub const ROC_PACKET_ENCODING_AVP_L16_STEREO: c_int = 10;
pub const ROC_FEC_ENCODING_RS8M: c_int = 1;
pub const ROC_FORMAT_PCM_FLOAT32: c_int = 1;
pub const ROC_CHANNEL_LAYOUT_MULTITRACK: c_int = 1;
pub const ROC_CHANNEL_LAYOUT_STEREO: c_int = 3;
pub const ROC_CLOCK_SOURCE_EXTERNAL: c_int = 1;
pub const ROC_LOG_ERROR: c_int = 1;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RocMediaEncoding {
    pub rate: u32,
    pub format: c_int,
    pub channels: c_int,
    pub tracks: u32,
}

impl RocMediaEncoding {
    pub fn stereo(rate: u32) -> Self {
        Self {
            rate,
            format: ROC_FORMAT_PCM_FLOAT32,
            channels: ROC_CHANNEL_LAYOUT_STEREO,
            tracks: 0,
        }
    }
    pub fn multitrack(rate: u32, tracks: u32) -> Self {
        Self {
            rate,
            format: ROC_FORMAT_PCM_FLOAT32,
            channels: ROC_CHANNEL_LAYOUT_MULTITRACK,
            tracks,
        }
    }
}

/// Zero means "the default" for every field.
#[repr(C)]
#[derive(Debug, Default)]
pub struct RocContextConfig {
    pub max_packet_size: u32,
    pub max_frame_size: u32,
}

#[repr(C)]
pub struct RocSenderConfig {
    pub frame_encoding: RocMediaEncoding,
    pub packet_encoding: c_int,
    pub packet_length: u64,
    pub packet_interleaving: u32,
    pub fec_encoding: c_int,
    pub fec_block_source_packets: u32,
    pub fec_block_repair_packets: u32,
    pub clock_source: c_int,
    pub latency_tuner_backend: c_int,
    pub latency_tuner_profile: c_int,
    pub resampler_backend: c_int,
    pub resampler_profile: c_int,
    pub target_latency: u64,
    pub latency_tolerance: u64,
}

#[repr(C)]
pub struct RocReceiverConfig {
    pub frame_encoding: RocMediaEncoding,
    pub clock_source: c_int,
    pub latency_tuner_backend: c_int,
    pub latency_tuner_profile: c_int,
    pub resampler_backend: c_int,
    pub resampler_profile: c_int,
    pub target_latency: u64,
    pub latency_tolerance: u64,
    pub no_playback_timeout: i64,
    pub choppy_playback_timeout: i64,
}

macro_rules! zeroed {
    ($t:ty) => {
        impl $t {
            /// All defaults (libroc treats zero as "default").
            pub fn zeroed() -> Self {
                // SAFETY: plain integers and a plain struct: zero is valid.
                unsafe { std::mem::zeroed() }
            }
        }
    };
}
zeroed!(RocSenderConfig);
zeroed!(RocReceiverConfig);

#[repr(C)]
pub struct RocFrame {
    pub samples: *mut c_void,
    pub samples_size: usize,
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct RocConnectionMetrics {
    pub e2e_latency: u64,
}

#[repr(C)]
#[derive(Debug, Default)]
pub struct RocReceiverMetrics {
    pub connection_count: u32,
}

type Open<C, T> = unsafe extern "C" fn(*mut RocContext, *const C, *mut *mut T) -> c_int;

pub struct Fns {
    pub context_open: unsafe extern "C" fn(*const RocContextConfig, *mut *mut RocContext) -> c_int,
    pub context_register_encoding:
        unsafe extern "C" fn(*mut RocContext, c_int, *const RocMediaEncoding) -> c_int,
    pub context_close: unsafe extern "C" fn(*mut RocContext) -> c_int,
    pub endpoint_allocate: unsafe extern "C" fn(*mut *mut RocEndpoint) -> c_int,
    pub endpoint_set_uri: unsafe extern "C" fn(*mut RocEndpoint, *const c_char) -> c_int,
    pub endpoint_deallocate: unsafe extern "C" fn(*mut RocEndpoint) -> c_int,
    pub sender_open: Open<RocSenderConfig, RocSender>,
    pub sender_connect:
        unsafe extern "C" fn(*mut RocSender, u64, c_int, *const RocEndpoint) -> c_int,
    pub sender_write: unsafe extern "C" fn(*mut RocSender, *const RocFrame) -> c_int,
    pub sender_close: unsafe extern "C" fn(*mut RocSender) -> c_int,
    pub receiver_open: Open<RocReceiverConfig, RocReceiver>,
    pub receiver_bind:
        unsafe extern "C" fn(*mut RocReceiver, u64, c_int, *mut RocEndpoint) -> c_int,
    pub receiver_query: unsafe extern "C" fn(
        *mut RocReceiver,
        u64,
        *mut RocReceiverMetrics,
        *mut RocConnectionMetrics,
        *mut usize,
    ) -> c_int,
    pub receiver_read: unsafe extern "C" fn(*mut RocReceiver, *mut RocFrame) -> c_int,
    pub receiver_close: unsafe extern "C" fn(*mut RocReceiver) -> c_int,
    pub log_set_level: unsafe extern "C" fn(c_int),
}

impl Fns {
    /// # Safety
    /// `lib` must be libroc 0.4 and outlive the returned pointers.
    pub unsafe fn load(lib: &libloading::Library) -> Result<Fns, libloading::Error> {
        macro_rules! sym {
            ($name:literal) => {
                *lib.get(concat!($name, "\0").as_bytes())?
            };
        }
        Ok(Fns {
            context_open: sym!("roc_context_open"),
            context_register_encoding: sym!("roc_context_register_encoding"),
            context_close: sym!("roc_context_close"),
            endpoint_allocate: sym!("roc_endpoint_allocate"),
            endpoint_set_uri: sym!("roc_endpoint_set_uri"),
            endpoint_deallocate: sym!("roc_endpoint_deallocate"),
            sender_open: sym!("roc_sender_open"),
            sender_connect: sym!("roc_sender_connect"),
            sender_write: sym!("roc_sender_write"),
            sender_close: sym!("roc_sender_close"),
            receiver_open: sym!("roc_receiver_open"),
            receiver_bind: sym!("roc_receiver_bind"),
            receiver_query: sym!("roc_receiver_query"),
            receiver_read: sym!("roc_receiver_read"),
            receiver_close: sym!("roc_receiver_close"),
            log_set_level: sym!("roc_log_set_level"),
        })
    }
}
