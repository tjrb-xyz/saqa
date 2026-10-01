# saqa

saqa carries audio between machines: links of up to 16 channels over [Roc](https://github.com/roc-streaming/roc-toolkit),
with a REST API at `/stream/v1` served by `saqad`. It started as dsper's streaming (github.com/tjrb-xyz/dsper,
imported at dsper@7afd988); see [docs/STREAMING.md](docs/STREAMING.md), [docs/API.md](docs/API.md) and
[sdk/STREAMS.md](sdk/STREAMS.md).

MIT ([LICENSE](LICENSE)). libroc (MPL-2.0) is loaded at run time; `scripts/roc.sh` builds it into `.saqa/lib`.
