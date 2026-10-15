fn main() {
    // Slint tracks the files it imports, but an explicit directory watch keeps edits to
    // theme/model files from being missed.
    println!("cargo:rerun-if-changed=ui/");
    println!("cargo:rerun-if-changed=build.rs");

    // Same style as apps/indicatrix-cut.
    slint_build::compile_with_config(
        "ui/app.slint",
        slint_build::CompilerConfiguration::new().with_style("fluent-dark".to_string()),
    )
    .expect("failed to compile the trainer Slint UI (ui/app.slint)");
}
