// Layout self-test shader for the zone table (`zoning` feature only). Standalone test
// target: `renderer::gpu::layout_check::run_zone_table` concatenates this file with
// `01_zone_table.wgsl` (which declares the structs), uploads a populated `GpuZoneTable`, has
// this shader copy every NAMED field (never the `_pad` fields) into a second buffer, and
// compares the raw bytes -- the same mechanism and the same reasoning as `layout_echo.wgsl`
// for `GpuGemMaterial`.

struct AbsorptionBand {
    center_nm: f32,
    width_nm: f32,
    peak: f32,
    shape: u32,
}

@group(0) @binding(0) var<storage, read> zone_in: GpuZoneTable;
@group(0) @binding(1) var<storage, read_write> zone_out: GpuZoneTable;

fn echo_bands(zi: u32, which: u32) {
    for (var b: u32 = 0u; b < 8u; b = b + 1u) {
        if (which == 0u) {
            zone_out.absorption[zi].o_ray_bands[b].center_nm = zone_in.absorption[zi].o_ray_bands[b].center_nm;
            zone_out.absorption[zi].o_ray_bands[b].width_nm = zone_in.absorption[zi].o_ray_bands[b].width_nm;
            zone_out.absorption[zi].o_ray_bands[b].peak = zone_in.absorption[zi].o_ray_bands[b].peak;
            zone_out.absorption[zi].o_ray_bands[b].shape = zone_in.absorption[zi].o_ray_bands[b].shape;
        } else if (which == 1u) {
            zone_out.absorption[zi].e_ray_bands[b].center_nm = zone_in.absorption[zi].e_ray_bands[b].center_nm;
            zone_out.absorption[zi].e_ray_bands[b].width_nm = zone_in.absorption[zi].e_ray_bands[b].width_nm;
            zone_out.absorption[zi].e_ray_bands[b].peak = zone_in.absorption[zi].e_ray_bands[b].peak;
            zone_out.absorption[zi].e_ray_bands[b].shape = zone_in.absorption[zi].e_ray_bands[b].shape;
        } else {
            zone_out.absorption[zi].beta_ray_bands[b].center_nm = zone_in.absorption[zi].beta_ray_bands[b].center_nm;
            zone_out.absorption[zi].beta_ray_bands[b].width_nm = zone_in.absorption[zi].beta_ray_bands[b].width_nm;
            zone_out.absorption[zi].beta_ray_bands[b].peak = zone_in.absorption[zi].beta_ray_bands[b].peak;
            zone_out.absorption[zi].beta_ray_bands[b].shape = zone_in.absorption[zi].beta_ray_bands[b].shape;
        }
    }
}

@compute @workgroup_size(1)
fn main() {
    zone_out.header.zone_count = zone_in.header.zone_count;
    zone_out.header.softness = zone_in.header.softness;
    zone_out.header.soft_subdiv = zone_in.header.soft_subdiv;
    zone_out.header.origin = zone_in.header.origin;
    zone_out.header.row0 = zone_in.header.row0;
    zone_out.header.row1 = zone_in.header.row1;
    zone_out.header.row2 = zone_in.header.row2;
    for (var i: u32 = 0u; i < 4u; i = i + 1u) {
        zone_out.shapes[i].kind = zone_in.shapes[i].kind;
        zone_out.shapes[i].n_sides = zone_in.shapes[i].n_sides;
        zone_out.shapes[i].flags = zone_in.shapes[i].flags;
        zone_out.shapes[i].p = zone_in.shapes[i].p;
        zone_out.shapes[i].d = zone_in.shapes[i].d;
        zone_out.shapes[i].u = zone_in.shapes[i].u;
        zone_out.shapes[i].v = zone_in.shapes[i].v;
        zone_out.shapes[i].x = zone_in.shapes[i].x;
        zone_out.shapes[i].e = zone_in.shapes[i].e;
        zone_out.absorption[i].o_ray_band_count = zone_in.absorption[i].o_ray_band_count;
        zone_out.absorption[i].e_ray_band_count = zone_in.absorption[i].e_ray_band_count;
        zone_out.absorption[i].has_beta_ray = zone_in.absorption[i].has_beta_ray;
        zone_out.absorption[i].beta_ray_band_count = zone_in.absorption[i].beta_ray_band_count;
        echo_bands(i, 0u);
        echo_bands(i, 1u);
        echo_bands(i, 2u);
    }
}
