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

fn start(args: &[&str], health: &str) -> Daemon {
    let dir = std::env::temp_dir().join(format!("saqad-{}-{}", std::process::id(), free_port()));
    std::fs::create_dir_all(&dir).unwrap();
    let port = free_port();
    let child = Command::new(env!("CARGO_BIN_EXE_saqad"))
        .args(args)
        .args(["--port", &port.to_string(), "--token", TOKEN])
        .env("XDG_CONFIG_HOME", &dir)
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
    let d = start(&["--memory"], "/stream/v1/health");
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
