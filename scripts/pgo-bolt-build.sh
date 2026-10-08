#!/usr/bin/env bash
#
# Profile-guided-optimization (PGO), optionally followed by BOLT, for indicatrix-worker
# and/or indicatrix-cut. Supports sequential execution or parallel dispatch across all
# tiers, and across either or both target binaries (`--target worker|cut|all`).
#
# Host/target OS: PGO training runs the instrumented binaries directly, so it needs the
# SAME target triple as the optimised build it feeds -- a target triple is part of a
# crate's mangled-symbol hash (`-C metadata`), same as the package set and feature list
# (see the parity note below), so training under one triple and building for another
# silently produces a profile that does not apply, even though `llvm-profdata merge`
# reports success (this script used to train on $HOST_TARGET unconditionally and cross-
# compile the optimised build to `x86_64-pc-windows-gnu`, which is exactly that bug).
# `--os` accepts `linux|windows`; the default, `auto`, resolves to this host's own OS.
# Three cases:
#   - Native (`--os` = host OS): PGO as described below.
#   - Cross WITH a runner (Linux host, `--os windows`): PGO trained under the runner.
#     Every instrumented training binary is built for `x86_64-pc-windows-gnu` and
#     EXECUTED through the runner, so the `.profraw` it writes carries the windows-gnu
#     crate hashes the optimised build then looks up. The runner is `$PGO_CROSS_RUNNER`
#     if set (word-split, e.g. `PGO_CROSS_RUNNER=wine64` or a wrapper script), else the
#     first of `wine64`, `wine` found in PATH. Training under it sets `WINEDEBUG=-all`
#     and `PGO_TRAIN_SKIP_GPU=1` (wgpu under Wine is not a training environment), so the
#     CPU-side GPU chunk-orchestration code stays COLD in such a profile -- run
#     `scripts/pgo-build.ps1` natively on Windows to get it warm.
#   - Cross WITHOUT a runner: a PLAIN optimised release build (`-C target-cpu` only, same
#     packages/features/output names), labelled "NO PGO" at the start of each such
#     combination and again in the final summary. This is what the old script silently
#     produced anyway: its Linux-trained profile never matched the windows-gnu symbol
#     hashes. `--collect-only` and `--skip-train` are refused in this case (nothing can
#     be trained, and no on-disk profile is known to match windows-gnu -- the
#     `windows-*.profdata` files scripts/pgo-build.ps1 writes to the same `pgo-profile/`
#     paths are x86_64-pc-windows-msvc profiles). A cross build only ever reuses a
#     profile whose `.target` sidecar (written by this script) names its triple.
# Any cross build needs the mingw linker (`x86_64-w64-mingw32-gcc` in PATH, or
# `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER` set) and the `x86_64-pc-windows-gnu`
# standard library in this toolchain's sysroot (checked there, so a self-compiled
# toolchain without rustup components works too). Windows host -> `--os linux` is refused:
# there is no runner for ELF binaries on Windows.
#
# Package/feature parity (the instrumented and optimised builds must produce the same
# crate hash, or the profile silently does not apply): the instrumented build and the
# optimised build must select IDENTICAL packages, target, target-dir and features --
# ONLY `RUSTFLAGS` (here, `CARGO_ENCODED_RUSTFLAGS`) may differ between them. Both now
# build `indicatrix` (which owns the `pgo_train` example) together with whichever of
# `indicatrix-cut` (plus `indicatrix-cut-core`, trained by `--example pgo_train_cutcore`)
# or `indicatrix-worker` `--target` selects, with `--bins --example pgo_train` and the
# SAME `--features` string, in one `cargo build` each. Previously the
# instrumented step selected only `-p indicatrix --example pgo_train` while the optimised
# step selected only `-p indicatrix-worker`/`-p indicatrix-cut` -- a different package
# set changes every downstream crate's feature-unification result (e.g. `glam` gaining
# `bytemuck` only through the app build), which changes that crate's `-C metadata` hash,
# which changes its mangled symbol names, so `-C profile-use` found nothing to match and
# every previously "PGO'd" binary was in fact built as if untrained.
#
# `indicatrix`'s `hdr` and `serde` features are always both enabled regardless of
# `--target`: `indicatrix-cut` depends on `indicatrix` with `features = ["hdr"]`
# unconditionally and always enables `indicatrix-net/render` (-> `indicatrix/serde`), and
# `indicatrix-worker`'s own `worker` feature does the same. `indicatrix-net/compression`
# is likewise always on (a default feature of that crate, named explicitly in
# `pgo_features` so it can never be dropped by accident). A `cut`-only build's profile
# is kept in `pgo-profile/cut/` and a `worker`-only build's in `pgo-profile/worker/` --
# each has a different package set (and therefore different crate hashes) from the
# combined `worker`+`cut` build, so they must not share a profile file with it or with
# each other.
#
# A profile trained under one `INDICATRIX_SIMD` tier leaves `indicatrix`'s SIMD kernels
# for every OTHER tier with zero profile counts (see `simd::simd_level`), so LLVM treats
# them as cold/size-optimised in that binary regardless of what the CPU it later runs on
# actually supports. Ship the avx2 build to AVX2-and-newer machines (including AVX-512
# ones) and the scalar build only to machines without AVX2.
#
# `avx512` (`x86-64-v4`) crashes an instrumented training run with SIGILL on any host CPU
# without AVX-512 -- `--isa` probes this host's support (via `rustc --print cfg -C
# target-cpu=native`) before attempting it: `--isa all` silently drops `avx512` from the
# tier list when unsupported (avx2 + scalar is the documented default pair), and an
# explicit `--isa avx512` on such a host refuses immediately instead of crashing mid-run.
#
# Up to three instrumented training binaries feed each combination's merged profile:
#   1. `indicatrix`'s `pgo_train` example -- the CPU spectral tracer (every material
#      optical character, dispersive/non-dispersive, plain/frosted-girdle, both the
#      float HDR buffer and the 8-bit tonemapped output), every lighting model plus a
#      synthetic HDR map, the meet-point solver (and through it the SIMD
#      candidate-vertex kernels) against both synthetic AND real catalogue `.asc`
#      designs, external solid measurement and CAD-preview mesh extraction, `.asc`
#      reader/writer round trip, B-Rep reconstruction, the tilt-performance sweep, and
#      (only in the `gpu` tier, when this host has an adapter) the CPU-side GPU chunk
#      dispatch/readback orchestration. See that example's own doc comment for the
#      full stage-by-stage coverage table.
#   2. `indicatrix-cut-core`'s OWN `pgo_train_cutcore` example (cut target only) -- the
#      editor-core layer built on top of `indicatrix`: `Design::solve`, `.asc` export,
#      the native `.indicatrix.toml` save/load round trip, `resolve_after_edit`, and
#      `optimize_design`'s search loop, run against every built-in template plus the
#      crate's own CrackOtto-Step fixture. It is built in the SAME `cargo build`
#      invocation as everything else for the `cut` target (`pgo_packages` includes
#      `indicatrix-cut-core`, and `--example pgo_train --example pgo_train_cutcore` are
#      both passed), so the instrumented and optimised builds resolve the identical
#      dependency graph and `indicatrix-cut-core`'s `-C metadata` (hence every mangled
#      symbol) matches. The example carries a distinct name because two examples named
#      `pgo_train` collide on Cargo's name-keyed "pretty path" uplift
#      (`target/<profile>/examples/<name>`) in one invocation. An earlier separate
#      invocation gave cut-core a different dependency graph and its profile never
#      applied (proven 2026-10-06). The worker target does not link cut-core and skips
#      this step. The workload runs from `examples/pgo_train_cutcore` right after the
#      main one, with its own `cutcore-*.profraw` naming.
#   3. `indicatrix-net`'s `scene_roundtrip` integration test, run under the same
#      instrumentation -- covers `SceneState`'s postcard wire-protocol encode/decode,
#      the one hot path that cannot be reached from `pgo_train` (a `indicatrix` example
#      cannot depend on `indicatrix-net`, which itself depends on `indicatrix`). Built
#      with the SAME `--features`/`--target`/`--target-dir` as the main instrumented
#      build above so `indicatrix-net`'s own crate hash lines up with what the final
#      binary resolves to, but through `cargo test --no-run` (a separate cargo
#      subcommand from the `cargo build` the other two steps share) -- this is a smaller,
#      not fully eliminated, version of the same crate-hash risk the parity note above
#      describes;
#      the post-build "hash mismatch"/"no profile data available" scan below is what
#      catches it if it ever matters in practice.
# Every `.profraw` from all three binaries is merged into one `.profdata` per combination.
#
# Training coverage is a known limitation, not a complete picture. None of the three
# training binaries exercises the radiance (HDR) image codecs, the solid rasterizer,
# `guide_pass`, the `indicatrix-vault` database, the desktop editor's event loops and GUI
# code, or the `GpuBackend` type; those paths are optimised as cold code. The `gpu` tier's
# `train_gpu` stage trains `render_hybrid`, which no app calls today. Two more limits:
# the `indicatrix-net` test build selects only that package with `--features render`.
# The post-build crate-hash check compares `indicatrix` tokens, and for the `cut` target
# also `indicatrix-cut-core` tokens (`test_profile_hash_applied`); it proves the profile
# applied to those crates, not how much of them the training covers.
#
# Environment knobs `pgo_train` honours:
#   INDICATRIX_SIMD=scalar|avx2|avx512   caps runtime SIMD dispatch during training (unset for native)
#   INDICATRIX_SAMPLES=<n>               overrides the tracer's samples-per-pixel directly
#   PGO_SCALE=<factor>               scales every stage's frame size/iteration count (default 1.0)
#   PGO_TRAIN_SKIP_GPU=1             skips the GPU chunk-orchestration training stage
#
# Usage:
#   scripts/pgo-bolt-build.sh                                # PGO only, sequential, host OS, all tiers, both targets
#   scripts/pgo-bolt-build.sh --native                       # PGO only, ONLY the host-CPU tier (-C target-cpu=native), no generic tiers
#   scripts/pgo-bolt-build.sh --parallel                     # PGO only, parallel, host OS
#   scripts/pgo-bolt-build.sh --os windows --parallel        # Windows binaries from a Linux host, trained under Wine (plain release, labelled NO PGO, without a runner)
#   scripts/pgo-bolt-build.sh --collect-only                 # Generate & copy Windows PGO profiles only (native on Windows, under Wine on Linux; needs a runner there)
#   scripts/pgo-bolt-build.sh --isa avx2 --gpu gpu           # single combination
#   scripts/pgo-bolt-build.sh --target worker                # indicatrix-worker only
#   scripts/pgo-bolt-build.sh --target cut                   # indicatrix-cut only
#   scripts/pgo-bolt-build.sh --bolt                         # PGO + BOLT (Linux only, indicatrix-worker only -- see below)
#   scripts/pgo-bolt-build.sh --bolt --native                # PGO + BOLT, host-CPU tier only
#   scripts/pgo-bolt-build.sh --bolt --target worker         # PGO + BOLT, indicatrix-worker only
#
# BOLT applies only to `indicatrix-worker`, on Linux, regardless of `--target`: it has a
# headless, deterministic `render` workload to profile. `indicatrix-cut` is an
# interactive desktop app with no scriptable workload -- a startup-only profile would
# give it a worse code layout than no BOLT at all (see the root README's "Optimized
# builds" section) -- so `--bolt --target cut`/`--target all` PGO-builds indicatrix-cut
# normally and skips the BOLT pass for it, with a notice explaining why.
#
set -euo pipefail

