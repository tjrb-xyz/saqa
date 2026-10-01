//! What saqa is told about this machine's devices, by whoever runs it
//! (docs/CONFIG.md). Until saqa can ask audio-engine for its loopbacks, this
//! is how it learns which device is the engine's streaming loopback:
//!
//! - **sinks**: the devices a received stream may play into: the streaming
//!   loopback. A receive link into anything else is refused (422). With none,
//!   no receive link is allowed: saqa never guesses which devices are safe to
//!   play into.
//! - **aliases**: words that name a device here, such as `stream` for the
//!   streaming loopback (`dsper stream 16ch` on a machine with dsper today).
//!   An alias may name one device, or one to capture
//!   from (`send`) and another to play into (`receive`), as an ALSA loopback
//!   does (`hw:CARD=dsperstream,DEV=1` and `DEV=0`).

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
}

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

    /// Every alias word is a word.
    pub fn check(&self) -> Result<(), String> {
        self.aliases.keys().try_for_each(|w| check_word(w))
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
            sinks: vec!["dsper stream 16ch".into(), "hw:CARD=dsper*,DEV=0".into()],
            ..Default::default()
        };
        for ok in [
            "dsper stream 16ch",
            "hw:CARD=dsperstream,DEV=0",
            "hw:CARD=dsperdaw,DEV=0",
        ] {
            assert!(d.allows(ok), "{ok}");
        }
        for no in [
            "EVO16",
            "dsper stream 16ch ",
            "dsper stream",
            "hw:CARD=dsperstream,DEV=1",
            "plughw:CARD=dsperstream,DEV=0",
        ] {
            assert!(!d.allows(no), "{no}");
        }
        assert!(
            !Devices::default().allows("dsper stream 16ch"),
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
        d.set_alias("stream=hw:CARD=dsperstream,DEV=0", Some(Direction::Receive))
            .unwrap();
        d.set_alias("stream=hw:CARD=dsperstream,DEV=1", Some(Direction::Send))
            .unwrap();
        assert_eq!(d.resolve("system", true), "dsper system 2ch");
        assert_eq!(d.resolve("system", false), "dsper system 2ch");
        assert_eq!(d.resolve("stream", true), "hw:CARD=dsperstream,DEV=0");
        assert_eq!(d.resolve("stream", false), "hw:CARD=dsperstream,DEV=1");
        assert_eq!(d.resolve("EVO16", true), "EVO16");
        assert!(d.set_alias("stream", None).is_err());
        assert!(d.set_alias("two words=x", None).is_err());

        let json: Devices = serde_json::from_str(
            r#"{"aliases": {"daw": "dsper daw 16ch",
                            "stream": {"send": "hw:CARD=dsperstream,DEV=1", "receive": "hw:CARD=dsperstream,DEV=0"}}}"#,
        )
        .unwrap();
        assert_eq!(json.resolve("daw", false), "dsper daw 16ch");
        assert_eq!(json.resolve("stream", true), "hw:CARD=dsperstream,DEV=0");
        assert!(json.sinks.is_empty());
    }
}
