# saqa on audio-engine

## Who does what

The owner (2026-10-01): audio is streamed from audio-engine; dsper only does DSP and patches audio routes between
devices for the situation at hand. dsper may summon virtual devices from audio-engine; they run separately, but
audio-engine owns them.

| | Owns | In saqa's terms |
|---|---|---|
| **audio-engine** | Audio itself: devices, streams, playback, the real-time path, the one view of how devices connect (its D-093), and the virtual devices (loopbacks, `ae-device`), whoever asked for them | Where saqa's audio comes from and goes to: a send link reads a loopback, a receive link plays into one |
| **dsper** | DSP (CamillaDSP, rooms, speaker systems) and routing: which device feeds which, for the situation. It may ask the engine for a virtual device (a loopback) and route through it | Routes what saqa receives on to speakers, through its DSP; routes what should be sent into the streaming loopback. Calls saqa's API for its Streams view and MCP tools |
| **saqa** | Carrying audio between machines (Roc), its links and its API | Plays received streams only into the engine's loopbacks it is told are receivable: the streaming loopback, at least one 2-channel loopback for stereo, and any number made later for rooms and creative uses |

So saqa never plays into a device dsper owns, or into an interface: only into a loopback the engine owns, which
dsper then routes. The engine itself does no network streaming (its D-088).

The engine's own record of this split is D-093 (branch `claude/devices`, idea 13). It still says dsper installs
the loopback devices; the owner's later words above put them under the engine. That is the engine's to
reconcile; saqa follows the owner.

## Where saqa is today

**saqa does not depend on audio-engine yet:** the engine has no licence (its D-033 is open, and its `main` has
no LICENSE file). saqa is MIT and cannot take a dependency it may not distribute. Until then:

- saqa opens devices through cpal (`CpalAudio`), as it did in dsper;
- it is told which devices are the engine's receivable loopbacks, as sinks with their widths, and aliases
  (docs/CONFIG.md). On a machine with dsper today they appear to the OS as `dsper stream 16ch` and
  `dsper system 2ch` (macOS) or `hw:CARD=dsperstream` and `hw:CARD=dspersystem` (Linux): the device names
  change, saqa does not;
- it learns of a loopback made later when its config file changes and it gets SIGHUP.

Engine facts below were checked on audio-engine `main` at `549c9cc` (`ae-device`, merged in #5) and
`claude/devices` at `b0a89e1` (`ae-io`, the daemon's devices).

## How saqa composes the engine

saqa has one seam for audio devices, the `Audio` trait (`crates/saqa-stream/src/audio.rs`):

```rust
pub trait Audio: Send + Sync {
    fn capture(&self, device: &str, channels: &[u32], sink: Sink, fault: Fault) -> Result<Open, String>;
    fn playback(&self, device: &str, channels: &[u32], source: Source, fault: Fault) -> Result<Open, String>;
}
```

`fault` hears when an open device stops for good, so a link into a loopback the engine removes fails and says
so, instead of reading `running`.

A link's pump (`pump.rs`) sits between that trait and Roc, with an rtrb ring of whole frames on each side, so the
device callback never waits on the network. Nothing else in saqa touches a device.

### 1. Loopbacks are where links read and play

`ae-device`'s loopbacks (`crates/ae-device/src/loopback.rs`): virtual devices apps play into, which the engine
sends on to any number of real devices, re-targeted live without a gap. `LoopbackRole::Streaming` is "what a
streaming program reads to send, or plays into after it receives". Roles are labels; the engine treats them
alike.

- **Receive:** a receive link plays into a receivable loopback: a streaming one, at least one 2 channels wide,
  and any made later for a room or a creative use. That is saqa's safety line: only loopbacks are sinks, and
  where they go next is the engine's audio path and dsper's routing.
- **Send:** a send link reads a loopback: whatever dsper routes into it.
- **Finding them:** once the engine's daemon lists loopbacks, saqad reads them instead of being told, with no
  Cargo dependency on the engine (plain HTTP). The mapping, from `ae-device`'s `Loopback` to saqa's `Devices`:

  | `Loopback` | saqa |
  |---|---|
  | `device` (the OS name of the side apps play into) | a sink, exact |
  | `channels` | that sink's width |
  | `id` or `name` | an alias word; `Split` when the engine names both ALSA sides |
  | `role` | which loopbacks are receivable: `Streaming` by default, `Named(…)` ones by name (a flag), `Main` never unless named |
  | `enabled: false` | still receivable: the device stays, and dsper's routing decides |

  saqad follows the engine's change events and calls `StreamService::set_devices` with the new set, the same hook
  SIGHUP uses: links into a loopback that went away wait, and start again when it is back. Sinks given in
  saqad's own configuration stay, beside the engine's.
- **What saqa asks of the engine for that:** a loopbacks listing on the daemon (`GET /api/v1/loopbacks`, the
  `LoopbackState`s) and an events topic for loopbacks; the OS-visible name of each side of a loopback (on Linux
  the `DEV=0` side to play into and the `DEV=1` side to read); and a dedicated 2-channel `Streaming` loopback
  for receiving, so stereo does not mix with what the computer plays, and on Linux does not share
  `dspersystem`'s single substream. The daemon at `claude/devices` `b0a89e1` has `GET /api/v1/devices` and the
  `devices` events only; saqa does not guess loopbacks from those.

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
