// saqa streams for apps: send audio to a machine running saqa, or receive what one sends.
//
// MIT licence, like all of saqa. Header-only; links libroc 0.4 (Roc Toolkit,
// MPL-2.0): `scripts/roc.sh` builds it into .saqa/lib and its headers into .saqa/include.
//
//   saqa::StreamSender tx (16, "studio.local", 20000);  // → that machine's receive link
//   tx.write (interleaved, frames);                        // from your audio callback's thread
//
//   saqa::StreamReceiver rx (2, 20010);                   // a saqa send link points here
//   rx.read (interleaved, frames);                         // silence until a sender streams
//
// The wire is exactly saqa's (sdk/STREAMS.md): Reed-Solomon FEC on P, repair on P+1, RTCP on
// P+2; stereo in Roc's standard L16 encoding (any Roc peer interoperates), other channel counts
// in saqa's multitrack float32 encodings (RTP payload type 100 + channels), 48 kHz.
//
// A receiving saqa plays what it gets only into an input it is told it may (dsper: one of its
// dsper inputs), never straight into an interface: that machine's own checked pipeline decides
// what reaches speakers. Nothing here can change that, from either end.
//
// From an audio callback (JUCE processBlock / audioDeviceIOCallbackWithContext), use StreamOut
// and StreamIn: push() and pull() never block, lock or allocate, and a thread of their own talks
// to Roc. StreamSender and StreamReceiver are the blocking layer beneath them, for a thread
// that already runs at audio pace.

#pragma once

#include <roc/config.h>
#include <roc/context.h>
#include <roc/endpoint.h>
#include <roc/frame.h>
#include <roc/metrics.h>
#include <roc/receiver.h>
#include <roc/sender.h>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cstddef>
#include <memory>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>

