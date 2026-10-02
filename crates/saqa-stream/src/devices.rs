//! What saqa is told about this machine's devices, by whoever runs it
//! (docs/CONFIG.md). Until saqa can ask audio-engine for its loopbacks, this
//! is how it learns which devices are the engine's loopbacks it may play into:
//!
//! - **sinks**: the loopbacks a received stream may play into, any number of
//!   them and of any width: the streaming loopback, at least one 2-channel
//!   loopback, and whatever loopbacks are made later for a room or a creative
//!   use. A sink is an exact name or one `*` pattern (`ae rx *`). A receive
//!   link into anything else is refused (422). With none, no receive link is
//!   allowed: saqa never guesses which devices are safe to play into.
//! - **widths**: how many channels a sink has, by its exact name, when
//!   known. A receive link past a declared width is refused before anything
//!   opens (400); without one, the device's own width is checked when it
//!   opens.
//! - **aliases**: words that name a device here, such as `stream` for the
//!   streaming loopback (`stream in 16ch` on a machine with dsper today).
//!   An alias may name one device, or one to capture from (`send`) and
//!   another to play into (`receive`), as an ALSA loopback does
//!   (`hw:CARD=stream,DEV=1` and `DEV=0`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The device an alias names: one for both directions, or one each.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Alias {
    Both(String),
    Split { send: String, receive: String },
}

