//! Apps using the SDK's `saqa/stream.h` speak saqa's wire: a C++ app streams into a saqa
//! receive link, and receives a saqa send link, 16 channels each, every channel on its own.
//! Runs when SAQA_CPP_STREAM names a built `sdk/cpp/examples/stream_levels` (and libroc is
//! here); SAQA_REQUIRE_ROC makes its absence a failure.

use saqa_stream::audio::MemoryAudio;
use saqa_stream::{Devices, LinkSpec, StreamService};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What dsper tells saqa (docs/CONFIG.md): its inputs on macOS, which the
/// memory devices here stand in for.
fn dsper() -> Devices {
    Devices {
        sinks: vec![
            "dsper system 2ch".into(),
            "dsper daw 16ch".into(),
            "dsper stream 16ch".into(),
        ],
        ..Default::default()
    }
}

fn app() -> Option<String> {
    let required = std::env::var_os("SAQA_REQUIRE_ROC").is_some();
    if let Err(e) = StreamService::available() {
        assert!(!required, "{e}");
        eprintln!("skipped: {e}");
        return None;
    }
    match std::env::var("SAQA_CPP_STREAM") {
        Ok(p) => Some(p),
        Err(_) => {
            assert!(
                !required,
                "SAQA_CPP_STREAM: build sdk/cpp/examples/stream_levels"
            );
            eprintln!("skipped: SAQA_CPP_STREAM is not set");
            None
        }
    }
}

fn free_base_port() -> u16 {
    loop {
        let s = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let p = s.local_addr().unwrap().port();
        if (1024..65000).contains(&p)
            && (1..3).all(|i| std::net::UdpSocket::bind(("127.0.0.1", p + i)).is_ok())
        {
            return p;
        }
    }
}

#[test]
fn an_app_streams_sixteen_channels_into_a_dsper_input() {
    let Some(app) = app() else { return };
    let audio = MemoryAudio::default();
    let saqa = StreamService::start(Arc::new(audio.clone()), dsper(), None);
    let port = free_base_port();
    saqa.put(
        "from-app",
        LinkSpec::Receive {
            device: "dsper stream 16ch".into(),
            channels: (0..16).collect(),
            port,
            latency_ms: 60,
        },
    )
    .unwrap();
    let sender = Command::new(&app)
        .args(["send", "16", "127.0.0.1", &port.to_string(), "4"])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(4);
    let frame = loop {
        let f = audio.played("dsper stream 16ch");
        if let Some(f) = f.filter(|f| f.iter().all(|s| s.abs() > 1e-3)) {
            break f;
        }
        assert!(Instant::now() < deadline, "the app's stream never arrived");
        std::thread::sleep(Duration::from_millis(50));
    };
    for (c, s) in frame.iter().enumerate() {
        assert!(
            (s - (c + 1) as f32 / 32.0).abs() < 1e-3,
            "app channel {} → stream {}: {frame:?}",
            c + 1,
            c + 1
        );
    }
    assert_eq!(saqa.links()[0].state, "running");
    // How the app ended, and what it said: a crash after its last line shows as a signal.
    let out = sender.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "the app exited {}: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr).trim()
    );
    saqa.shutdown();
}

#[test]
fn an_app_receives_a_saqa_send_link() {
    let Some(app) = app() else { return };
    let port = free_base_port();
    let receiver = Command::new(&app)
        .args(["receive", "16", &port.to_string(), "3"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // The app listens first; saqa sends while it does.
    let levels: Vec<f32> = (0..16).map(|c| (c + 1) as f32 / 32.0).collect();
    let saqa = StreamService::start(Arc::new(MemoryAudio::with_levels(levels)), dsper(), None);
    let started = std::thread::spawn({
        let saqa = saqa.clone();
        move || {
            std::thread::sleep(Duration::from_millis(300));
            saqa.put(
                "to-app",
                LinkSpec::Send {
                    device: "dsper daw 16ch".into(),
                    channels: (0..16).collect(),
                    to: format!("127.0.0.1:{port}"),
                },
            )
            .unwrap();
        }
    });
    let out = receiver.wait_with_output().unwrap();
    started.join().unwrap();
    saqa.shutdown();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}");
    assert!(text.starts_with("connections 1"), "{text}");
    // A gap under load lowers every channel alike; a channel landing on the
    // wrong channel changes its ratio. Channel c carries (c)/32.
    let ratios: Vec<f32> = text
        .lines()
        .skip(1)
        .map(|line| {
            let (c, level) = line.split_once(' ').unwrap();
            level.parse::<f32>().unwrap() / (c.parse::<f32>().unwrap() / 32.0)
        })
        .collect();
    assert_eq!(ratios.len(), 16, "{text}");
    for r in &ratios {
        assert!(
            (r - ratios[0]).abs() < 0.01 && *r > 0.9,
            "each daw channel on its own app channel: {text}"
        );
    }
}
