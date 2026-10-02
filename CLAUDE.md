# CLAUDE.md: saqa

saqa carries audio between machines (Roc links, `/stream/v1`). It came from dsper (tjrb-xyz/dsper@7afd988;
dsper's docs/SAQA.md and docs/research/OWNER-DECISIONS.md #17). audio-engine carries the audio and owns the virtual
devices (dsper may summon them); dsper does DSP and routing; saqa reads from and plays into the engine's loopbacks
it is told are receivable: the streaming loopback, at least one 2-channel loopback, and any number made later
(docs/AUDIO-ENGINE.md, "Who does what"). dsper calls this API through dsperd, so **the API is a contract**:
routes, bodies, statuses, `LinkSpec` and `LinkView` change only together with dsper (docs/API.md lists every
difference from dsper's service). If code and docs disagree, fix one of them in the same change.

## Non-negotiables
1. **A stream never plays straight into an interface.** A receive link plays only into a configured sink
   (`Devices::allows`); anything else is `Refused::NotAnInput` → 422, and a channel past a sink's declared width
   (`sink_widths`) is 400, checked after it. No sinks means no receive link; a sink is never `*` alone. saqa
   never guesses a safe device, and never hard-codes a host's device names (the loopbacks' names live in
   docs/CONFIG.md, passed as configuration).
2. **The API is behind the guard.** Every route needs the bearer token except `/stream/v1/health`; Host and
   Origin are checked (`saqa_stream::service`). saqad listens on loopback only; no TLS here.
3. **Speakers never wait on the network.** Device callbacks only touch the rtrb ring of whole frames (`pump.rs`);
   Roc runs on the link's own thread.
4. **The wire is one contract in two places.** `saqa-roc` and `sdk/cpp/include/saqa/stream.h` change together:
   48 kHz, 1–16 channels, multitrack payload type 100 + channels, 1200-byte packets, ports P/P+1/P+2.
   `saqa-roc/tests/header.rs` and `saqa-stream/tests/sdk.rs` check it.
5. **libroc is loaded at run time** (libloading), never linked: saqa builds and runs without it and says what to
   install (503, `/state.detail`).
6. **MIT** for all of saqa (`Copyright (c) 2026 tjrb-xyz`). No dependency on audio-engine until it has a licence
   (docs/AUDIO-ENGINE.md).

## Layout
```
crates/
  saqa-roc/     # libroc 0.4's C ABI (ffi.rs, hand-written) + a safe Sender/Receiver/Context;
                #   finds libroc at SAQA_LIBROC, .saqa/lib, Homebrew, the system
  saqa-stream/  # links: LinkSpec/LinkView, check, StreamService (kept in a JSON file; links that cannot run
                #   wait, `set_devices` re-checks them), pump.rs (rings), audio.rs (Audio trait with a Fault
                #   for a device gone: CpalAudio, MemoryAudio for tests), devices.rs (sinks, widths, aliases),
                #   rest.rs (/stream/v1), service.rs (token/Host/Origin guard, err, health)
  saqad/        # the daemon: config.rs (flags + JSON file, docs/CONFIG.md), main.rs (SIGHUP reloads devices)
sdk/            # saqa/stream.h (header-only C++, JUCE-friendly), stream_levels.cpp, STREAMS.md
scripts/roc.sh  # builds libroc 0.4 (pinned commit) into .saqa/lib and .saqa/include
docs/           # API.md, CONFIG.md, STREAMING.md, AUDIO-ENGINE.md, CI.md, research/
```

## Working here
- Before a change is done: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`. With libroc (`scripts/roc.sh`): `SAQA_REQUIRE_ROC=1` and `SAQA_CPP_STREAM` set
  (docs/CI.md), so nothing skips.
- Without libroc the Roc tests print `skipped:` and pass; `SAQA_REQUIRE_ROC` makes that a failure. CI's `linux`
  and `macos` jobs set it.
- Tests use `MemoryAudio` (memory devices with a level per channel, 16 channels wide unless given a width with
  `with_width`/`set_width`, and removable with `remove`) and dsper's device names as test data; saqad's tests run
  the binary with `--memory`, whose devices take their widths from `sink_widths`.
- Linux builds need ALSA headers (`libasound2-dev`) for cpal.
- Log lines start `saqad:`; environment variables start `SAQA_`; the build tree is `.saqa/`.
- Write docs in short, plain sentences; say what a thing is and why, with file:line where it helps.
