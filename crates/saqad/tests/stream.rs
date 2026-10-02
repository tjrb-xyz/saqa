//! The stream service as saqad serves it: at /stream/v1 behind the token and
//! the Host/Origin guard (from dsperd's tests/stream.rs, where it ran as
//! `dsperd --sim --streaming` and `dsperd --only stream`).

use serde_json::{json, Value};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TOKEN: &str = "streamtest0123456789abcdef012345";

struct Daemon(Child, u16);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .new_agent()
}

fn temp_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("saqad-{}-{}", std::process::id(), free_port()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn start(args: &[&str], health: &str) -> Daemon {
    start_in(&temp_dir(), args, health)
}

/// saqad with `XDG_CONFIG_HOME` at `dir`: its own files are in `dir/saqa`.
fn start_in(dir: &std::path::Path, args: &[&str], health: &str) -> Daemon {
    let port = free_port();
    let child = Command::new(env!("CARGO_BIN_EXE_saqad"))
        .args(args)
        .args(["--port", &port.to_string(), "--token", TOKEN])
        .env("XDG_CONFIG_HOME", dir)
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let d = Daemon(child, port);
    let deadline = Instant::now() + Duration::from_secs(20);
    while agent()
        .get(format!("http://127.0.0.1:{port}{health}"))
        .call()
        .map_or(true, |r| r.status() != 200)
    {
        assert!(Instant::now() < deadline, "saqad did not come up");
        std::thread::sleep(Duration::from_millis(100));
    }
    d
}

fn call(port: u16, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let url = format!("http://127.0.0.1:{port}{path}");
    let a = agent();
    let auth = format!("Bearer {TOKEN}");
    let r = match (method, body) {
        ("PUT", Some(b)) => a.put(&url).header("Authorization", &auth).send_json(b),
        ("DELETE", _) => a.delete(&url).header("Authorization", &auth).call(),
        _ => a.get(&url).header("Authorization", &auth).call(),
    };
    let mut r = r.unwrap();
    let status = r.status().as_u16();
    (status, r.body_mut().read_json().unwrap_or(Value::Null))
}

#[test]
fn saqad_serves_streams_and_holds_the_safety_line() {
    let d = start(
        &["--memory", "--sink", "dsper stream 16ch"],
        "/stream/v1/health",
    );
    let (status, _) = agent()
        .get(format!("http://127.0.0.1:{}/stream/v1/state", d.1))
        .call()
        .map(|r| (r.status().as_u16(), ()))
        .unwrap();
    assert_eq!(status, 401, "the token guards streams too");
    let (status, state) = call(d.1, "GET", "/stream/v1/state", None);
    assert_eq!(status, 200);
    let roc = state["available"] == true;

    let (status, v) = call(
        d.1,
        "PUT",
        "/stream/v1/links/bad",
        Some(
            json!({"direction": "receive", "device": "EVO 16", "channels": [0, 1], "port": 20000}),
        ),
    );
    assert_eq!(status, 422, "{v}");

    let (status, v) = call(
        d.1,
        "PUT",
        "/stream/v1/links/from-studio",
        Some(
            json!({"direction": "receive", "device": "dsper stream 16ch", "channels": [0, 1],
                    "port": free_port().clamp(1024, 60000)}),
        ),
    );
    if roc {
        assert_eq!(status, 200, "{v}");
        let (_, links) = call(d.1, "GET", "/stream/v1/links", None);
        assert_eq!(links[0]["id"], "from-studio");
        let (status, _) = call(d.1, "DELETE", "/stream/v1/links/from-studio", None);
        assert_eq!(status, 200);
    } else {
        assert_eq!(status, 503, "no libroc: it says how to get it: {v}");
        assert!(v["error"].as_str().unwrap().contains("roc"), "{v}");
    }
}

#[test]
fn the_stream_service_runs_on_its_own() {
    let d = start(&[], "/stream/v1/health");
    let (status, v) = call(d.1, "GET", "/stream/v1/links", None);
    assert_eq!((status, v), (200, json!([])));
    let (status, _) = call(d.1, "GET", "/api/v1/state", None);
    assert_eq!(status, 404, "only the stream service is here");
}

#[test]
fn a_foreign_host_or_origin_is_refused() {
    let d = start(&["--memory"], "/stream/v1/health");
    let url = format!("http://127.0.0.1:{}/stream/v1/links", d.1);
    let auth = format!("Bearer {TOKEN}");
    let status = |host: &str, origin: Option<&str>| {
        let mut r = agent()
            .get(&url)
            .header("Authorization", &auth)
            .header("Host", host);
        if let Some(o) = origin {
            r = r.header("Origin", o);
        }
        r.call().unwrap().status().as_u16()
    };
    let here = format!("127.0.0.1:{}", d.1);
    assert_eq!(status(&here, None), 200);
    assert_eq!(status(&format!("evil.example:{}", d.1), None), 421);
    assert_eq!(status(&here, Some("https://evil.example")), 403);
    assert_eq!(status(&here, Some(&format!("http://{here}"))), 200);
}

#[test]
fn without_sinks_nothing_is_received() {
    let d = start(&["--memory"], "/stream/v1/health");
    let (status, v) = call(
        d.1,
        "PUT",
        "/stream/v1/links/from-studio",
        Some(
            json!({"direction": "receive", "device": "dsper stream 16ch", "channels": [0, 1], "port": 20000}),
        ),
    );
    assert_eq!(status, 422, "{v}");
    assert!(
        v["error"].as_str().unwrap().contains("none is configured"),
        "{v}"
    );
}

#[test]
fn sinks_and_aliases_come_from_the_config_file() {
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("saqa")).unwrap();
    std::fs::write(
        dir.join("saqa/saqad.json"),
        r#"{"sinks": ["dsper * 16ch"], "aliases": {"stream": "dsper stream 16ch"}}"#,
    )
    .unwrap();
    let d = start_in(&dir, &["--memory"], "/stream/v1/health");
    let (status, state) = call(d.1, "GET", "/stream/v1/state", None);
    assert_eq!(status, 200);
    let (status, v) = call(
        d.1,
        "PUT",
        "/stream/v1/links/from-studio",
        Some(
            json!({"direction": "receive", "device": "stream", "channels": [0, 1],
                    "port": free_port().clamp(1024, 60000)}),
        ),
    );
    if state["available"] == true {
        assert_eq!(status, 200, "{v}");
        assert_eq!(v["device"], "dsper stream 16ch", "the alias, resolved: {v}");
    } else {
        assert_eq!(status, 503, "{v}");
    }
    let (status, v) = call(
        d.1,
        "PUT",
        "/stream/v1/links/x",
        Some(
            json!({"direction": "receive", "device": "dsper system 2ch", "channels": [0], "port": 20000}),
        ),
    );
    assert_eq!(status, 422, "{v}");
}

