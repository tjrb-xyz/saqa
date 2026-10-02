# Configuring saqad

saqad knows nothing about the machine it runs on until it is told. Whoever runs it (dsperd, or a person) says:

- **which devices a received stream may play into** (`sinks`): the engine's streaming loopback
  (docs/AUDIO-ENGINE.md, "Who does what"). A receive link into anything else is refused with 422. With no sinks,
  every receive link is refused: saqa never guesses which devices are safe to play into;
- **what the role words mean** (`aliases`), such as `stream` for the streaming loopback's device. An alias names
  one device, or one to capture from for a send link and another to play into for a receive link;
- **where it keeps its links** and **its token**, when the defaults below do not suit.

Everything can be given as flags, in a JSON file, or both.

## Flags

```text
saqad [--config FILE] [--port N] [--token-file FILE | --token T]
      [--sink DEVICE]... [--sink-width DEVICE=N]... [--alias NAME=DEVICE]...
      [--alias-send NAME=DEVICE]... [--alias-receive NAME=DEVICE]...
      [--links FILE] [--import-links FILE]
      [--allow-origin URL]... [--allow-host NAME]... [--memory]
```

| Flag | File key | Default | What |
|---|---|---|---|
| `--config FILE` | | `<dir>/saqad.json`, if it is there | The JSON file below. A file named with `--config` must exist. |
| `--port N` | `port` | `8486` | saqad listens on `127.0.0.1:N`, plain HTTP, loopback only. |
| `--token-file FILE` | `token_file` | `<dir>/token` | The bearer token, read from FILE. If FILE is missing or holds fewer than 16 characters, saqad writes a new random one there (mode 0600) and uses it. |
| `--token T` | | | The token itself (at least 16 characters). Prefer `--token-file`: a flag shows in `ps`. |
| `--sink DEVICE` | `sinks` | none | A loopback a receive link may play into. Exact device name, or a pattern where `*` matches any run of characters (never `*` alone). Repeatable; flags add to the file's. |
| `--sink-width DEVICE=N` | `sink_widths` | none | How many channels a sink has (1–64), by its exact name. A receive link past it is refused before anything opens (400). Without one, the device's own width is checked when it opens. Split at the last `=`, so `hw:CARD=x,DEV=0=2` works. |
| `--alias NAME=DEVICE` | `aliases` | none | `NAME` in a `LinkSpec`'s `device` means `DEVICE`, both directions. |
| `--alias-send NAME=DEVICE` | `aliases` | | What `NAME` means for a send link (captured from). |
| `--alias-receive NAME=DEVICE` | `aliases` | | What `NAME` means for a receive link (played into). The resolved device must still be a sink. |
| `--links FILE` | `links` | `<dir>/streams.json` | Where links are kept; they restart with saqad. |
| `--import-links FILE` | `import_links` | | Copies FILE (dsper's `streams.json`; the same format) to the links file, **only while saqad has no links file yet**. Safe to pass on every start. |
| `--allow-origin URL` | `allow_origins` | none | A browser origin allowed to call (gets CORS headers). |
| `--allow-host NAME` | `allow_hosts` | `127.0.0.1`, `localhost` | Another `Host` name it answers to (with the port). |
| `--memory` | | | Memory devices instead of the machine's (16 channels each, or as `--sink-width` says), and nothing kept (tests, trying it out). |

`<dir>` is saqa's config directory: `$XDG_CONFIG_HOME/saqa`, else `~/.config/saqa` (macOS too).

An alias word is letters, digits, `-` and `_`. A device name that is not an alias is used as given.
`--alias-send` and `--alias-receive` change one direction of an alias and keep the other.

The devices are checked after the flags: a sink that is empty or `*` alone is an error (it could match an
interface), and a width must name one sink exactly.

## The file

```json
{
  "port": 8486,
  "token_file": "/Users/me/.config/saqa/token",
  "sinks": ["dsper stream 16ch", "dsper system 2ch"],
  "sink_widths": { "dsper stream 16ch": 16, "dsper system 2ch": 2 },
  "aliases": {
    "daw": "dsper daw 16ch",
    "stream": { "send": "hw:CARD=dsperstream,DEV=1", "receive": "hw:CARD=dsperstream,DEV=0" }
  },
  "links": "/Users/me/.config/saqa/streams.json",
  "import_links": "/Users/me/.config/dsper/streams.json",
  "allow_origins": [],
  "allow_hosts": []
}
```

Every key is optional; an unknown key is an error (so a typo does not silently allow nothing). Flags apply
over the file: lists add, an alias flag replaces that alias (or that direction of it), the rest replace.

## Pointing saqad at the loopbacks

audio-engine owns the virtual devices; dsper may summon them from the engine and routes through them. Until saqa
can ask the engine for its loopbacks itself (docs/AUDIO-ENGINE.md), whoever runs saqad names them. On a machine
with dsper today, the loopbacks show to the OS under dsper's names:

| Loopback | Channels | macOS | Linux: the side saqa plays into | Linux: the side saqa reads |
|---|---|---|---|---|
| streaming | 16 | `dsper stream 16ch` | `hw:CARD=dsperstream,DEV=0` | `hw:CARD=dsperstream,DEV=1` |
| what the computer plays | 2 | `dsper system 2ch` | `hw:CARD=dspersystem,DEV=0` | `hw:CARD=dspersystem,DEV=1` |
| what the DAW plays | 16 | `dsper daw 16ch` | | `hw:CARD=dsperdaw,DEV=1` |

**The setup to use.** A received stream may land in two loopbacks: the streaming loopback (16 channels) and
the 2-channel loopback, so stereo playback always has a stereo device to land in. dsper routes both from there.
A send link may read any of the three. Each sink's width is declared, so a channel a loopback does not have is
refused before anything opens.

macOS:

```sh
saqad --sink 'dsper stream 16ch' --sink 'dsper system 2ch' \
  --sink-width 'dsper stream 16ch=16' --sink-width 'dsper system 2ch=2' \
  --alias 'stream=dsper stream 16ch' --alias 'system=dsper system 2ch' --alias 'daw=dsper daw 16ch' \
  --import-links ~/.config/dsper/streams.json
```

Linux (play into `DEV=0`, read from `DEV=1`):

```sh
saqad --sink 'hw:CARD=dsperstream,DEV=0' --sink 'plughw:CARD=dsperstream,DEV=0' \
  --sink 'hw:CARD=dspersystem,DEV=0' --sink 'plughw:CARD=dspersystem,DEV=0' \
  --sink-width 'hw:CARD=dsperstream,DEV=0=16' --sink-width 'plughw:CARD=dsperstream,DEV=0=16' \
  --sink-width 'hw:CARD=dspersystem,DEV=0=2' --sink-width 'plughw:CARD=dspersystem,DEV=0=2' \
  --alias-receive 'stream=hw:CARD=dsperstream,DEV=0' --alias-send 'stream=hw:CARD=dsperstream,DEV=1' \
  --alias-receive 'system=hw:CARD=dspersystem,DEV=0' --alias-send 'system=hw:CARD=dspersystem,DEV=1' \
  --alias 'daw=hw:CARD=dsperdaw,DEV=1' \
  --import-links ~/.config/dsper/streams.json
```

The same as a file (say `~/.config/dsper/saqad.json`, passed with `--config`), Linux:

```json
{
  "sinks": [
    "hw:CARD=dsperstream,DEV=0", "plughw:CARD=dsperstream,DEV=0",
    "hw:CARD=dspersystem,DEV=0", "plughw:CARD=dspersystem,DEV=0"
  ],
  "sink_widths": {
    "hw:CARD=dsperstream,DEV=0": 16, "plughw:CARD=dsperstream,DEV=0": 16,
    "hw:CARD=dspersystem,DEV=0": 2, "plughw:CARD=dspersystem,DEV=0": 2
  },
  "aliases": {
    "stream": { "send": "hw:CARD=dsperstream,DEV=1", "receive": "hw:CARD=dsperstream,DEV=0" },
    "system": { "send": "hw:CARD=dspersystem,DEV=1", "receive": "hw:CARD=dspersystem,DEV=0" },
    "daw": "hw:CARD=dsperdaw,DEV=1"
  },
  "import_links": "/home/me/.config/dsper/streams.json"
}
```

Declare the `plughw` widths too: `plughw` offers any channel count, so only the declared width catches a third
channel on a 2-channel loopback.

Things to know about the 2-channel loopback today:

- **It is what the computer plays.** A stream received there is mixed with the computer's own sound before
  dsper's DSP; dsper's routing decides where both go. A dedicated 2-channel receive loopback from audio-engine
  would keep them apart (docs/AUDIO-ENGINE.md lists it as an ask). Pointing saqad at it is only a change of
  names here.
- **On Linux it takes one client.** dsper loads snd-aloop with `pcm_substreams=1`, so `DEV=0` takes one player.
  On a headless machine (a Pi by the speakers) it is free; where the desktop holds it, the link goes to
  `failed` with the device's busy error in its detail. The same limit holds for the streaming loopback.
- A receive into `daw` is refused (422): that loopback carries what the DAW plays, and receiving into it is
  dsper's routing to decide.

**dsper's old behaviour, exactly.** `dsper-stream` also let a receive link play into `dsper daw Nch`
(`is_dsper_input` and `resolve`, dsper@7afd988 `crates/dsper-stream/src/lib.rs:69-106`). To keep that, add it
as a sink, with its width, and on Linux give `daw` its `DEV=0` side for receiving:

```sh
# macOS, in addition to the above
--sink 'dsper daw 16ch' --sink-width 'dsper daw 16ch=16'
# Linux, in addition to the above
--sink 'hw:CARD=dsperdaw,DEV=0' --sink 'plughw:CARD=dsperdaw,DEV=0' \
--sink-width 'hw:CARD=dsperdaw,DEV=0=16' --sink-width 'plughw:CARD=dsperdaw,DEV=0=16' \
--alias-receive 'daw=hw:CARD=dsperdaw,DEV=0'
```

Never a sink, either way: `dsper in 16ch` / `hw:CARD=dsperin16,…` (what dsper hands back to apps), any interface,
and on Linux any `DEV=1` side.

## More loopbacks later

Loopbacks will be made over time, by audio-engine (perhaps at dsper's request), for receiving in a room, a
booth, a DAW's return, or a creative use. saqa takes any number of them, of any width:

- **Name each one as a sink** (`--sink`, or `sinks` in the file), with its width (`--sink-width`, `sink_widths`)
  and, if useful, an alias word (`--alias booth=…`).
- **Or cover them with one specific pattern**, when the engine gives receive loopbacks a common prefix:
  `"sinks": ["ae rx *"]`. Keep the pattern narrow: never `*` alone (refused), and never one as broad as
  `hw:*` or `dsper *`, which could match an interface or a loopback that is not for receiving. A
  pattern-matched loopback's width is checked when its link opens, since widths name exact devices.
- **No restart.** Edit the config file and send saqad `SIGHUP` (`kill -HUP <pid>`, `systemctl reload`,
  `launchctl kill HUP …`). saqad reads its sinks, widths and aliases again and re-checks every link against
  them:
  - a running receive link whose loopback is no longer allowed stops, and waits;
  - a waiting link that fits again starts by itself;
  - a link into a loopback that still fits keeps running, untouched.

  A reload changes only what the config file says: flags given at start stay as they were. Other keys (port,
  token, links, import_links, allow_origins, allow_hosts) need a restart, and saqad says so. A file that does
  not parse, or a set that fails the checks above, changes nothing.
- **A loopback that goes away** (the engine removes it, a device is unplugged) fails its link, with "the device
  went away" in the detail. The link stays kept; a PUT starts it again.

A link that waits shows as `failed`, with a detail that starts `waiting: ` and says why. It stays in
`GET /links` and in the links file, and keeps its ports.

### The token

By default saqad keeps its token in `~/.config/saqa/token` (mode 0600, made on first start). dsperd, running as
the same user, reads it from there and sends `Authorization: Bearer <token>`. Or dsperd chooses the file and
passes `--token-file FILE`; saqad fills it if it is empty. saqad never prints the token.

### dsperd in front of saqad

dsperd keeps serving `/stream/v1` to its own clients (web UI, MCP, client) behind its own guard, and forwards
each request to `http://127.0.0.1:8486/stream/v1/…` with:

- `Host: 127.0.0.1:8486` (saqad answers 421 to any other `Host` not allowed with `--allow-host`);
- no `Origin` (dsperd has checked it already; or allow dsper's origins with `--allow-origin`);
- `Authorization: Bearer <saqa's token>` in place of dsper's.

`GET /stream/v1/health` needs no token: `{"ok": true, "service": "stream", "api": 1}`.

## Moving dsper's kept links

dsper kept links in `~/.config/dsper/streams.json`; saqa keeps them in `~/.config/saqa/streams.json`. The format
is the same (link name → `LinkSpec`). Either:

- pass `--import-links ~/.config/dsper/streams.json` (or `"import_links"`): on the first start without a links
  file of its own, saqad copies it and says how many links it kept. After that, saqa's own file wins, and dsper's
  is left where it is; or
- copy it once by hand while saqad is stopped:
  `mkdir -p ~/.config/saqa && cp ~/.config/dsper/streams.json ~/.config/saqa/streams.json`.

Kept links are checked again when saqad starts them. A link that cannot start (its loopback is not a sink
now, a channel is past a declared width, or there is no libroc) is not dropped: it waits, as `failed` with a
`waiting: ` detail, and saqad says so on stderr (`saqad: stream link '…': waiting: …`). It starts by itself
when a reload allows it. dsper dropped such links from the file at the next change; saqa keeps them.
