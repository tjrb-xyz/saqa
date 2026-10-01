// An app streaming with a machine running saqa, without saqa on this computer.
//
//   stream_levels send    CHANNELS HOST PORT SECONDS   channel c carries the constant (c+1)/32
//   stream_levels receive CHANNELS PORT SECONDS        prints each channel's level over the last second
//
// A real app writes its own audio where this writes constants. Build (after scripts/roc.sh):
//   c++ -std=c++17 -I sdk/cpp/include -I .saqa/include sdk/cpp/examples/stream_levels.cpp
//       -L .saqa/lib -lroc -Wl,-rpath,"$PWD/.saqa/lib" -o stream_levels

#include <saqa/stream.h>

#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <string>
#include <thread>
#include <vector>

namespace
{
// An audio device's callback: 256 frames every 5.33 ms (not Roc's 10 ms, as real devices aren't).
constexpr std::size_t kCallback = 256;

int usage()
{
    std::fprintf (stderr, "usage: stream_levels send CHANNELS HOST PORT SECONDS\n"
                          "       stream_levels receive CHANNELS PORT SECONDS\n");
    return 2;
}

unsigned number (const char* s) { return static_cast<unsigned> (std::strtoul (s, nullptr, 10)); }

/// Calls `callback()` at a device's pace for `seconds`.
template <typename F>
void runLikeADevice (unsigned seconds, F callback)
{
    const auto start = std::chrono::steady_clock::now();
    const auto calls = static_cast<unsigned long long> (seconds) * saqa::kStreamRate / kCallback;
    for (unsigned long long i = 1; i <= calls; ++i)
    {
        callback();
        std::this_thread::sleep_until (start + std::chrono::microseconds (i * kCallback * 1000000ULL
                                                                          / saqa::kStreamRate));
    }
}
} // namespace

int main (int argc, char** argv)
{
    if (argc < 2)
        return usage();
    const std::string mode = argv[1];
    try
    {
        if (mode == "send" && argc == 6)
        {
            const unsigned channels = number (argv[2]);
            saqa::StreamOut out (channels, argv[3], number (argv[4]));
            std::vector<float> block (kCallback * channels);
            for (std::size_t i = 0; i < block.size(); ++i)
                block[i] = static_cast<float> (i % channels + 1) / 32.0f;
            // What would be your processBlock: hand the block over, never wait.
            runLikeADevice (number (argv[5]), [&] { out.push (block.data(), kCallback); });
            std::fprintf (stderr, "dropped %llu frames\n", out.droppedFrames());
            return 0;
        }
        if (mode == "receive" && argc == 5)
        {
            const unsigned channels = number (argv[2]);
            const unsigned seconds = number (argv[4]);
            saqa::StreamIn in (channels, number (argv[3]), 60);
            std::vector<float> block (kCallback * channels);
            std::vector<double> sum (channels);
            const auto lastSecond = static_cast<unsigned long long> (seconds - 1) * saqa::kStreamRate / kCallback;
            unsigned long long call = 0, counted = 0;
            runLikeADevice (seconds, [&] {
                in.pull (block.data(), kCallback);
                if (++call <= lastSecond)
                    return;
                ++counted;
                for (std::size_t i = 0; i < block.size(); ++i)
                    sum[i % channels] += std::fabs (block[i]);
            });
            std::printf ("connections %u\n", in.connections());
            for (unsigned c = 0; c < channels; ++c)
                std::printf ("%u %.4f\n", c + 1, sum[c] / static_cast<double> (counted * kCallback));
            return 0;
        }
    }
    catch (const saqa::StreamError& e)
    {
        std::fprintf (stderr, "stream_levels: %s\n", e.what());
        return 1;
    }
    return usage();
}
