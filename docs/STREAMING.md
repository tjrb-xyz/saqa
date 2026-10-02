# Streaming between machines

saqa sends audio between machines over the network with [Roc](https://github.com/roc-streaming/roc-toolkit)
(roc-toolkit 0.4, MPL-2.0): real-time, up to 16 channels per link, with forward error correction (Reed-Solomon)
so a lost packet is repaired rather than heard, and a latency tuner that keeps two machines' clocks from drifting
apart.

A **link** goes one way:

- a **send** link captures chosen channels of a device on this machine and streams them to `host:port`: the
  **stream loopback** (`stream`, with dsper today **stream 16ch**), which carries dsper's mix or the system sound
  as saqa chooses, or another loopback (`daw`, `system`), or an interface's inputs where an instrument is plugged
  in;
- a **receive** link listens on a port and plays what arrives into chosen channels of one of audio-engine's
  loopbacks, virtual devices the engine owns: the **receive loopback** (16 channels; with dsper today
  **stream in 16ch**, dsper's `stream` input), a **2-channel loopback** for stereo (with dsper today **dsper system 2ch**), or any
  loopback made later for a room or a creative use. dsper routes it from there, through its DSP, to speakers.

audio-engine carries the audio and owns the virtual devices; dsper does DSP and routing, and may summon those
devices from the engine; saqa carries audio between machines (docs/AUDIO-ENGINE.md, "Who does what").

**A stream never plays straight into an interface.** It lands only in a loopback saqad was told it may play into
(its *sinks*, docs/CONFIG.md), and from there the receiving machine's own pipeline decides what reaches its
speakers. With dsper, its mix, patches and safety check apply to streamed audio exactly as to local audio. A
receive link into anything else is refused (422); with no sinks configured, every receive link is.

This was dsper's guarantee, enforced inside dsper's stream service. It is now saqa's rule; what protects the
speakers after the sink is the host's (dsper protects what enters its inputs).

## Running it

```sh
scripts/roc.sh      # libroc 0.4 into .saqa/lib (needs SCons, ragel, CMake; on Linux autotools)
cargo build --release
target/release/saqad --sink 'stream in 16ch' --sink 'dsper system 2ch' \
  --sink-width 'stream in 16ch=16' --sink-width 'dsper system 2ch=2' \
  --alias-receive 'stream=stream in 16ch' --alias-send 'stream=stream 16ch' --alias 'system=dsper system 2ch'
```

saqad serves `/stream/v1` on `127.0.0.1:8486` (docs/API.md). With dsper, dsperd runs or reaches saqad, names the
loopbacks and the role words (docs/CONFIG.md, "Pointing saqad at the loopbacks"), and serves
the same API to dsper's web UI and MCP tools; whether streaming is on is dsper's setting.

saqad finds libroc at `SAQA_LIBROC`, then in the checkout's `.saqa/lib`, then in Homebrew's lib directories
(macOS), then through the system's library path. Without it saqad still runs: `/state` says what to install, and
a new link is refused with 503. `SAQA_ROC_LOG` sets libroc's log level (0 none … 5 trace; 1, errors, by default).

Roc uses three UDP ports from the one you choose: P (audio), P+1 (repair), P+2 (RTCP). Open them on the receiver
if it has a firewall.

## Vintage hi-fi speakers in another corner of the room

The main machine plays; a second dsper machine (a Mac mini, a Raspberry Pi) sits by the old speakers with its
own interface or amp, and protects them where they are.

1. **Corner machine:** receive `from-studio` on port `20000`, into `stream`, channels `1–2` (dsper's Streams
   view, or `PUT /stream/v1/links/from-studio`). In dsper, set up the vintage pair with Source `stream 1–2`.
2. **Main machine:** send `corner` from `daw`, channels `5–6`, to `corner.local:20000`. Whatever the DAW plays on
   its outputs 5–6 now plays in the corner.

To send what the whole Mac plays instead, send `system 1–2`. A stereo stream can also be received into the
2-channel loopback, `system 1–2`, where it plays like the corner machine's own sound.

## More loopbacks

audio-engine can make more loopbacks over time (dsper may ask it to): one per room, a booth monitor, a DAW's
return, an installation. Each becomes a place a stream can land once saqad is told it is a sink, and a reload
(SIGHUP) is enough: no restart (docs/CONFIG.md, "More loopbacks later").

## A DAW on one machine, a drum machine on another

The drum machine is plugged into an interface on a bridge machine; the DAW runs on another Mac.

1. **DAW machine:** receive `drums` on port `20010` into `stream 1–8`. In the DAW, pick **stream in 16ch** as
   the input device: tracks on inputs 1–8 record the drum machine.
2. **Bridge machine:** send `drums` from the interface (its name in the list, e.g. `TR-8S`), channels `1–8`, to
   `daw-mac.local:20010`.
3. For the other way (the DAW's click to the drum machine), send from the DAW machine's `daw` channels to the
   bridge machine, which receives into its stream input and patches it to the drum machine's outputs.

## Interoperability

Links are standard Roc: `rtp+rs8m` on P, `rs8m` on P+1, `rtcp` on P+2. **Stereo** links use Roc's built-in
16-bit encoding, so `roc-send`, `roc-recv`, [roc-vad](https://github.com/roc-streaming/roc-vad) (a macOS virtual
device) and PipeWire's Roc modules can send to a saqa receive link or receive from a send link. Other channel
counts use saqa's multitrack encodings (32-bit float, payload type 100 + channels), which both ends must know:
saqa to saqa, or an app with the SDK.

## Apps

An app can be either end of a link without saqa on its computer: the SDK's `saqa/stream.h` (MIT, C++ and JUCE)
sends into a receive link or receives a send link, up to 16 channels, real-time safe from an audio callback.
[sdk/STREAMS.md](../sdk/STREAMS.md).

## API

[API.md](API.md). dsper's MCP server keeps its `list_streams`, `stream_link` and `remove_stream` tools, calling
this API through dsperd.