# Disable sccache/cache wrappers.
export RUSTC_WRAPPER=""
export RUSTC_WORKSPACE_WRAPPER=""

# ---------------------------------------------------------------------------------
# Argument defaults & parsing
# ---------------------------------------------------------------------------------
ISA_ARG=all
GPU_ARG=both
OS_ARG=auto
TARGET_ARG=all
OUT_DIR=""
SKIP_TRAIN=0
SAMPLES=64
USE_BOLT=0
PARALLEL=0
COLLECT_ONLY=0
WITH_NATIVE=0
# "no profile data available" fires in bulk, and expectedly, for indicatrix-cut/
# indicatrix-worker's own GUI/server/CLI code that pgo_train never exercises. Measured
# on 2026-09-28 (Linux host, windows-gnu target, Wine-trained, crate-hash check OK):
# indicatrix-cut ~66,150-66,170 per combination, indicatrix-worker well under 50,000.
# This count is therefore only a FALLBACK gate, used when test_profile_hash_applied
# could not verify the profile (a parsing miss); once the crate-hash check has proven
# the profile applied, the count is reported and never fatal -- see
# test_profile_warnings.
#
# Since 2026-10-06 the fallback gate is the trained LIBRARY crates' count
# (MAX_LIBRARY_MISSING_FUNCTION_WARNINGS; everything except indicatrix_cut/
# indicatrix_worker's own code): indicatrix-cut's own count grows with every GUI feature
# (~161,000 that day with the profile applied) and proves nothing. The overall count is
# kept only as a generous ceiling against a runaway.
MAX_MISSING_FUNCTION_WARNINGS=400000
MAX_LIBRARY_MISSING_FUNCTION_WARNINGS=15000
# Set to 1 by test_profile_hash_applied when it CONFIRMED the merged profile's crate
# hash matches the optimised rlib; reset to 0 before every combination's checks.
PROFILE_HASH_VERIFIED=0

die() { printf '%s\n' "error: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --isa)          ISA_ARG="${2:?--isa needs a value}"; shift 2 ;;
        --gpu)          GPU_ARG="${2:?--gpu needs a value}"; shift 2 ;;
        --os)           OS_ARG="${2:?--os needs a value}"; shift 2 ;;
        --target)       TARGET_ARG="${2:?--target needs a value}"; shift 2 ;;
        --out-dir)      OUT_DIR="${2:?--out-dir needs a value}"; shift 2 ;;
        --samples)      SAMPLES="${2:?--samples needs a value}"; shift 2 ;;
        --skip-train)   SKIP_TRAIN=1; shift ;;
        --bolt)         USE_BOLT=1; shift ;;
        --parallel)     PARALLEL=1; shift ;;
        --collect-only) COLLECT_ONLY=1; shift ;;
        --native)       WITH_NATIVE=1; shift ;;
        --max-missing-function-warnings) MAX_MISSING_FUNCTION_WARNINGS="${2:?--max-missing-function-warnings needs a value}"; shift 2 ;;
        --max-library-missing-function-warnings) MAX_LIBRARY_MISSING_FUNCTION_WARNINGS="${2:?--max-library-missing-function-warnings needs a value}"; shift 2 ;;
        -h|--help)      sed -n '2,/^set -euo/p' "$0" | sed 's/^# \{0,1\}//' | sed '$d'; exit 0 ;;
        *)              die "unknown argument: $1 (try --help)" ;;
    esac
done

if [ "$COLLECT_ONLY" -eq 1 ]; then
    if [ "$SKIP_TRAIN" -eq 1 ]; then
        die "--collect-only and --skip-train cannot be used together"
    fi
    # Training must run under the SAME target triple the profile is for (see this
    # file's header comment). On a Windows host that is a native run; on a Linux host
    # the instrumented windows-gnu binaries are executed through the cross runner
    # (Wine or $PGO_CROSS_RUNNER, resolved below), so this works there too -- and is
    # refused there when no runner is available.
    OS_ARG=windows
fi

case "$ISA_ARG" in
    all)                       ISA_LIST=(avx512 avx2 scalar) ;;
    avx512|avx2|scalar|native) ISA_LIST=("$ISA_ARG") ;;
    *)                         die "--isa must be avx512, avx2, scalar, native, or all" ;;
esac

# --native builds ONLY the host-CPU tier (-C target-cpu=native), replacing whatever
# --isa selected (it used to be appended on top of the generic tiers, which meant a
# quick "build for this machine" run paid for avx2 + scalar as well).
if [ "$WITH_NATIVE" -eq 1 ]; then
    ISA_LIST=(native)
