#!/usr/bin/env bash
# Builds libroc (Roc Toolkit 0.4.0, MPL-2.0) into .saqa/lib, where saqad
# (saqa-roc) finds it. Pinned by commit; its own
# third-party libraries (libuv, OpenFEC, SpeexDSP) are built in, so nothing
# is installed system-wide. Needs a C++ compiler and the build tools below.
#
#   scripts/roc.sh
set -euo pipefail

ROC_TAG=v0.4.0
ROC_COMMIT=62401be9877ee087de49a40c35ae64c75a820387

root=$(cd "$(dirname "$0")/.." && pwd)
src=$root/.saqa/src/roc-toolkit
lib=$root/.saqa/lib

say() { printf '\033[1m%s\033[0m\n' "$*"; }
die() { printf '\033[31m%s\033[0m\n' "$*" >&2; exit 1; }
# The end of the build log, where a failure says why (CI shows only what is printed).
# scons runs without -Q, so the log has the full commands and the tools Roc chose.
log_tail() {
  [[ -f $1 ]] || return 0
  echo "--- the tools Roc chose, and the end of $1:" >&2
  grep -E 'Searching (CXX|CC|AR) executable|Checking for C\+\+ compiler' "$1" >&2 || true
  grep -m1 -E '^[^ ]*(clang\+\+|g\+\+|c\+\+) .* -c ' "$1" >&2 || true # one full compile command
  tail -n 40 "$1" >&2
}

# Roc's build needs SCons and ragel, and builds libuv, OpenFEC and SpeexDSP with CMake
# (and on Linux libuv and SpeexDSP with autotools).
tools=(scons ragel cmake)
[[ $(uname -s) == Darwin ]] || tools+=(autoreconf libtoolize pkg-config make)
missing=()
for t in "${tools[@]}"; do command -v "$t" >/dev/null || missing+=("$t"); done
if ((${#missing[@]})); then
  die "libroc's build needs ${missing[*]}: brew install scons ragel cmake (macOS), or
  sudo apt install scons ragel cmake autoconf automake libtool pkg-config make (Debian/Ubuntu)"
fi

# macOS: Roc's build names the dylib by its absolute path in the build tree, so an app linked
# against .saqa/lib would look there, and fail once that tree is gone (a CI cache, a clean).
# Name it @rpath/… instead (apps link with -rpath .saqa/lib), then re-sign it: on Apple
# silicon a changed, unsigned dylib does not load.
rpath_name() {
  [[ $(uname -s) == Darwin ]] || return 0
  local f id
  for f in "$lib"/libroc*.dylib; do
    [[ -f $f && ! -L $f ]] || continue
    id=$(otool -D "$f" | tail -n 1)
    [[ $id == @rpath/* ]] && continue
    install_name_tool -id "@rpath/$(basename "$id")" "$f"
    codesign --force --sign - "$f" >/dev/null 2>&1 || die "could not re-sign $f after naming it"
  done
}

# Already built from the pinned commit: nothing to do (SAQA_ROC_REBUILD=1 builds anyway).
if [[ -z ${SAQA_ROC_REBUILD:-} && $(cat "$lib/roc.commit" 2>/dev/null) == "$ROC_COMMIT" ]] &&
  compgen -G "$lib/libroc.*" >/dev/null && [[ -d $root/.saqa/include/roc ]]; then
  rpath_name
  say "libroc $ROC_TAG is already built in $lib"
  exit 0
fi

if [[ ! -d $src/.git ]]; then
  say "Fetching Roc Toolkit $ROC_TAG"
  git clone -q --depth 1 --branch "$ROC_TAG" https://github.com/roc-streaming/roc-toolkit "$src"
fi
[[ $(git -C "$src" rev-parse HEAD) == "$ROC_COMMIT" ]] ||
  die "roc-toolkit $ROC_TAG is not the pinned commit $ROC_COMMIT"

# macOS: 11.0, the same target as dsper's devices (dsper's macos/driver/build.sh), not
# whatever the building Mac runs; Roc derives it from sw_vers otherwise.
platform=()
if [[ $(uname -s) == Darwin ]]; then
  # /usr/bin's clang shims find the SDK and its C++ headers themselves. Without CXX/CC,
  # Roc searches for a compiler of its own (versioned ones first, e.g. clang++-21).
  platform=(--macos-platform=11.0 CXX=/usr/bin/clang++ CC=/usr/bin/clang)
  # Roc's C++ needs libc++'s headers. Xcode's command line tools sometimes lose them (after
  # a macOS upgrade): then every file fails with "'algorithm' file not found". Say so first.
  if ! printf '#include <algorithm>\nint main() {}\n' | /usr/bin/clang++ -x c++ -fsyntax-only - 2>/dev/null; then
    die "This Mac's C++ compiler has no C++ headers (<algorithm> is missing): reinstall Xcode's
  command line tools (sudo rm -rf /Library/Developer/CommandLineTools && xcode-select --install),
  or install Xcode and run: sudo xcode-select -s /Applications/Xcode.app"
  fi
fi
# CMake 4 (Homebrew's) refuses projects that ask for CMake < 3.5, as the libuv that Roc
# 0.4 builds does; this lets it configure them. Older CMake ignores it.
export CMAKE_POLICY_VERSION_MINIMUM=3.5

say "Building libroc (a minute or two)"
(
  cd "$src"
  scons -j"$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)" ${platform[@]+"${platform[@]}"} \
    --disable-tools --disable-sox --disable-sndfile --disable-pulseaudio \
    --disable-alsa --disable-openssl --disable-libunwind \
    --build-3rdparty=libuv,openfec,speexdsp >"$src/saqa-build.log" 2>&1
) || { log_tail "$src/saqa-build.log"; die "building libroc failed; see $src/saqa-build.log"; }

mkdir -p "$lib"
found=0
for f in "$src"/bin/*/libroc.*; do
  cp -P "$f" "$lib/" && found=1
done
((found)) || { log_tail "$src/saqa-build.log"; die "the build made no libroc (see $src/saqa-build.log)"; }
rpath_name
# Its headers too, for apps built with the SDK's saqa/stream.h.
mkdir -p "$root/.saqa/include"
cp -R "$src/src/public_api/include/roc" "$root/.saqa/include/"
echo "$ROC_COMMIT" >"$lib/roc.commit"
say "libroc $ROC_TAG is in $lib (headers in .saqa/include): saqad streams now (restart it if it runs)"
