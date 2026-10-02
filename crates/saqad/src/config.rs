//! saqad's configuration: a JSON file (`--config FILE`, else
//! `<config dir>/saqad.json` when there is one), then flags over it. Lists
//! (`--sink`, `--allow-origin`, `--allow-host`) add to the file's; an alias
//! flag replaces that alias (or one direction of it); anything else a flag
//! gives wins. The devices (sinks, widths, aliases) are checked after the
//! flags, so a flag cannot slip past the file's rules. docs/CONFIG.md
//! describes every key.

use saqa_stream::devices::Direction;
use saqa_stream::{Alias, Devices};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const DEFAULT_PORT: u16 = 8486;

pub const USAGE: &str = "usage: saqad [--config FILE] [--port N] [--token-file FILE | --token T]
             [--sink DEVICE]... [--sink-width DEVICE=N]... [--alias NAME=DEVICE]...
             [--alias-send NAME=DEVICE]... [--alias-receive NAME=DEVICE]...
             [--links FILE] [--import-links FILE]
             [--allow-origin URL]... [--allow-host NAME]... [--memory]
SIGHUP reloads sinks, widths and aliases from the config file (docs/CONFIG.md)";

/// The file's keys; every one is optional.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    port: Option<u16>,
    token_file: Option<PathBuf>,
    #[serde(default)]
    sinks: Vec<String>,
    #[serde(default)]
    sink_widths: BTreeMap<String, u16>,
    #[serde(default)]
    aliases: BTreeMap<String, Alias>,
    links: Option<PathBuf>,
    import_links: Option<PathBuf>,
    #[serde(default)]
    allow_origins: Vec<String>,
    #[serde(default)]
    allow_hosts: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub enum Token {
    /// Given on the command line.
    Given(String),
    /// Read from a file (`token_file`, else `<config dir>/token`, made when missing).
    File(PathBuf),
}

#[derive(Debug, PartialEq)]
pub struct Config {
    pub port: u16,
    pub token: Token,
    pub devices: Devices,
    /// Where links are kept; `None` with `--memory`.
    pub links: Option<PathBuf>,
    pub import_links: Option<PathBuf>,
    pub allow_origins: Vec<String>,
    pub allow_hosts: Vec<String>,
    pub memory: bool,
}

/// `$XDG_CONFIG_HOME/saqa`, else `~/.config/saqa`.
pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("saqa")
}

