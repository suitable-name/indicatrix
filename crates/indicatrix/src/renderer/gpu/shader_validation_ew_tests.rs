//! Event wavefront shader checks (plan 7.1): per-kernel storage-buffer tables, struct layouts
//! against the Rust twins, and text invariants that make "verbatim move" a checkable property.

use naga::{
    front::wgsl,
    valid::{Capabilities, ValidationFlags, Validator},
};

use super::TRANSPORT_BOUNCE_PIECE_SOURCES;

/// One generated unit per kernel: the unit name is also the entry point name.
macro_rules! ew_unit {
    ($entry:literal) => {
        (
            $entry,
            include_str!(concat!(env!("OUT_DIR"), "/", $entry, ".generated.wgsl")),
        )
    };
}

/// The 13 event wavefront generated units, by (entry point name, source text).
const EW_UNITS: &[(&str, &str)] = &[
    ew_unit!("ew_generate"),
    ew_unit!("ew_extend"),
    ew_unit!("ew_scatter"),
    ew_unit!("ew_nee_hg"),
    ew_unit!("ew_surface"),
    ew_unit!("ew_arm_frosted"),
    ew_unit!("ew_arm_tir"),
    ew_unit!("ew_arm_uni_entry"),
    ew_unit!("ew_arm_uni_internal"),
    ew_unit!("ew_arm_generic"),
    ew_unit!("ew_split_resolve"),
    ew_unit!("ew_tail"),
    ew_unit!("ew_finalize"),
];

/// The exact storage buffers each kernel reaches (plan 4.3, with the HG NEE split out of
/// `ew_scatter`). A binding that vanishes from a kernel (the HG NEE block once did) changes
/// this table and fails the test below; so does a binding that sneaks in.
const EXPECTED_STORAGE: &[(&str, &[&str])] = &[
    ("ew_generate", &["ray_core", "ray_chan"]),
    (
        "ew_extend",
        &[
            "material",
            "planes",
            "hdr_texels",
            "dist_func",
            "ray_core",
            "ray_chan",
            "ray_hit",
        ],
    ),
    (
        "ew_scatter",
        &["material", "planes", "ray_core", "ray_chan", "ray_hit"],
    ),
    (
        "ew_nee_hg",
        &[
            "material",
            "planes",
            "facet_finishes",
            "hdr_texels",
            "dist_func",
            "dist_cdf",
            "ray_core",
            "ray_chan",
        ],
    ),
    (
        "ew_surface",
        &[
            "material",
            "planes",
            "facet_finishes",
            "ray_core",
            "ray_chan",
            "ray_hit",
        ],
    ),
    (
        "ew_arm_frosted",
        &[
            "material",
            "planes",
            "hdr_texels",
            "dist_func",
            "dist_cdf",
            "ray_core",
            "ray_chan",
            "ray_hit",
        ],
    ),
    (
        "ew_arm_tir",
        &["material", "ray_core", "ray_chan", "ray_hit"],
    ),
    (
        "ew_arm_uni_entry",
        &["material", "ray_core", "ray_chan", "ray_hit"],
    ),
    (
        "ew_arm_uni_internal",
        &["material", "dist_func", "ray_core", "ray_chan", "ray_hit"],
    ),
    (
        "ew_arm_generic",
        &["material", "dist_func", "ray_core", "ray_chan", "ray_hit"],
    ),
    (
        "ew_split_resolve",
        &[
            "material",
            "planes",
            "hdr_texels",
            "ray_core",
            "ray_chan",
            "ray_hit",
        ],
    ),
    ("ew_tail", &["material", "ray_core", "ray_chan", "ray_hit"]),
    (
        "ew_finalize",
        &[
            "out_xyz",
            "out_radiance",
            "out_lambdas",
            "out_path_pdf",
            "out_compat",
            "ray_core",
            "ray_chan",
        ],
    ),
];