namespace saqa
{

constexpr unsigned kStreamRate = 48000;
constexpr unsigned kStreamMaxChannels = 16;

class StreamError : public std::runtime_error
{
public:
    using std::runtime_error::runtime_error;
};

namespace detail
{
inline void check (int r, const std::string& what)
{
    if (r != 0)
        throw StreamError ("roc: " + what + " failed");
}

inline void checkShape (unsigned channels, unsigned port)
{
    if (channels < 1 || channels > kStreamMaxChannels)
        throw StreamError ("a stream carries 1-16 channels");
    if (port < 1024 || port > 65533)
        throw StreamError ("port: 1024-65533 (a stream uses it and the next two)");
}

inline int multitrackId (unsigned channels) { return 100 + static_cast<int> (channels); }

inline roc_media_encoding frameEncoding (unsigned channels)
{
    roc_media_encoding e {};
    e.rate = kStreamRate;
    e.format = ROC_FORMAT_PCM_FLOAT32;
    e.channels = channels == 2 ? ROC_CHANNEL_LAYOUT_STEREO : ROC_CHANNEL_LAYOUT_MULTITRACK;
    e.tracks = channels == 2 ? 0 : channels;
    return e;
}

/// saqa's packet length: Roc's 5 ms for stereo; for float32 multitrack, what fits 1200 bytes.
inline unsigned long long packetLengthNs (unsigned channels)
{
    if (channels == 2)
        return 5000000ULL;
    unsigned frames = 1200u / (channels * 4u);
    if (frames > kStreamRate / 200u)
        frames = kStreamRate / 200u;
    return static_cast<unsigned long long> (frames) * 1000000000ULL / kStreamRate;
}

/// A context with saqa's 16 multitrack encodings registered, as saqad's are.
class Context
{
public:
    Context()
    {
        roc_context_config c {};
        check (roc_context_open (&c, &ptr), "context");
        for (unsigned ch = 1; ch <= kStreamMaxChannels; ++ch)
        {
            roc_media_encoding e {};
            e.rate = kStreamRate;
            e.format = ROC_FORMAT_PCM_FLOAT32;
            e.channels = ROC_CHANNEL_LAYOUT_MULTITRACK;
            e.tracks = ch;
            if (roc_context_register_encoding (ptr, multitrackId (ch), &e) != 0)
            {
                roc_context_close (ptr);
                throw StreamError ("roc: register encoding failed");
            }
        }
    }
    ~Context() { roc_context_close (ptr); }
    Context (const Context&) = delete;
    Context& operator= (const Context&) = delete;
    roc_context* ptr = nullptr;
};

/// Calls `f (interface, uri)` for P, P+1, P+2 of `host` (brackets added for IPv6).
template <typename F>
void forEachEndpoint (std::string host, unsigned port, F f)
{
    if (host.find (':') != std::string::npos && host.front() != '[')
        host = "[" + host + "]";
    const auto at = [&] (unsigned p) { return host + ":" + std::to_string (p); };
    f (ROC_INTERFACE_AUDIO_SOURCE, "rtp+rs8m://" + at (port));
    f (ROC_INTERFACE_AUDIO_REPAIR, "rs8m://" + at (port + 1));
    f (ROC_INTERFACE_AUDIO_CONTROL, "rtcp://" + at (port + 2));
}

struct Endpoint
{
    explicit Endpoint (const std::string& uri)
    {
        check (roc_endpoint_allocate (&ptr), "endpoint");
        if (roc_endpoint_set_uri (ptr, uri.c_str()) != 0)
        {
            roc_endpoint_deallocate (ptr);
            throw StreamError ("roc: bad address " + uri);
        }
    }
    ~Endpoint() { roc_endpoint_deallocate (ptr); }
    Endpoint (const Endpoint&) = delete;
    Endpoint& operator= (const Endpoint&) = delete;
    roc_endpoint* ptr = nullptr;
};
} // namespace detail

/// Streams interleaved float32 audio to a saqa receive link (or any Roc receiver,
/// when stereo).
class StreamSender
{
public:
    StreamSender (unsigned channels, const std::string& host, unsigned port) : numChannels (channels)
    {
        detail::checkShape (channels, port);
        roc_sender_config c {};
        c.frame_encoding = detail::frameEncoding (channels);
        c.packet_encoding = channels == 2 ? ROC_PACKET_ENCODING_AVP_L16_STEREO
                                          : static_cast<roc_packet_encoding> (detail::multitrackId (channels));
        c.packet_length = detail::packetLengthNs (channels);
        c.fec_encoding = ROC_FEC_ENCODING_RS8M;
        c.clock_source = ROC_CLOCK_SOURCE_INTERNAL; // write() paces itself at 48 kHz
        detail::check (roc_sender_open (context.ptr, &c, &ptr), "sender");
        try
        {
            detail::forEachEndpoint (host, port, [&] (roc_interface iface, const std::string& uri) {
                detail::Endpoint e (uri);
                detail::check (roc_sender_connect (ptr, ROC_SLOT_DEFAULT, iface, e.ptr), "connect " + uri);
            });
        }
        catch (...)
        {
            roc_sender_close (ptr);
            throw;
        }
    }
    ~StreamSender() { roc_sender_close (ptr); }
    StreamSender (const StreamSender&) = delete;
    StreamSender& operator= (const StreamSender&) = delete;

    unsigned channels() const { return numChannels; }

