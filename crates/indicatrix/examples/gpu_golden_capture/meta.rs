//! `meta.json` parsing and cross-capture sanity checks for `gpu_golden_capture compare`.
//!
//! The capture side writes the file by hand (no JSON dependency), so this reads it back
//! with a tiny key search. Two captures are only comparable when they were taken on the
//! same adapter with the same scene parameters; when both used the same pipeline their
//! shader hashes must also agree (a different hash means the shaders changed between the
//! captures, which the owner needs to know before reading a TOL or EXACT verdict).

use std::path::Path;

/// The fields of one capture's `meta.json` that decide whether two captures compare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureMeta {
    /// Adapter name, vendor id, device id, backend and driver, joined for display.
    pub adapter: String,
    /// `megakernel` or `event`.
    pub pipeline: String,
    /// Shader hash text exactly as written (`0x...`).
    pub shader_hash: String,
    /// Per-scene `(width, height, spp, max_bounces, sample_offset)` in file order.
    pub scene_params: Vec<(String, String, String, String, String)>,
}

/// Every value of `"key":` in `json`, in file order (strings unquoted).
fn all_values(json: &str, key: &str) -> Vec<String> {
    let needle = format!("\"{key}\"");
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = json[from..].find(&needle) {
        let after = from + rel + needle.len();
        from = after;
        let rest = json[after..].trim_start();
        let Some(rest) = rest.strip_prefix(':') else {
            continue;
        };
        let rest = rest.trim_start();
        if let Some(quoted) = rest.strip_prefix('"') {
            if let Some(end) = quoted.find('"') {
                out.push(quoted[..end].to_string());
            }
        } else {
            let end = rest.find([',', '\n', '}']).unwrap_or(rest.len());
            out.push(rest[..end].trim().to_string());
        }
    }
    out
}

fn first(json: &str, key: &str) -> String {
    all_values(json, key)
        .into_iter()
        .next()
        .unwrap_or_else(|| "?".to_string())
}

/// Parses the text of a `meta.json` written by `gpu_golden_capture capture`.
#[must_use]
pub fn parse_meta(json: &str) -> CaptureMeta {
    let adapter = format!(
        "{} vendor={} device={} backend={} driver={}",
        first(json, "name"),
        first(json, "vendor"),
        first(json, "device"),
        first(json, "backend"),
        first(json, "driver"),
    );
    let widths = all_values(json, "width");
    let heights = all_values(json, "height");
    let spps = all_values(json, "spp");
    let bounces = all_values(json, "max_bounces");
    let offsets = all_values(json, "sample_offset");
    let scene_params = (0..spps.len())
        .map(|i| {
            let get = |v: &Vec<String>| v.get(i).cloned().unwrap_or_else(|| "?".to_string());
            (
                get(&widths),
                get(&heights),
                get(&spps),
                get(&bounces),
                get(&offsets),
            )
        })
        .collect();
    CaptureMeta {
        adapter,
        pipeline: first(json, "pipeline"),
        shader_hash: first(json, "shader_src_fnv1a64"),
        scene_params,
    }
}

/// Reads and parses `<dir>/meta.json`.
///
/// # Errors
///
/// A readable message when the file is missing or unreadable.
pub fn read_meta(dir: &Path) -> Result<CaptureMeta, String> {
    let path = dir.join("meta.json");
    std::fs::read_to_string(&path)
        .map(|text| parse_meta(&text))
        .map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// Everything that makes a comparison of `a` against `b` unreliable, as human-readable
/// lines. Empty means the captures are comparable. Different pipelines legitimately have
/// different shader hashes, so the hash is only compared within one pipeline.
#[must_use]
pub fn mismatches(a: &CaptureMeta, b: &CaptureMeta) -> Vec<String> {
    let mut out = Vec::new();
    if a.adapter != b.adapter {
        out.push(format!(
            "adapter differs:\n    a: {}\n    b: {}",
            a.adapter, b.adapter
        ));
    }
    if a.scene_params != b.scene_params {
        out.push("scene parameters (size, spp, bounces, sample offset) differ".to_string());
    }
    if a.pipeline == b.pipeline && a.shader_hash != b.shader_hash {
        out.push(format!(
            "shader hash differs within pipeline '{}': a={} b={}",
            a.pipeline, a.shader_hash, b.shader_hash
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(adapter: &str, pipeline: &str, hash: &str, spp: u32) -> String {
        format!(
            "{{\n  \"adapter\": {{\n    \"name\": \"{adapter}\",\n    \"vendor\": 4318,\n    \"device\": 9,\n    \"device_type\": \"DiscreteGpu\",\n    \"backend\": \"Vulkan\",\n    \"driver\": \"d\",\n    \"driver_info\": \"i\"\n  }},\n  \"date\": \"x\",\n  \"pipeline\": \"{pipeline}\",\n  \"shader_src_fnv1a64\": \"{hash}\",\n  \"scenes\": [\n    {{\n      \"name\": \"s1\",\n      \"file\": \"s1.f32\",\n      \"description\": \"d\",\n      \"width\": 32,\n      \"height\": 32,\n      \"spp\": {spp},\n      \"max_bounces\": 8,\n      \"sample_offset\": 0,\n      \"material\": \"Spinel\"\n    }}\n  ]\n}}\n"
        )
    }

    #[test]
    fn parses_adapter_pipeline_hash_and_scene_params() {
        let m = parse_meta(&sample("RTX", "event", "0xabc", 4));
        assert!(
            m.adapter
                .starts_with("RTX vendor=4318 device=9 backend=Vulkan")
        );
        assert_eq!(m.pipeline, "event");
        assert_eq!(m.shader_hash, "0xabc");
        assert_eq!(m.scene_params.len(), 1);
        assert_eq!(m.scene_params[0].2, "4");
    }

    #[test]
    fn identical_captures_have_no_mismatch() {
        let a = parse_meta(&sample("RTX", "event", "0xabc", 4));
        assert!(mismatches(&a, &a.clone()).is_empty());
    }

    #[test]
    fn adapter_and_params_and_same_pipeline_hash_mismatch_are_reported() {
        let a = parse_meta(&sample("RTX", "event", "0xabc", 4));
        let other_adapter = parse_meta(&sample("Radeon", "event", "0xabc", 4));
        let other_spp = parse_meta(&sample("RTX", "event", "0xabc", 8));
        let other_hash = parse_meta(&sample("RTX", "event", "0xdef", 4));
        assert_eq!(mismatches(&a, &other_adapter).len(), 1);
        assert_eq!(mismatches(&a, &other_spp).len(), 1);
        assert_eq!(mismatches(&a, &other_hash).len(), 1);
    }

    #[test]
    fn different_pipelines_do_not_compare_shader_hashes() {
        let mega = parse_meta(&sample("RTX", "megakernel", "0x111", 4));
        let event = parse_meta(&sample("RTX", "event", "0x222", 4));
        assert!(mismatches(&mega, &event).is_empty());
    }
}
