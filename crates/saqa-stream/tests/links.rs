//! Two machines in one process: one sends, one receives, through the
//! real libroc over localhost, with memory devices. Needs libroc 0.4
//! (SAQA_LIBROC=…); without it the stream tests say so and pass, unless
//! SAQA_REQUIRE_ROC is set.

use axum::body::Body;
use http_body_util::BodyExt;
use saqa_stream::audio::MemoryAudio;
use saqa_stream::{rest, Devices, LinkSpec, StreamService};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tower::ServiceExt;

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

fn roc_here() -> bool {
    match StreamService::available() {
        Ok(()) => true,
        Err(e) if std::env::var_os("SAQA_REQUIRE_ROC").is_none() => {
            eprintln!("skipped: {e}");
            false
        }
        Err(e) => panic!("{e}"),
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

fn eventually<T>(what: &str, secs: u64, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The vintage-corner rig: the main machine sends daw 5–6; the corner
/// machine plays them into its dsper stream input, channels 1–2.
#[test]
fn channels_travel_from_one_machine_into_the_others_stream_input() {
    if !roc_here() {
        return;
    }
    // Machine A's daw input carries channel c at level (c+1)/32.
    let levels: Vec<f32> = (0..16).map(|c| (c + 1) as f32 / 32.0).collect();
    let a = StreamService::start(Arc::new(MemoryAudio::with_levels(levels)), dsper(), None);
    let b_audio = MemoryAudio::default();
    let b = StreamService::start(Arc::new(b_audio.clone()), dsper(), None);
    let port = free_base_port();

    b.put(
        "from-studio",
        LinkSpec::Receive {
            device: "dsper stream 16ch".into(),
            channels: vec![0, 1],
            port,
            latency_ms: 60,
        },
    )
    .unwrap();
    a.put(
        "corner",
        LinkSpec::Send {
            device: "dsper daw 16ch".into(),
            channels: vec![4, 5],
            to: format!("127.0.0.1:{port}"),
        },
    )
    .unwrap();

    let frame = eventually("the corner never heard the studio", 5, || {
        b_audio
            .played("dsper stream 16ch")
            .filter(|f| f[0].abs() > 1e-3 && f[1].abs() > 1e-3)
    });
    assert!(
        (frame[0] - 5.0 / 32.0).abs() < 0.01,
        "daw 5 → stream 1: {frame:?}"
    );
    assert!(
        (frame[1] - 6.0 / 32.0).abs() < 0.01,
        "daw 6 → stream 2: {frame:?}"
    );
    assert!(
        frame[2..].iter().all(|&s| s == 0.0),
        "nothing else: {frame:?}"
    );

    let link = eventually("the receiver never counted the sender", 3, || {
        b.links().into_iter().find(|l| l.connections == 1)
    });
    assert_eq!(link.state, "running");
    assert_eq!(a.links()[0].state, "running");

    // Stopping the sender leaves the receiver listening, and silent.
    assert!(a.delete("corner"));
    eventually("silence after the sender stopped", 5, || {
        b_audio
            .played("dsper stream 16ch")
            .filter(|f| f.iter().all(|s| s.abs() < 1e-3))
    });
    b.shutdown();
}

#[test]
fn a_device_that_is_not_there_fails_the_link_and_says_why() {
    if !roc_here() {
        return;
    }
    let s = StreamService::start(Arc::new(MemoryAudio::default()), dsper(), None);
    s.put(
        "x",
        LinkSpec::Receive {
            device: "dsper stream 16ch".into(),
            channels: vec![16],
            port: free_base_port(),
            latency_ms: 100,
        },
    )
    .unwrap();
    let v = eventually("the link never failed", 3, || {
        s.links().into_iter().find(|l| l.state == "failed")
    });
    assert!(v.detail.unwrap().contains("no channel 17"));
}

#[test]
fn links_are_kept_and_come_back_with_saqad() {
    if !roc_here() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("streams.json");
    let spec = LinkSpec::Receive {
        device: "dsper stream 16ch".into(),
        channels: vec![0, 1],
        port: free_base_port(),
        latency_ms: 100,
    };
    {
        let s = StreamService::start(
            Arc::new(MemoryAudio::default()),
            dsper(),
            Some(file.clone()),
        );
        s.put("kept", spec.clone()).unwrap();
        s.shutdown();
    }
    let again = StreamService::start(Arc::new(MemoryAudio::default()), dsper(), Some(file));
    let links = again.links();
    assert_eq!((links[0].id.as_str(), &links[0].spec), ("kept", &spec));
    again.shutdown();
}

async fn call(app: &axum::Router, method: &str, path: &str, body: Value) -> (u16, Value) {
    let r = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = r.status().as_u16();
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn the_api_refuses_a_stream_into_an_interface() {
    let app = rest::router(StreamService::start(
        Arc::new(MemoryAudio::default()),
        dsper(),
        None,
    ));
    let (status, v) = call(
        &app,
        "PUT",
        "/links/bad",
        json!({"direction": "receive", "device": "EVO16", "channels": [0, 1], "port": 20000}),
    )
    .await;
    assert_eq!(status, 422, "{v}");
    assert!(v["error"].as_str().unwrap().contains("checked pipeline"));
    let (status, _) = call(&app, "PUT", "/links/x", json!({"direction": "sideways"})).await;
    assert!(status == 400 || status == 422);
    let (status, v) = call(&app, "GET", "/state", Value::Null).await;
    assert_eq!(status, 200);
    assert_eq!(v["available"], StreamService::available().is_ok());
    let (status, _) = call(&app, "DELETE", "/links/none", Value::Null).await;
    assert_eq!(status, 404);
}

/// Sixteen channels, each where it was sent: a frame torn in the ring would
/// shift every channel after it (it did, before frames crossed whole).
#[test]
fn sixteen_channels_arrive_each_on_its_own_channel() {
    if !roc_here() {
        return;
    }
    let levels: Vec<f32> = (0..16).map(|c| (c + 1) as f32 / 32.0).collect();
    let a = StreamService::start(Arc::new(MemoryAudio::with_levels(levels)), dsper(), None);
    let b_audio = MemoryAudio::default();
    let b = StreamService::start(Arc::new(b_audio.clone()), dsper(), None);
    let port = free_base_port();
    b.put(
        "in",
        LinkSpec::Receive {
            device: "dsper stream 16ch".into(),
            channels: (0..16).collect(),
            port,
            latency_ms: 60,
        },
    )
    .unwrap();
    a.put(
        "out",
        LinkSpec::Send {
            device: "dsper daw 16ch".into(),
            channels: (0..16).collect(),
            to: format!("127.0.0.1:{port}"),
        },
    )
    .unwrap();
    let frame = eventually("sixteen channels never arrived", 5, || {
        b_audio
            .played("dsper stream 16ch")
            .filter(|f| f.iter().all(|s| s.abs() > 1e-3))
    });
    for (c, s) in frame.iter().enumerate() {
        assert!(
            (s - (c + 1) as f32 / 32.0).abs() < 1e-3,
            "channel {}: {frame:?}",
            c + 1
        );
    }
    // And they stay there: sample it again a second later.
    std::thread::sleep(Duration::from_secs(1));
    let later = b_audio.played("dsper stream 16ch").unwrap();
    for (c, s) in later.iter().enumerate() {
        assert!(
            (s - (c + 1) as f32 / 32.0).abs() < 1e-3,
            "channel {} later: {later:?}",
            c + 1
        );
    }
    a.shutdown();
    b.shutdown();
}