    /// `frames` frames of `channels()` interleaved samples. Blocks for their duration.
    void write (const float* interleaved, std::size_t frames)
    {
        roc_frame f {};
        f.samples = const_cast<float*> (interleaved);
        f.samples_size = frames * numChannels * sizeof (float);
        detail::check (roc_sender_write (ptr, &f), "send");
    }

private:
    detail::Context context;
    unsigned numChannels;
    roc_sender* ptr = nullptr;
};

/// Listens on `port` (and the next two) for a saqa send link, or any Roc sender when stereo.
/// Several senders are mixed.
class StreamReceiver
{
public:
    StreamReceiver (unsigned channels, unsigned port, unsigned latencyMs = 100,
                    const std::string& host = "0.0.0.0")
        : numChannels (channels)
    {
        detail::checkShape (channels, port);
        if (latencyMs < 10 || latencyMs > 2000)
            throw StreamError ("latency: 10-2000 ms");
        roc_receiver_config c {};
        c.frame_encoding = detail::frameEncoding (channels);
        c.clock_source = ROC_CLOCK_SOURCE_INTERNAL; // read() paces itself at 48 kHz
        c.target_latency = static_cast<unsigned long long> (latencyMs) * 1000000ULL;
        detail::check (roc_receiver_open (context.ptr, &c, &ptr), "receiver");
        try
        {
            detail::forEachEndpoint (host, port, [&] (roc_interface iface, const std::string& uri) {
                detail::Endpoint e (uri);
                detail::check (roc_receiver_bind (ptr, ROC_SLOT_DEFAULT, iface, e.ptr),
                               "listen on " + uri + " (is the port free?)");
            });
        }
        catch (...)
        {
            roc_receiver_close (ptr);
            throw;
        }
    }
    ~StreamReceiver() { roc_receiver_close (ptr); }
    StreamReceiver (const StreamReceiver&) = delete;
    StreamReceiver& operator= (const StreamReceiver&) = delete;

    unsigned channels() const { return numChannels; }

    /// Fills `frames` interleaved frames; silence while nothing arrives. Blocks for their duration.
    void read (float* interleaved, std::size_t frames)
    {
        roc_frame f {};
        f.samples = interleaved;
        f.samples_size = frames * numChannels * sizeof (float);
        detail::check (roc_receiver_read (ptr, &f), "receive");
    }

    /// Senders streaming here now.
    unsigned connections()
    {
        roc_receiver_metrics m {};
        roc_connection_metrics conns[1] {};
        std::size_t n = 1;
        if (roc_receiver_query (ptr, ROC_SLOT_DEFAULT, &m, conns, &n) != 0)
            return 0;
        return m.connection_count;
    }

private:
    detail::Context context;
    unsigned numChannels;
    roc_receiver* ptr = nullptr;
};

namespace detail
{
/// Single-producer single-consumer ring of interleaved frames. A frame is published whole or not
/// at all, so the reader never sees part of one (a torn frame shifts every channel after it).
class FrameRing
{
public:
    FrameRing (unsigned channels, std::size_t frames) : n (channels), data ((frames + 1) * channels) {}

    std::size_t readableFrames() const
    {
        const auto w = head.load (std::memory_order_acquire), r = tail.load (std::memory_order_relaxed);
        return (w + data.size() - r) % data.size() / n;
    }
    std::size_t writableFrames() const
    {
        const auto w = head.load (std::memory_order_relaxed), r = tail.load (std::memory_order_acquire);
        return (r + data.size() - w - n) % data.size() / n;
    }
    /// Writes all `frames` or none; says which.
    bool write (const float* src, std::size_t frames) noexcept
    {
        if (writableFrames() < frames)
            return false;
        auto w = head.load (std::memory_order_relaxed);
        for (std::size_t i = 0; i < frames * n; ++i)
        {
            data[w] = src[i];
            w = (w + 1) % data.size();
        }
        head.store (w, std::memory_order_release);
        return true;
    }
    /// Reads up to `frames` whole frames; returns how many.
    std::size_t read (float* dst, std::size_t frames) noexcept
    {
        frames = std::min (frames, readableFrames());
        auto r = tail.load (std::memory_order_relaxed);
        for (std::size_t i = 0; i < frames * n; ++i)
        {
            dst[i] = data[r];
            r = (r + 1) % data.size();
        }
        tail.store (r, std::memory_order_release);
        return frames;
    }

private:
    unsigned n;
    std::vector<float> data;
    std::atomic<std::size_t> head { 0 }, tail { 0 };
};

constexpr std::size_t kBlockFrames = kStreamRate / 100; // Roc is fed in 10 ms blocks
} // namespace detail

/// Streams to a saqa receive link from an audio callback: push() is real-time safe.
class StreamOut
{
public:
    StreamOut (unsigned channels, const std::string& host, unsigned port)
        : sender (std::make_unique<StreamSender> (channels, host, port)),
          ring (channels, detail::kBlockFrames * 20) // 200 ms of room
    {
        worker = std::thread ([this] { run(); });
    }
    ~StreamOut()
    {
        stop.store (true);
        worker.join();
    }
    StreamOut (const StreamOut&) = delete;
    StreamOut& operator= (const StreamOut&) = delete;

