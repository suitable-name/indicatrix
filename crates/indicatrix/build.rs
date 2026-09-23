//! Computes a stable content hash of this crate's own `src/**/*.rs` and `src/**/*.wgsl`
//! tree (plus `Cargo.toml`) and exposes it to `lib.rs` as `INDICATRIX_SOURCE_HASH`
//! (`indicatrix::SOURCE_HASH`), and a hash of the crate version as `INDICATRIX_BUILD_ID`
//! (`indicatrix::BUILD_ID`). Both feed the handshake identity (see
//! `indicatrix-net`'s `handshake` module): `BUILD_ID` is the primary, always-enforced
//! check; `SOURCE_HASH` is a second, finer check enforced only when both peers can
//! establish their own (a `warn!`, not a refusal, when either side's is unknown).
//!
//! # Why this exists
//!
//! `indicatrix-net`'s wire protocol handshake refuses to pair a viewer and a remote
//! worker whose `indicatrix` builds disagree -- see `indicatrix-net`'s `handshake`
//! module. Mixing samples traced by two different physics implementations produces a
//! silently, plausibly wrong image: no crash, just numbers that look like a render and
//! aren't. `BUILD_ID` alone can't fully catch this on its own: it's a hash of the crate
//! VERSION, so it only catches a mismatch when the release rule below ("bump the
//! version whenever the physics changes") was actually followed. `SOURCE_HASH`, being a
//! content hash of the actual source tree, catches a physics-affecting edit that never
//! bumped the version at all. A plain "protocol
//! version" field could never catch either case (the wire format didn't change, the
//! *physics* did).
//!
//! # Why a content hash, not `git describe`
//!
//! This repository has a `.git` directory but no commits yet, so `git describe`/
//! `git rev-parse HEAD` both fail unconditionally. A content hash of the source works
//! regardless of VCS state, and (unlike a commit hash) also flags uncommitted local
//! edits that a `git`-based check would silently treat as whatever the last commit was.
//!
//! # Why not `DefaultHasher`
//!
//! `DefaultHasher`'s output is explicitly NOT guaranteed stable across Rust versions --
//! two machines building the same source on different toolchains could disagree and
//! refuse a compatible pairing. FNV-1a below is a fixed, fully-specified algorithm: same
//! input bytes always produce the same output, on any platform or Rust version, forever,
//! for about ten lines and zero dependencies.
//!
//! # Determinism
//!
//! - Files are visited in sorted, POSIX-normalized relative-path order (`/`, not `\`),
//!   so the hash does not depend on the host OS's directory-listing order or path
//!   separator.
//! - The relative path itself is hashed alongside each file's contents, so renaming a
//!   file changes the id even if no byte of any file's content changed.
//! - Line endings are normalized (`\r\n` -> `\n`) before hashing, so an identical
//!   checkout differing only by CRLF/LF (e.g. Windows author, Linux worker) hashes
//!   identically.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// The 64-bit FNV-1a offset basis and prime -- see
/// <https://en.wikipedia.org/wiki/Fowler%E2%80%93Noll%E2%80%93Vo_hash_function>. Fully
/// specified, versioned nowhere, and will never change out from under this build.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv1a_update(mut hash: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Recursively collects every file under `dir`, returned as paths relative to `root`.
fn collect_files(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, root, out);
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_path_buf());
        }
    }
}

