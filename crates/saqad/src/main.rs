//! saqad: saqa's stream service on its own, at `/stream/v1`.
//!
//! ```text
//! saqad [--port 8486] [--token T] [--allow-origin URL]... [--allow-host NAME]... [--memory]
//! ```
//!
//! It serves plain HTTP on loopback (127.0.0.1) only, behind a token and a
//! Host/Origin guard; `/stream/v1/health` is open. Links are kept in
//! `$XDG_CONFIG_HOME/saqa/streams.json` (else `~/.config/saqa/streams.json`)
//! and restart with saqad. `--memory` runs links over memory devices instead of
//! the machine's (tests, trying it out), and keeps nothing.

use saqa_stream::{
    audio::MemoryAudio,
    rest,
    service::{guarded, Access},
    Audio, CpalAudio, StreamService,
};
use std::io::Read;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const DEFAULT_PORT: u16 = 8486;

fn usage() -> ! {
    eprintln!(
        "usage: saqad [--port N] [--token T] [--allow-origin URL]... [--allow-host NAME]... [--memory]"
    );
    std::process::exit(2)
}

/// `$XDG_CONFIG_HOME/saqa`, else `~/.config/saqa`.
fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("saqa")
}

fn random_token() -> String {
    let mut buf = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .expect("read /dev/urandom");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// A token that survives restarts, in `<dir>/token` (mode 0600).
fn stored_token(dir: &Path) -> String {
    let path = dir.join("token");
    if let Ok(t) = std::fs::read_to_string(&path) {
        let t = t.trim().to_string();
        if t.len() >= 16 {
            return t;
        }
    }
    let t = random_token();
    let _ = std::fs::create_dir_all(dir);
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
        {
            let _ = f.write_all(t.as_bytes());
        }
    }
    t
}

/// Ctrl-C, or SIGTERM (how launchd and systemd stop a service).
async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

#[tokio::main]
async fn main() {
    let mut port = DEFAULT_PORT;
    let mut token = None;
    let mut origins = Vec::new();
    let mut allow_hosts: Vec<String> = Vec::new();
    let mut memory = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || args.next().unwrap_or_else(|| usage());
        match a.as_str() {
            "--port" => port = value().parse().unwrap_or_else(|_| usage()),
            "--token" => token = Some(value()),
            "--allow-origin" => origins.push(value()),
            "--allow-host" => allow_hosts.push(value().to_lowercase()),
            "--memory" => memory = true,
            "-h" | "--help" => usage(),
            _ => usage(),
        }
    }
    let dir = config_dir();
    let token = token.unwrap_or_else(|| stored_token(&dir));
    if token.len() < 16 {
        eprintln!("--token must be at least 16 characters");
        std::process::exit(2);
    }

    let (audio, file): (Arc<dyn Audio>, Option<PathBuf>) = if memory {
        (Arc::new(MemoryAudio::default()), None)
    } else {
        (Arc::new(CpalAudio), Some(dir.join("streams.json")))
    };
    let st = StreamService::start(audio, file);

    let prefix = "/stream/v1/";
    let mut access = Access::loopback(token.clone(), port, &[prefix]);
    for h in &allow_hosts {
        access.hosts.push(format!("{h}:{port}"));
    }
    access.origins = origins;
    let app = guarded(
        axum::Router::new().nest("/stream/v1", rest::router(st.clone())),
        Arc::new(access),
    );
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("saqad: cannot serve on {addr}: {e}");
            std::process::exit(1);
        }
    };
    println!("saqa stream service on http://{addr}{prefix} (Authorization: Bearer {token})");
    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(stop_signal())
        .await
    {
        eprintln!("saqad: {e}");
    }
    st.shutdown();
}