#[test]
fn dspers_kept_links_are_imported_once() {
    let dir = temp_dir();
    let old = dir.join("dsper/streams.json");
    std::fs::create_dir_all(old.parent().unwrap()).unwrap();
    let kept = r#"{"corner": {"direction": "send", "device": "nowhere 2ch", "channels": [0, 1], "to": "corner.local:20000"}}"#;
    std::fs::write(&old, kept).unwrap();
    let ours = dir.join("saqa/streams.json");
    {
        let _d = start_in(
            &dir,
            &["--import-links", old.to_str().unwrap()],
            "/stream/v1/health",
        );
        let text = std::fs::read_to_string(&ours).unwrap();
        assert!(text.contains("corner.local:20000"), "{text}");
    }
    // Once saqa keeps its own, dsper's file is not read again.
    std::fs::write(&ours, "{}").unwrap();
    let _d = start_in(
        &dir,
        &["--import-links", old.to_str().unwrap()],
        "/stream/v1/health",
    );
    assert_eq!(std::fs::read_to_string(&ours).unwrap().trim(), "{}");
    assert!(old.exists(), "dsper's file is left where it was");
}

/// docs/CONFIG.md's macOS setup: a received stream lands in the streaming
/// loopback or the 2-channel one; a send link may read any loopback.
const RECOMMENDED: &[&str] = &[
    "--memory",
    "--sink",
    "dsper stream 16ch",
    "--sink",
    "dsper system 2ch",
    "--sink-width",
    "dsper stream 16ch=16",
    "--sink-width",
    "dsper system 2ch=2",
    "--alias",
    "stream=dsper stream 16ch",
    "--alias",
    "system=dsper system 2ch",
    "--alias",
    "daw=dsper daw 16ch",
];

