# Streaming between dsper machines

dsper machines send each other audio over the network with [Roc](https://github.com/roc-streaming/roc-toolkit)
(roc-toolkit 0.4, MPL-2.0): real-time, up to 16 channels per link, with forward error correction (Reed-Solomon)
so a lost packet is repaired rather than heard, and a latency tuner that keeps two machines' clocks from drifting
apart.

A **link** goes one way:

- a **send** link captures chosen channels of a device on this machine — one of dsper's inputs (`daw 5–6`), or
  an interface's inputs where an instrument is plugged in — and streams them to `host:port`;
- a **receive** link listens on a port and plays what arrives into chosen channels of one of dsper's inputs on
  this machine, normally **dsper stream 16ch**.

**A stream never plays straight into an interface.** It lands in a dsper input, and from there the receiving
machine's own mix, patches and safety check decide what reaches its speakers — bands, limiters and ceilings apply
to streamed audio exactly as to local audio. A receive link into anything else is refused (422).

## Turn it on

Streaming is **optional and off by default**: dsper works fully without it, and until it is on, dsperd serves no
stream API, loads no libroc, opens no stream ports and starts no kept links, and the web UI shows no Streams tab.
On each machine that streams:

```sh
scripts/mac.sh roc                      # or scripts/linux.sh roc: libroc 0.4 into .saqa/lib (needs SCons, ragel, CMake; on Linux autotools)
dsperd settings set streaming on        # or the menu bar: Stream between dsper machines
```

then restart dsperd (the menu bar does that itself), or run a single `dsperd --streaming …`. `set streaming off`
turns it off again; links are kept and come back when it is on. If it is on and libroc is missing, the Streams
tab says so and how. dsperd finds libroc in `.saqa/lib`, at `SAQA_LIBROC`, or through the system's library
path.

Roc uses three UDP ports from the one you choose: P (audio), P+1 (repair), P+2 (RTCP). Open them on the receiver
if it has a firewall.

## Vintage hi-fi speakers in another corner of the room

The main machine plays; a second dsper machine (a Mac mini, a Raspberry Pi) sits by the old speakers with its
own interface or amp, and protects them where they are.

1. **Corner machine:** Streams → *Receive from another dsper machine*: name `from-studio`, port `20000`, into
   `stream`, channels `1–2`. Speakers → set up the vintage pair (passive: its ceiling is lower) with Source
   `stream 1–2`.
2. **Main machine:** Streams → *Send to another dsper machine*: name `corner`, from `daw`, channels `5–6`, to
   `corner.local:20000`. Whatever Ableton plays on its outputs 5–6 now plays in the corner.

To send what the whole Mac plays instead, send `system 1–2`.

## A DAW on one machine, a drum machine on another

The drum machine is plugged into an interface on a bridge machine; the DAW runs on another Mac.

1. **DAW machine:** receive `drums` on port `20010` into `stream 1–8`. In the DAW, pick **dsper stream 16ch** as
   the input device: tracks on inputs 1–8 record the drum machine. To hear it too, patch speakers from
   `stream 1–2` like any input.
2. **Bridge machine:** send `drums` from the interface (its name in the list, e.g. `TR-8S`), channels `1–8`, to
   `daw-mac.local:20010`.
3. For the other way (the DAW's click or sequencer to the drum machine's inputs), send from the DAW machine's
   `daw` channels to the bridge machine, which receives into its stream input and patches it to the drum
   machine's outputs.

## Interoperability

Links are standard Roc: `rtp+rs8m` on P, `rs8m` on P+1, `rtcp` on P+2. **Stereo** links use Roc's built-in
16-bit encoding, so `roc-send`, `roc-recv`, [roc-vad](https://github.com/roc-streaming/roc-vad) (a macOS virtual
device) and PipeWire's Roc modules can send to a dsper receive link or receive from a send link — a Mac without
dsper can stream to one with roc-vad. Other channel counts use dsper's multitrack encodings (32-bit float,
payload type 100 + channels), which both ends must know: dsper to dsper.

## Apps

An app can be either end of a link without dsper on its computer: the SDK's `saqa/stream.h` (MIT, C++ and JUCE)
sends into a dsper machine's receive link or receives its send link, up to 16 channels, real-time safe from an
audio callback. [sdk/STREAMS.md](../sdk/STREAMS.md).

## API

The stream service has its own REST API at `/stream/v1` ([API.md](API.md#stream--streamv1)); `dsperd --only
stream` runs it alone. The MCP server has `list_streams`, `stream_link` and `remove_stream`
([MCP.md](MCP.md)).
