//! saqad: saqa's stream service on its own, at `/stream/v1`.
//!
//! ```text
//! saqad [--config FILE] [--port 8486] [--token-file FILE | --token T]
//!       [--sink DEVICE]... [--sink-width DEVICE=N]... [--alias NAME=DEVICE]...
//!       [--alias-send NAME=DEVICE]... [--alias-receive NAME=DEVICE]...
//!       [--links FILE] [--import-links FILE]
//!       [--allow-origin URL]... [--allow-host NAME]... [--memory]
//! ```
//!
//! It serves plain HTTP on loopback (127.0.0.1) only, behind a token and a
//! Host/Origin guard; `/stream/v1/health` is open. What it may play into and
//! what the role words mean come from whoever runs it (docs/CONFIG.md);
//! without sinks it refuses every receive link. On SIGHUP it reads its
//! sinks, widths and aliases again (a loopback made for a new room needs no
//! restart), and re-checks every link against them. Links are kept in
//! `<config dir>/streams.json` (`$XDG_CONFIG_HOME/saqa`, else `~/.config/saqa`)
//! and restart with saqad. `--memory` runs links over memory devices instead of
//! the machine's (tests, trying it out), as wide as `--sink-width` says, and
//! keeps nothing.

mod config;

use config::{Config, Token};
use saqa_stream::{
    audio::MemoryAudio,
    rest,
    service::{guarded, Access},
    Audio, CpalAudio, Devices, LinkSpec, StreamService,
};
use std::collections::BTreeMap;
use std::io::Read;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
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

/// Says where a received stream may land, and warns when nothing shows a
/// stereo stream has somewhere to go.
fn say_where(devices: &Devices) {
    if devices.sinks.is_empty() {
        return;
    }
    eprintln!("saqad: receives into: {}", devices.summary());
    if !devices.stereo_ready() {
        eprintln!("saqad: {NO_STEREO}");
    }
}

const NO_STEREO: &str = "no sink is declared 2 or more channels wide (--sink-width DEVICE=N): \
                         nothing shows a stereo stream has somewhere to land";

/// Under `--memory`, the memory devices are as wide as the widths say.
fn sized(memory: &MemoryAudio, devices: &Devices) {
    for (device, &n) in &devices.widths {
        memory.set_width(device, n);
    }
}

/// On SIGHUP, reads the configuration again and applies its devices (sinks,
/// widths, aliases); every link is re-checked against them. Everything else
/// needs a restart, and a bad configuration changes nothing.
#[cfg(unix)]
async fn reload_on_hangup(
    mut hup: tokio::signal::unix::Signal,
    args: Vec<String>,
    dir: PathBuf,
    mut now: Config,
    memory: Option<MemoryAudio>,
    st: Arc<StreamService>,
) {
    while hup.recv().await.is_some() {
        let c = match Config::parse(&args, &dir) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("saqad: reload: {e}; keeping the sinks and aliases it had");
                continue;
            }
        };
        for (key, same) in [
            ("port", c.port == now.port),
            ("token", c.token == now.token),
            ("links", c.links == now.links),
            ("import_links", c.import_links == now.import_links),
            ("allow_origins", c.allow_origins == now.allow_origins),
            ("allow_hosts", c.allow_hosts == now.allow_hosts),
        ] {
            if !same {
                eprintln!("saqad: reload: {key} changes need a restart");
            }
        }
        if let Some(m) = &memory {
            sized(m, &c.devices);
        }
        let devices = c.devices.clone();
        let st2 = st.clone();
        match tokio::task::spawn_blocking(move || st2.set_devices(devices)).await {
            Ok(Ok(done)) => {
                eprintln!(
                    "saqad: reloaded: receives into: {}; waiting now: {}; running again: {}",
                    c.devices.summary(),
                    list(&done.parked),
                    list(&done.resumed)
                );
                if !c.devices.stereo_ready() && !c.devices.sinks.is_empty() {
                    eprintln!("saqad: {NO_STEREO}");
                }
                now = c;
            }
            Ok(Err(e)) => eprintln!("saqad: reload: {e}; keeping the sinks and aliases it had"),
            Err(e) => eprintln!("saqad: reload: {e}"),
        }
    }
}

#[cfg(unix)]
fn list(ids: &[String]) -> String {
    if ids.is_empty() {
        "none".into()
    } else {
        ids.join(", ")
    }
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
    say_where(&c.devices);

    let memory = c.memory.then(MemoryAudio::default);
    if let Some(m) = &memory {
        sized(m, &c.devices);
    }
    let audio: Arc<dyn Audio> = match &memory {
        Some(m) => Arc::new(m.clone()),
        None => Arc::new(CpalAudio),
    };
    let st = StreamService::start(audio, c.devices.clone(), c.links.clone());
    // Installed before saqad answers: SIGHUP's default would end it.
    #[cfg(unix)]
    let hup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup()).ok();

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
    #[cfg(unix)]
    match hup {
        Some(hup) => {
            tokio::spawn(reload_on_hangup(hup, args, dir, c, memory, st.clone()));
        }
        None => eprintln!("saqad: no SIGHUP handler: changing devices needs a restart"),
    }
    #[cfg(not(unix))]
    let _ = (args, dir, c, memory);
    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(stop_signal())
        .await
    {
        eprintln!("saqad: {e}");
    }
    st.shutdown();
}
