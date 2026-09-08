//! Compiles the Slint UI (`ui/app.slint`) into the generated Rust module
//! `src/lib.rs`'s `slint::include_modules!()` pulls in.
//!
//! Runs on the host for every build of this crate, `wasm32-unknown-unknown` included --
//! `slint-build` is a build-dependency, always compiled for and run on the machine
//! running `cargo`/`trunk`. Unconditional (no `wasm32` guard, unlike almost everything
//! in `src/`) so `cargo check -p indicatrix-web` from a native host still produces the
//! generated module, even though `src/lib.rs` gates every module that uses it behind
//! `wasm32` and so never references it there.
fn main() {
    slint_build::compile("ui/app.slint").unwrap();
}