impl Alias {
    pub fn device(&self, receive: bool) -> &str {
        match self {
            Alias::Both(d) => d,
            Alias::Split { send, receive: r } => {
                if receive {
                    r
                } else {
                    send
                }
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Devices {
    /// Device names a receive link may play into; `*` matches any run of characters.
    #[serde(default)]
    pub sinks: Vec<String>,
    #[serde(default)]
    pub aliases: BTreeMap<String, Alias>,
    /// Declared channel counts of sinks, by exact device name.
    #[serde(default)]
    pub widths: BTreeMap<String, u16>,
}

/// The widest loopback a width may declare.
pub const MAX_WIDTH: u16 = 64;

/// `pattern` matches all of `name`, `*` standing for any run of characters.
fn matches(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        return rest.is_empty();
    };
    for p in middle {
        match rest.find(p) {
            Some(i) => rest = &rest[i + p.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

/// An alias word: letters, digits, '-' and '_'.
fn check_word(word: &str) -> Result<(), String> {
    if word.is_empty()
        || !word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "alias '{word}': a word of letters, digits, '-' and '_'"
        ));
    }
    Ok(())
}

impl Devices {
    /// Whether a received stream may play into `device`.
    pub fn allows(&self, device: &str) -> bool {
        self.sinks.iter().any(|p| matches(p, device))
    }

    /// The declared width of `device`, if it has one.
    pub fn width(&self, device: &str) -> Option<u16> {
        self.widths.get(device).copied()
    }

    /// `DEVICE=N` (`--sink-width`): declares a sink's channel count. Split at
    /// the last `=`, so ALSA names (`hw:CARD=x,DEV=0=2`) work.
    pub fn set_width(&mut self, arg: &str) -> Result<(), String> {
        let (device, n) = arg
            .rsplit_once('=')
            .filter(|(d, _)| !d.is_empty())
            .ok_or_else(|| format!("'{arg}': give DEVICE=N"))?;
        let n: u16 = n
            .parse()
            .ok()
            .filter(|n| (1..=MAX_WIDTH).contains(n))
            .ok_or_else(|| format!("'{arg}': the width is 1–{MAX_WIDTH} channels"))?;
        self.widths.insert(device.to_string(), n);
        Ok(())
    }

    /// Where a received stream may land, for a log line: each sink, and its
    /// width when declared.
    pub fn summary(&self) -> String {
        if self.sinks.is_empty() {
            return "none".into();
        }
        self.sinks
            .iter()
            .map(|s| match self.width(s) {
                Some(n) => format!("{s} ({n}ch)"),
                None => format!("{s} (width not declared)"),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Whether some sink is declared 2 or more channels wide: somewhere a
    /// stereo stream can land.
    pub fn stereo_ready(&self) -> bool {
        self.widths.values().any(|&n| n >= 2)
    }

    /// An alias → its device for this direction; anything else is a device name already.
    pub fn resolve(&self, device: &str, receive: bool) -> String {
        match self.aliases.get(device) {
            Some(a) => a.device(receive).to_string(),
            None => device.to_string(),
        }
    }

    /// `NAME=DEVICE` (`--alias`), `--alias-send` or `--alias-receive`: sets
    /// that alias, or one direction of it.
    pub fn set_alias(&mut self, arg: &str, only: Option<Direction>) -> Result<(), String> {
        let (word, device) = arg
            .split_once('=')
            .filter(|(_, d)| !d.is_empty())
            .ok_or_else(|| format!("'{arg}': give NAME=DEVICE"))?;
        check_word(word)?;
        let device = device.to_string();
        let new = match (only, self.aliases.get(word)) {
            (None, _) => Alias::Both(device),
            (Some(dir), old) => {
                let (mut send, mut receive) = match old {
                    Some(a) => (a.device(false).to_string(), a.device(true).to_string()),
                    None => (device.clone(), device.clone()),
                };
                match dir {
                    Direction::Send => send = device,
                    Direction::Receive => receive = device,
                }
                if send == receive {
                    Alias::Both(send)
                } else {
                    Alias::Split { send, receive }
                }
            }
        };
        self.aliases.insert(word.to_string(), new);
        Ok(())
    }

    /// Every alias word is a word; no sink could be every device; every
    /// width names a sink exactly and is 1–64.
    pub fn check(&self) -> Result<(), String> {
        self.aliases.keys().try_for_each(|w| check_word(w))?;
        for sink in &self.sinks {
            if sink.chars().all(|c| c == '*') {
                return Err(format!(
                    "sink '{sink}': a pattern that matches every device could match an \
                     interface; name the loopbacks"
                ));
            }
        }
        for (device, &n) in &self.widths {
            if device.contains('*') {
                return Err(format!(
                    "width for '{device}': name one device, not a pattern"
                ));
            }
            if !self.allows(device) {
                return Err(format!("width for '{device}': no sink allows it"));
            }
            if !(1..=MAX_WIDTH).contains(&n) {
                return Err(format!(
                    "width for '{device}': 1–{MAX_WIDTH} channels, not {n}"
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Direction {
    Send,
    Receive,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sinks_match_exactly_or_by_wildcard() {
        let d = Devices {
            sinks: vec!["stream in 16ch".into(), "hw:CARD=dsper*,DEV=0".into()],
            ..Default::default()
        };
        for ok in [
            "stream in 16ch",
            "hw:CARD=dspersystem,DEV=0",
            "hw:CARD=dsperdaw,DEV=0",
        ] {
            assert!(d.allows(ok), "{ok}");
        }
        for no in [
            "EVO16",
            "stream in 16ch ",
            "stream in",
            "hw:CARD=dspersystem,DEV=1",
            "plughw:CARD=dspersystem,DEV=0",
            "hw:CARD=streamin,DEV=0",
        ] {
            assert!(!d.allows(no), "{no}");
        }
        assert!(
            !Devices::default().allows("stream in 16ch"),
            "no sinks, no receiving"
        );
        assert!(matches("*", ""));
        assert!(matches("a*b*c", "abc") && matches("a*b*c", "axxbyyc"));
        assert!(!matches("a*b*c", "acb") && !matches("ab*ba", "aba"));
    }

    #[test]
    fn aliases_name_a_device_or_one_per_direction() {
        let mut d = Devices::default();
        d.set_alias("system=dsper system 2ch", None).unwrap();
        d.set_alias("stream=hw:CARD=streamin,DEV=0", Some(Direction::Receive))
            .unwrap();
        d.set_alias("stream=hw:CARD=stream,DEV=1", Some(Direction::Send))
            .unwrap();
        assert_eq!(d.resolve("system", true), "dsper system 2ch");
        assert_eq!(d.resolve("system", false), "dsper system 2ch");
        assert_eq!(d.resolve("stream", true), "hw:CARD=streamin,DEV=0");
        assert_eq!(d.resolve("stream", false), "hw:CARD=stream,DEV=1");
        assert_eq!(d.resolve("EVO16", true), "EVO16");
        assert!(d.set_alias("stream", None).is_err());
        assert!(d.set_alias("two words=x", None).is_err());

        let json: Devices = serde_json::from_str(
            r#"{"aliases": {"daw": "dsper daw 16ch",
                            "stream": {"send": "hw:CARD=stream,DEV=1", "receive": "hw:CARD=streamin,DEV=0"}}}"#,
        )
        .unwrap();
        assert_eq!(json.resolve("daw", false), "dsper daw 16ch");
        assert_eq!(json.resolve("stream", true), "hw:CARD=streamin,DEV=0");
        assert!(json.sinks.is_empty());
        assert!(json.widths.is_empty(), "widths are optional");
    }

    #[test]
    fn widths_name_exact_sinks() {
        let mut d = Devices {
            sinks: vec![
                "stream in 16ch".into(),
                "hw:CARD=dspersystem,DEV=0".into(),
                "ae rx *".into(),
            ],
            ..Default::default()
        };
        assert!(!d.stereo_ready());
        d.set_width("hw:CARD=dspersystem,DEV=0=2").unwrap();
        assert_eq!(d.width("hw:CARD=dspersystem,DEV=0"), Some(2));
        d.set_width("stream in 16ch=16").unwrap();
        assert_eq!(d.width("stream in 16ch"), Some(16));
        assert_eq!(d.width("ae rx booth 2ch"), None);
        for bad in ["x", "=2", "x=0", "x=65", "x=two"] {
            assert!(d.clone().set_width(bad).is_err(), "{bad}");
        }
        assert!(d.check().is_ok());
        assert!(d.stereo_ready());
        assert_eq!(
            d.summary(),
            "stream in 16ch (16ch), hw:CARD=dspersystem,DEV=0 (2ch), ae rx * (width not declared)"
        );
        assert_eq!(Devices::default().summary(), "none");

        let mut stray = d.clone();
        stray.widths.insert("EVO16".into(), 16);
        assert!(stray.check().unwrap_err().contains("no sink allows it"));
        let mut pattern = d.clone();
        pattern.widths.insert("ae rx *".into(), 2);
        assert!(pattern.check().unwrap_err().contains("not a pattern"));
        let mut mono = Devices {
            sinks: vec!["m".into()],
            ..Default::default()
        };
        mono.set_width("m=1").unwrap();
        assert!(!mono.stereo_ready(), "one channel is not stereo");
    }

    #[test]
    fn a_sink_cannot_be_every_device() {
        for every in ["", "*", "**"] {
            let d = Devices {
                sinks: vec![every.into()],
                ..Default::default()
            };
            assert!(d.check().is_err(), "'{every}'");
        }
        let d = Devices {
            sinks: vec!["ae rx *".into()],
            ..Default::default()
        };
        assert!(d.check().is_ok());
        assert!(d.allows("ae rx booth 2ch"));
        assert!(!d.allows("ae tx booth"));
    }
}
