# saqa on audio-engine

## Who does what

The owner's words, as the engine and dsper record them:

- "the audio should be streamed from audio-engine; dsper is only in charge of dsp and patching audio routes
  between devices" (2026-10-01; the engine's D-093).
- "audio devices including virtual audio devices (loopbacks, virtual outputs and input interfaces) including
  physical interfaces and instruments are all ultimately owned by audio-engine" (the engine's D-094), and
  "for the device manager per saqa it's owned by audio engine and managed by saqa" (D-099). One rule for every
  device: the engine owns it, the program that uses it manages it, by a lease (D-100).
- "saqa is the device manager and can ask for input from main or from dsper or other audio passthrough
  devices" (D-095): saqa chooses what feeds its streaming loopback, through the engine's SDK.
- "dsper comes in the chain before saqa, saqa in itself can be thought of as an output", and "When listening to
  saqa's stream loopback device, we should hear dsper's mix, or the system sound. saqa chooses which device
  sends to stream loopback device" (dsper's owner decision #20, 2026-10-02). Received audio goes to a second
  device, which dsper reads as its `stream` input.
- We need to receive playback on at least one 2-channel loopback, and to make more devices later "for
  receiving playback and creative use cases in various places" (2026-10-02).

| | Owns or manages | In saqa's terms |
|---|---|---|
| **audio-engine** | Owns every device, physical or virtual, and the audio itself: streams, playback, the real-time path, the one view of how devices connect (D-093, D-094). Gives control of a device as a lease | Where saqa's audio comes from and goes to |
| **dsper** | DSP (CamillaDSP, rooms, speaker systems) and routing, on devices it manages by lease. Builds and installs the loopbacks today, as the engine's stand-in, until the engine ships them (dsper's #18, #19) | Reads what saqa receives (`stream in 16ch`) as its `stream` input and routes it through its DSP; its mix is one source saqa may send. Calls saqa's API for its Streams view and MCP tools |
| **saqa** | Carries audio between machines (Roc), its links and its API. **Manages the streaming loopback** (D-099), and chooses its source (D-095) | Sends what the `stream 16ch` loopback carries; plays received streams only into the loopbacks it is told are receivable: `stream in 16ch`, at least one 2-channel loopback, and any number made later |

So saqa never plays into a device dsper routes to speakers, or into an interface: only into a receive loopback,
which dsper then routes. The engine itself does no network streaming (its D-088).

Open, and the owner's (dsper's #19 and its PR comment on saqa#1):
- **"Single tap"**: if the engine becomes each loopback's only reader, saqa's send link reads the engine's tap as
  an attached client instead of opening `stream 16ch` itself, and the sink check becomes role plus lease.
- **One streaming device or two**: the engine's D-095 names one streaming loopback; dsper's #20 split it into a
  send device and a receive device.
- **Whether a send link may still capture any device**, as `LinkSpec.device` allows today, or only what the
  engine routes into `stream 16ch`.

## Where saqa is today

**saqa does not depend on audio-engine yet:** the engine has no licence (its D-033 is open, and neither `main`
nor `claude/devices` has a LICENSE file). saqa is MIT and cannot take a dependency it may not distribute. Until
then:

- saqa opens devices through cpal (`CpalAudio`), as it did in dsper;
- it is told which devices are the loopbacks, as sinks with their widths, and aliases (docs/CONFIG.md). With
  dsper today they are `stream in 16ch`, `stream 16ch` and `dsper system 2ch` (macOS), or `hw:CARD=streamin`,
  `hw:CARD=stream` and `hw:CARD=dspersystem` (Linux): the device names change, saqa does not;
- it learns of a loopback made later when its config file changes and it gets SIGHUP;
- it cannot yet choose what feeds `stream 16ch`: that is the engine routing a source into the loopback (D-095),
  which nothing builds yet. Until then whatever plays into `stream 16ch` (dsper's routing, or a person) is what
  `stream` sends.

Engine facts below were checked on audio-engine `main` at `549c9cc` (`ae-device`, merged in #5) and
`claude/devices` at `7c14d87` (`ae-io`, the daemon's devices, D-094 to D-100).

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

| # | saqa needs | Used by | `ae-io` at `claude/devices` `7c14d87` |
|---|---|---|---|
| **G4** | Capture from a **named** device; capture-only and playback-only streams; several streams at once (one per link) | `Audio::capture` / `playback`, the pump | Devices open by id (G1–G3), and input now follows the device asked for (`cpal_dev.rs:139-146`); but streams are still output-driven, with input on a second stream through a ring |
| **G5** | I16 and I32 devices converted to f32; the exact channels asked (a subset of a wider device, in the link's order), at 48 kHz | `CpalAudio::config`, `pick`/`spread` | f32 configs with exactly the requested channel count only (`cpal_dev.rs:88`) |
| **G6** | Xruns and stream errors, per stream | a link's `dropouts` and `detail` | `reports_xruns: false` (`cpal_dev.rs:197`); errors through `Reporter` |

And two more that were dsper's and now apply to saqa:

| # | saqa needs | Why |
|---|---|---|
| G8 | The real-time contract (no allocation, no print in callbacks), which saqa adopts | `CpalAudio`'s callbacks allocate (`audio.rs`, `input_as!`/`output_as!` resize and extend buffers) and print errors; the engine's rules forbid both |
| G9 | A licence under which MIT saqa may depend on `ae-io` and `ae-device` (the engine's D-033), and a pinned tag | everything above |

Also noted for whoever builds `AeAudio`: open devices with `ae-io`'s `Device::Id`, not `Device::Named`, which
matches a name by *substring* (`cpal_dev.rs:59`, still on `7c14d87`). saqa matches the exact name or id, and that
matters: by substring, `stream 16ch` also matches `dsper stream 16ch`, the old device still installed on a
machine that has not reinstalled dsper's devices, and a link must never open a device it was not named.

## If saqa adopts the engine's build settings

audio-engine uses edition 2024, toolchain 1.94.1, and denies `unsafe_op_in_unsafe_fn`, `unwrap`, `expect` and
`panic` outside tests. saqa is edition 2021 with dsper's settings today. Moving would need, per dsper's
inventory: an `unsafe { }` around `lib.get` in `saqa-roc`'s `ffi.rs` (16 errors otherwise), and the `expect`s in
saqa-stream's non-test code (about a dozen, mostly mutex locks) replaced. Not needed until saqa depends on the engine.