/// POSIX-normalizes a relative path (`/` separators) so the hash is identical whether
/// computed on Windows or Linux.
fn normalize_path(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Concatenates `shaders/transport_physics.wgsl` (the single shared source of the
/// transport physics functions) ahead of each of `spectral_transport.wgsl`,
/// `transport_functions.wgsl`, and `wavefront_transport.wgsl`, and writes each result
/// into `$OUT_DIR`, since WGSL has no `#include` and every one of those files assumes
/// the prelude's symbols are already in scope.
///
/// `shaders/transport_bounce.wgsl` -- the scene bindings plus the
/// per-bounce `transport_bounce_step`/`transport_finalize_xyz` functions shared between
/// the megakernel and the wavefront pipeline (see that file's own header comment) -- is
/// ALSO concatenated in, for `spectral_transport.wgsl` and `wavefront_transport.wgsl`
/// only: `transport_functions.wgsl` (Tier 2's standalone per-function kernels) never
/// calls either shared function or needs their scene bindings, so it is not
/// concatenated there.
///
/// Deliberately writes into `$OUT_DIR` (under `target/`), never under `src/`: the
/// content hash below walks `src/**/*.wgsl` on disk, and a generated file leaked into
/// `src/` would either double-count as a redundant hash input or, if `.gitignore`d, go
/// missing on a clean checkout and make the hash nondeterministic across `cargo clean`.
///
/// `include_str!(concat!(env!("OUT_DIR"), "/..."))` in `renderer::gpu::estimator_check`,
/// `renderer::gpu::transport_check`, and `renderer::gpu::frame` reads these generated
/// files back at compile time.
fn generate_transport_shaders(manifest_dir: &Path, out_dir: &Path) {
    let shaders_dir = manifest_dir.join("src").join("renderer").join("shaders");
    let read = |name: &str| -> String {
        fs::read_to_string(shaders_dir.join(name))
            .unwrap_or_else(|e| panic!("failed to read shaders/{name}: {e}"))
    };
    let prelude = read("transport_physics.wgsl");
    let bounce_shared = read("transport_bounce.wgsl");

    // (body file, whether `transport_bounce.wgsl` is concatenated in between the
    // prelude and the body) -- see this function's own doc comment.
    let units: [(&str, bool); 3] = [
        ("spectral_transport.wgsl", true),
        ("transport_functions.wgsl", false),
        ("wavefront_transport.wgsl", true),
    ];

    for (name, needs_bounce_shared) in units {
        let body = read(name);
        let generated = if needs_bounce_shared {
            format!(
                "// GENERATED by build.rs: shaders/transport_physics.wgsl + \
                 shaders/transport_bounce.wgsl + shaders/{name}.\n\
                 // Do not edit this file -- edit the sources above instead.\n\n\
                 {prelude}\n{bounce_shared}\n{body}"
            )
        } else {
            format!(
                "// GENERATED by build.rs: shaders/transport_physics.wgsl + shaders/{name}.\n\
                 // Do not edit this file -- edit the two sources above instead.\n\n{prelude}\n{body}"
            )
        };
        let out_name = name.replace(".wgsl", ".generated.wgsl");
        fs::write(out_dir.join(&out_name), generated)
            .unwrap_or_else(|e| panic!("failed to write {out_name} to OUT_DIR: {e}"));
    }
}

fn main() {
    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo"),
    );
    let src_dir = manifest_dir.join("src");
    let cargo_toml = manifest_dir.join("Cargo.toml");
    let out_dir =
        PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo for build scripts"));

    generate_transport_shaders(&manifest_dir, &out_dir);

    let mut rel_paths = Vec::new();
    collect_files(&src_dir, &manifest_dir, &mut rel_paths);
    // .rs and .wgsl both carry physics this hash must fingerprint: a worker running a
    // GPU backend executes WGSL as its physics, not as decoration on top of it. Two
    // workers with byte-identical Rust but divergent WGSL would otherwise handshake as
    // identical, which is exactly the silent-divergence failure this hash exists to
    // catch. Deliberately still ONE hash, not two: verify_compatible has no "close
    // enough" tier, so any change to either language is a build-identity change.
    rel_paths.retain(|p| {
        p.extension()
            .is_some_and(|ext| ext == "rs" || ext == "wgsl")
    });

    // Pathological case: no source files found under src/ at all. Rather than silently
    // emit a hash computed from Cargo.toml alone, fall back to an explicit sentinel --
    // parse_build_id maps any non-16-hex-char string (this included) to
    // UNKNOWN_BUILD_HASH, which verify_compatible refuses unconditionally.
    if rel_paths.is_empty() {
        println!("cargo:rerun-if-changed={}", src_dir.display());
        println!("cargo:rustc-env=INDICATRIX_SOURCE_HASH=unknown");
        println!("cargo:rustc-env=INDICATRIX_BUILD_ID=unknown");
        return;
    }

    rel_paths.push(PathBuf::from("Cargo.toml"));
    rel_paths.sort();

    let mut hash = FNV_OFFSET_BASIS;
    for rel in &rel_paths {
        let abs = manifest_dir.join(rel);
        let Ok(contents) = fs::read(&abs) else {
            continue;
        };
        // Normalize CRLF -> LF before hashing so a Windows checkout and a Linux
        // checkout of byte-identical source hash identically.
        let normalized: Vec<u8> = {
            let mut out = Vec::with_capacity(contents.len());
            let mut i = 0;
            while i < contents.len() {
                if contents[i] == b'\r' && contents.get(i + 1) == Some(&b'\n') {
                    // Drop the \r; the loop picks up the \n on the next iteration.
                } else {
                    out.push(contents[i]);
                }
                i += 1;
            }
            out
        };

        let rel_str = normalize_path(rel);
        hash = fnv1a_update(hash, rel_str.as_bytes());
        hash = fnv1a_update(hash, &[0u8]); // separator between path and contents
        hash = fnv1a_update(hash, &normalized);

        println!("cargo:rerun-if-changed={}", abs.display());
    }
    // Also react to files being added/removed (rerun-if-changed on the directory
    // itself catches that on most platforms/cargo versions).
    println!("cargo:rerun-if-changed={}", src_dir.display());
    println!("cargo:rerun-if-changed={}", cargo_toml.display());

    let source_hash = format!("{hash:016x}");
    println!("cargo:rustc-env=INDICATRIX_SOURCE_HASH={source_hash}");

    // The PRIMARY handshake identity is keyed on the crate VERSION, not on the source
    // hash: a worker built on Linux and a viewer built on Windows from the same release
    // must pair even when their checkouts differ in metadata, docs, or platform-only
    // code. That guarantee rests on the release rule "bump the version whenever
    // anything under src/ that affects a traced sample changes" -- a promise nothing
    // enforces, which is exactly why `verify_compatible` ALSO compares the source hash
    // above (`indicatrix::SOURCE_HASH`) as a second, finer check when both peers can
    // establish it (see `indicatrix-net::handshake`'s module doc comment).
    // `CARGO_PKG_VERSION` is the package's resolved version: cargo substitutes a
    // `version = { workspace = true }` inheritance before running this script, and
    // `cargo package` writes the literal version into the published manifest, so a
    // workspace build and a crates.io build of the same release agree. An empty or
    // missing value must not hash into a plausible id, so it falls back to the
    // `unknown` sentinel the handshake refuses.
    let build_id = match std::env::var("CARGO_PKG_VERSION") {
        Ok(version) if !version.trim().is_empty() => {
            format!(
                "{:016x}",
                fnv1a_update(FNV_OFFSET_BASIS, version.trim().as_bytes())
            )
        }
        _ => "unknown".to_string(),
    };
    println!("cargo:rustc-env=INDICATRIX_BUILD_ID={build_id}");
}
