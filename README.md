# saqa

saqa carries audio between machines: one-way **links** of up to 16 channels over
[Roc](https://github.com/roc-streaming/roc-toolkit), with forward error correction and clock-drift correction. A
send link captures channels of a device here and streams them to another machine; a receive link plays what
arrives into one of audio-engine's loopbacks, never straight into an interface: the streaming loopback, a
2-channel loopback for stereo, and any made later for rooms and creative uses. `saqad` serves its REST API at
`/stream/v1`.

saqa is built from [audio-engine](https://github.com/tjrb-xyz/audio-engine) and other libraries, Roc today. It
started as dsper's streaming (github.com/tjrb-xyz/dsper, imported at dsper@7afd988).

**Who does what.** audio-engine carries the audio and owns the virtual devices (loopbacks). dsper does DSP and
routes audio between devices for the situation; it may summon virtual devices from the engine, which run
separately but belong to the engine. saqa reads from and plays into the engine's loopbacks it is told are receivable,
and carries that audio between machines. dsper calls saqa's API for anything about streams.
[docs/AUDIO-ENGINE.md](docs/AUDIO-ENGINE.md).

MIT ([LICENSE](LICENSE)). libroc (MPL-2.0) is loaded at run time, never linked.

## Quick start

```sh
scripts/roc.sh                    # libroc 0.4 into .saqa/lib (SCons, ragel, CMake; on Linux autotools too)
cargo build --release
target/release/saqad --sink 'stream in 16ch' --sink 'dsper system 2ch' \
  --sink-width 'stream in 16ch=16' --sink-width 'dsper system 2ch=2' \
  --alias-receive 'stream=stream in 16ch' --alias-send 'stream=stream 16ch' --alias 'system=dsper system 2ch'
curl -H "Authorization: Bearer $(cat ~/.config/saqa/token)" http://127.0.0.1:8486/stream/v1/state
```

## For dsper: what to pass saqad

dsperd forwards `/stream/v1` to saqad, so dsper's web UI, MCP tools and client keep working. Everything below is
in [docs/CONFIG.md](docs/CONFIG.md); this is the summary to wire against.

| | |
|---|---|
| Address | `http://127.0.0.1:8486/stream/v1` (`--port N`); loopback only, plain HTTP |
| Token | `~/.config/saqa/token` (`$XDG_CONFIG_HOME/saqa/token`), mode 0600, made on first start; or `--token-file FILE` of dsper's choosing (filled if empty). Send `Authorization: Bearer <token>`; `/stream/v1/health` is open |
| Forwarding | `Host: 127.0.0.1:8486`, no `Origin` (or `--allow-origin`), saqa's token in place of dsper's |
| Sinks | `--sink DEVICE` per loopback a receive link may play into (exact, or one `*` pattern), any number; `--sink-width DEVICE=N` declares its channels (a channel past it is 400). **None given: every receive link is 422** |
| Roles | `--alias NAME=DEVICE`, or `--alias-send` / `--alias-receive` for a device per direction |
| Links | kept in `~/.config/saqa/streams.json` (`--links FILE`); `--import-links ~/.config/dsper/streams.json` copies dsper's once, while saqa has none |
| A file instead | `--config FILE`: the same as JSON (`port`, `token_file`, `sinks`, `sink_widths`, `aliases`, `links`, `import_links`, `allow_origins`, `allow_hosts`) |
| New loopbacks | add them to the config file and send saqad `SIGHUP`: no restart. Links re-check; a link into a loopback that is gone waits, and starts again when it is back |

A received stream lands in the receive loopback (`stream in 16ch`) or the 2-channel loopback, so stereo always has
a stereo device. `stream` sends from the `stream 16ch` loopback, which carries dsper's mix or the system sound as
saqa chooses; a send link may also read `system` or `daw`. The loopbacks are dsper's build of the engine's devices
today (dsper's owner decisions #18 to #20; `dsper stream 16ch` was split into these two). macOS:

```sh
saqad --sink 'stream in 16ch' --sink 'dsper system 2ch' \
  --sink-width 'stream in 16ch=16' --sink-width 'dsper system 2ch=2' \
  --alias-receive 'stream=stream in 16ch' --alias-send 'stream=stream 16ch' \
  --alias 'system=dsper system 2ch' --alias 'daw=dsper daw 16ch' \
  --import-links ~/.config/dsper/streams.json
```

Linux (play into `DEV=0`, read from `DEV=1`):

```sh
saqad --sink 'hw:CARD=streamin,DEV=0' --sink 'plughw:CARD=streamin,DEV=0' \
  --sink 'hw:CARD=dspersystem,DEV=0' --sink 'plughw:CARD=dspersystem,DEV=0' \
  --sink-width 'hw:CARD=streamin,DEV=0=16' --sink-width 'plughw:CARD=streamin,DEV=0=16' \
  --sink-width 'hw:CARD=dspersystem,DEV=0=2' --sink-width 'plughw:CARD=dspersystem,DEV=0=2' \
  --alias-receive 'stream=hw:CARD=streamin,DEV=0' --alias-send 'stream=hw:CARD=stream,DEV=1' \
  --alias-receive 'system=hw:CARD=dspersystem,DEV=0' --alias-send 'system=hw:CARD=dspersystem,DEV=1' \
  --alias 'daw=hw:CARD=dsperdaw,DEV=1' \
  --import-links ~/.config/dsper/streams.json
```

A receive into `daw` or `stream 16ch` is 422 (dsper allowed `daw`; docs/CONFIG.md has the flags). On Linux each
loopback side takes one client (`pcm_substreams=1`): saqa is the only one on `streamin` and `stream`, but where
the desktop holds `dspersystem`, a stereo link there fails and says the device is busy.

**The contract** is dsper's, unchanged: `GET /state` → `{available, detail, links}`, `GET /links`,
`PUT /links/{id}` with a `LinkSpec` (400 invalid, 422 a receive link not into an allowed loopback, 503 no libroc),
`DELETE /links/{id}` (404 unknown), `GET /health`. What differs: the 422 and 503 texts (they named dsper);
`system`/`daw`/`stream` mean what saqad is told; a channel past a sink's declared width is 400; a kept link that
cannot run waits (`failed`, `waiting: …`) instead of vanishing; and a link whose loopback goes away fails.
[docs/API.md](docs/API.md) lists every difference.

## What is here

| | From dsper@7afd988 |
|---|---|
| `crates/saqa-roc`: libroc 0.4 through libloading | `crates/dsper-roc` |
| `crates/saqa-stream`: links, the pump, `CpalAudio`, the REST API, the token/Host/Origin guard, sinks and aliases (`devices`) | `crates/dsper-stream`, with `err`/`health` and the guard from `crates/dsper-service` |
| `crates/saqad`: the daemon, and its tests | `dsperd --only stream`, `crates/dsperd/tests/stream.rs` |
| `sdk/cpp/include/saqa/stream.h`, `sdk/cpp/examples/stream_levels.cpp`, `sdk/STREAMS.md` | `sdk/cpp/include/dsper/stream.h` and the rest |
| `scripts/roc.sh` | `scripts/roc.sh` |
| `docs/STREAMING.md`, `docs/API.md`, `docs/research/DIGEST-streaming.md` | `docs/STREAMING.md`, `docs/API.md` (Stream), `docs/research/DIGEST.md` §streaming |
| `.github/workflows/ci.yml` ([docs/CI.md](docs/CI.md)) | the `streaming` job and the macOS libroc step |

Environment: `SAQA_LIBROC` (libroc's path), `SAQA_ROC_LOG` (its log level), `SAQA_REQUIRE_ROC` (tests fail
instead of skipping without libroc), `SAQA_CPP_STREAM` (the built SDK example, for the SDK test),
`SAQA_ROC_REBUILD` (`roc.sh` builds even when up to date).

Next: [docs/AUDIO-ENGINE.md](docs/AUDIO-ENGINE.md), how saqa will run on audio-engine.
