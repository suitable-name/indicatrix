//! Compiles the Slint UI (`ui/app.slint` and everything it imports) into the module
//! `src/lib.rs`'s `slint::include_modules!()` pulls in.
//!
//! Runs on the host for every build, `wasm32-unknown-unknown` included, and is not
//! `wasm32`-guarded, so a native `cargo check -p indicatrix-web` still compiles the
//! UI (and reports `.slint` errors) even though nothing native uses the result.
fn main() {
    slint_build::compile("ui/app.slint").unwrap();
}
