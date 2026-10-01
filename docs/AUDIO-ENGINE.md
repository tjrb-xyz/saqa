# saqa on audio-engine

saqa is built from [audio-engine](https://github.com/tjrb-xyz/audio-engine) (the org's Rust audio engine) and other
libraries, Roc today (the owner, 2026-10-01; dsper's docs/research/OWNER-DECISIONS.md #17). This page is saqa's
side: how it will use the engine, and what it needs from it.

**saqa does not depend on audio-engine yet**, for two reasons:
- audio-engine has no licence (its D-033 is open). saqa is MIT and cannot take a dependency it may not
  distribute;
- its device model (`ae-device`, with the streaming loopback) is still under review in
  tjrb-xyz/audio-engine#3 (branch `claude/device-model`, its D-088 and ARCHITECTURE §8.6).

Until both settle, saqa's device side stays `CpalAudio` (cpal directly), as it was in dsper. What follows is the
plan, and the engine facts are as dsper recorded them against audio-engine `engine-e0` at `f5b9b67` (dsper's
docs/AUDIO-ENGINE.md and docs/research/SAQA-INVENTORY.md); check them again before building.

## How saqa composes the engine

saqa has one seam for audio devices, the `Audio` trait (`crates/saqa-stream/src/audio.rs`):

```rust
pub trait Audio: Send + Sync {
    fn capture(&self, device: &str, channels: &[u32], sink: Sink) -> Result<Open, String>;
    fn playback(&self, device: &str, channels: &[u32], source: Source) -> Result<Open, String>;
}
```

A link's pump (`pump.rs`) sits between that trait and Roc, with an rtrb ring of whole frames on each side, so the
device callback never waits on the network. Nothing else in saqa touches a device.

### 1. `AeAudio`: an `Audio` over `ae-io`, replacing `CpalAudio`

- A new `AeAudio` implements `Audio` with `ae_io::open` (named device, a `StreamRequest`, a callback, a
  `Reporter`). Its callbacks follow the engine's real-time rules: no allocation, no printing, no locks (G8).
- saqad gets a switch, `--audio cpal|engine`, `cpal` the default, the engine behind a cargo feature that is off
  until the licence is settled. With the feature off, `--audio engine` is refused with a sentence.
- Both backends pass the same tests; then `CpalAudio` goes and cpal reaches saqa only through `ae-io`.
- `MemoryAudio` stays saqa's own test device: the engine's sim device gives every channel the same level
  (`ae-io` `input_dc`), and saqa's tests need a different level on each channel to catch a channel out of
  order.

### 2. The streaming loopback

The engine offers loopbacks (`ae-device`'s `LoopbackRole::Main | Streaming | Named`). A **streaming** loopback
is the virtual device a streaming program reads to send, and plays into after it receives; the engine itself
streams nothing (its D-088: "Streaming is not the engine's").

- **Send:** a send link captures from the streaming loopback (whatever the engine routes to it) instead of a
  named device.
- **Receive:** a receive link plays into the streaming loopback, and the engine's checked pipeline takes it from
  there. That is the same safety line saqa's sinks draw today (docs/CONFIG.md), so the streaming loopback becomes
  a sink, and an alias such as `stream` names it.
- With dsper, dsper keeps its own loopback devices (`dsper stream 16ch` on macOS, `hw:CARD=dsperstream` on
  Linux) and the `stream` input role, and tells saqa about them as sinks and aliases. When dsper runs as a driver
  under the engine, those become the engine's streaming loopback, and dsper's configuration of saqad changes,
  not saqa.

## What saqa needs from `ae-io`: dsper's G4–G6, now saqa's

These were dsper's asks of the engine for its streaming (dsper docs/AUDIO-ENGINE.md, parity table); with
streaming in saqa, they are saqa's.

| # | saqa needs | Used by | `ae-io` at `f5b9b67` |
|---|---|---|---|
| **G4** | Capture from a **named** device; capture-only and playback-only streams; several streams at once (one per link) | `Audio::capture` / `playback`, the pump | Input always opens the default input (`cpal_dev.rs:204`); streams are output-driven duplex (`cpal_dev.rs:142-153`) |
| **G5** | I16 and I32 devices converted to f32; the exact channels asked (a subset of a wider device, in the link's order), at 48 kHz | `CpalAudio::config`, `pick`/`spread` | f32 configs with exactly the requested channel count only (`cpal_dev.rs:63-71`) |
| **G6** | Xruns and stream errors, per stream | a link's `dropouts` and `detail` | `reports_xruns: false` (`cpal_dev.rs:172`); errors through `Reporter` |

And two more that were dsper's and now apply to saqa:

| # | saqa needs | Why |
|---|---|---|
| G8 | The real-time contract (no allocation, no print in callbacks), which saqa adopts | `CpalAudio`'s callbacks allocate (`audio.rs`, `input_as!`/`output_as!` resize and extend buffers) and print errors; the engine's rules forbid both |
| G9 | A licence under which MIT saqa may depend on `ae-io` and `ae-device` (the engine's D-033), and a pinned tag | everything above |

Also noted for whoever builds `AeAudio`: `ae-io`'s `Device::Named` matches a name by *contains*
(`cpal_dev.rs:51`), where saqa matches the exact name or id. Exact matching matters for the safety line: a sink
named `dsper stream 16ch` must not open some other device whose name contains it.

## If saqa adopts the engine's build settings

audio-engine uses edition 2024, toolchain 1.94.1, and denies `unsafe_op_in_unsafe_fn`, `unwrap`, `expect` and
`panic` outside tests. saqa is edition 2021 with dsper's settings today. Moving would need, per dsper's
inventory: an `unsafe { }` around `lib.get` in `saqa-roc`'s `ffi.rs` (16 errors otherwise), and the `expect`s in
saqa-stream's non-test code (about a dozen, mostly mutex locks) replaced. Not needed until saqa depends on the engine.