#[test]
fn ew_kernels_bind_exactly_the_expected_storage_buffers() {
    assert_eq!(
        EXPECTED_STORAGE.len(),
        EW_UNITS.len(),
        "every kernel needs an expected-storage row"
    );
    println!("\n{:<22} {:<6} Storage Buffers", "Kernel", "Count");
    println!("{}", "-".repeat(80));

    for &(entry_name, source) in EW_UNITS {
        let module = wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{entry_name}: parse failed: {}", e.emit_to_string(source)));
        let info = Validator::new(ValidationFlags::all(), Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| {
                panic!(
                    "{entry_name}: validation failed: {}",
                    e.emit_to_string(source)
                )
            });

        let ep_idx = module
            .entry_points
            .iter()
            .position(|ep| ep.name == entry_name)
            .unwrap_or_else(|| panic!("{entry_name}: entry point not found"));
        let ep_info = info.get_entry_point(ep_idx);

        let mut used: Vec<String> = module
            .global_variables
            .iter()
            .filter(|(handle, var)| {
                matches!(var.space, naga::AddressSpace::Storage { .. })
                    && !ep_info[*handle].is_empty()
            })
            .map(|(handle, var)| {
                var.name
                    .clone()
                    .unwrap_or_else(|| format!("unnamed_{handle:?}"))
            })
            .collect();
        used.sort();
        println!("{:<22} {:<6} [{}]", entry_name, used.len(), used.join(", "));
        assert!(
            used.len() <= 8,
            "kernel {entry_name} reached {} storage buffers (> 8 max allowed): {used:?}",
            used.len()
        );

        let (_, expected) = EXPECTED_STORAGE
            .iter()
            .find(|(name, _)| *name == entry_name)
            .unwrap_or_else(|| panic!("{entry_name}: no EXPECTED_STORAGE row"));
        let mut expected: Vec<String> = expected.iter().map(|s| (*s).to_string()).collect();
        expected.sort();
        assert_eq!(
            used, expected,
            "{entry_name}: storage buffers reached by the entry point changed; update the \
             kernel's bind table in chunk/bindings.rs and EXPECTED_STORAGE together"
        );
    }
}

/// The `transport_bounce/08*` pieces (the phase functions), in concatenation order.
fn bounce_phase_text() -> String {
    TRANSPORT_BOUNCE_PIECE_SOURCES
        .iter()
        .filter(|(name, _)| name.starts_with("transport_bounce/08"))
        .map(|&(_, src)| src)
        .collect()
}

/// The text of top-level `fn name(...)` in `text`, up to the next top-level `fn`.
fn fn_text<'a>(text: &'a str, name: &str) -> &'a str {
    let needle = format!("\nfn {name}(");
    let start = text
        .find(&needle)
        .unwrap_or_else(|| panic!("fn {name} not found"));
    let rest = &text[start + 1..];
    let end = rest[3..].find("\nfn ").map_or(rest.len(), |e| e + 3);
    &rest[..end]
}

/// "Verbatim move" as a checkable property: the HG NEE call exists exactly once in the phase
/// functions and sits behind the HDR gate in `bounce_scatter`; the deferred exit-split
/// resolution is the only caller of `try_split_exit_channel`.
#[test]
fn bounce_phases_keep_the_hg_nee_and_the_split_resolution_call_sites() {
    let text = bounce_phase_text().replace("\r\n", "\n");
    assert_eq!(
        text.matches("nee_contribution_hg_scatter(").count(),
        1,
        "the HG scatter NEE must be called exactly once (in bounce_scatter_nee)"
    );
    assert!(
        fn_text(&text, "bounce_scatter_nee").contains("nee_contribution_hg_scatter("),
        "bounce_scatter_nee must call nee_contribution_hg_scatter"
    );
    let scatter = fn_text(&text, "bounce_scatter");
    assert!(
        scatter.contains("params.env_mode == 2u") && scatter.contains("bounce_scatter_nee("),
        "bounce_scatter must run the HG NEE for HDR environments (env_mode == 2)"
    );
    assert!(
        scatter.find("bounce_scatter_nee(") < scatter.find("bounce_scatter_commit("),
        "the HG NEE runs before the commit (pre-roulette stokes)"
    );
    assert_eq!(text.matches("try_split_exit_channel(").count(), 1);
    assert!(
        fn_text(&text, "bounce_resolve_splits").contains("try_split_exit_channel("),
        "try_split_exit_channel is only called from bounce_resolve_splits"
    );
    assert_eq!(text.matches("FACET_FINISH_FROSTED").count(), 2);
}

/// The event kernels split the scatter phase over two dispatches; both must use the same
/// shared pieces the megakernel composes, in the same order.
#[test]
fn event_scatter_kernels_use_the_shared_scatter_pieces() {
    let scatter = include_str!("../shaders/event_wavefront/ew_scatter.wgsl");
    let nee = include_str!("../shaders/event_wavefront/ew_nee_hg.wgsl");
    for piece in [
        "bounce_scatter_applies(",
        "bounce_scatter_alphas(",
        "bounce_scatter_sample(",
        "bounce_scatter_commit(",
    ] {
        assert!(scatter.contains(piece), "ew_scatter lacks {piece}");
    }
    assert!(scatter.contains("TAG_NEE_HG"));
    assert!(
        !scatter.contains("bounce_scatter_nee("),
        "NEE belongs to ew_nee_hg"
    );
    assert!(
        nee.find("bounce_scatter_nee(") < nee.find("bounce_scatter_commit("),
        "ew_nee_hg: NEE first, then the commit"
    );
}

