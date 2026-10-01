//! Audio between machines, over Roc (roc-toolkit).
//!
//! A **link** carries up to 16 channels one way:
//!
//! - a *send* link captures chosen channels of a device here (`dsper daw
//!   16ch` 5–6, an interface's inputs where a drum machine is plugged in)
//!   and streams them to another machine;
//! - a *receive* link listens on a port and plays what arrives into chosen
//!   channels of an input this machine allows (its [`Devices`] sinks: with
//!   dsper, one of dsper's own inputs, normally `dsper stream 16ch`), where
//!   the local mix, patches and safety check take over.
//!
//! That last rule is the safety line: audio from the network never reaches
//! an interface directly. It lands in an allowed input, and only this
//! machine's checked pipeline decides what the speakers get. A receive link
//! into anything else is refused, and with no sinks configured every receive
//! link is.
//!
//! The service keeps its links in a file and restarts them with saqad. Its
//! REST API is at `/stream/v1` (see [`rest`]).

pub mod audio;
pub mod devices;
mod pump;
pub mod rest;
pub mod service;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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
            port,
            latency_ms,
            ..
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

struct Running {
    view: Arc<Mutex<LinkView>>,
    stop: pump::Stop,
}

/// The links, their pumps, and where they are kept.
pub struct StreamService {
    audio: Arc<dyn Audio>,
    devices: Devices,
    file: Option<PathBuf>,
    links: Mutex<BTreeMap<String, (LinkSpec, Running)>>,
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
    pub fn start(
        audio: Arc<dyn Audio>,
        devices: Devices,
        file: Option<PathBuf>,
    ) -> Arc<StreamService> {
        let s = Arc::new(StreamService {
            audio,
            devices,
            file,
            links: Mutex::new(BTreeMap::new()),
        });
        let kept: BTreeMap<String, LinkSpec> = s
            .file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        for (id, spec) in kept {
            if let Err(e) = s.put(&id, spec) {
                eprintln!("saqad: stream link '{id}': {e:?}");
            }
        }
        s
    }

    /// Whether libroc is here, or what to do about it.
    pub fn available() -> Result<(), String> {
        saqa_roc::Roc::get().map(|_| ())
    }

    pub fn links(&self) -> Vec<LinkView> {
        self.links
            .lock()
            .expect("links")
            .values()
            .map(|(_, r)| r.view.lock().expect("view").clone())
            .collect()
    }

    /// Adds or replaces a link, and starts it.
    pub fn put(&self, id: &str, mut spec: LinkSpec) -> Result<LinkView, Refused> {
        check_id(id).map_err(Refused::Invalid)?;
        match &mut spec {
            LinkSpec::Send { device, .. } => *device = self.devices.resolve(device, false),
            LinkSpec::Receive { device, .. } => *device = self.devices.resolve(device, true),
        }
        let mut links = self.links.lock().expect("links");
        let others: BTreeMap<String, LinkSpec> = links
            .iter()
            .filter(|(k, _)| k.as_str() != id)
            .map(|(k, (s, _))| (k.clone(), s.clone()))
            .collect();
        check_link(&spec, &others, &self.devices)?;
        Self::available().map_err(Refused::Unavailable)?;
        if let Some((_, old)) = links.remove(id) {
            old.stop.stop_and_wait();
        }
        let view = Arc::new(Mutex::new(LinkView {
            id: id.to_string(),
            spec: spec.clone(),
            state: "starting".into(),
            detail: None,
            connections: 0,
            e2e_latency_ms: None,
            dropouts: 0,
        }));
        let stop = pump::start(self.audio.clone(), spec.clone(), view.clone());
        let now = view.lock().expect("view").clone();
        links.insert(id.to_string(), (spec, Running { view, stop }));
        drop(links);
        self.save();
        Ok(now)
    }

    pub fn delete(&self, id: &str) -> bool {
        let gone = self.links.lock().expect("links").remove(id);
        let found = gone.is_some();
        if let Some((_, r)) = gone {
            r.stop.stop_and_wait();
        }
        self.save();
        found
    }

    /// Stops every link (saqad is exiting); they are still kept.
    pub fn shutdown(&self) {
        let mut links = self.links.lock().expect("links");
        for (_, (_, r)) in std::mem::take(&mut *links) {
            r.stop.stop_and_wait();
        }
    }

    fn save(&self) {
        let Some(file) = &self.file else { return };
        let specs: BTreeMap<String, LinkSpec> = self
            .links
            .lock()
            .expect("links")
            .iter()
            .map(|(k, (s, _))| (k.clone(), s.clone()))
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
