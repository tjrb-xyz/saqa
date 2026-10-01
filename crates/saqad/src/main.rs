//! saqad: saqa's stream service on its own, at `/stream/v1`.
//!
//! ```text
//! saqad [--config FILE] [--port 8486] [--token-file FILE | --token T]
//!       [--sink DEVICE]... [--alias NAME=DEVICE]...
//!       [--alias-send NAME=DEVICE]... [--alias-receive NAME=DEVICE]...
//!       [--links FILE] [--import-links FILE]
//!       [--allow-origin URL]... [--allow-host NAME]... [--memory]
//! ```
//!
//! It serves plain HTTP on loopback (127.0.0.1) only, behind a token and a
//! Host/Origin guard; `/stream/v1/health` is open. What it may play into and
//! what the role words mean come from whoever runs it (docs/CONFIG.md);
//! without sinks it refuses every receive link. Links are kept in
//! `<config dir>/streams.json` (`$XDG_CONFIG_HOME/saqa`, else `~/.config/saqa`)
//! and restart with saqad. `--memory` runs links over memory devices instead of
//! the machine's (tests, trying it out), and keeps nothing.

mod config;

use config::{Config, Token};
use saqa_stream::{
    audio::MemoryAudio,
    rest,
    service::{guarded, Access},
    Audio, CpalAudio, LinkSpec, StreamService,
};
use std::collections::BTreeMap;
use std::io::Read;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

fn random_token() -> String {
    let mut buf = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .expect("read /dev/urandom");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// The token in `path`, or a new one written there (mode 0600) when it has none.
fn stored_token(path: &Path) -> Result<String, String> {
    if let Ok(t) = std::fs::read_to_string(path) {
        let t = t.trim().to_string();
        if t.len() >= 16 {
            return Ok(t);
        }
    }
    let t = random_token();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    std::io::Write::write_all(
        &mut o
            .open(path)
            .map_err(|e| format!("{}: {e}", path.display()))?,
        t.as_bytes(),
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(t)
}

/// Copies the links in `from` (dsper's `streams.json`: the same format) to
/// `to`, once: only while saqad keeps no links file of its own.
fn import_links(from: &Path, to: &Path) -> Result<Option<usize>, String> {
    if to.exists() || !from.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(from).map_err(|e| format!("{}: {e}", from.display()))?;
    let links: BTreeMap<String, LinkSpec> =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", from.display()))?;
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(to, text).map_err(|e| format!("{}: {e}", to.display()))?;
    Ok(Some(links.len()))
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

fn fail(e: String) -> ! {
    eprintln!("saqad: {e}");
    std::process::exit(2)
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{}", config::USAGE);
        return;
    }
    let dir = config::config_dir();
    let c = Config::parse(&args, &dir).unwrap_or_else(|e| fail(format!("{e}\n{}", config::USAGE)));
    let token = match &c.token {
        Token::Given(t) => t.clone(),
        Token::File(p) => stored_token(p).unwrap_or_else(|e| fail(e)),
    };
    if token.len() < 16 {
        fail("the token must be at least 16 characters".into());
    }
    if let (Some(from), Some(to)) = (&c.import_links, &c.links) {
        match import_links(from, to) {
            Ok(Some(n)) => eprintln!(
                "saqad: kept {n} links from {} in {}",
                from.display(),
                to.display()
            ),
            Ok(None) => {}
            Err(e) => fail(format!("--import-links: {e}")),
        }
    }
    if c.devices.sinks.is_empty() {
        eprintln!("saqad: no --sink given: every receive link is refused");
    }

    let audio: Arc<dyn Audio> = if c.memory {
        Arc::new(MemoryAudio::default())
    } else {
        Arc::new(CpalAudio)
    };
    let st = StreamService::start(audio, c.devices.clone(), c.links.clone());

    let prefix = "/stream/v1/";
    let mut access = Access::loopback(token.clone(), c.port, &[prefix]);
    for h in &c.allow_hosts {
        access.hosts.push(format!("{h}:{}", c.port));
    }
    access.origins = c.allow_origins.clone();
    let app = guarded(
        axum::Router::new().nest("/stream/v1", rest::router(st.clone())),
        Arc::new(access),
    );
    let addr = SocketAddr::from(([127, 0, 0, 1], c.port));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("saqad: cannot serve on {addr}: {e}");
            std::process::exit(1);
        }
    };
    let whose = match &c.token {
        Token::Given(_) => "the one given".to_string(),
        Token::File(p) => format!("in {}", p.display()),
    };
    println!(
        "saqa stream service on http://{addr}{prefix} (Authorization: Bearer <token {whose}>)"
    );
    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(stop_signal())
        .await
    {
        eprintln!("saqad: {e}");
    }
    st.shutdown();
}
