#!/usr/bin/env bash
#
# Profile-guided-optimization (PGO), optionally followed by BOLT, for indicatrix-worker
# and/or indicatrix-cut. Supports sequential execution or parallel dispatch across all
# tiers, and across either or both target binaries (`--target worker|cut|all`).
#
# PGO training runs natively on the host target to generate target-agnostic LLVM profile
# data, allowing clean cross-compilation (e.g. Linux -> Windows) without Wine or binfmt_misc.
#
# Two instrumented training binaries feed each combination's merged profile:
#   1. `indicatrix`'s `pgo_train` example -- the CPU spectral tracer (every material
#      optical character, dispersive/non-dispersive, plain/frosted-girdle, both the
#      float HDR buffer and the 8-bit tonemapped output), the meet-point solver (and
#      through it the SIMD candidate-vertex kernels), external solid measurement,
#      `.asc` reader/writer round trip, B-Rep reconstruction, the tilt-performance
#      sweep, and (only in the `gpu` tier, when this host has an adapter) the CPU-side
#      GPU chunk dispatch/readback orchestration. See that example's own doc comment
#      for the full stage-by-stage coverage table.
#   2. `indicatrix-net`'s `scene_roundtrip` integration test, run under the same
#      instrumentation -- covers `SceneState`'s postcard wire-protocol encode/decode,
#      the one hot path that cannot be reached from `pgo_train` (a `indicatrix` example
#      cannot depend on `indicatrix-net`, which itself depends on `indicatrix`).
# Every `.profraw` from both binaries is merged into one `.profdata` per combination.
#
# `indicatrix-worker`'s training additionally enables `indicatrix`'s `hdr` feature (the
# worker's own render path always has it on); `indicatrix-cut` trains without it, matching
# each binary's real feature set. Worker profiles are written to `pgo-profile/` (unchanged
# layout, so existing collected profiles keep working); `indicatrix-cut` profiles are
# written to `pgo-profile/cut/`, since the two binaries' training features differ and a
# profile trained for one is not valid for the other.
#
# Environment knobs `pgo_train` honours:
#   INDICATRIX_SIMD=scalar|avx2|avx512   caps runtime SIMD dispatch during training (unset for native)
#   INDICATRIX_SAMPLES=<n>               overrides the tracer's samples-per-pixel directly
#   PGO_SCALE=<factor>               scales every stage's frame size/iteration count (default 1.0)
#   PGO_TRAIN_SKIP_GPU=1             skips the GPU chunk-orchestration training stage
#
# Usage:
#   scripts/pgo-bolt-build.sh                                # PGO only, sequential, host OS, all tiers, both targets
#   scripts/pgo-bolt-build.sh --native                       # PGO only, all tiers + march=native
#   scripts/pgo-bolt-build.sh --os windows                   # PGO only, sequential cross-compilation for Windows
#   scripts/pgo-bolt-build.sh --os windows --parallel        # PGO only, parallel cross-compilation for Windows
#   scripts/pgo-bolt-build.sh --collect-only                 # Generate & copy Windows PGO profiles only
#   scripts/pgo-bolt-build.sh --isa avx2 --gpu gpu           # single combination
#   scripts/pgo-bolt-build.sh --target worker                # indicatrix-worker only
#   scripts/pgo-bolt-build.sh --target cut                   # indicatrix-cut only
#   scripts/pgo-bolt-build.sh --bolt                         # PGO + BOLT (Linux only, indicatrix-worker only -- see below)
#   scripts/pgo-bolt-build.sh --bolt --native                # PGO + BOLT with march=native
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
        -h|--help)      sed -n '2,/^set -euo/p' "$0" | sed 's/^# \{0,1\}//' | sed '$d'; exit 0 ;;
        *)              die "unknown argument: $1 (try --help)" ;;
    esac
done

if [ "$COLLECT_ONLY" -eq 1 ]; then
    if [ "$SKIP_TRAIN" -eq 1 ]; then
        die "--collect-only and --skip-train cannot be used together"
    fi
    OS_ARG=windows
fi

case "$ISA_ARG" in
    all)                       ISA_LIST=(avx512 avx2 scalar) ;;
    avx512|avx2|scalar|native) ISA_LIST=("$ISA_ARG") ;;
    *)                         die "--isa must be avx512, avx2, scalar, native, or all" ;;