fi

case "$GPU_ARG" in
    both)    GPU_LIST=(gpu cpu) ;;
    gpu|cpu) GPU_LIST=("$GPU_ARG") ;;
    *)       die "--gpu must be gpu, cpu, or both" ;;
esac

case "$TARGET_ARG" in
    all)          TARGET_LIST=(worker cut) ;;
    worker|cut)   TARGET_LIST=("$TARGET_ARG") ;;
    *)            die "--target must be worker, cut, or all" ;;
esac

# ---------------------------------------------------------------------------------
# Host and target resolution
# ---------------------------------------------------------------------------------
HOST_OS="$(uname -s)"
HOST_TARGET="$(rustc -vV | sed -n 's/^host: //p')"
HOST_EXE=""
[[ "$HOST_TARGET" == *windows* ]] && HOST_EXE=".exe"

# This host's own OS, in the vocabulary --os/OS_LIST use.
if [[ "$HOST_TARGET" == *windows* ]]; then
    HOST_OS_KEY=windows
else
    HOST_OS_KEY=linux
fi

if [ "$OS_ARG" = auto ]; then
    # Was "linux windows" unconditionally (BOLT aside) -- training always ran on
    # $HOST_TARGET regardless, so the non-host OS in that list silently built with a
    # profile trained under the WRONG target triple (see this file's header comment).
    # Default to the host's own OS only; cross-OS is opt-in via an explicit --os and
    # needs a runner that can execute the other OS's instrumented binaries (resolved
    # below).
    OS_LIST=("$HOST_OS_KEY")
else
    case "$OS_ARG" in
        linux|windows) OS_LIST=("$OS_ARG") ;;
        *)             die "--os must be windows or linux" ;;
    esac
fi

# Rust target triple for an OS key ("linux" or "windows").
os_rust_target() {
    case "$1" in
        linux)   printf 'x86_64-unknown-linux-gnu' ;;
        windows) printf 'x86_64-pc-windows-gnu' ;;
        *)       die "unsupported os: $1" ;;
    esac
}

# --- Cross-OS training runner ----------------------------------------------------
# Training must execute the instrumented binaries built for the SAME target triple as
# the optimised build (the triple is part of every crate's `-C metadata` hash, so a
# profile trained on $HOST_TARGET never applies to another triple's build -- see this
# file's header comment). For an OS whose triple differs from $HOST_TARGET, resolve ONCE
# here how those binaries get executed:
#   - Windows host: a windows-gnu binary runs directly (even when the host toolchain is
#     windows-msvc), so CROSS_RUNNER_WINDOWS stays empty and PGO proceeds as normal.
#   - Linux host with a runner ($PGO_CROSS_RUNNER, else wine64, else wine): PGO trained
#     under that runner.
#   - Linux host without a runner: CROSS_NO_PGO_WINDOWS=1 -- the windows combinations
#     are cross-compiled as PLAIN release builds (no instrumentation, no profile-use),
#     loudly labelled as such. That is what the old script silently produced anyway: its
#     Linux-trained profile never matched the windows-gnu symbol hashes.
CROSS_RUNNER_WINDOWS=()
CROSS_RUNNER_WINDOWS_IS_WINE=0
CROSS_NO_PGO_WINDOWS=0
for os in "${OS_LIST[@]}"; do
    os_target="$(os_rust_target "$os")"
    [ "$os_target" = "$HOST_TARGET" ] && continue

    # Is the target's standard library present in this toolchain? Checked against the
    # sysroot, not via `rustup target list --installed`: that command refuses for a
    # self-compiled toolchain ("toolchain 'x' does not support components"), and a
    # toolchain built from source has its targets in the same place anyway.
    cross_sysroot="$(rustc --print sysroot)"
    if [ ! -d "$cross_sysroot/lib/rustlib/$os_target/lib" ]; then
        die "--os $os needs the $os_target Rust target, but $cross_sysroot/lib/rustlib/$os_target/lib does not exist. With rustup: rustup target add $os_target; with a self-compiled toolchain: build it with $os_target in its target list."
    fi

    case "$os:$HOST_OS_KEY" in
        windows:windows)
            # Native: a windows-gnu binary runs directly on a Windows host.
            ;;
        windows:*)
            if [ -n "${PGO_CROSS_RUNNER:-}" ]; then
                read -r -a CROSS_RUNNER_WINDOWS <<< "$PGO_CROSS_RUNNER"
                [ "${#CROSS_RUNNER_WINDOWS[@]}" -gt 0 ] || die "PGO_CROSS_RUNNER is set but empty"
                command -v "${CROSS_RUNNER_WINDOWS[0]}" >/dev/null 2>&1 || \
                    die "PGO_CROSS_RUNNER=$PGO_CROSS_RUNNER: '${CROSS_RUNNER_WINDOWS[0]}' not found in PATH"
            else
                for candidate in wine64 wine; do
                    if command -v "$candidate" >/dev/null 2>&1; then
                        CROSS_RUNNER_WINDOWS=("$(command -v "$candidate")")
                        CROSS_RUNNER_WINDOWS_IS_WINE=1
                        break
                    fi
                done
                if [ "${#CROSS_RUNNER_WINDOWS[@]}" -eq 0 ]; then
                    CROSS_NO_PGO_WINDOWS=1
                fi
            fi
            # Needed for ANY cross build, PGO or not.
            if [ -z "${CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER:-}" ] && \
               ! command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
                die "--os windows on a $HOST_OS_KEY host needs the mingw-w64 linker: install x86_64-w64-mingw32-gcc (e.g. the mingw-w64 / gcc-mingw-w64-x86-64 package) or set CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER."
            fi
            ;;
        linux:windows)
            die "--os linux on a Windows host is not supported: PGO training must execute the instrumented $os_target binaries, and there is no runner for ELF binaries on Windows. Run this script on a Linux host instead."
            ;;
        *)
            die "--os $os on this host ($HOST_TARGET): no known way to execute $os_target training binaries."
            ;;
    esac
done
unset os os_target candidate cross_sysroot

# Initialise the Wine prefix once, up front, when Wine was auto-detected: under
# --parallel several combinations reach their training run at nearly the same time, and
# concurrent first-run prefix creation races. This also proves the runner actually
# starts a Windows program before hours of instrumented builds depend on it; an
# auto-detected Wine that cannot is treated as "no runner" (plain release builds, loudly
# labelled) rather than blocking a cross build that works without it. Skipped for
# PGO_CROSS_RUNNER, whose setup is the caller's responsibility.
if [ "$CROSS_RUNNER_WINDOWS_IS_WINE" -eq 1 ]; then
    if ! WINEDEBUG=-all WINEDLLOVERRIDES="mscoree,mshtml=" \
         "${CROSS_RUNNER_WINDOWS[@]}" cmd /c exit >/dev/null 2>&1; then
        printf 'warning: Wine (%s) was found but failed to run '"'"'cmd /c exit'"'"' (prefix: %s) -- ignoring it; windows binaries will be plain release builds (NO PGO). Fix Wine or set PGO_CROSS_RUNNER to train a real profile.\n' \
            "${CROSS_RUNNER_WINDOWS[*]}" "${WINEPREFIX:-$HOME/.wine}" >&2
        CROSS_RUNNER_WINDOWS=()
        CROSS_RUNNER_WINDOWS_IS_WINE=0
        CROSS_NO_PGO_WINDOWS=1
    fi
fi

