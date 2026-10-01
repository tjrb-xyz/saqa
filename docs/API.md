# saqa's REST API

Copied from dsper's docs/API.md (the Stream section) at dsper@7afd988.

## Stream — `/stream/v1`

| Route | Body | Reply |
|---|---|---|
| `GET /state` | | `{available, detail, links}`: whether libroc is installed (and how to, when not), every link |
| `GET /links` | | `[LinkView]` |
| `PUT /links/{id}` | `LinkSpec` | `LinkView`; 400 invalid, 422 a receive link not into a dsper input, 503 no libroc |
| `DELETE /links/{id}` | | `null`, 404 when unknown |

A `LinkSpec` is `{"direction": "send", "device", "channels", "to": "host:port"}` or `{"direction": "receive",
"device", "channels", "port", "latency_ms"?}`, channels 0-based. `device` may be `system`, `daw` or `stream`
(dsper's inputs, resolved for this platform) or a device name. A `LinkView` adds `id`, `state` (`starting`,
`running`, `failed`), `detail`, `connections`, `e2e_latency_ms` and `dropouts`. Links are kept in
`~/.config/dsper/streams.json` and restart with dsperd. See [STREAMING.md](STREAMING.md).
