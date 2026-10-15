//! Compiles the Slint UI. The `material` style follows the system's light or dark
//! scheme and has touch-sized widget metrics; the app's own tokens live in
//! `ui/theme.slint`.

fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("material".to_string());
    slint_build::compile_with_config("ui/app.slint", config).expect("the Slint UI compiles");
    println!("cargo:rerun-if-changed=ui/");
}