#[test]
fn the_recommended_setup_receives_into_the_streaming_and_the_stereo_loopback() {
    let d = start(RECOMMENDED, "/stream/v1/health");
    let (_, state) = call(d.1, "GET", "/stream/v1/state", None);
    let ok = if state["available"] == true { 200 } else { 503 };
    let receive = |device: &str, channels: Value| {
        json!({"direction": "receive", "device": device, "channels": channels,
               "port": free_port().clamp(1024, 60000)})
    };
    for (id, into) in [("in", "stream"), ("stereo", "system")] {
        let (status, v) = call(
            d.1,
            "PUT",
            &format!("/stream/v1/links/{id}"),
            Some(receive(into, json!([0, 1]))),
        );
        assert_eq!(status, ok, "{into}: {v}");
    }
    let (status, v) = call(
        d.1,
        "PUT",
        "/stream/v1/links/x",
        Some(receive("system", json!([0, 2]))),
    );
    assert_eq!(status, 400, "{v}");
    assert!(v["error"]
        .as_str()
        .unwrap()
        .contains("no channel 3 (2 channels)"));
    for into in ["daw", "dsper daw 16ch", "EVO16"] {
        let (status, v) = call(
            d.1,
            "PUT",
            "/stream/v1/links/x",
            Some(receive(into, json!([0, 1]))),
        );
        assert_eq!(status, 422, "{into}: {v}");
    }
    let (status, v) = call(
        d.1,
        "PUT",
        "/stream/v1/links/out",
        Some(
            json!({"direction": "send", "device": "daw", "channels": [4, 5], "to": "127.0.0.1:20000"}),
        ),
    );
    assert_eq!(status, ok, "{v}");
    if ok == 200 {
        assert_eq!(v["device"], "dsper daw 16ch", "{v}");
    }
}

/// saqad with its stderr in a file, and what it said once it was up.
fn said(args: &[&str]) -> String {
    let dir = temp_dir();
    let log = dir.join("stderr");
    let port = free_port();
    let child = Command::new(env!("CARGO_BIN_EXE_saqad"))
        .args(args)
        .args(["--port", &port.to_string(), "--token", TOKEN])
        .env("XDG_CONFIG_HOME", &dir)
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&log).unwrap())
        .spawn()
        .unwrap();
    let _d = Daemon(child, port);
    let deadline = Instant::now() + Duration::from_secs(20);
    while agent()
        .get(format!("http://127.0.0.1:{port}/stream/v1/health"))
        .call()
        .map_or(true, |r| r.status() != 200)
    {
        assert!(Instant::now() < deadline, "saqad did not come up");
        std::thread::sleep(Duration::from_millis(100));
    }
    std::fs::read_to_string(&log).unwrap()
}

#[test]
fn saqad_says_where_a_stereo_stream_can_land() {
    let text = said(RECOMMENDED);
    assert!(
        text.contains("receives into: dsper stream 16ch (16ch), dsper system 2ch (2ch)"),
        "{text}"
    );
    assert!(!text.contains("no sink is declared 2"), "{text}");
    let text = said(&["--memory", "--sink", "dsper stream 16ch"]);
    assert!(
        text.contains("no sink is declared 2 or more channels wide"),
        "{text}"
    );
}

/// A loopback made later, for a room or a creative use, is receivable after
/// a reload, with no restart; a bad configuration changes nothing.
#[cfg(unix)]
#[test]
fn a_reload_changes_what_may_be_received() {
    let dir = temp_dir();
    let file = dir.join("saqad.json");
    std::fs::write(&file, r#"{"sinks": ["dsper stream 16ch"]}"#).unwrap();
    let d = start_in(
        &dir,
        &["--memory", "--config", file.to_str().unwrap()],
        "/stream/v1/health",
    );
    let pid = d.0.id().to_string();
    let hup = || {
        assert!(Command::new("kill")
            .args(["-HUP", &pid])
            .status()
            .unwrap()
            .success());
    };
    let booth = |port: u16| json!({"direction": "receive", "device": "ae rx booth 2ch", "channels": [0, 1], "port": port});
    let (status, v) = call(d.1, "PUT", "/stream/v1/links/booth", Some(booth(20000)));
    assert_eq!(status, 422, "not a sink yet: {v}");

    std::fs::write(&file, r#"{"sinks": ["dsper stream 16ch", "ae rx *"]}"#).unwrap();
    hup();
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        let (status, _) = call(d.1, "PUT", "/stream/v1/links/booth", Some(booth(20000)));
        if status != 422 || Instant::now() > deadline {
            break status;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(
        status == 200 || status == 503,
        "the new loopback is receivable: {status}"
    );

    std::fs::write(&file, "{ not json").unwrap();
    hup();
    std::thread::sleep(Duration::from_millis(500));
    let (status, v) = call(d.1, "PUT", "/stream/v1/links/booth", Some(booth(20010)));
    assert!(
        status == 200 || status == 503,
        "a bad file keeps what was allowed: {status} {v}"
    );
    assert_eq!(
        d.0.id().to_string(),
        pid,
        "still the same saqad, never restarted"
    );
}
