//! Audio between machines, over Roc (roc-toolkit).
//!
//! A **link** carries up to 16 channels one way:
//!
//! - a *send* link captures chosen channels of a device here (a loopback
//!   such as the DAW's, 5–6, or an interface's inputs where a drum machine is
//!   plugged in) and streams them to another machine;
//! - a *receive* link listens on a port and plays what arrives into chosen
//!   channels of one of audio-engine's loopbacks that saqad is told it may
//!   play into (its [`Devices`] sinks): the streaming loopback, a 2-channel
//!   loopback for stereo, and any made later for a room or a creative use.
//!   dsper routes it from there, through its DSP.
//!
//! That last rule is the safety line: audio from the network never reaches
//! an interface directly. It lands in a loopback, and only this machine's
//! engine and dsper decide what the speakers get. A receive link into
//! anything else is refused, and with no sinks configured every receive link
//! is.
//!
//! The service keeps its links in a file and restarts them with saqad. A
//! kept link that cannot run now waits (`failed`, detail [`WAITING`]…) and
//! starts by itself when [`StreamService::set_devices`] lets it. Its REST API
//! is at `/stream/v1` (see [`rest`]).

pub mod audio;
pub mod devices;
mod pump;
pub mod rest;
pub mod service;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

pub use audio::{Audio, CpalAudio};
pub use devices::{Alias, Devices};

/// One link, as it is asked for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "direction", rename_all = "lowercase")]
pub enum LinkSpec {
    /// Capture `channels` (0-based) of `device` and stream them to `to` (`host:port`).
    Send {
        device: String,
        channels: Vec<u32>,
        to: String,
    },
    /// Listen on `port` and play the stream into `channels` of `device`, an allowed input.
    Receive {
        device: String,
        channels: Vec<u32>,
        port: u16,
        /// What is kept buffered against the network's jitter.
        #[serde(default = "default_latency")]
        latency_ms: u32,
    },
}

fn default_latency() -> u32 {
    100
}

impl LinkSpec {
    pub fn device(&self) -> &str {
        match self {
            LinkSpec::Send { device, .. } | LinkSpec::Receive { device, .. } => device,
        }
    }
    pub fn channels(&self) -> &[u32] {
        match self {
            LinkSpec::Send { channels, .. } | LinkSpec::Receive { channels, .. } => channels,
        }
    }
}

/// `host:port` (or `[v6]:port`), with room for the three ports a stream uses.
pub fn parse_peer(to: &str) -> Result<saqa_roc::Address, String> {
    let (host, port) = to
        .rsplit_once(':')
        .ok_or_else(|| format!("'{to}': give host:port"))?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let port: u16 = port
        .parse()
        .map_err(|_| format!("'{to}': the port is not a number"))?;
    if host.is_empty() {
        return Err(format!("'{to}': give a host"));
    }
    check_port(port)?;
    Ok(saqa_roc::Address {
        host: host.to_string(),
        port,
    })
}

/// A stream uses three UDP ports from its base: P, P+1, P+2.
fn check_port(port: u16) -> Result<(), String> {
    if !(1024..=65533).contains(&port) {
        return Err(format!(
            "port {port}: choose 1024–65533 (a stream uses it and the next two)"
        ));
    }
    Ok(())
}

/// Why a link cannot be what it asks, before anything is opened.
pub fn check(
    spec: &LinkSpec,
    others: &BTreeMap<String, LinkSpec>,
    devices: &Devices,
) -> Result<(), String> {
    check_link(spec, others, devices).map_err(|r| match r {
        Refused::Invalid(m) | Refused::NotAnInput(m) | Refused::Unavailable(m) => m,
    })
}

