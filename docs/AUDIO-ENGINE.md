# saqa on audio-engine

## Who does what

The owner (2026-10-01): audio is streamed from audio-engine; dsper only does DSP and patches audio routes between
devices for the situation at hand. dsper may summon virtual devices from audio-engine; they run separately, but
audio-engine owns them.

| | Owns | In saqa's terms |
|---|---|---|
| **audio-engine** | Audio itself: devices, streams, playback, the real-time path, the one view of how devices connect (its D-093), and the virtual devices (loopbacks, `ae-device`), whoever asked for them | Where saqa's audio comes from and goes to: a send link reads the engine's **streaming loopback**, a receive link plays into it |
| **dsper** | DSP (CamillaDSP, rooms, speaker systems) and routing: which device feeds which, for the situation. It may ask the engine for a virtual device (a loopback) and route through it | Routes what saqa receives on to speakers, through its DSP; routes what should be sent into the streaming loopback. Calls saqa's API for its Streams view and MCP tools |
| **saqa** | Carrying audio between machines (Roc), its links and its API | Reads from and plays into the engine's streaming loopback, and nothing else on the receive side |

So saqa never plays into a device dsper owns, or into an interface: only into a loopback the engine owns, which
dsper then routes. The engine itself does no network streaming (its D-088).

The engine's own record of this split is D-093 (branch `claude/devices`, idea 13). It still says dsper installs
the loopback devices; the owner's later words above put them under the engine. That is the engine's to
reconcile; saqa follows the owner.

## Where saqa is today

**saqa does not depend on audio-engine yet:** the engine has no licence (its D-033 is open, and its `main` has
no LICENSE file). saqa is MIT and cannot take a dependency it may not distribute. Until then:

- saqa opens devices through cpal (`CpalAudio`), as it did in dsper;
- it is told which devices are the engine's streaming loopback, as sinks and aliases (docs/CONFIG.md). On a
  machine with dsper today, that loopback appears to the OS as `dsper stream 16ch` (macOS) or
  `hw:CARD=dsperstream` (Linux): the device names change, saqa does not.

Engine facts below were checked on audio-engine `main` at `549c9cc` (`ae-device`, merged in #5) and
`claude/devices` at `b0a89e1` (`ae-io`, the daemon's devices).

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

### 1. The streaming loopback is where links read and play

`ae-device`'s loopbacks (`crates/ae-device/src/loopback.rs`): virtual devices apps play into, which the engine
sends on to any number of real devices, re-targeted live without a gap. `LoopbackRole::Streaming` is "what a
streaming program reads to send, or plays into after it receives". Roles are labels; the engine treats them
alike.

- **Receive:** a receive link plays into a streaming loopback. That is saqa's safety line: the loopback is the
  only sink, and where it goes next is the engine's audio path and dsper's routing.
- **Send:** a send link reads a streaming loopback: whatever dsper routes into it.
- **Finding it:** once saqa can depend on the engine, saqad asks the engine's device view for loopbacks with role
  `Streaming` and uses them as its sinks and its `stream` alias, instead of being told. The `--sink`/`--alias`
  flags stay for machines without the engine.

### 2. `AeAudio`: an `Audio` over `ae-io`, replacing `CpalAudio`

- A new `AeAudio` implements `Audio` with `ae_io::open` (a device by id, a `StreamRequest`, a callback, a
  `Reporter`). Its callbacks follow the engine's real-time rules: no allocation, no printing, no locks (G8).
- saqad gets a switch, `--audio cpal|engine`, `cpal` the default, the engine behind a cargo feature that is off
  until the licence is settled. With the feature off, `--audio engine` is refused with a sentence.
- Both backends pass the same tests; then `CpalAudio` goes and cpal reaches saqa only through `ae-io`.
- `MemoryAudio` stays saqa's own test device: the engine's sim device gives every channel the same level
  (`ae-io` `input_dc`), and saqa's tests need a different level on each channel to catch a channel out of
  order.

## What saqa needs from `ae-io`: dsper's G4–G6, now saqa's

These were dsper's asks of the engine for its streaming (dsper docs/AUDIO-ENGINE.md, parity table); with
streaming in saqa, they are saqa's.

| # | saqa needs | Used by | `ae-io` at `claude/devices` `b0a89e1` |
|---|---|---|---|
| **G4** | Capture from a **named** device; capture-only and playback-only streams; several streams at once (one per link) | `Audio::capture` / `playback`, the pump | Devices open by id now (G1–G3, `claude/devices`), but input still opens the default input (`cpal_dev.rs:69`); streams are output-driven duplex |
| **G5** | I16 and I32 devices converted to f32; the exact channels asked (a subset of a wider device, in the link's order), at 48 kHz | `CpalAudio::config`, `pick`/`spread` | f32 configs with exactly the requested channel count only (`cpal_dev.rs:88`) |
| **G6** | Xruns and stream errors, per stream | a link's `dropouts` and `detail` | `reports_xruns: false` (`cpal_dev.rs:197`); errors through `Reporter` |

And two more that were dsper's and now apply to saqa:

| # | saqa needs | Why |
|---|---|---|
| G8 | The real-time contract (no allocation, no print in callbacks), which saqa adopts | `CpalAudio`'s callbacks allocate (`audio.rs`, `input_as!`/`output_as!` resize and extend buffers) and print errors; the engine's rules forbid both |
| G9 | A licence under which MIT saqa may depend on `ae-io` and `ae-device` (the engine's D-033), and a pinned tag | everything above |

Also noted for whoever builds `AeAudio`: open devices with `ae-io`'s `Device::Id`, not `Device::Named`, which
matches a name by *substring* (`cpal_dev.rs:59`). saqa matches the exact name or id, and that matters for the
safety line: a sink named `dsper stream 16ch` must not open some other device whose name contains it.

## If saqa adopts the engine's build settings

audio-engine uses edition 2024, toolchain 1.94.1, and denies `unsafe_op_in_unsafe_fn`, `unwrap`, `expect` and
`panic` outside tests. saqa is edition 2021 with dsper's settings today. Moving would need, per dsper's
inventory: an `unsafe { }` around `lib.get` in `saqa-roc`'s `ffi.rs` (16 errors otherwise), and the `expect`s in
saqa-stream's non-test code (about a dozen, mostly mutex locks) replaced. Not needed until saqa depends on the engine.