fn read_file(path: &Path, required: bool) -> Result<File, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if !required && e.kind() == std::io::ErrorKind::NotFound => Ok(File::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

impl Config {
    /// From `args` (without the program name), the file they or `dir` name, and `dir`.
    pub fn parse(args: &[String], dir: &Path) -> Result<Config, String> {
        let mut file_path = None;
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if a == "--config" {
                file_path = Some(PathBuf::from(it.next().ok_or("--config FILE")?));
            }
        }
        let file = match &file_path {
            Some(p) => read_file(p, true)?,
            None => read_file(&dir.join("saqad.json"), false)?,
        };
        let devices = Devices {
            sinks: file.sinks,
            aliases: file.aliases,
            widths: file.sink_widths,
        };
        let mut c = Config {
            port: file.port.unwrap_or(DEFAULT_PORT),
            token: Token::File(file.token_file.unwrap_or_else(|| dir.join("token"))),
            devices,
            links: Some(file.links.unwrap_or_else(|| dir.join("streams.json"))),
            import_links: file.import_links,
            allow_origins: file.allow_origins,
            allow_hosts: file.allow_hosts,
            memory: false,
        };
        let mut it = args.iter();
        while let Some(a) = it.next() {
            let mut value = || {
                it.next()
                    .cloned()
                    .ok_or_else(|| format!("{a} needs a value"))
            };
            match a.as_str() {
                "--config" => {
                    value()?;
                }
                "--port" => {
                    let v = value()?;
                    c.port = v.parse().map_err(|_| format!("--port {v}: not a port"))?;
                }
                "--token" => c.token = Token::Given(value()?),
                "--token-file" => c.token = Token::File(value()?.into()),
                "--sink" => c.devices.sinks.push(value()?),
                "--sink-width" => c.devices.set_width(&value()?)?,
                "--alias" => c.devices.set_alias(&value()?, None)?,
                "--alias-send" => c.devices.set_alias(&value()?, Some(Direction::Send))?,
                "--alias-receive" => c.devices.set_alias(&value()?, Some(Direction::Receive))?,
                "--links" => c.links = Some(value()?.into()),
                "--import-links" => c.import_links = Some(value()?.into()),
                "--allow-origin" => c.allow_origins.push(value()?),
                "--allow-host" => c.allow_hosts.push(value()?),
                "--memory" => c.memory = true,
                other => return Err(format!("unknown option '{other}'")),
            }
        }
        c.devices.check()?;
        for h in &mut c.allow_hosts {
            *h = h.to_lowercase();
        }
        if c.memory {
            c.links = None;
            c.import_links = None;
        }
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn with_nothing_given_it_keeps_its_own_files_and_allows_no_input() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::parse(&[], dir.path()).unwrap();
        assert_eq!(c.port, DEFAULT_PORT);
        assert_eq!(c.token, Token::File(dir.path().join("token")));
        assert_eq!(c.links, Some(dir.path().join("streams.json")));
        assert_eq!(c.devices, Devices::default());
        assert!(!c.devices.allows("dsper stream 16ch"));
    }

    #[test]
    fn flags_add_to_the_file_and_win_over_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("saqad.json"),
            r#"{"port": 9000, "sinks": ["hw:CARD=dsperstream,DEV=0"],
                "aliases": {"stream": {"send": "hw:CARD=dsperstream,DEV=1", "receive": "hw:CARD=dsperstream,DEV=0"},
                            "daw": "hw:CARD=dsperdaw,DEV=1"},
                "links": "/var/lib/saqa/streams.json", "allow_hosts": ["Studio.local"]}"#,
        )
        .unwrap();
        let c = Config::parse(
            &args(&[
                "--port",
                "9001",
                "--sink",
                "plughw:CARD=dsperstream,DEV=0",
                "--alias-send",
                "daw=hw:CARD=dsperdaw,DEV=2",
                "--allow-host",
                "studio",
                "--token-file",
                "/run/dsper/saqa-token",
            ]),
            dir.path(),
        )
        .unwrap();
        assert_eq!(c.port, 9001);
        assert_eq!(c.token, Token::File("/run/dsper/saqa-token".into()));
        assert!(c.devices.allows("hw:CARD=dsperstream,DEV=0"));
        assert!(c.devices.allows("plughw:CARD=dsperstream,DEV=0"));
        assert_eq!(
            c.devices.resolve("stream", false),
            "hw:CARD=dsperstream,DEV=1"
        );
        assert_eq!(c.devices.resolve("daw", false), "hw:CARD=dsperdaw,DEV=2");
        assert_eq!(c.devices.resolve("daw", true), "hw:CARD=dsperdaw,DEV=1");
        assert_eq!(c.links, Some("/var/lib/saqa/streams.json".into()));
        assert_eq!(c.allow_hosts, ["studio.local", "studio"]);
    }

    #[test]
    fn a_named_file_must_be_there_and_say_only_what_saqad_knows() {
        let dir = tempfile::tempdir().unwrap();
        let named = dir.path().join("dsper-saqa.json");
        assert!(Config::parse(&args(&["--config", named.to_str().unwrap()]), dir.path()).is_err());
        std::fs::write(&named, r#"{"sink": ["typo"]}"#).unwrap();
        let e =
            Config::parse(&args(&["--config", named.to_str().unwrap()]), dir.path()).unwrap_err();
        assert!(e.contains("sink"), "{e}");
        assert!(Config::parse(&args(&["--port"]), dir.path()).is_err());
        assert!(Config::parse(&args(&["--sideways"]), dir.path()).is_err());
        assert!(Config::parse(&args(&["--alias", "stream"]), dir.path()).is_err());
    }

    /// docs/CONFIG.md's Linux setup: a stream plays into each loopback's
    /// DEV=0 side and is read from its DEV=1 side, never the other way.
    #[test]
    fn the_recommended_linux_setup_splits_system() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::parse(
            &args(&[
                "--sink",
                "hw:CARD=dsperstream,DEV=0",
                "--sink",
                "plughw:CARD=dsperstream,DEV=0",
                "--sink",
                "hw:CARD=dspersystem,DEV=0",
                "--sink",
                "plughw:CARD=dspersystem,DEV=0",
                "--sink-width",
                "hw:CARD=dsperstream,DEV=0=16",
                "--sink-width",
                "plughw:CARD=dsperstream,DEV=0=16",
                "--sink-width",
                "hw:CARD=dspersystem,DEV=0=2",
                "--sink-width",
                "plughw:CARD=dspersystem,DEV=0=2",
                "--alias-receive",
                "stream=hw:CARD=dsperstream,DEV=0",
                "--alias-send",
                "stream=hw:CARD=dsperstream,DEV=1",
                "--alias-receive",
                "system=hw:CARD=dspersystem,DEV=0",
                "--alias-send",
                "system=hw:CARD=dspersystem,DEV=1",
                "--alias",
                "daw=hw:CARD=dsperdaw,DEV=1",
            ]),
            dir.path(),
        )
        .unwrap();
        let d = &c.devices;
        assert_eq!(d.resolve("system", true), "hw:CARD=dspersystem,DEV=0");
        assert_eq!(d.resolve("system", false), "hw:CARD=dspersystem,DEV=1");
        assert!(d.allows("hw:CARD=dspersystem,DEV=0"));
        assert!(!d.allows("hw:CARD=dspersystem,DEV=1"));
        assert_eq!(d.width("hw:CARD=dspersystem,DEV=0"), Some(2));
        assert_eq!(d.width("plughw:CARD=dspersystem,DEV=0"), Some(2));
        assert_eq!(d.width("hw:CARD=dsperstream,DEV=0"), Some(16));
        assert!(d.stereo_ready());
        assert_eq!(d.resolve("daw", true), "hw:CARD=dsperdaw,DEV=1");
        assert!(
            !d.allows(&d.resolve("daw", true)),
            "daw is read, not played into"
        );
    }

    #[test]
    fn sink_widths_come_from_the_file_and_flags() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("saqad.json"),
            r#"{"sinks": ["dsper system 2ch"], "sink_widths": {"dsper system 2ch": 1}}"#,
        )
        .unwrap();
        let c = Config::parse(&[], dir.path()).unwrap();
        assert_eq!(c.devices.width("dsper system 2ch"), Some(1));
        let c = Config::parse(&args(&["--sink-width", "dsper system 2ch=2"]), dir.path()).unwrap();
        assert_eq!(c.devices.width("dsper system 2ch"), Some(2), "a flag wins");
        assert!(
            Config::parse(&args(&["--sink-width", "EVO16=16"]), dir.path())
                .unwrap_err()
                .contains("no sink allows it")
        );
        assert!(
            Config::parse(&args(&["--sink", "*"]), dir.path()).is_err(),
            "flags are checked too"
        );
        std::fs::write(dir.path().join("saqad.json"), r#"{"sink_width": {"x": 2}}"#).unwrap();
        assert!(
            Config::parse(&[], dir.path()).is_err(),
            "a typo is an error"
        );
    }

    #[test]
    fn memory_keeps_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::parse(&args(&["--memory", "--import-links", "/x"]), dir.path()).unwrap();
        assert_eq!((c.links, c.import_links), (None, None));
    }
}
