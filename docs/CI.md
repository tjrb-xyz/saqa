# CI

`.github/workflows/ci.yml`, from dsper's `streaming` job and its macOS libroc step (dsper@7afd988). It runs on
every pull request and every push to `main`, and on demand.

| Job | Runner | What |
|---|---|---|
| `check` | `ubuntu-24.04` | `cargo fmt --check`, `cargo clippy --all-targets -D warnings`, `cargo test --workspace` without libroc (the Roc tests say "skipped" and pass) |
| `linux` | `ubuntu-24.04` | `scripts/roc.sh` builds libroc 0.4; `saqa/stream.h` compiled with g++ and clang++ with `-Werror`; `cargo test --workspace` with `SAQA_REQUIRE_ROC=1` (a missing libroc or SDK app fails), and the SDK test against both C++ builds (`SAQA_CPP_STREAM`) |
| `macos` | `macos-15` | The same on Apple silicon, with clang++ |

libroc is cached under `.saqa/` keyed on `scripts/roc.sh`'s hash (it pins Roc's commit), so it is rebuilt only
when the pin changes.

**Why GitHub-hosted runners.** saqa is public. GitHub's runners cost nothing for a public repository, and a
public repository must not use self-hosted runners: a pull request from any fork would run its code on the
machines that host them. dsper is private and runs only on its own machines (bana); that rule is about cost and
control of a private repository, and does not carry over.

## Locally

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                        # Roc tests skip without libroc
scripts/roc.sh                                # then, with libroc:
c++ -std=c++17 -O2 -pthread -Wall -Wextra -Wpedantic -Wsign-conversion -Werror \
  -I sdk/cpp/include -I .saqa/include sdk/cpp/examples/stream_levels.cpp \
  -L .saqa/lib -lroc -Wl,-rpath,"$PWD/.saqa/lib" -o /tmp/stream-levels
SAQA_REQUIRE_ROC=1 SAQA_CPP_STREAM=/tmp/stream-levels cargo test --workspace
```