esac

if [ "$WITH_NATIVE" -eq 1 ]; then
    if [[ ! " ${ISA_LIST[*]} " =~ " native " ]]; then
        ISA_LIST+=("native")
    fi
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

if [ "$OS_ARG" = auto ]; then
    if [ "$USE_BOLT" -eq 1 ]; then
        OS_LIST=(linux)
    else
        OS_LIST=(linux windows)
    fi
else
    case "$OS_ARG" in
        linux|windows) OS_LIST=("$OS_ARG") ;;
        *)             die "--os must be windows or linux" ;;
    esac
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
[ -n "$OUT_DIR" ] || OUT_DIR="$ROOT/bin"
PGO_PROFILE_DIR="$ROOT/pgo-profile"

SYSROOT="$(rustc --print sysroot)"
PROFDATA="$SYSROOT/lib/rustlib/$HOST_TARGET/bin/llvm-profdata$HOST_EXE"
if [ ! -x "$PROFDATA" ]; then
    if command -v llvm-profdata >/dev/null 2>&1; then
        PROFDATA="$(command -v llvm-profdata)"
    else
        die "llvm-profdata not found in host rustlib ($PROFDATA) or system PATH"
    fi
fi

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

# ---------------------------------------------------------------------------------
# Combination worker
# ---------------------------------------------------------------------------------
build_combination() {
    local current_os="$1" isa="$2" gpu="$3" target="$4"
    local name="$isa-$gpu"
    local bin
    bin="$(target_bin "$target")"

    local target_exe=""
    local rust_target=""
    case "$current_os" in
        linux)   rust_target=x86_64-unknown-linux-gnu; target_exe="" ;;
        windows) rust_target=x86_64-pc-windows-gnu;   target_exe=".exe" ;;
        *)       die "unsupported os in build_combination: $current_os" ;;
    esac

    # Worker profiles keep the original, un-prefixed layout so already-collected
    # profiles stay valid; the editor gets its own subdirectory since its training
    # features (no `hdr`) differ from the worker's.
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

    local target_cpu simd_cap train_features
    local -a app_features

    case "$isa" in
        avx512) target_cpu=x86-64-v4; simd_cap=avx512 ;;
        avx2)   target_cpu=x86-64-v3; simd_cap=avx2 ;;
        scalar) target_cpu=x86-64;    simd_cap=scalar ;;
        native) target_cpu=native;    simd_cap="" ;;
    esac

    if [ "$target" = worker ]; then
        if [ "$gpu" = gpu ]; then
            train_features='hdr,serde,gpu'
            app_features=(--features 'indicatrix-worker/gpu,indicatrix-worker/worker')
        else
            train_features='hdr,serde'
            app_features=(--features 'indicatrix-worker/worker')
        fi
    else
        if [ "$gpu" = gpu ]; then
            train_features='serde,gpu'
            app_features=(--features 'indicatrix-cut/gpu')
        else
            train_features='serde'
            app_features=()
        fi
    fi

    local cpu_flag="-C target-cpu=$target_cpu"
    local bolt_link_flags=""
    if [ "$BOLT_ENABLED" -eq 1 ] && [ "$current_os" = linux ]; then
        bolt_link_flags=" -C link-arg=-Wl,--emit-relocs -C force-frame-pointers=yes"
    fi

    local active_profile=""

    # 1. Native host profiling stage
    if [ "$SKIP_TRAIN" -eq 0 ] || [ ! -f "$shared_merged" ]; then
        rm -rf "$pdir"
        mkdir -p "$rawdir"

        step "[$target][$current_os-$name][train] Building instrumented runner on host ($HOST_TARGET)"
        RUSTFLAGS="$cpu_flag -C profile-generate=$rawdir" \
        cargo build --release --target "$HOST_TARGET" --target-dir "$bdir" \
        -p indicatrix --features "$train_features" --example pgo_train

        step "[$target][$current_os-$name][train] Running workload (${simd_cap:+INDICATRIX_SIMD=$simd_cap, }INDICATRIX_SAMPLES=$SAMPLES)"
        env LLVM_PROFILE_FILE="$rawdir/train-%p-%m.profraw" \
        ${simd_cap:+INDICATRIX_SIMD="$simd_cap"} \
        INDICATRIX_SAMPLES="$SAMPLES" \
        "$bdir/$HOST_TARGET/release/examples/pgo_train$HOST_EXE"

        step "[$target][$current_os-$name][train] Building instrumented indicatrix-net wire-protocol test (scene_roundtrip)"
        RUSTFLAGS="$cpu_flag -C profile-generate=$rawdir" \
        cargo test --release --target "$HOST_TARGET" --target-dir "$bdir" \
        -p indicatrix-net --features render --test scene_roundtrip --no-run \
        --message-format=json > "$bdir/net-test-build.json"

        local net_test_exe
        net_test_exe="$(grep -o '"executable":"[^"]*"' "$bdir/net-test-build.json" | tail -n1 | sed -E 's/.*:"(.*)"/\1/')"
        [ -n "$net_test_exe" ] && [ -x "$net_test_exe" ] || \
        die "[$target][$current_os-$name][train] could not locate the instrumented scene_roundtrip test binary"

        step "[$target][$current_os-$name][train] Running indicatrix-net wire-protocol training (scene_roundtrip)"
        LLVM_PROFILE_FILE="$rawdir/net-%p-%m.profraw" "$net_test_exe" --test-threads=1

        shopt -s nullglob
        local -a prof_files=("$rawdir"/*.profraw)
        shopt -u nullglob

        [ "${#prof_files[@]}" -gt 0 ] || die "[$target][$current_os-$name][train] No .profraw files generated in $rawdir"

        step "[$target][$current_os-$name][merge] Merging ${#prof_files[@]} profile(s) with $PROFDATA"
        "$PROFDATA" merge -o "$local_merged" "${prof_files[@]}"

        [ -s "$local_merged" ] || die "[$target][$current_os-$name][merge] llvm-profdata failed or output is empty: $local_merged"

        mkdir -p "$(dirname "$shared_merged")"
        cp -f "$local_merged" "$shared_merged"
        active_profile="$shared_merged"
    else
        step "[$target][$current_os-$name][train] Reusing existing shared profile: $shared_merged"
        active_profile="$shared_merged"
    fi

    if [ "$COLLECT_ONLY" -eq 1 ]; then
        step "[$target][$current_os-$name][collect] Profile generated: $shared_merged"
        return 0
    fi

    # 2. Optimized release compilation for the target
    step "[$target][$current_os-$name][build] Building PGO-optimized $bin for $rust_target"
    RUSTFLAGS="$cpu_flag -C profile-use=$active_profile$bolt_link_flags" \
    cargo build --release --target "$rust_target" --target-dir "$bdir" \
    -p "$bin" "${app_features[@]}"

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

bolt_binary() {
    local current_os="$1" name="$2" release_dir="$3" bdir="$4" simd_cap="$5" target="$6"
    local bin
    bin="$(target_bin "$target")"
    local src="$release_dir/$bin"
    local work="$bdir/bolt"
    mkdir -p "$work"

    step "[$target][$current_os-$name][bolt] Instrumenting $bin"
    llvm-bolt "$src" -instrument --instrumentation-file="$work/prof.fdata" \
    --instrumentation-file-append-pid -o "$work/$bin.inst"

    step "[$target][$current_os-$name][bolt] Collecting profile"
    env ${simd_cap:+INDICATRIX_SIMD="$simd_cap"} "$work/$bin.inst" render \
    --out "$work/train.png" \
    --width 640 --height 360 --samples "$SAMPLES" --no-gpu

    merge-fdata "$work"/prof.fdata* > "$work/merged.fdata"

    step "[$target][$current_os-$name][bolt] Rewriting binary"
    llvm-bolt "$src" -data="$work/merged.fdata" -o "$work/$bin.bolt" \
    -reorder-blocks=ext-tsp -reorder-functions=hfsort+ \
    -split-functions -split-all-cold -icf=1 -dyno-stats

    cp -f "$work/$bin.bolt" "$OUT_DIR/$bin-$current_os-$name"
    printf '    %s (BOLT)\n' "$OUT_DIR/$bin-$current_os-$name"
}

# ---------------------------------------------------------------------------------
# Execution Entry Point
# ---------------------------------------------------------------------------------
printf 'Host System:  %s (%s)\n' "$HOST_OS" "$HOST_TARGET"
printf 'Target OS:    %s\n' "${OS_LIST[*]}"
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
