# saqa

saqa carries audio between machines: one-way **links** of up to 16 channels over
[Roc](https://github.com/roc-streaming/roc-toolkit), with forward error correction and clock-drift correction. A
send link captures channels of a device here and streams them to another machine; a receive link plays what
arrives into audio-engine's streaming loopback, never straight into an interface. `saqad` serves its REST API at
`/stream/v1`.

saqa is built from [audio-engine](https://github.com/tjrb-xyz/audio-engine) and other libraries, Roc today. It
started as dsper's streaming (github.com/tjrb-xyz/dsper, imported at dsper@7afd988).

**Who does what.** audio-engine carries the audio and owns the virtual devices (loopbacks). dsper does DSP and
routes audio between devices for the situation; it may summon virtual devices from the engine, which run
separately but belong to the engine. saqa reads from and plays into the engine's streaming loopback, and carries
that audio between machines. dsper calls saqa's API for anything about streams.
[docs/AUDIO-ENGINE.md](docs/AUDIO-ENGINE.md).

MIT ([LICENSE](LICENSE)). libroc (MPL-2.0) is loaded at run time, never linked.

## Quick start

```sh
scripts/roc.sh                    # libroc 0.4 into .saqa/lib (SCons, ragel, CMake; on Linux autotools too)
cargo build --release
target/release/saqad --sink 'dsper stream 16ch' --alias 'stream=dsper stream 16ch'
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
| Sinks | `--sink DEVICE`: the streaming loopback's device (exact, `*` wildcard). **None given: every receive link is 422** |
| Roles | `--alias NAME=DEVICE`, or `--alias-send` / `--alias-receive` for a device per direction |
| Links | kept in `~/.config/saqa/streams.json` (`--links FILE`); `--import-links ~/.config/dsper/streams.json` copies dsper's once, while saqa has none |
| A file instead | `--config FILE`: the same as JSON (`port`, `token_file`, `sinks`, `aliases`, `links`, `import_links`, `allow_origins`, `allow_hosts`) |

A received stream lands only in the streaming loopback; a send link may read it, or the `system` and `daw`
loopbacks. The loopbacks show under dsper's names today. macOS:

```sh
saqad --sink 'dsper stream 16ch' \
  --alias 'stream=dsper stream 16ch' --alias 'system=dsper system 2ch' --alias 'daw=dsper daw 16ch' \
  --import-links ~/.config/dsper/streams.json
```

Linux (play into `DEV=0`, read from `DEV=1`):

```sh
saqad --sink 'hw:CARD=dsperstream,DEV=0' --sink 'plughw:CARD=dsperstream,DEV=0' \
  --alias-receive 'stream=hw:CARD=dsperstream,DEV=0' --alias-send 'stream=hw:CARD=dsperstream,DEV=1' \
  --alias 'system=hw:CARD=dspersystem,DEV=1' --alias 'daw=hw:CARD=dsperdaw,DEV=1' \
  --import-links ~/.config/dsper/streams.json
```

dsper also let a stream be received into `system` and `daw`; with this setup that is 422. docs/CONFIG.md shows the
flags that keep dsper's old behaviour exactly.

**The contract** is dsper's, unchanged: `GET /state` → `{available, detail, links}`, `GET /links`,
`PUT /links/{id}` with a `LinkSpec` (400 invalid, 422 a receive link not into an allowed input, 503 no libroc),
`DELETE /links/{id}` (404 unknown), `GET /health`. Only the 422 and 503 texts changed (they named dsper), and
`system`/`daw`/`stream` mean what saqad is told. [docs/API.md](docs/API.md) lists every difference.

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
