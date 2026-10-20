//! Tests of the concave-stone GPU self-tests; see the parent module.

use super::*;
use crate::renderer::gpu::merge_tests::renderer_or_skip;

/// Environment variable that turns a missing adapter into a failure.
const REQUIRE_GPU_ENV: &str = "INDICATRIX_REQUIRE_GPU";

fn context_or_skip(test_name: &str) -> Option<GpuContext> {
    match GpuContext::acquire() {
        Ok(ctx) => Some(ctx),
        Err(error) => {
            assert!(
                !std::env::var(REQUIRE_GPU_ENV).is_ok_and(|v| v == "1"),
                "{test_name}: {REQUIRE_GPU_ENV}=1 but no usable GPU adapter: {error}"
            );
            println!("skipping {test_name}: no GPU adapter");
            None
        }
    }
}

#[test]
fn scenarios_cover_every_kind_and_sweep() {
    let all = scenarios();
    assert_eq!(all.len(), KINDS.len() * SWEEPS.len() + 1);
    for (name, tools) in &all {
        for tool in tools {
            assert_eq!(tool.validate(), Ok(()), "{name}");
        }
    }
}

/// Per tool primitive (cylinder, cone, ball, disc; plunge and both reciprocating
/// sweeps) the megakernel's intersection, containment and shading-normal agree with the
/// CPU, one scene of mixed tools included.
#[test]
fn gpu_tool_intersection_matches_cpu_for_every_primitive() {
    let Some(ctx) = context_or_skip("gpu_tool_intersection_matches_cpu_for_every_primitive") else {
        return;
    };
    let result = run(&ctx);
    println!(
        "tool check: {} compared, {} knife-edge excluded, {} tool hits, {} mismatches",
        result.compared, result.excluded_knife_edge, result.tool_hits, result.total_mismatches
    );
    assert!(result.passed(), "{:#?}", result.mismatches);
    assert!(
        result.tool_hits > 1_500,
        "the scenarios must exercise tool surfaces ({} hits)",
        result.tool_hits
    );
    assert!(
        result.excluded_knife_edge * 10 < result.compared,
        "too many knife-edge exclusions: {} of {}",
        result.excluded_knife_edge,
        result.compared
    );
}

/// A concave stone rendered through the whole megakernel path shows the CPU's tool
/// effect: cut minus flat agrees within 25 %, the cut is not the flat stone, and the
/// total brightness agrees within 3 %.
#[test]
fn gpu_concave_image_reproduces_the_cpu_tool_effect() {
    let Some(mut renderer) = renderer_or_skip("gpu_concave_image_reproduces_the_cpu_tool_effect")
    else {
        return;
    };
    let r = run_image_comparison(&mut renderer).expect("the GPU renders the concave scene");
    println!("{r:?}");
    let effect = r.cpu_effect();
    assert!(
        effect.abs() > 0.01 * r.cpu_flat_mean,
        "test premise: the tools must visibly change the CPU image ({r:?})"
    );
    assert!(
        (r.gpu_effect() - effect).abs() <= 0.25 * effect.abs(),
        "GPU tool effect {} vs CPU {effect} ({r:?})",
        r.gpu_effect()
    );
    assert!(
        (r.gpu_cut_mean - r.cpu_cut_mean).abs() <= 0.03 * r.cpu_cut_mean,
        "GPU cut mean vs CPU ({r:?})"
    );
}