fn check_link(
    spec: &LinkSpec,
    others: &BTreeMap<String, LinkSpec>,
    devices: &Devices,
) -> Result<(), Refused> {
    let invalid = Refused::Invalid;
    let ch = spec.channels();
    if ch.is_empty() || ch.len() > saqa_roc::MAX_CHANNELS as usize {
        return Err(invalid(format!(
            "a link carries 1–{} channels",
            saqa_roc::MAX_CHANNELS
        )));
    }
    let mut seen = ch.to_vec();
    seen.sort();
    seen.dedup();
    if seen.len() != ch.len() {
        return Err(invalid("each channel once".into()));
    }
    match spec {
        LinkSpec::Send { to, .. } => {
            parse_peer(to).map_err(invalid)?;
        }
        LinkSpec::Receive {
            device,
            channels,
            port,
            latency_ms,
        } => {
            if !devices.allows(device) {
                let allowed = if devices.sinks.is_empty() {
                    "none is configured here".to_string()
                } else {
                    devices.sinks.join(", ")
                };
                return Err(Refused::NotAnInput(format!(
                    "a stream plays only into an input this machine allows ({allowed}), \
                     never straight into '{device}': this machine's checked pipeline decides \
                     what reaches speakers"
                )));
            }
            if let Some(w) = devices.width(device) {
                if let Some(c) = channels.iter().find(|&&c| c >= u32::from(w)) {
                    return Err(invalid(format!(
                        "{device} has no channel {} ({w} channels)",
                        c + 1
                    )));
                }
            }
            check_port(*port).map_err(invalid)?;
            if !(10..=2000).contains(latency_ms) {
                return Err(invalid("latency: 10–2000 ms".into()));
            }
            for (id, o) in others {
                if let LinkSpec::Receive { port: p, .. } = o {
                    if p.abs_diff(*port) < 3 {
                        return Err(invalid(format!(
                            "ports {port}–{} overlap link '{id}'",
                            port + 2
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

/// A link as it runs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LinkView {
    pub id: String,
    #[serde(flatten)]
    pub spec: LinkSpec,
    /// `starting`, `running`, `failed`.
    pub state: String,
    pub detail: Option<String>,
    /// Receive: senders streaming to it now.
    pub connections: u32,
    /// Receive: end-to-end latency, when Roc knows it (`latency_ms` is the target).
    pub e2e_latency_ms: Option<f64>,
    /// Audio the device could not take or give in time (dropouts), since start.
    pub dropouts: u64,
}

/// A kept link that cannot run now (its device is not allowed, or there is
/// no libroc) shows as `failed`, with a detail that starts with this. It
/// starts by itself when it can.
pub const WAITING: &str = "waiting: ";

struct Link {
    spec: LinkSpec,
    view: Arc<Mutex<LinkView>>,
    /// The pump while the link runs; `None` while it waits.
    run: Option<pump::Stop>,
}

/// The links, their pumps, what may be received, and where links are kept.
///
/// Locks are taken in one order: `devices`, then `links`.
pub struct StreamService {
    audio: Arc<dyn Audio>,
    devices: RwLock<Devices>,
    file: Option<PathBuf>,
    links: Mutex<BTreeMap<String, Link>>,
}

/// What a change of devices did to the links.
#[derive(Debug, Default, PartialEq)]
pub struct Reconciled {
    /// Links that ran, and now wait: their device is no longer allowed.
    pub parked: Vec<String>,
    /// Links that waited, and now run.
    pub resumed: Vec<String>,
}

/// What a link becomes when what may be received changes.
#[derive(Debug, PartialEq)]
enum Action {
    Keep,
    Park(String),
    Resume,
}

/// A pure decision: the same check a PUT runs, against the new devices.
fn plan(
    spec: &LinkSpec,
    others: &BTreeMap<String, LinkSpec>,
    devices: &Devices,
    running: bool,
    roc: &Result<(), String>,
) -> Action {
    match check_link(spec, others, devices) {
        Err(r) => Action::Park(text(r)),
        Ok(()) if running => Action::Keep,
        Ok(()) => match roc {
            Ok(()) => Action::Resume,
            Err(m) => Action::Park(m.clone()),
        },
    }
}

fn text(r: Refused) -> String {
    match r {
        Refused::Invalid(m) | Refused::NotAnInput(m) | Refused::Unavailable(m) => m,
    }
}

fn new_view(id: &str, spec: &LinkSpec) -> LinkView {
    LinkView {
        id: id.to_string(),
        spec: spec.clone(),
        state: "starting".into(),
        detail: None,
        connections: 0,
        e2e_latency_ms: None,
        dropouts: 0,
    }
}

fn wait(view: &Mutex<LinkView>, why: &str) {
    let mut v = view.lock().expect("view");
    v.state = "failed".into();
    v.detail = Some(format!("{WAITING}{why}"));
    v.connections = 0;
    v.e2e_latency_ms = None;
}

fn resolve(devices: &Devices, spec: &mut LinkSpec) {
    match spec {
        LinkSpec::Send { device, .. } => *device = devices.resolve(device, false),
        LinkSpec::Receive { device, .. } => *device = devices.resolve(device, true),
    }
}

fn others(links: &BTreeMap<String, Link>, id: &str) -> BTreeMap<String, LinkSpec> {
    links
        .iter()
        .filter(|(k, _)| k.as_str() != id)
        .map(|(k, l)| (k.clone(), l.spec.clone()))
        .collect()
}

/// Link ids are what people name them: letters, digits, '-'.
fn check_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 40 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("a link's name is letters, digits and '-'".into());
    }
    Ok(())
}

/// What refused a change: bad input, a receive link into an input this
/// machine does not allow, or no libroc.
#[derive(Debug, PartialEq)]
pub enum Refused {
    Invalid(String),
    NotAnInput(String),
    Unavailable(String),
}

impl StreamService {
    /// Starts the links kept in `file` (if any), with what `devices` allows.
    /// A kept link that cannot start now is kept, waiting, never dropped.
    pub fn start(
        audio: Arc<dyn Audio>,
        devices: Devices,
        file: Option<PathBuf>,
    ) -> Arc<StreamService> {
        let s = Arc::new(StreamService {
            audio,
            devices: RwLock::new(devices),
            file,
            links: Mutex::new(BTreeMap::new()),
        });
        let kept: BTreeMap<String, LinkSpec> = s
            .file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        if kept.is_empty() {
            return s;
        }
        {
            let devices = s.devices.read().expect("devices");
            let mut links = s.links.lock().expect("links");
            for (id, mut spec) in kept {
                resolve(&devices, &mut spec);
                if let Err(r) = s.admit(&devices, &mut links, &id, spec.clone()) {
                    let why = text(r);
                    eprintln!("saqad: stream link '{id}': {WAITING}{why}");
                    let view = Arc::new(Mutex::new(new_view(&id, &spec)));
                    wait(&view, &why);
                    links.insert(
                        id,
                        Link {
                            spec,
                            view,
                            run: None,
                        },
                    );
                }
            }
        }
        s.save();
        s
    }

    /// Whether libroc is here, or what to do about it.
    pub fn available() -> Result<(), String> {
        saqa_roc::Roc::get().map(|_| ())
    }

    /// What may be received now.
    pub fn devices(&self) -> Devices {
        self.devices.read().expect("devices").clone()
    }

    pub fn links(&self) -> Vec<LinkView> {
        self.links
            .lock()
            .expect("links")
            .values()
            .map(|l| l.view.lock().expect("view").clone())
            .collect()
    }

    /// Checks `spec` (already resolved) and starts it as `id`, replacing a
    /// link of that id. A refusal leaves the links as they were.
    fn admit(
        &self,
        devices: &Devices,
        links: &mut BTreeMap<String, Link>,
        id: &str,
        spec: LinkSpec,
    ) -> Result<LinkView, Refused> {
        check_id(id).map_err(Refused::Invalid)?;
        check_link(&spec, &others(links, id), devices)?;
        Self::available().map_err(Refused::Unavailable)?;
        if let Some(run) = links.remove(id).and_then(|old| old.run) {
            run.stop_and_wait();
        }
        let view = Arc::new(Mutex::new(new_view(id, &spec)));
        let run = pump::start(self.audio.clone(), spec.clone(), view.clone());
        let now = view.lock().expect("view").clone();
        links.insert(
            id.to_string(),
            Link {
                spec,
                view,
                run: Some(run),
            },
        );
        Ok(now)
    }

    /// Adds or replaces a link, and starts it.
    pub fn put(&self, id: &str, mut spec: LinkSpec) -> Result<LinkView, Refused> {
        let devices = self.devices.read().expect("devices");
        resolve(&devices, &mut spec);
        let mut links = self.links.lock().expect("links");
        let now = self.admit(&devices, &mut links, id, spec)?;
        drop(links);
        drop(devices);
        self.save();
        Ok(now)
    }

    pub fn delete(&self, id: &str) -> bool {
        let Some(gone) = self.links.lock().expect("links").remove(id) else {
            return false;
        };
        if let Some(run) = gone.run {
            run.stop_and_wait();
        }
        self.save();
        true
    }

    /// Changes what may be received (a reload, or later the engine's
    /// loopbacks), and re-checks every link against it with the check a PUT
    /// runs: a running link its device no longer allows stops and waits; a
    /// waiting link that now fits starts. A bad set changes nothing.
    pub fn set_devices(&self, new: Devices) -> Result<Reconciled, String> {
        new.check()?;
        *self.devices.write().expect("devices") = new;
        let roc = Self::available();
        let mut done = Reconciled::default();
        {
            let devices = self.devices.read().expect("devices");
            let mut links = self.links.lock().expect("links");
            let ids: Vec<String> = links.keys().cloned().collect();
            for id in ids {
                let others = others(&links, &id);
                let link = links.get_mut(&id).expect("listed");
                match plan(&link.spec, &others, &devices, link.run.is_some(), &roc) {
                    Action::Keep => {}
                    Action::Park(why) => {
                        if let Some(run) = link.run.take() {
                            run.stop_and_wait();
                            done.parked.push(id.clone());
                        }
                        wait(&link.view, &why);
                    }
                    Action::Resume => {
                        *link.view.lock().expect("view") = new_view(&id, &link.spec);
                        link.run = Some(pump::start(
                            self.audio.clone(),
                            link.spec.clone(),
                            link.view.clone(),
                        ));
                        done.resumed.push(id.clone());
                    }
                }
            }
        }
        self.save();
        Ok(done)
    }

    /// Stops every link (saqad is exiting); they are still kept.
    pub fn shutdown(&self) {
        let mut links = self.links.lock().expect("links");
        for (_, l) in std::mem::take(&mut *links) {
            if let Some(run) = l.run {
                run.stop_and_wait();
            }
        }
    }

    fn save(&self) {
        let Some(file) = &self.file else { return };
        let specs: BTreeMap<String, LinkSpec> = self
            .links
            .lock()
            .expect("links")
            .iter()
            .map(|(k, l)| (k.clone(), l.spec.clone()))
            .collect();
        let text = serde_json::to_string_pretty(&specs).expect("links serialize");
        let tmp = file.with_extension("tmp");
        if let Err(e) = std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, file)) {
            eprintln!(
                "saqad: could not keep stream links in {}: {e}",
                file.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recv(device: &str, port: u16) -> LinkSpec {
        LinkSpec::Receive {
            device: device.into(),
            channels: vec![0, 1],
            port,
            latency_ms: 100,
        }
    }

    /// What dsper tells saqa on macOS (docs/CONFIG.md).
    fn dsper() -> Devices {
        let mut d = Devices {
            sinks: vec![
                "dsper system 2ch".into(),
                "dsper daw 16ch".into(),
                "dsper stream 16ch".into(),
            ],
            ..Default::default()
        };
        d.set_alias("stream=dsper stream 16ch", None).unwrap();
        d
    }

    #[test]
    fn streams_land_only_in_allowed_inputs() {
        let e = check(&recv("EVO16", 20000), &BTreeMap::new(), &dsper()).unwrap_err();
        assert!(e.contains("never straight into 'EVO16'"), "{e}");
        assert!(
            e.contains("dsper stream 16ch"),
            "says which are allowed: {e}"
        );
        assert!(check(
            &recv("dsper stream 16ch", 20000),
            &BTreeMap::new(),
            &dsper()
        )
        .is_ok());
        let none = Devices::default();
        let e = check(&recv("dsper stream 16ch", 20000), &BTreeMap::new(), &none).unwrap_err();
        assert!(e.contains("none is configured"), "{e}");
        assert_eq!(
            check_link(&recv("EVO16", 80), &BTreeMap::new(), &dsper()),
            Err(Refused::NotAnInput(
                check(&recv("EVO16", 20000), &BTreeMap::new(), &dsper()).unwrap_err()
            )),
            "an interface is refused as such, whatever else is wrong"
        );
        let mut bad_channels = recv("EVO16", 20000);
        if let LinkSpec::Receive { channels, .. } = &mut bad_channels {
            channels.clear();
        }
        assert!(matches!(
            check_link(&bad_channels, &BTreeMap::new(), &dsper()),
            Err(Refused::Invalid(_))
        ));
        // The 2-channel loopback takes a stereo stream; without it as a
        // sink, it is refused like any device not allowed.
        assert!(check(&recv("dsper system 2ch", 20000), &BTreeMap::new(), &dsper()).is_ok());
        let stream_only = Devices {
            sinks: vec!["dsper stream 16ch".into()],
            ..Default::default()
        };
        assert!(matches!(
            check_link(
                &recv("dsper system 2ch", 20000),
                &BTreeMap::new(),
                &stream_only
            ),
            Err(Refused::NotAnInput(_))
        ));
    }

    fn recv_on(device: &str, channels: Vec<u32>) -> LinkSpec {
        LinkSpec::Receive {
            device: device.into(),
            channels,
            port: 20000,
            latency_ms: 100,
        }
    }

    #[test]
    fn a_declared_width_is_checked_after_the_safety_line() {
        let none = BTreeMap::new();
        let mut d = dsper();
        assert!(
            check(&recv_on("dsper system 2ch", vec![0, 2]), &none, &d).is_ok(),
            "no width declared: the device's own width is checked when it opens"
        );
        d.set_width("dsper system 2ch=2").unwrap();
        for ok in [vec![0, 1], vec![1, 0], vec![1]] {
            assert!(check(&recv_on("dsper system 2ch", ok), &none, &d).is_ok());
        }
        assert_eq!(
            check_link(&recv_on("dsper system 2ch", vec![0, 2]), &none, &d),
            Err(Refused::Invalid(
                "dsper system 2ch has no channel 3 (2 channels)".into()
            ))
        );
        assert!(
            matches!(
                check_link(&recv_on("EVO16", vec![0, 9]), &none, &d),
                Err(Refused::NotAnInput(_))
            ),
            "an interface is refused as such first"
        );
    }

    #[test]
    fn links_follow_what_is_allowed() {
        let none = BTreeMap::new();
        let roc: Result<(), String> = Ok(());
        let no_roc: Result<(), String> = Err("streaming needs libroc".into());
        let system = recv_on("dsper system 2ch", vec![0, 1]);
        let stream_only = Devices {
            sinks: vec!["dsper stream 16ch".into()],
            ..Default::default()
        };
        match plan(&system, &none, &stream_only, true, &roc) {
            Action::Park(why) => assert!(
                why.contains("never straight into 'dsper system 2ch'"),
                "{why}"
            ),
            other => panic!("{other:?}"),
        }
        assert_eq!(plan(&system, &none, &dsper(), false, &roc), Action::Resume);
        assert_eq!(
            plan(&system, &none, &dsper(), false, &no_roc),
            Action::Park("streaming needs libroc".into())
        );
        assert_eq!(plan(&system, &none, &dsper(), true, &roc), Action::Keep);
        let send = LinkSpec::Send {
            device: "dsper daw 16ch".into(),
            channels: vec![4, 5],
            to: "corner.local:20000".into(),
        };
        assert_eq!(
            plan(&send, &none, &Devices::default(), true, &roc),
            Action::Keep,
            "a send link needs no sink"
        );
        let mut mono = dsper();
        mono.set_width("dsper system 2ch=1").unwrap();
        match plan(&system, &none, &mono, true, &roc) {
            Action::Park(why) => assert!(why.contains("no channel 2"), "{why}"),
            other => panic!("{other:?}"),
        }
        let waiting: BTreeMap<String, LinkSpec> =
            [("parked".into(), recv("dsper stream 16ch", 20001))].into();
        assert!(
            matches!(
                plan(&system, &waiting, &dsper(), false, &roc),
                Action::Park(why) if why.contains("overlap link 'parked'")
            ),
            "a waiting link still holds its ports"
        );
    }

    #[test]
    fn links_are_checked_before_anything_opens() {
        let none = BTreeMap::new();
        let d = dsper();
        let check = |spec: &LinkSpec, others: &BTreeMap<String, LinkSpec>| check(spec, others, &d);
        let send = |to: &str, ch: Vec<u32>| LinkSpec::Send {
            device: "dsper daw 16ch".into(),
            channels: ch,
            to: to.into(),
        };
        assert!(check(&send("corner.local:20000", vec![4, 5]), &none).is_ok());
        assert!(check(&send("[fe80::1]:20000", vec![0]), &none).is_ok());
        assert!(check(&send("corner.local", vec![0]), &none).is_err());
        assert!(check(&send(":20000", vec![0]), &none).is_err());
        assert!(check(&send("pi:80", vec![0]), &none)
            .unwrap_err()
            .contains("1024"));
        assert!(check(&send("pi:20000", vec![]), &none).is_err());
        assert!(check(&send("pi:20000", (0..17).collect()), &none).is_err());
        assert!(check(&send("pi:20000", vec![1, 1]), &none).is_err());
        let taken: BTreeMap<String, LinkSpec> =
            [("a".into(), recv("dsper stream 16ch", 20000))].into();
        assert!(check(&recv("dsper stream 16ch", 20002), &taken)
            .unwrap_err()
            .contains("overlap link 'a'"));
        assert!(check(&recv("dsper stream 16ch", 20003), &taken).is_ok());
        assert_eq!(check_id("vintage-corner"), Ok(()));
        assert!(check_id("a b").is_err());
    }

    #[test]
    fn a_link_view_names_each_field_once() {
        let v = LinkView {
            id: "x".into(),
            spec: recv("dsper stream 16ch", 20000),
            state: "running".into(),
            detail: None,
            connections: 1,
            e2e_latency_ms: Some(42.0),
            dropouts: 0,
        };
        let text = serde_json::to_string(&v).unwrap();
        assert_eq!(text.matches("\"latency_ms\"").count(), 1, "{text}");
        let back: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            (back["latency_ms"].clone(), back["e2e_latency_ms"].clone()),
            (100.into(), 42.0.into())
        );
    }

    #[test]
    fn specs_are_plain_json() {
        let s: LinkSpec = serde_json::from_str(
            r#"{"direction": "receive", "device": "dsper stream 16ch", "channels": [0, 1], "port": 20000}"#,
        )
        .unwrap();
        assert_eq!(
            s,
            recv("dsper stream 16ch", 20000),
            "latency defaults to 100 ms"
        );
    }
}
