//! Adapter-free checks of the actual generated small kernel and its Rust layout.
use crate::renderer::buffers::whitted::GpuWhittedParams;
use core::mem::offset_of;
use naga::{
    front::wgsl,
    valid::{Capabilities, ValidationFlags, Validator},
};
const SOURCE: &str = include_str!(concat!(env!("OUT_DIR"), "/whitted.generated.wgsl"));

#[test]
fn whitted_layout_and_storage_budget_match_the_host() {
    let module =
        wgsl::parse_str(SOURCE).unwrap_or_else(|error| panic!("{}", error.emit_to_string(SOURCE)));
    let info = Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .expect("valid small renderer");
    let entry = module
        .entry_points
        .iter()
        .position(|entry| entry.name == "whitted_main")
        .expect("entry");
    let function = info.get_entry_point(entry);
    let mut storage: Vec<&str> = module
        .global_variables
        .iter()
        .filter_map(|(handle, global)| {
            if matches!(global.space, naga::AddressSpace::Storage { .. })
                && !function[handle].is_empty()
            {
                global.name.as_deref()
            } else {
                None
            }
        })
        .collect();
    storage.sort_unstable();
    assert_eq!(storage, ["out_xyz", "wt_env", "wt_frost", "wt_planes"]);
    let mut layout = naga::proc::Layouter::default();
    layout.update(module.to_ctx()).expect("layout");
    let (handle, ty) = module
        .types
        .iter()
        .find(|(_, ty)| ty.name.as_deref() == Some("GpuWhittedParams"))
        .expect("uniform type");
    let naga::TypeInner::Struct { members, .. } = &ty.inner else {
        panic!("uniform must be struct")
    };
    assert_eq!(layout[handle].size as usize, size_of::<GpuWhittedParams>());
    let fields = [
        ("counts", offset_of!(GpuWhittedParams, counts)),
        ("range", offset_of!(GpuWhittedParams, range)),
        ("modes", offset_of!(GpuWhittedParams, modes)),
        ("values", offset_of!(GpuWhittedParams, values)),
        ("indices", offset_of!(GpuWhittedParams, indices)),
        ("alpha_o", offset_of!(GpuWhittedParams, alpha_o)),
        ("alpha_e", offset_of!(GpuWhittedParams, alpha_e)),
        ("axis", offset_of!(GpuWhittedParams, axis)),
        ("masks", offset_of!(GpuWhittedParams, masks)),
        ("rgb_to_xyz", offset_of!(GpuWhittedParams, rgb_to_xyz)),
        ("white_balance", offset_of!(GpuWhittedParams, white_balance)),
        ("env_rgb", offset_of!(GpuWhittedParams, env_rgb)),
        ("key_dir", offset_of!(GpuWhittedParams, key_dir)),
        ("fill_dir", offset_of!(GpuWhittedParams, fill_dir)),
        ("studio", offset_of!(GpuWhittedParams, studio)),
        ("shadow", offset_of!(GpuWhittedParams, shadow)),
        ("tent", offset_of!(GpuWhittedParams, tent)),
        ("extra", offset_of!(GpuWhittedParams, extra)),
        ("aa", offset_of!(GpuWhittedParams, aa)),
        ("ring_dirs", offset_of!(GpuWhittedParams, ring_dirs)),
    ];
    for (name, offset) in fields {
        assert_eq!(
            members
                .iter()
                .find(|member| member.name.as_deref() == Some(name))
                .expect("field")
                .offset as usize,
            offset,
            "{name}"
        );
    }
}

#[test]
fn the_probe_uses_the_production_physics_and_validates() {
    let source = format!(
        "{SOURCE}\n{}",
        include_str!("../shaders/whitted/probe.wgsl")
    );
    let module = wgsl::parse_str(&source)
        .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&source)));
    Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .expect("valid probe");
}