# A profile cannot be collected, or trusted from disk, without executing the
# instrumented windows-gnu binaries. `pgo-profile/[cut/|worker/]windows-*.profdata`
# may also hold profiles scripts/pgo-build.ps1 trained on an x86_64-pc-windows-msvc
# host -- a different triple, so different crate hashes, so they never apply to an
# x86_64-pc-windows-gnu build (see also the `.target` sidecar check in
# build_combination).
if [ "$CROSS_NO_PGO_WINDOWS" -eq 1 ]; then
    if [ "$COLLECT_ONLY" -eq 1 ]; then
        die "--collect-only needs the instrumented x86_64-pc-windows-gnu binaries to be executed, and this $HOST_TARGET host has no runner for them. Install Wine (wine64 or wine in PATH) or set PGO_CROSS_RUNNER, or run scripts/pgo-build.ps1 on Windows."
    fi
    if [ "$SKIP_TRAIN" -eq 1 ]; then
        die "--skip-train with --os windows on this $HOST_TARGET host and no cross runner: there is no profile to reuse safely. The windows-*.profdata files under pgo-profile/ were trained either by scripts/pgo-build.ps1 on x86_64-pc-windows-msvc or not at all for x86_64-pc-windows-gnu; a different target triple means different crate hashes, so -C profile-use would silently apply nothing. Drop --skip-train (windows binaries are then plain release builds, clearly labelled), or install Wine / set PGO_CROSS_RUNNER to train a real profile."
    fi
fi

# --- AVX-512 host support probe --------------------------------------------------
# See this file's header comment: avx512 (x86-64-v4) SIGILLs immediately on a host CPU
# without AVX-512, so probe once, up front, instead of discovering it mid-training-run.
avx512_supported() {
    rustc --print cfg -C target-cpu=native 2>/dev/null | grep -q 'target_feature="avx512f"'
}

if [[ " ${ISA_LIST[*]} " =~ " avx512 " ]] && ! avx512_supported; then
    if [ "$ISA_ARG" = avx512 ]; then
        die "--isa avx512 requested, but this host's native CPU has no AVX-512 support (rustc --print cfg -C target-cpu=native has no target_feature=\"avx512f\"). An instrumented avx512 binary would crash immediately (SIGILL) instead of training. Use --isa avx2 or --isa scalar (the documented default pair), or run this on AVX-512-capable hardware."
    fi
    printf 'warning: dropping avx512 from --isa all: this host'"'"'s native CPU has no AVX-512 support. Building avx2 and scalar only -- the documented default pair. Pass --isa avx512 explicitly on AVX-512-capable hardware to opt back in.\n' >&2
    filtered=()
    for isa in "${ISA_LIST[@]}"; do
        [ "$isa" = avx512 ] && continue
        filtered+=("$isa")
    done
    ISA_LIST=("${filtered[@]}")
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
[ -n "$OUT_DIR" ] || OUT_DIR="$ROOT/bin"
PGO_PROFILE_DIR="$ROOT/pgo-profile"
# Unit separator: how CARGO_ENCODED_RUSTFLAGS joins individual rustc arguments, used
# instead of space-joined RUSTFLAGS so a repo path containing a space (unlike this one
# today) can't split a flag in two.
US=$'\x1f'

SYSROOT="$(rustc --print sysroot)"

# llvm-profdata and llvm-nm must be the SAME LLVM major.minor as rustc. The llvm-tools
# in a sysroot are installed separately from rustc and are not updated with it: on
# 2026-10-06, rustc was on LLVM 22.1.8 but the sysroot's llvm-nm was still LLVM 21.1.5.
# Such a stale llvm-nm cannot read the bitcode inside an LTO build's rlibs ("Unknown
# attribute kind"). The crate-hash check then fails to verify every combination, and
# an older llvm-profdata writes the merged profile.
#
# Candidates, first matching version wins:
#   1. $PGO_LLVM_BIN (an explicit directory);
#   2. the sysroot's rustlib;
#   3. /usr/lib/llvm/<major>/bin (Gentoo's slotted LLVM);
#   4. PATH.
llvm_major_minor() { grep -oE 'LLVM version:? [0-9]+\.[0-9]+' | grep -oE '[0-9]+\.[0-9]+' | head -n1; }
RUSTC_LLVM="$(rustc -vV | llvm_major_minor || true)"
find_llvm_tool() {
    local name="$1" candidate version first_found=""
    local -a candidates=()
    [ -n "${PGO_LLVM_BIN:-}" ] && candidates+=("$PGO_LLVM_BIN/$name$HOST_EXE")
    candidates+=("$SYSROOT/lib/rustlib/$HOST_TARGET/bin/$name$HOST_EXE")
    [ -n "$RUSTC_LLVM" ] && candidates+=("/usr/lib/llvm/${RUSTC_LLVM%%.*}/bin/$name")
    command -v "$name" >/dev/null 2>&1 && candidates+=("$(command -v "$name")")
    for candidate in "${candidates[@]}"; do
        [ -x "$candidate" ] || continue
        [ -z "$first_found" ] && first_found="$candidate"
        version="$("$candidate" --version 2>&1 | llvm_major_minor || true)"
        if [ -z "$RUSTC_LLVM" ] || [ "$version" = "$RUSTC_LLVM" ]; then
            printf '%s' "$candidate"
            return 0
        fi
    done
    if [ -n "$first_found" ]; then
        die "no $name matching rustc's LLVM $RUSTC_LLVM found (first candidate $first_found is LLVM $("$first_found" --version 2>&1 | llvm_major_minor || true)). Rebuild/reinstall the toolchain's llvm-tools, install LLVM $RUSTC_LLVM, or point PGO_LLVM_BIN at a directory holding a matching $name."
    fi
    die "$name not found (PGO_LLVM_BIN, host rustlib, /usr/lib/llvm/<major>/bin or PATH)"
}
PROFDATA="$(find_llvm_tool llvm-profdata)"
LLVM_NM="$(find_llvm_tool llvm-nm)"

# BOLT validation
BOLT_ENABLED=0
if [ "$USE_BOLT" -eq 1 ]; then
    for current_os in "${OS_LIST[@]}"; do
        if [ "$current_os" != linux ]; then
            die "--bolt is only supported on Linux targets"
        fi
    done
    if command -v llvm-bolt >/dev/null 2>&1 && command -v merge-fdata >/dev/null 2>&1; then
        BOLT_ENABLED=1
    else
        die "--bolt requested but llvm-bolt or merge-fdata not found in system PATH"
    fi
fi

step() { printf '\n==> %s\n' "$*"; }

# Binary/package name for a target key ("worker" or "cut").
target_bin() {
    case "$1" in
        worker) printf 'indicatrix-worker' ;;
        cut)    printf 'indicatrix-cut' ;;
        *)      die "unknown target: $1" ;;
    esac
}

# The full package set for a -Target key: `indicatrix` (owns pgo_train) plus whichever
# app it selects (cut also lists indicatrix-cut-core, which owns pgo_train_cutcore).
# Used IDENTICALLY by the instrumented and optimised builds -- see this
# file's header comment on package/feature parity.
pgo_packages() {
    case "$1" in
        worker) printf 'indicatrix indicatrix-worker' ;;
        cut)    printf 'indicatrix indicatrix-cut indicatrix-cut-core' ;;
        *)      die "unknown target: $1" ;;
    esac
}

# The full --features string for a (target, gpu) combination -- see this file's header
# comment: `indicatrix/serde` and `indicatrix/hdr` are always both on, regardless of
# target, since both apps' own dependency edges always turn them on anyway.
# `indicatrix-net/compression` (the v14 zstd/LZ4/PNG payload codecs) is a DEFAULT feature
# of `indicatrix-net` that both apps inherit, so it is already on in every variant --
# named here too so a later change to that crate's default set can never silently drop
# it from a PGO build (the owner wants it active in every PGO variant).
pgo_features() {
    local target="$1" gpu="$2"
    local -a feats=(indicatrix/serde indicatrix/hdr indicatrix-net/compression)
    [ "$target" = worker ] && feats+=(indicatrix-worker/worker)
    if [ "$gpu" = gpu ]; then
        feats+=(indicatrix/gpu)
        [ "$target" = cut ] && feats+=(indicatrix-cut/gpu)
        [ "$target" = worker ] && feats+=(indicatrix-worker/gpu)
    fi
    local IFS=,
    printf '%s' "${feats[*]}"
}