    /// `frames` interleaved frames at 48 kHz. Never blocks; when the stream has fallen behind,
    /// the block is dropped whole and counted.
    void push (const float* interleaved, std::size_t frames) noexcept
    {
        if (! ring.write (interleaved, frames))
            dropped.fetch_add (frames, std::memory_order_relaxed);
    }
    /// Frames dropped since start (the network or this machine could not keep up).
    unsigned long long droppedFrames() const { return dropped.load (std::memory_order_relaxed); }

private:
    void run()
    {
        std::vector<float> block (detail::kBlockFrames * sender->channels());
        while (! stop.load())
        {
            if (ring.readableFrames() >= detail::kBlockFrames)
            {
                ring.read (block.data(), detail::kBlockFrames);
                try { sender->write (block.data(), detail::kBlockFrames); }
                catch (const StreamError&) { dropped.fetch_add (detail::kBlockFrames); }
            }
            else
                std::this_thread::sleep_for (std::chrono::milliseconds (2));
        }
    }

    std::unique_ptr<StreamSender> sender;
    detail::FrameRing ring;
    std::atomic<bool> stop { false };
    std::atomic<unsigned long long> dropped { 0 };
    std::thread worker;
};

/// Receives a saqa send link into an audio callback: pull() is real-time safe.
class StreamIn
{
public:
    StreamIn (unsigned channels, unsigned port, unsigned latencyMs = 100)
        : receiver (std::make_unique<StreamReceiver> (channels, port, latencyMs)),
          ring (channels, detail::kBlockFrames * 3) // two blocks ahead of the callback
    {
        worker = std::thread ([this] { run(); });
    }
    ~StreamIn()
    {
        stop.store (true);
        worker.join();
    }
    StreamIn (const StreamIn&) = delete;
    StreamIn& operator= (const StreamIn&) = delete;

    /// Fills `frames` interleaved frames: what has arrived, then silence. Never blocks.
    void pull (float* interleaved, std::size_t frames) noexcept
    {
        const auto got = ring.read (interleaved, frames);
        std::fill (interleaved + got * n, interleaved + frames * n, 0.0f);
        if (got < frames && primed.load (std::memory_order_relaxed))
            short_.fetch_add (frames - got, std::memory_order_relaxed);
    }
    /// Senders streaming here now.
    unsigned connections() const { return conns.load (std::memory_order_relaxed); }
    /// Frames played as silence for want of audio, since the first arrived.
    unsigned long long shortFrames() const { return short_.load (std::memory_order_relaxed); }

private:
    void run()
    {
        std::vector<float> block (detail::kBlockFrames * n);
        unsigned tick = 0;
        while (! stop.load())
        {
            if (ring.writableFrames() >= detail::kBlockFrames)
            {
                try { receiver->read (block.data(), detail::kBlockFrames); }
                catch (const StreamError&) { std::fill (block.begin(), block.end(), 0.0f); }
                ring.write (block.data(), detail::kBlockFrames);
                primed.store (true, std::memory_order_relaxed);
            }
            else
                std::this_thread::sleep_for (std::chrono::milliseconds (2));
            if (++tick % 25 == 0)
                conns.store (receiver->connections(), std::memory_order_relaxed);
        }
    }

    std::unique_ptr<StreamReceiver> receiver;
    unsigned n = receiver->channels();
    detail::FrameRing ring;
    std::atomic<bool> stop { false }, primed { false };
    std::atomic<unsigned> conns { 0 };
    std::atomic<unsigned long long> short_ { 0 };
    std::thread worker;
};

} // namespace saqa