#[test]
fn ew_struct_layouts_match_rust() {
    use crate::renderer::buffers::{EwHit, EwParams, EwRayChan, EwRayCore};
    use core::mem::offset_of;

    let generate_src = include_str!(concat!(env!("OUT_DIR"), "/ew_generate.generated.wgsl"));
    let module = wgsl::parse_str(generate_src).unwrap_or_else(|e| {
        panic!(
            "ew_generate: parse failed: {}",
            e.emit_to_string(generate_src)
        )
    });

    let mut layouter = naga::proc::Layouter::default();
    layouter
        .update(module.to_ctx())
        .expect("layouter update failed");

    let get_struct = |name: &str| {
        for (handle, ty) in module.types.iter() {
            if ty.name.as_deref() == Some(name)
                && let naga::TypeInner::Struct { ref members, .. } = ty.inner
            {
                return (members.clone(), layouter[handle]);
            }
        }
        panic!("struct {name} not found in WGSL module");
    };

    // The WGSL size and every field offset of `name` must equal the Rust twin's.
    let check_struct = |name: &str, rust_size: usize, fields: &[(&str, usize)]| {
        let (members, layout) = get_struct(name);
        assert_eq!(layout.size as usize, rust_size, "{name} size");
        for &(field, rust_offset) in fields {
            let wgsl_offset = members
                .iter()
                .find(|m| m.name.as_deref() == Some(field))
                .map_or_else(
                    || panic!("field `{field}` not found in struct {name}"),
                    |m| m.offset as usize,
                );
            assert_eq!(wgsl_offset, rust_offset, "{name}.{field} offset");
        }
    };

    // EwParams (32 bytes)
    check_struct(
        "EwParams",
        size_of::<EwParams>(),
        &[
            ("chunk_rays", offset_of!(EwParams, chunk_rays)),
            ("bounce", offset_of!(EwParams, bounce)),
            ("finalize_all", offset_of!(EwParams, finalize_all)),
            ("has_scatter", offset_of!(EwParams, has_scatter)),
            ("pad0", offset_of!(EwParams, pad0)),
            ("pad1", offset_of!(EwParams, pad1)),
            ("pad2", offset_of!(EwParams, pad2)),
            ("pad3", offset_of!(EwParams, pad3)),
        ],
    );

    // EwRayCore (128 bytes)
    check_struct(
        "EwRayCore",
        size_of::<EwRayCore>(),
        &[
            ("origin", offset_of!(EwRayCore, origin)),
            ("dir", offset_of!(EwRayCore, dir)),
            ("k", offset_of!(EwRayCore, k)),
            (
                "prev_plane_normal",
                offset_of!(EwRayCore, prev_plane_normal),
            ),
            ("pending_mis_dir", offset_of!(EwRayCore, pending_mis_dir)),
            ("nee_xyz", offset_of!(EwRayCore, nee_xyz)),
            ("observer", offset_of!(EwRayCore, observer)),
            ("ray_meta", offset_of!(EwRayCore, ray_meta)),
        ],
    );

    // EwRayChan (288 bytes)
    check_struct(
        "EwRayChan",
        size_of::<EwRayChan>(),
        &[
            ("stokes", offset_of!(EwRayChan, stokes)),
            ("radiance", offset_of!(EwRayChan, radiance)),
            ("path_pdf", offset_of!(EwRayChan, path_pdf)),
            ("split_radiance", offset_of!(EwRayChan, split_radiance)),
            ("lambdas", offset_of!(EwRayChan, lambdas)),
            ("compat", offset_of!(EwRayChan, compat)),
        ],
    );

    // EwHit (240 bytes)
    check_struct(
        "EwHit",
        size_of::<EwHit>(),
        &[
            ("hit_point", offset_of!(EwHit, hit_point)),
            ("hit_normal", offset_of!(EwHit, hit_normal)),
            ("shading_normal", offset_of!(EwHit, shading_normal)),
            ("wave_dir_at_bounce", offset_of!(EwHit, wave_dir_at_bounce)),
            (
                "phase_dir_this_check",
                offset_of!(EwHit, phase_dir_this_check),
            ),
            ("misc", offset_of!(EwHit, misc)),
            ("split", offset_of!(EwHit, split)),
            ("split_dir", offset_of!(EwHit, split_dir)),
        ],
    );
}