# Scans an optimised build's log for the warning classes -pgo-warn-missing-function and
# profile staleness produce -- see MAX_MISSING_FUNCTION_WARNINGS' doc comment for why
# "no profile data available" is thresholded rather than required to be zero, and why
# "hash mismatch" is not.
test_profile_warnings() {
    local logfile="$1" label="$2"
    [ -f "$logfile" ] || return 0

    local hash_mismatch_count no_profile_count
    hash_mismatch_count="$(grep -c 'hash mismatch' "$logfile" || true)"
    no_profile_count="$(grep -c 'no profile data available' "$logfile" || true)"

    if [ "${hash_mismatch_count:-0}" -gt 0 ]; then
        printf '[%s] %s "hash mismatch" warning(s) in %s. With identical --target/--target-dir/package-set/--features between the instrumented and optimised builds and no source edits between the two steps, this should be zero -- a function'"'"'s compiled code differs from what was profiled. First few:\n' "$label" "$hash_mismatch_count" "$logfile" >&2
        grep -m5 'hash mismatch' "$logfile" >&2 || true
        die "[$label] profile verification failed (hash mismatch) -- see above"
    fi
    if [ "${PROFILE_HASH_VERIFIED:-0}" -eq 1 ]; then
        # The crate-hash check already proved the profile applied; the count is only
        # code the training run never executes (the whole GUI for indicatrix-cut).
        printf '  [%s] %s function(s) without profile data (untrained code; the crate-hash check confirmed the profile applied)\n' "$label" "${no_profile_count:-0}"
        return 0
    fi
    # The fallback gate counts only the TRAINED library crates' warnings. The app's own
    # crate (GUI/server/CLI, never trained) grows with every feature: indicatrix-cut alone
    # was ~66,000 on 2026-09-28 and ~161,000 on 2026-10-06 with a fully applied profile,
    # which says nothing about whether the profile applied. A profile that silently stopped
    # applying shows up as the libraries' counts jumping instead: measured 2026-10-06 with
    # the profile applied, indicatrix under 103 and every library together about 5,500.
    local library_count
    library_count="$(grep 'no profile data available' "$logfile" \
        | grep -vcE '^warning: (indicatrix_cut|indicatrix_worker)\.' || true)"
    if [ "${library_count:-0}" -gt "$MAX_LIBRARY_MISSING_FUNCTION_WARNINGS" ]; then
        die "[$label] $library_count 'no profile data available' warning(s) from the trained library crates in $logfile, over the threshold ($MAX_LIBRARY_MISSING_FUNCTION_WARNINGS, --max-library-missing-function-warnings), and the crate-hash check could not verify the profile: it most likely did not apply. Per-crate counts: $(grep 'no profile data available' "$logfile" | grep -oE '^warning: [a-z_]+' | sort | uniq -c | sort -rn | head -n 8 | tr -s ' \n' ' ')"
    fi
    if [ "${no_profile_count:-0}" -gt "$MAX_MISSING_FUNCTION_WARNINGS" ]; then
        die "[$label] $no_profile_count 'no profile data available' warning(s) in $logfile, over the overall ceiling ($MAX_MISSING_FUNCTION_WARNINGS, --max-missing-function-warnings), and the crate-hash check could not verify the profile. The trained libraries are under their own threshold, so this is the app's untrained code growing: raise the ceiling if that is expected."
    fi
    printf '  [%s] %s function(s) without profile data, %s of them in the trained libraries (crate-hash check could not verify; libraries under the %s threshold)\n' "$label" "${no_profile_count:-0}" "${library_count:-0}" "$MAX_LIBRARY_MISSING_FUNCTION_WARNINGS"
}

# Verifies the merged profile's crate-hash tokens match the tokens actually baked into
# the optimised build's rlibs for `indicatrix` and, for the `cut` target, also
# `indicatrix-cut-core`, i.e. the profile actually applies to both. (The worker does not
# link cut-core: not applicable there.) A feature-unification difference between the
# training and optimised builds would apply nothing while the missing-function
# tolerance hid the loss. Best-effort: a parsing miss prints a warning
# and leaves PROFILE_HASH_VERIFIED unset rather than failing the whole build over a
# diagnostic script's own pattern; a CONFIRMED mismatch in either crate is a hard failure.
test_profile_hash_applied() {
    local profdata="$1" bdir="$2" rust_target="$3" label="$4" target="$5"
    local core_rc=0 cutcore_rc=0
    test_crate_hash_token "$profdata" "$bdir" "$rust_target" "$label" \
        trace_spectral_ray_with_finish_soa 'Cs[0-9a-zA-Z]+_10indicatrix' indicatrix indicatrix || core_rc=$?
    if [ "$target" = cut ]; then
        test_crate_hash_token "$profdata" "$bdir" "$rust_target" "$label" \
            resolve_after_edit 'Cs[0-9a-zA-Z]+_19indicatrix_cut_core' indicatrix-cut-core indicatrix_cut_core || cutcore_rc=$?
    fi
    if [ "$core_rc" -eq 0 ] && [ "$cutcore_rc" -eq 0 ]; then
        PROFILE_HASH_VERIFIED=1
    fi
}

# Compares one crate's hash token between the merged profile and the newest optimised
# rlib of that crate. Returns 0 when CONFIRMED equal, 2 when it could not be verified
# (a warning is printed), and dies on a confirmed mismatch. `pkg_dir` is the package's
# directory name under the cargo build-dir layout, `rlib_stem` the rlib's crate-name
# stem (underscores).
test_crate_hash_token() {
    local profdata="$1" bdir="$2" rust_target="$3" label="$4"
    local func="$5" pattern="$6" pkg_dir="$7" rlib_stem="$8"

    # The merged profile can hold several copies of the same function under different
    # crate-hash tokens: a profile merged from several runs may hold copies of one
    # function under different metadata. The profile applies when ANY
    # copy carries the optimised rlib's token, so collect every token, not just the first
    # match. Taking only the first match reported a false "did NOT apply" on 2026-10-06,
    # while the per-crate warning counts showed `indicatrix` fully profiled.
    local prof_lines
    prof_lines="$("$PROFDATA" show --all-functions "$profdata" 2>&1 | grep "$func" || true)"
    if [ -z "$prof_lines" ]; then
        printf 'warning: [%s] "%s" not found in %s -- cannot verify the %s crate-hash match.\n' "$label" "$func" "$profdata" "$pkg_dir" >&2
        return 2
    fi
    local prof_tokens
    prof_tokens="$(printf '%s\n' "$prof_lines" | grep -oE "$pattern" | sort -u || true)"
    local prof_token
    prof_token="$(printf '%s\n' "$prof_tokens" | head -n1)"

    # The compiled library is `lib<stem>-<hash>.rlib` in one of two layouts: the new
    # cargo build-dir layout (`build/<package>/<hash>/out/`, current nightly) or the
    # classic one (`deps/`). More than one may exist (the instrumented build's, or a
    # different feature set); take the newest across both layouts -- the optimised build
    # that just finished. llvm-nm reads the COFF members of a cross-built windows-gnu
    # rlib as well as ELF ones.
    local rel="$bdir/$rust_target/release"
    local -a rlib_candidates
    shopt -s nullglob
    rlib_candidates=("$rel/build/$pkg_dir/"*"/out/lib$rlib_stem-"*.rlib "$rel/deps/lib$rlib_stem-"*.rlib)
    shopt -u nullglob
    local rlib=""
    if [ "${#rlib_candidates[@]}" -gt 0 ]; then
        rlib="$(ls -t "${rlib_candidates[@]}" 2>/dev/null | head -n1 || true)"
    fi
    if [ -z "$rlib" ]; then
        printf 'warning: [%s] no lib%s-*.rlib found under %s/build/%s/*/out or %s/deps -- cannot verify the %s crate-hash match.\n' "$label" "$rlib_stem" "$rel" "$pkg_dir" "$rel" "$pkg_dir" >&2
        return 2
    fi
    local nm_line
    nm_line="$("$LLVM_NM" "$rlib" 2>&1 | grep -m1 "$func" || true)"
    if [ -z "$nm_line" ]; then
        printf 'warning: [%s] "%s" not found in %s -- cannot verify the %s crate-hash match.\n' "$label" "$func" "$rlib" "$pkg_dir" >&2
        return 2
    fi
    local nm_token
    nm_token="$(printf '%s' "$nm_line" | grep -oE "$pattern" | head -n1 || true)"

    if [ -z "$prof_token" ] || [ -z "$nm_token" ]; then
        printf 'warning: [%s] could not extract a "%s" crate-hash token from one or both symbols -- cannot verify the %s crate-hash match.\n' "$label" "$pattern" "$pkg_dir" >&2
        return 2
    fi
    if ! printf '%s\n' "$prof_tokens" | grep -qxF "$nm_token"; then
        die "[$label] profile did NOT apply to $pkg_dir: none of the merged profile's crate-hash tokens for '$func' ($(printf '%s' "$prof_tokens" | tr '\n' ' ')) matches the optimised build's rlib ($nm_token). The instrumented and optimised builds must use identical --target, --target-dir, package set and --features; a crate-hash token mismatch means the profile did not apply."
    fi
    printf '  [%s] crate-hash check OK (%s): %s -> %s (profile holds %s token(s))\n' "$label" "$pkg_dir" "$func" "$nm_token" "$(printf '%s\n' "$prof_tokens" | grep -c .)"
    return 0
}

