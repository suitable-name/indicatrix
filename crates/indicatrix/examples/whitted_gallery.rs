//! Side-by-side deterministic and spectral reference gallery; run explicitly by the owner.
//!
//! `--out DIRECTORY` writes every built-in material under three studio presets.
//! Both columns share geometry, material, camera, bounce limit and tone mapping.

use std::{error::Error, fmt::Write as _, fs, path::PathBuf};

use indicatrix::{
    geometry::StandardGemCuts,
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset, build_plane_soa},
    },
    render_setup::encode_preview_png,
    renderer::{
        cpu_frame::trace_pixels_interleaved_geom,
        engine::RayOptions,
        frame_scene::FrameScene,
        whitted::{WhittedPrep, whitted_pixels_interleaved_geom},
    },
};

fn output_dir() -> Result<PathBuf, String> {
    let mut args = std::env::args_os().skip(1);
    let mut output = None;
    while let Some(argument) = args.next() {
        if argument == "--out" {
            output = Some(PathBuf::from(
                args.next().ok_or("--out requires a directory")?,
            ));
        } else {
            return Err(format!(
                "Unknown argument: {}. Use --out DIRECTORY.",
                argument.to_string_lossy()
            ));
        }
    }
    output.ok_or_else(|| "Use --out DIRECTORY.".to_owned())
}

fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn main() -> Result<(), Box<dyn Error>> {
    let output = output_dir().map_err(std::io::Error::other)?;
    fs::create_dir_all(&output)?;
    let camera = Camera::new(0.6, 0.45, 3.5, 42.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let soa = build_plane_soa(&planes);
    let mut html = String::from(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\">\
        <meta name=\"viewport\" content=\"width=device-width\"><title>Ray-Traced Gallery</title>\
        <style>body{font:16px system-ui;background:#181818;color:#eee;margin:2rem}\
        table{border-collapse:collapse}td,th{padding:.5rem;text-align:left}img{width:256px;height:256px}\
        caption{text-align:left;font-size:1.4rem;margin:1rem 0}</style>\
        <h1>Ray-Traced And Path-Traced</h1><p>Ray-Traced: 8 fixed taps, Fire on.\
        Path-Traced: 256 samples per pixel, CPU. Both use depth 64 and the same tone mapping.\
        Ray-Traced approximates tint and frost and omits polarization, doubling, scattering,\
        fluorescence, zoning and edge rounding.</p><table><tr><th>Material And Lighting</th>\
        <th>Ray-Traced</th><th>Path-Traced</th></tr>",
    );
    for (material_index, material) in GemMaterial::built_in().iter().enumerate() {
        for (preset_index, preset) in [
            LightingPreset::LightTent,
            LightingPreset::RingLights,
            LightingPreset::IsoHemisphere,
        ]
        .into_iter()
        .enumerate()
        {
            let scene = FrameScene {
                camera: &camera,
                width: 256,
                height: 256,
                planes: &planes,
                facet_finishes: &[],
                material,
                max_bounces: 64,
                environment: preset.studio(1.0, 0.85, 0.95),
            };
            let prep = WhittedPrep::new(&scene, Fluorescence::none(), RayOptions::default())?;
            let ray = whitted_pixels_interleaved_geom(&scene, &[], &soa, &prep, 0, 1, 0, 8);
            let path = trace_pixels_interleaved_geom(
                &scene,
                &[],
                Fluorescence::none(),
                &soa,
                0,
                1,
                0,
                256,
            );
            let stem = format!("material_{material_index:02}_light_{preset_index}");
            let ray_name = format!("{stem}_ray.png");
            let path_name = format!("{stem}_path.png");
            for (name, sums, samples) in [(&ray_name, &ray, 8), (&path_name, &path, 256)] {
                let png = encode_preview_png(256, 256, sums, samples)
                    .ok_or_else(|| std::io::Error::other("PNG encoding failed"))?;
                fs::write(output.join(name), png)?;
            }
            let label = escaped(&format!("{} — {preset:?}", material.name));
            write!(
                html,
                "<tr><th>{label}</th><td><img src=\"{ray_name}\" alt=\"{label}, Ray-Traced\"></td>\
                <td><img src=\"{path_name}\" alt=\"{label}, Path-Traced\"></td></tr>"
            )?;
            eprintln!("Wrote {} / {preset:?}", material.name);
        }
    }
    html.push_str("</table></html>");
    fs::write(output.join("index.html"), html)?;
    Ok(())
}
