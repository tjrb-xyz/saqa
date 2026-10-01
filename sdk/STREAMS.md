# saqa streams, for apps

An app can send its audio to a machine running saqa, or play what one sends it, over the network, with no saqa
and no virtual device on the app's own computer. A synth sends its 16 outputs to the studio; a DAW records a drum
machine plugged into another machine. The app is the far end of a saqa **link**
([docs/STREAMING.md](../docs/STREAMING.md)):

- **app → saqa:** the receiving machine has a *receive* link on a port (with dsper: Streams → Receive, into
  `stream`). The app sends to `host:port`. What arrives lands only in an input that machine allows (with dsper,
  its **dsper stream 16ch** input), and its speakers take it from there under their own protection. **The app
  cannot reach an interface directly**: that rule is the receiving saqa's, and nothing a sender does changes it.
- **saqa → app:** the app listens on a port; the other machine has a *send* link to `app-host:port`.

MIT, like all of saqa. It uses [Roc](https://github.com/roc-streaming/roc-toolkit) (libroc 0.4,
MPL-2.0): `scripts/roc.sh` builds it into `.saqa/lib`, with its headers in `.saqa/include`.

## C++ and JUCE: `cpp/include/saqa/stream.h`

Header-only. From an audio callback, use `StreamOut` and `StreamIn`: `push` and `pull` never block, lock or
allocate, and each has a thread of its own that talks to Roc.

```cpp
#include <saqa/stream.h>

// In a juce::AudioProcessor (or an AudioIODeviceCallback): send this app's output to the studio.
std::unique_ptr<saqa::StreamOut> out;   // made in prepareToPlay, at 48 kHz:
out = std::make_unique<saqa::StreamOut> (2, "studio.local", 20000);

void processBlock (juce::AudioBuffer<float>& buffer, juce::MidiBuffer&) override
{
    // … your processing …
    const int n = buffer.getNumSamples(), ch = buffer.getNumChannels();
    for (int i = 0; i < n; ++i)                        // interleave into a member vector
        for (int c = 0; c < ch; ++c)                    // sized in prepareToPlay
            interleaved[size_t (i * ch + c)] = buffer.getSample (c, i);
    out->push (interleaved.data(), size_t (n));        // never waits
}
```

`saqa::StreamIn in (16, 20010)` and `in.pull (interleaved, frames)` receive the same way: what has arrived,
then silence. `connections()`, `droppedFrames()` and `shortFrames()` say how the stream is doing.

Build: `-I sdk/cpp/include -I .saqa/include -L .saqa/lib -lroc` (and `-pthread`). In CMake:

```cmake
target_include_directories(my_app PRIVATE path/to/saqa/sdk/cpp/include path/to/saqa/.saqa/include)
target_link_directories(my_app PRIVATE path/to/saqa/.saqa/lib)
target_link_libraries(my_app PRIVATE roc)
```

The working example is `cpp/examples/stream_levels.cpp`: an app sending 16 channels from a simulated device
callback of 256 frames, and one receiving. `crates/saqa-stream/tests/sdk.rs` runs it against real saqa links,
both directions, and checks that every one of 16 channels arrives on its own channel.

The stream is 48 kHz. An app running at another rate resamples before `push` (juce::LagrangeInterpolator,
libsamplerate) — saqa's receive side corrects clock *drift*, not a different nominal rate.

`StreamSender` and `StreamReceiver` are the blocking layer beneath them (`write` and `read` take as long as the
audio lasts), for a thread that already runs at audio pace.

## The wire (for any other language)

A link is standard Roc on three UDP ports from the one chosen, P:

| port | Roc interface | URI |
|---|---|---|
| P | audio source, Reed-Solomon FEC | `rtp+rs8m://host:P` |
| P+1 | audio repair | `rs8m://host:P+1` |
| P+2 | control (RTCP) | `rtcp://host:P+2` |

- **Stereo:** Roc's built-in `ROC_PACKET_ENCODING_AVP_L16_STEREO`, 5 ms packets. Any Roc peer interoperates:
  `roc-send`, `roc-recv`, roc-vad, PipeWire's Roc modules.
- **1 and 3–16 channels:** saqa's multitrack encodings. Register, in the context of both ends, encoding id
  **100 + channels** as `{rate: 48000, format: ROC_FORMAT_PCM_FLOAT32, channels: ROC_CHANNEL_LAYOUT_MULTITRACK,
  tracks: channels}`, and send with that packet encoding. Packets are `min(240, 1200 / (channels × 4))` frames
  long, so a packet fits one UDP datagram.
- FEC: `ROC_FEC_ENCODING_RS8M`. Frames: interleaved float32.

## Why a stream lands in an input

A stream is audio from another computer. saqa plays it only into an input the receiving machine allows, never
into an interface. With dsper, that is one of dsper's inputs, treated as any app's audio: the receiving machine's
checked pipeline — bands, limiter, ceiling, gate — decides what reaches its speakers. So an app can send anything,
including silence, noise or a full-scale sine, and the speakers are protected exactly as they are from a local
app. The protection after the input is the host's (dsper's); saqa's part is that nothing else is played into.
