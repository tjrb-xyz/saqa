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
      [--sink DEVICE]... [--alias NAME=DEVICE]...
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
| `--sink DEVICE` | `sinks` | none | An input a receive link may play into. Exact device name; `*` matches any run of characters. Repeatable; flags add to the file's. |
| `--alias NAME=DEVICE` | `aliases` | none | `NAME` in a `LinkSpec`'s `device` means `DEVICE`, both directions. |
| `--alias-send NAME=DEVICE` | `aliases` | | What `NAME` means for a send link (captured from). |
| `--alias-receive NAME=DEVICE` | `aliases` | | What `NAME` means for a receive link (played into). The resolved device must still be a sink. |
| `--links FILE` | `links` | `<dir>/streams.json` | Where links are kept; they restart with saqad. |
| `--import-links FILE` | `import_links` | | Copies FILE (dsper's `streams.json`; the same format) to the links file, **only while saqad has no links file yet**. Safe to pass on every start. |
| `--allow-origin URL` | `allow_origins` | none | A browser origin allowed to call (gets CORS headers). |
| `--allow-host NAME` | `allow_hosts` | `127.0.0.1`, `localhost` | Another `Host` name it answers to (with the port). |
| `--memory` | | | Memory devices instead of the machine's, and nothing kept (tests, trying it out). |

`<dir>` is saqa's config directory: `$XDG_CONFIG_HOME/saqa`, else `~/.config/saqa` (macOS too).

An alias word is letters, digits, `-` and `_`. A device name that is not an alias is used as given.
`--alias-send` and `--alias-receive` change one direction of an alias and keep the other.

## The file

```json
{
  "port": 8486,
  "token_file": "/Users/me/.config/saqa/token",
  "sinks": ["dsper stream 16ch"],
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

## Pointing saqad at the streaming loopback

audio-engine owns the virtual devices; dsper may summon them from the engine and routes through them. Until saqa
can ask the engine for its loopbacks itself (docs/AUDIO-ENGINE.md), whoever runs saqad names them. On a machine
with dsper today, the loopbacks show to the OS under dsper's names:

| Loopback | macOS | Linux: the side saqa plays into | Linux: the side saqa reads |
|---|---|---|---|
| streaming | `dsper stream 16ch` | `hw:CARD=dsperstream,DEV=0` | `hw:CARD=dsperstream,DEV=1` |
| what the computer plays | `dsper system 2ch` | | `hw:CARD=dspersystem,DEV=1` |
| what the DAW plays | `dsper daw 16ch` | | `hw:CARD=dsperdaw,DEV=1` |

**The setup to use:** a received stream lands only in the streaming loopback, and dsper routes it from there. A
send link may read any of the three.

macOS:

```sh
saqad --sink 'dsper stream 16ch' \
  --alias 'stream=dsper stream 16ch' --alias 'system=dsper system 2ch' --alias 'daw=dsper daw 16ch' \
  --import-links ~/.config/dsper/streams.json
```

Linux:

```sh
saqad --sink 'hw:CARD=dsperstream,DEV=0' --sink 'plughw:CARD=dsperstream,DEV=0' \
  --alias-receive 'stream=hw:CARD=dsperstream,DEV=0' --alias-send 'stream=hw:CARD=dsperstream,DEV=1' \
  --alias 'system=hw:CARD=dspersystem,DEV=1' --alias 'daw=hw:CARD=dsperdaw,DEV=1' \
  --import-links ~/.config/dsper/streams.json
```

The same as a file (say `~/.config/dsper/saqad.json`, passed with `--config`), Linux:

```json
{
  "sinks": ["hw:CARD=dsperstream,DEV=0", "plughw:CARD=dsperstream,DEV=0"],
  "aliases": {
    "stream": { "send": "hw:CARD=dsperstream,DEV=1", "receive": "hw:CARD=dsperstream,DEV=0" },
    "system": "hw:CARD=dspersystem,DEV=1",
    "daw": "hw:CARD=dsperdaw,DEV=1"
  },
  "import_links": "/home/me/.config/dsper/streams.json"
}
```

With these, a receive link into `system` or `daw` is refused (422): those loopbacks carry what this machine
plays, and receiving into them is dsper's routing to decide, not saqa's.

**dsper's old behaviour, exactly.** `dsper-stream` also let a receive link play into `dsper system Nch` and
`dsper daw Nch` (`is_dsper_input` and `resolve`, dsper@7afd988 `crates/dsper-stream/src/lib.rs:69-106`). To keep
that, add them as sinks, and on Linux give `system` and `daw` their `DEV=0` side for receiving:

```sh
# macOS, in addition to the above
--sink 'dsper system 2ch' --sink 'dsper daw 16ch'
# Linux, in addition to the above
--sink 'hw:CARD=dspersystem,DEV=0' --sink 'plughw:CARD=dspersystem,DEV=0' \
--sink 'hw:CARD=dsperdaw,DEV=0' --sink 'plughw:CARD=dsperdaw,DEV=0' \
--alias-receive 'system=hw:CARD=dspersystem,DEV=0' --alias-receive 'daw=hw:CARD=dsperdaw,DEV=0'
```

Never a sink, either way: `dsper in 16ch` / `hw:CARD=dsperin16,…` (what dsper hands back to apps), any interface,
and on Linux any `DEV=1` side.

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

Kept links are checked again when saqad starts them: a receive link into an input that is no longer a sink is
refused, and saqad says so on stderr (`saqad: stream link '…'`). As in dsper, a refused kept link is not
running, and leaves the file at the next change of links: pass the sinks before importing.
