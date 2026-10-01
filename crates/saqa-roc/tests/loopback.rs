//! Real Roc sessions over localhost, with the real libroc: each channel of a
//! stream arrives as itself. Needs libroc 0.4 (SAQA_LIBROC=…/libroc.so);
//! without it the tests say so and pass, unless SAQA_REQUIRE_ROC is set.

use saqa_roc::{Address, Context, Receiver, Sender, StreamConfig, SAMPLE_RATE};
use std::time::{Duration, Instant};

fn context() -> Option<std::sync::Arc<Context>> {
    match Context::open() {
        Ok(c) => Some(c),
        Err(e) if std::env::var_os("SAQA_REQUIRE_ROC").is_none() => {
            eprintln!("skipped: {e}");
            None
        }
        Err(e) => panic!("{e}"),
    }
}

fn free_base_port() -> u16 {
    // Three consecutive UDP ports: find a base whose three are free.
    loop {
        let s = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let p = s.local_addr().unwrap().port();
        if p < 65000 && (1..3).all(|i| std::net::UdpSocket::bind(("127.0.0.1", p + i)).is_ok()) {
            return p;
        }
    }
}

/// Streams `channels` channels, channel k at level (k+1)/32, and returns what
/// the receiver played per channel once the stream settled.
fn stream(channels: u32) -> Option<Vec<f32>> {
    let ctx = context()?;
    let config = StreamConfig {
        channels,
        target_latency_ms: 60,
    };
    let addr = Address {
        host: "127.0.0.1".into(),
        port: free_base_port(),
    };
    let mut rx = Receiver::bind(&ctx, &config, &addr).expect("bind");
    let mut tx = Sender::open(&ctx, &config, &addr).expect("connect");

    let n = channels as usize;
    let frames = (SAMPLE_RATE / 100) as usize; // 10 ms
    let level = |k: usize| (k + 1) as f32 / 32.0;
    let out: Vec<f32> = (0..frames * n).map(|i| level(i % n)).collect();
    let mut got = vec![0f32; frames * n];

    let start = Instant::now();
    let mut heard = None;
    for tick in 0..300u32 {
        // Both ends run at the audio rate, as devices would pace them.
        let due = start + Duration::from_millis(10 * tick as u64);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        tx.write(&out).expect("write");
        rx.read(&mut got).expect("read");
        // Settled: the whole frame carries signal on every channel.
        if got.iter().all(|s| s.abs() > 1e-3) {
            let m = rx.metrics();
            assert_eq!(m.connections, 1, "{m:?}");
            heard = Some(tick);
            break;
        }
    }
    assert!(heard.is_some(), "nothing arrived in 3 s");
    // The last frame of the settled stream, one value per channel.
    Some(got[(frames - 1) * n..].to_vec())
}

fn assert_levels(got: &[f32]) {
    for (k, v) in got.iter().enumerate() {
        let want = (k + 1) as f32 / 32.0;
        assert!(
            (v - want).abs() < 0.01,
            "channel {} carried {v}, not {want}: {got:?}",
            k + 1
        );
    }
}

#[test]
fn sixteen_channels_arrive_each_on_its_own() {
    if let Some(got) = stream(16) {
        assert_levels(&got);
    }
}

#[test]
fn stereo_uses_the_standard_encoding_any_roc_peer_plays() {
    // L16 on the wire: levels survive to within 16-bit precision.
    if let Some(got) = stream(2) {
        assert_levels(&got);
    }
}

#[test]
fn odd_channel_counts_work_too() {
    if let Some(got) = stream(6) {
        assert_levels(&got);
    }
}
