# saqa's REST API — `/stream/v1`

saqad serves one versioned API, the one dsper's stream service served (dsper's docs/API.md, Stream, at
dsper@7afd988). dsper's web UI, MCP tools and client call it through dsperd, so its routes, bodies and statuses
are kept as they were. What changed is listed at the end.

saqad listens on `127.0.0.1:8486` (`--port`), plain HTTP, loopback only. Every route needs
`Authorization: Bearer <token>` except `/stream/v1/health`; a `Host` it does not answer to gets 421, a foreign
`Origin` 403 (docs/CONFIG.md). Errors are `{"error": "…"}`.

| Route | Body | Reply |
|---|---|---|
| `GET /state` | | `{available, detail, links}`: whether libroc is here (`detail` says how to get it when not), every link |
| `GET /links` | | `[LinkView]` |
| `PUT /links/{id}` | `LinkSpec` | `LinkView`; 400 invalid, 422 a receive link not into an allowed input, 503 no libroc |
| `DELETE /links/{id}` | | `null`, 404 when unknown |
| `GET /health` | | `{"ok": true, "service": "stream", "api": 1}`, no token needed |

A link's `id` is 1–40 letters, digits and `-`.

A `LinkSpec` is

```json
{"direction": "send",    "device": "daw",    "channels": [4, 5], "to": "corner.local:20000"}
{"direction": "receive", "device": "stream", "channels": [0, 1], "port": 20000, "latency_ms": 100}
```

channels 0-based, 1–16 of them, each once; ports 1024–65533 (a stream uses P, P+1 and P+2, and two receive
links' ranges may not overlap); `latency_ms` 10–2000, default 100. `device` is an alias saqad was given
(`system`, `daw`, `stream` from dsper), resolved for the link's direction, or a device name. A receive link's
device, once resolved, must be one of saqad's sinks: otherwise 422, and with no sinks configured always 422.

A `LinkView` is the `LinkSpec` (its `device` resolved) plus `id`, `state` (`starting`, `running`, `failed`),
`detail` (why it failed), `connections` (senders streaming to a receive link now), `e2e_latency_ms` (when Roc
knows it) and `dropouts` (audio the device could not take or give in time, since start).

Links are kept in saqad's links file (`~/.config/saqa/streams.json` by default) and restart with saqad.
See [STREAMING.md](STREAMING.md) for what links are for, and [CONFIG.md](CONFIG.md) for running saqad.

## What changed from dsper's stream service

Nothing in the routes, bodies, statuses or `LinkSpec`/`LinkView`. Only:

- **The 422 text.** It was "a stream plays only into one of dsper's inputs (dsper stream 16ch, …), never straight
  into '…'"; it is now "a stream plays only into an input this machine allows (*the sinks, or "none is
  configured here"*), never straight into '…': this machine's checked pipeline decides what reaches speakers".
  It is chosen by the kind of refusal, no longer by matching that text.
- **The 503 text** (and `/state`'s `detail`) says to run saqa's `scripts/roc.sh` or set `SAQA_LIBROC`, where it
  named dsper's `scripts/mac.sh roc` / `scripts/linux.sh roc` and `DSPER_LIBROC`.
- **Sinks and aliases are configuration.** `system`, `daw` and `stream`, and which devices may receive, mean what
  saqad is told (docs/CONFIG.md, "Pointing saqad at the streaming loopback"); unconfigured, it allows no receive
  link. The recommended setup allows only the engine's streaming loopback, so a receive link into `system` or
  `daw`, which dsper allowed, is now 422 unless they are added as sinks.
- **Served alone, on loopback only.** dsperd's `--listen` and TLS did not move: dsperd stays the door other
  machines' clients use, and saqad answers it on 127.0.0.1.