# The one-line label for a cross-built combination that got NO PGO (no runner) --
# printed at the start of that combination and again in the final summary.
no_pgo_notice() {
    local target="$1" combo="$2"
    printf '[%s][%s] NO PGO: cross-compiled on %s without a runner for the instrumented windows-gnu binaries -- plain release build. Set PGO_CROSS_RUNNER=wine64 (or install wine) to train a real profile, or run scripts/pgo-build.ps1 on Windows.' \
        "$target" "$combo" "$HOST_TARGET"
}

# ---------------------------------------------------------------------------------
# Combination worker
# ---------------------------------------------------------------------------------
build_combination() {
    local current_os="$1" isa="$2" gpu="$3" target="$4"
    local name="$isa-$gpu"
    local bin
    bin="$(target_bin "$target")"

    local target_exe=""
    local rust_target
    rust_target="$(os_rust_target "$current_os")"
    if [ "$current_os" = windows ]; then target_exe=".exe"; fi

    # Training always executes the instrumented binaries built for $rust_target itself
    # (never host-triple stand-ins -- see this file's header comment). `runner` is how:
    # empty = run directly, otherwise the cross runner resolved up front (Wine or
    # $PGO_CROSS_RUNNER). Under a runner, `train_env` adds WINEDEBUG=-all (keeps the log
    # readable) and PGO_TRAIN_SKIP_GPU=1: wgpu under Wine is not a training
    # environment, so the CPU-side GPU chunk-orchestration code stays COLD in a
    # cross-built profile -- a native Windows run of scripts/pgo-build.ps1 is the way to
    # get it warm.
    local -a runner=() train_env=()
    if [ "$current_os" = windows ] && [ "${#CROSS_RUNNER_WINDOWS[@]}" -gt 0 ]; then
        runner=("${CROSS_RUNNER_WINDOWS[@]}")
        train_env=(WINEDEBUG=-all PGO_TRAIN_SKIP_GPU=1)
    fi

    # Worker-only and cut-only profiles each get their own subdirectory: their package
    # sets (and therefore crate hashes) differ from each other and from the combined
    # build, so a profile trained for one must never be reused for -- or silently
    # overwrite -- a profile trained for a different -target. Worker keeps the original,
    # un-prefixed layout so already-collected profiles stay valid.
    local bdir pdir shared_merged
    if [ "$target" = worker ]; then
        bdir="$ROOT/target/pgo-builds/$current_os-$name"
        pdir="$ROOT/target/pgo-profiles/$current_os-$name"
        shared_merged="$PGO_PROFILE_DIR/$current_os-$name.profdata"
    else
        bdir="$ROOT/target/pgo-builds/$target-$current_os-$name"
        pdir="$ROOT/target/pgo-profiles/$target-$current_os-$name"
        shared_merged="$PGO_PROFILE_DIR/$target/$current_os-$name.profdata"
    fi
    local rawdir="$pdir/raw"
    local local_merged="$pdir/merged.profdata"
    local logfile="$bdir/build.log"
    mkdir -p "$bdir"
    : > "$logfile"

    local target_cpu simd_cap
    case "$isa" in
        avx512) target_cpu=x86-64-v4; simd_cap=avx512 ;;
        avx2)   target_cpu=x86-64-v3; simd_cap=avx2 ;;
        scalar) target_cpu=x86-64-v2; simd_cap=scalar ;;
        native) target_cpu=native;    simd_cap="" ;;
    esac

    # ONE feature list and ONE package set, used verbatim by both the instrumented and
    # the optimised cargo invocation below -- see this file's header comment on
    # package/feature parity.
    local features pkg_list
    features="$(pgo_features "$target" "$gpu")"
    pkg_list="$(pgo_packages "$target")"
    local -a pkg_args=()
    for p in $pkg_list; do pkg_args+=(-p "$p"); done
    # `cut` also builds cut-core's trainer (distinct example name, same invocation).
    local -a example_args=(--example pgo_train)
    [ "$target" = cut ] && example_args+=(--example pgo_train_cutcore)

    # BOLT needs the relocations (--emit-relocs) and the symbols, so a binary BOLT will
    # rewrite is linked unstripped (GNU ld also refuses --strip-all with --emit-relocs:
    # "final link failed: invalid operation") and stripped after the BOLT pass. Only the
    # worker is BOLTed, so only its build gets these flags; cut is a normal PGO build.
    local bolt_link_flags=""
    local release_strip="symbols"
    if [ "$BOLT_ENABLED" -eq 1 ] && [ "$current_os" = linux ] && [ "$target" = worker ]; then
        bolt_link_flags="${US}-C${US}force-frame-pointers=yes${US}-C${US}link-arg=-Wl,--emit-relocs"
        release_strip="none"
    fi

    local active_profile=""

    # Cross target with no runner (see the "Cross-OS training runner" section): a plain
    # release build, no instrumentation/training/profile-use/profile checks. The up-front
    # checks already refused --collect-only and --skip-train for this case.
    local no_pgo=0
    if [ "$current_os" = windows ] && [ "$CROSS_NO_PGO_WINDOWS" -eq 1 ]; then
        no_pgo=1
        printf '\n%s\n%s\n%s\n\n' \
            '!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!' \
            "$(no_pgo_notice "$target" "$current_os-$name")" \
            '!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!' \
            | tee -a "$logfile" >&2
    fi

    # Reusing an on-disk profile (--skip-train) for a triple other than $HOST_TARGET needs
    # proof it was trained for THAT triple: the `.target` sidecar this script writes next
    # to every profile it trains (triple + checksum of the profile it describes). A
    # windows-*.profdata written by scripts/pgo-build.ps1 (x86_64-pc-windows-msvc, same
    # file names) has no matching sidecar and is never applied to a windows-gnu build.
    local profile_target_file="$shared_merged.target"
    local reuse_profile=0
    if [ "$no_pgo" -eq 0 ] && [ "$SKIP_TRAIN" -eq 1 ] && [ -f "$shared_merged" ]; then
        if [ "$rust_target" = "$HOST_TARGET" ]; then
            reuse_profile=1
        elif [ -f "$profile_target_file" ] && \
             [ "$(cat "$profile_target_file")" = "$rust_target $(cksum < "$shared_merged")" ]; then
            reuse_profile=1
        else
            step "[$target][$current_os-$name][train] $shared_merged is not recorded as trained for $rust_target (no matching $profile_target_file; e.g. an x86_64-pc-windows-msvc profile from scripts/pgo-build.ps1) -- retraining instead of reusing it"
        fi
    fi

    # 1. Instrumented build + training run
    if [ "$no_pgo" -eq 1 ]; then
        : # plain release build below; nothing to train
    elif [ "$reuse_profile" -eq 0 ]; then
        rm -rf "$pdir"
        mkdir -p "$rawdir"

        step "[$target][$current_os-$name][train] Instrumented build (packages: $pkg_list; features: $features)"
        CARGO_PROFILE_RELEASE_STRIP="$release_strip" \
        CARGO_ENCODED_RUSTFLAGS="-C${US}target-cpu=$target_cpu${US}-C${US}profile-generate=$rawdir" \
        cargo build --release --target "$rust_target" --target-dir "$bdir" \
        "${pkg_args[@]}" --bins "${example_args[@]}" --features "$features" 2>&1 | tee -a "$logfile"

        # All training runs execute from inside $rawdir with a RELATIVE
        # LLVM_PROFILE_FILE pattern: a Windows profile runtime interprets that path with
        # Windows semantics, so an absolute Unix path under Wine is unreliable, while a
        # bare file name resolves against the (Wine-mapped) working directory either
        # way. Native and cross runs share this one code path. Executable paths stay
        # absolute; `${arr[@]+"${arr[@]}"}` keeps empty arrays safe under `set -u`.
        step "[$target][$current_os-$name][train] Running workload (${simd_cap:+INDICATRIX_SIMD=$simd_cap, }INDICATRIX_SAMPLES=$SAMPLES${runner[0]:+; runner: ${runner[*]}})"
        (
            cd "$rawdir"
            env LLVM_PROFILE_FILE="train-%p-%m.profraw" \
            ${train_env[@]+"${train_env[@]}"} \
            ${simd_cap:+INDICATRIX_SIMD="$simd_cap"} \
            INDICATRIX_SAMPLES="$SAMPLES" \
            ${runner[@]+"${runner[@]}"} \
            "$bdir/$rust_target/release/examples/pgo_train$target_exe"
        ) 2>&1 | tee -a "$logfile"

        # `indicatrix-cut-core`'s OWN training binary (cut target only), built by the
        # invocation above (see the header comment, step 2).
        if [ "$target" = cut ]; then
            step "[$target][$current_os-$name][train] Running indicatrix-cut-core training workload"
            (
                cd "$rawdir"
                env LLVM_PROFILE_FILE="cutcore-%p-%m.profraw" \
                ${train_env[@]+"${train_env[@]}"} \
                ${runner[@]+"${runner[@]}"} \
                "$bdir/$rust_target/release/examples/pgo_train_cutcore$target_exe"
            ) 2>&1 | tee -a "$logfile"
        fi

        step "[$target][$current_os-$name][train] Building instrumented indicatrix-net wire-protocol test (scene_roundtrip)"
        CARGO_PROFILE_RELEASE_STRIP="$release_strip" \
        CARGO_ENCODED_RUSTFLAGS="-C${US}target-cpu=$target_cpu${US}-C${US}profile-generate=$rawdir" \
        cargo test --release --target "$rust_target" --target-dir "$bdir" \
        -p indicatrix-net --features render --test scene_roundtrip --no-run \
        --message-format=json > "$bdir/net-test-build.json"

        # The JSON-extracted path is used unchanged: it is already absolute, and `-x`
        # holds for a cross-built windows-gnu `.exe` on Linux too (rustc sets the exec
        # bit on every linked executable regardless of target).
        local net_test_exe
        net_test_exe="$(grep -o '"executable":"[^"]*"' "$bdir/net-test-build.json" | tail -n1 | sed -E 's/.*:"(.*)"/\1/')"
        [ -n "$net_test_exe" ] && [ -x "$net_test_exe" ] || \
        die "[$target][$current_os-$name][train] could not locate the instrumented scene_roundtrip test binary"

        step "[$target][$current_os-$name][train] Running indicatrix-net wire-protocol training (scene_roundtrip)"
        (
            cd "$rawdir"
            env LLVM_PROFILE_FILE="net-%p-%m.profraw" \
            ${train_env[@]+"${train_env[@]}"} \
            ${runner[@]+"${runner[@]}"} \
            "$net_test_exe" --test-threads=1
        ) 2>&1 | tee -a "$logfile"

        shopt -s nullglob
        local -a prof_files=("$rawdir"/*.profraw)
        shopt -u nullglob

        [ "${#prof_files[@]}" -gt 0 ] || die "[$target][$current_os-$name][train] No .profraw files generated in $rawdir"

        step "[$target][$current_os-$name][merge] Merging ${#prof_files[@]} profile(s) with $PROFDATA"
        "$PROFDATA" merge -o "$local_merged" "${prof_files[@]}"

        [ -s "$local_merged" ] || die "[$target][$current_os-$name][merge] llvm-profdata failed or output is empty: $local_merged"

        mkdir -p "$(dirname "$shared_merged")"
        cp -f "$local_merged" "$shared_merged"
        # Record which triple this profile was trained for (see reuse_profile above).
        printf '%s %s\n' "$rust_target" "$(cksum < "$shared_merged")" > "$profile_target_file"
        active_profile="$shared_merged"
    else
        step "[$target][$current_os-$name][train] Reusing existing shared profile: $shared_merged"
        active_profile="$shared_merged"
    fi

    if [ "$COLLECT_ONLY" -eq 1 ]; then
        step "[$target][$current_os-$name][collect] Profile generated: $shared_merged"
        return 0
    fi

    # 2. Optimised release build -- SAME packages/target/target-dir/features as the
    #    instrumented build above; only RUSTFLAGS (profile-use instead of
    #    profile-generate, plus -pgo-warn-missing-function so a totally-unapplied
    #    profile shows up as a flood of warnings in $logfile instead of compiling
    #    silently as if untrained again) and the optional BOLT relocation/frame-pointer
    #    flags differ. The no-runner cross case builds the same selection with only
    #    `-C target-cpu`, so the output layout is identical; there is no profile to check.
    if [ "$no_pgo" -eq 1 ]; then
        step "[$target][$current_os-$name][build] Plain cross release build, NO PGO (packages: $pkg_list; features: $features)"
        CARGO_ENCODED_RUSTFLAGS="-C${US}target-cpu=$target_cpu" \
        cargo build --release --target "$rust_target" --target-dir "$bdir" \
        "${pkg_args[@]}" --bins "${example_args[@]}" --features "$features" 2>&1 | tee -a "$logfile"
    else
        step "[$target][$current_os-$name][build] PGO release build (packages: $pkg_list; features: $features)"
        CARGO_PROFILE_RELEASE_STRIP="$release_strip" \
        CARGO_ENCODED_RUSTFLAGS="-C${US}target-cpu=$target_cpu${US}-C${US}profile-use=$active_profile${US}-C${US}llvm-args=-pgo-warn-missing-function$bolt_link_flags" \
        cargo build --release --target "$rust_target" --target-dir "$bdir" \
        "${pkg_args[@]}" --bins "${example_args[@]}" --features "$features" 2>&1 | tee -a "$logfile"

        # Crate-hash check FIRST: it is the real proof the profile applied, and
        # test_profile_warnings only treats the missing-function count as fatal when
        # this could not verify it.
        PROFILE_HASH_VERIFIED=0
        test_profile_hash_applied "$active_profile" "$bdir" "$rust_target" "$target-$current_os-$name" "$target"
        test_profile_warnings "$logfile" "$target-$current_os-$name"
    fi

    local release_dir="$bdir/$rust_target/release"
    local src="$release_dir/$bin$target_exe"
    [ -f "$src" ] || die "[$target][$current_os-$name][build] Expected binary missing: $src"
    local dst="$OUT_DIR/$bin-$current_os-$name$target_exe"
    cp -f "$src" "$dst"
    printf '    %s (%s)\n' "$dst" "$(du -h "$dst" | cut -f1)"

    # 3. Optional BOLT post-processing (Linux only, indicatrix-worker only -- see the
    #    root README's "Optimized builds" section: indicatrix-cut is an interactive
    #    GUI binary with no scriptable workload, and a startup-only BOLT profile would
    #    give it a worse code layout than no BOLT at all).
    if [ "$BOLT_ENABLED" -eq 1 ] && [ "$current_os" = linux ]; then
        if [ "$target" = worker ]; then
            bolt_binary "$current_os" "$name" "$release_dir" "$bdir" "$simd_cap" "$target"
        else
            step "[$target][$current_os-$name][bolt] Skipping BOLT for indicatrix-cut (no scriptable workload -- see root README)"
        fi
    fi
}

# Cached path to the fixed training scene BOLT's `render` invocation needs -- built and
# generated once (via `indicatrix-worker`'s `write_pgo_scene` example, a plain release
# build outside any PGO target-dir) and reused by every BOLT combination.
PGO_BOLT_SCENE=""
ensure_pgo_bolt_scene() {
    [ -n "$PGO_BOLT_SCENE" ] && return 0
    local scene_dir="$ROOT/target/pgo-bolt-scene"
    local scene_json="$scene_dir/scene.json"
    mkdir -p "$scene_dir"
    if [ ! -f "$scene_json" ]; then
        step "[bolt] Writing training scene ($scene_json)"
        cargo run --release --target "$HOST_TARGET" --target-dir "$scene_dir/build" \
        -p indicatrix-worker --features worker --example write_pgo_scene -- "$scene_json"
    fi
    PGO_BOLT_SCENE="$scene_json"
}

bolt_binary() {
    local current_os="$1" name="$2" release_dir="$3" bdir="$4" simd_cap="$5" target="$6"
    local bin
    bin="$(target_bin "$target")"
    local src="$release_dir/$bin"
    local work="$bdir/bolt"
    mkdir -p "$work"
    ensure_pgo_bolt_scene

    step "[$target][$current_os-$name][bolt] Instrumenting $bin"
    llvm-bolt "$src" -instrument --instrumentation-file="$work/prof.fdata" \
    --instrumentation-file-append-pid -o "$work/$bin.inst"

    # `render` needs `--scene` (there is no built-in demo scene); `--no-gpu` is not a
    # real flag on this CLI -- `--only-cpu` is what forces the CPU tracer (see
    # apps/indicatrix-worker/src/cli/mod.rs's USAGE_RENDER). BOLT profiles this binary's
    # OWN code layout, not indicatrix's GPU dispatch, so CPU-only is also the right
    # workload to profile here regardless.
    step "[$target][$current_os-$name][bolt] Collecting profile"
    env ${simd_cap:+INDICATRIX_SIMD="$simd_cap"} "$work/$bin.inst" render \
    --scene "$PGO_BOLT_SCENE" \
    --out "$work/train.png" \
    --width 640 --height 360 --samples "$SAMPLES" --only-cpu

    merge-fdata "$work"/prof.fdata* > "$work/merged.fdata"

    step "[$target][$current_os-$name][bolt] Rewriting binary"
    llvm-bolt "$src" -data="$work/merged.fdata" -o "$work/$bin.bolt" \
    -reorder-blocks=ext-tsp -reorder-functions=hfsort+ \
    -split-functions -split-all-cold -icf=1 -dyno-stats
    # The build kept the symbols for BOLT; strip them now, as a plain release would.
    strip --strip-all "$work/$bin.bolt"

    cp -f "$work/$bin.bolt" "$OUT_DIR/$bin-$current_os-$name"
    printf '    %s (BOLT)\n' "$OUT_DIR/$bin-$current_os-$name"
}

# ---------------------------------------------------------------------------------
# Execution Entry Point
# ---------------------------------------------------------------------------------
printf 'Host System:  %s (%s)\n' "$HOST_OS" "$HOST_TARGET"
printf 'Target OS:    %s\n' "${OS_LIST[*]}"
if [ "${#CROSS_RUNNER_WINDOWS[@]}" -gt 0 ]; then
    printf 'Cross runner: %s (windows training runs under it; GPU stage skipped, stays cold)\n' "${CROSS_RUNNER_WINDOWS[*]}"
elif [ "$CROSS_NO_PGO_WINDOWS" -eq 1 ]; then
    printf 'Cross runner: none (windows binaries will be plain release builds, no PGO)\n'
fi
printf 'ISA Tiers:    %s\n' "${ISA_LIST[*]}"
printf 'GPU Tiers:    %s\n' "${GPU_LIST[*]}"
printf 'Targets:      %s\n' "${TARGET_LIST[*]}"
printf 'Execution:    %s\n' "$([ "$PARALLEL" -eq 1 ] && echo 'parallel' || echo 'sequential')"
printf 'BOLT:         %s\n' "$([ "$BOLT_ENABLED" -eq 1 ] && echo 'enabled (indicatrix-worker only)' || echo 'disabled')"
printf 'Output Dir:   %s\n' "$([ "$COLLECT_ONLY" -eq 1 ] && echo "$PGO_PROFILE_DIR (collect-only)" || echo "$OUT_DIR")"

mkdir -p "$OUT_DIR"
mkdir -p "$PGO_PROFILE_DIR"

if [ "$PARALLEL" -eq 1 ]; then
    printf '\n==> Prefetching crate dependencies for target and host...\n'
    if [ "$COLLECT_ONLY" -eq 0 ]; then
        for current_os in "${OS_LIST[@]}"; do
            case "$current_os" in
                linux)   cargo fetch --target x86_64-unknown-linux-gnu ;;
                windows) cargo fetch --target x86_64-pc-windows-gnu ;;
            esac
        done
    fi
    cargo fetch --target "$HOST_TARGET"

    declare -A JOB_PIDS
    declare -A JOB_LOGS

    for current_os in "${OS_LIST[@]}"; do
        for isa in "${ISA_LIST[@]}"; do
            for gpu in "${GPU_LIST[@]}"; do
                for target in "${TARGET_LIST[@]}"; do
                    name="$current_os-$isa-$gpu-$target"

                    bdir="$ROOT/target/pgo-builds/$name"
                    mkdir -p "$bdir"
                    logfile="$bdir/build.log"

                    printf '==> [%s] Dispatched build in background (log: %s)\n' "$name" "$logfile"

                    (
                        build_combination "$current_os" "$isa" "$gpu" "$target"
                    ) > "$logfile" 2>&1 &

                    pid=$!
                    JOB_PIDS["$pid"]="$name"
                    JOB_LOGS["$pid"]="$logfile"
                done
            done
        done
    done

    printf '\n==> Waiting for %d background builds to complete...\n' "${#JOB_PIDS[@]}"

    FAILED=0
    for pid in "${!JOB_PIDS[@]}"; do
        name="${JOB_PIDS[$pid]}"
        logfile="${JOB_LOGS[$pid]}"

        if wait "$pid"; then
            printf '  [SUCCESS] %s\n' "$name"
        else
            printf '  [FAILED]  %s (see tail of %s)\n' "$name" "$logfile" >&2
            tail -n 35 "$logfile" >&2 || true
            FAILED=1
        fi
    done

    [ "$FAILED" -eq 0 ] || die "One or more parallel builds failed."
else
    for current_os in "${OS_LIST[@]}"; do
        for isa in "${ISA_LIST[@]}"; do
            for gpu in "${GPU_LIST[@]}"; do
                for target in "${TARGET_LIST[@]}"; do
                    build_combination "$current_os" "$isa" "$gpu" "$target"
                done
            done
        done
    done
fi

if [ "$COLLECT_ONLY" -eq 1 ]; then
    printf '\nProfiles collected and copied to %s:\n' "$PGO_PROFILE_DIR"
    find "$PGO_PROFILE_DIR" -maxdepth 2 -name 'windows-*.profdata' -exec ls -lh {} + 2>/dev/null || true
else
    printf '\nAll requested artifacts generated in %s:\n' "$OUT_DIR"
    ls -lh "$OUT_DIR"
fi

# Repeat the NO PGO label for every cross-built combination that got none, so it cannot
# scroll away (under --parallel the per-combination notice lives only in its log file).
if [ "$CROSS_NO_PGO_WINDOWS" -eq 1 ]; then
    printf '\n!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!\n' >&2
    for isa in "${ISA_LIST[@]}"; do
        for gpu in "${GPU_LIST[@]}"; do
            for target in "${TARGET_LIST[@]}"; do
                printf '%s\n' "$(no_pgo_notice "$target" "windows-$isa-$gpu")" >&2
            done
        done
    done
    printf '!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!\n' >&2
fi
