#!/usr/bin/env python3
"""Generates the constant-index unrolled 4x4 complex Gauss-Jordan solvers.

Rewrites the region between the markers

    // BEGIN GENERATED solve4 (tools/unroll_solve4.py)
    // END GENERATED solve4

in `src/renderer/shaders/transport_physics/06_uniaxial_fullwave_complex.wgsl`
with `solve4_single`, `Solve4TwoRhsResult` and `solve4_two_rhs`.

Why: the rolled originals index the private matrix with a data-dependent row
(`a[piv][j]`). The AMD DXIL->ISA driver compile of that construct is the dominant
browser compile cost of the uniaxial kernels (docs: web_gpu_kernel_research_2026-09-28.md
section 2). Every index below is a compile-time constant.

Bit-identity by construction (same IEEE operations, same order, as the rolled code):
  * pivot search: ascending rows, strict `>` against the running maximum;
  * row swap: at most one conditional swap of whole rows (pure moves, no arithmetic);
  * row `col` is divided by the pivot (a copy taken after the swap) for j in col..3;
  * elimination: ascending rows, skipping row == col and any factor with
    `cplx_norm_sqr(factor) == 0.0` (written as `!= 0.0`, so NaN factors still eliminate,
    exactly like the rolled `continue`), for j in col..3, then the right-hand sides.

Usage (run from anywhere):
    python tools/unroll_solve4.py            # rewrite the WGSL file in place
    python tools/unroll_solve4.py --check    # exit 1 if the file is out of date
    python tools/unroll_solve4.py --print    # print the generated text only
"""

import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
WGSL = (
    HERE.parent
    / "src"
    / "renderer"
    / "shaders"
    / "transport_physics"
    / "06_uniaxial_fullwave_complex.wgsl"
)

BEGIN = "// BEGIN GENERATED solve4 (tools/unroll_solve4.py)"
END = "// END GENERATED solve4"
# First-run anchor: the comment line directly above the original rolled `solve4_single`.
LEGACY_START = "// optics::raytracer::uniaxial_fresnel::solve4 -- Gauss-Jordan, single RHS."


def pivot_and_swap(col, rhs):
    """Pivot search (strict >, ascending rows) plus one conditional whole-row swap."""
    out = []
    out.append(f"    // column {col}: pivot search")
    out.append(f"    var piv{col}: u32 = {col}u;")
    out.append(f"    var piv_mag{col}: f32 = cplx_norm_sqr(a[{col}][{col}]);")
    for row in range(col + 1, 4):
        out.append(f"    let mag{col}_{row} = cplx_norm_sqr(a[{row}][{col}]);")
        out.append(f"    if (mag{col}_{row} > piv_mag{col}) {{")
        out.append(f"        piv{col} = {row}u;")
        out.append(f"        piv_mag{col} = mag{col}_{row};")
        out.append("    }")
    for row in range(col + 1, 4):
        out.append(f"    if (piv{col} == {row}u) {{")
        for j in range(4):
            out.append(f"        let t{col}_{row}_{j} = a[{col}][{j}];")
            out.append(f"        a[{col}][{j}] = a[{row}][{j}];")
            out.append(f"        a[{row}][{j}] = t{col}_{row}_{j};")
        for name in rhs:
            out.append(f"        let tb{col}_{row}_{name} = {name}[{col}];")
            out.append(f"        {name}[{col}] = {name}[{row}];")
            out.append(f"        {name}[{row}] = tb{col}_{row}_{name};")
        out.append("    }")
    return out


def normalise_and_eliminate(col, rhs):
    """Divide row `col` by the pivot, then eliminate the column from every other row."""
    out = []
    out.append(f"    let pivot{col} = a[{col}][{col}];")
    for j in range(col, 4):
        out.append(f"    a[{col}][{j}] = cplx_div(a[{col}][{j}], pivot{col});")
    for name in rhs:
        out.append(f"    {name}[{col}] = cplx_div({name}[{col}], pivot{col});")
    for row in range(4):
        if row == col:
            continue
        out.append(f"    let factor{col}_{row} = a[{row}][{col}];")
        out.append(f"    if (cplx_norm_sqr(factor{col}_{row}) != 0.0) {{")
        for j in range(col, 4):
            out.append(
                f"        a[{row}][{j}] = cplx_sub(a[{row}][{j}], "
                f"cplx_mul(factor{col}_{row}, a[{col}][{j}]));"
            )
        for name in rhs:
            out.append(
                f"        {name}[{row}] = cplx_sub({name}[{row}], "
                f"cplx_mul(factor{col}_{row}, {name}[{col}]));"
            )
        out.append("    }")
    return out


def body(rhs):
    out = []
    for col in range(4):
        out.extend(pivot_and_swap(col, rhs))
        out.extend(normalise_and_eliminate(col, rhs))
    return out


def generate():
    lines = [BEGIN]
    lines.append("// Constant-index unrolled Gauss-Jordan: same arithmetic, same order, same pivoting as")
    lines.append("// the rolled form (strict `>`, ascending rows, whole-row conditional swap, `!= 0.0`")
    lines.append("// factor skip). DO NOT EDIT BY HAND: regenerate with")
    lines.append("//     python crates/indicatrix/tools/unroll_solve4.py")
    lines.append("// (`--check` verifies this region is current.)")
    lines.append("")
    lines.append("// optics::raytracer::uniaxial_fresnel::solve4 -- Gauss-Jordan, single RHS.")
    lines.append(
        "fn solve4_single(a_in: array<array<Cplx, 4>, 4>, b_in: array<Cplx, 4>) -> array<Cplx, 4> {"
    )
    lines.append("    var a = a_in;")
    lines.append("    var b = b_in;")
    lines.extend(body(["b"]))
    lines.append("    return b;")
    lines.append("}")
    lines.append("")
    lines.append("// optics::raytracer::uniaxial_fresnel::solve4_two_rhs")
    lines.append("struct Solve4TwoRhsResult {")
    lines.append("    b1: array<Cplx, 4>,")
    lines.append("    b2: array<Cplx, 4>,")
    lines.append("}")
    lines.append("")
    lines.append(
        "fn solve4_two_rhs(a_in: array<array<Cplx, 4>, 4>, b1_in: array<Cplx, 4>, "
        "b2_in: array<Cplx, 4>) -> Solve4TwoRhsResult {"
    )
    lines.append("    var a = a_in;")
    lines.append("    var b1 = b1_in;")
    lines.append("    var b2 = b2_in;")
    lines.extend(body(["b1", "b2"]))
    lines.append("    var result: Solve4TwoRhsResult;")
    lines.append("    result.b1 = b1;")
    lines.append("    result.b2 = b2;")
    lines.append("    return result;")
    lines.append("}")
    lines.append(END)
    return "\n".join(lines) + "\n"


def splice(text, gen):
    if BEGIN in text:
        s = text.index(BEGIN)
        e = text.index(END) + len(END)
        # Consume the newline after END.
        if text[e : e + 1] == "\n":
            e += 1
        return text[:s] + gen + text[e:]
    # First run: the two rolled functions are the tail of the file.
    s = text.index(LEGACY_START)
    tail = text[s:].rstrip()
    if not tail.endswith("return result;\n}"):
        raise SystemExit("unexpected tail in WGSL file; refusing to rewrite")
    return text[:s] + gen + "\n"


def main():
    gen = generate()
    if "--print" in sys.argv:
        sys.stdout.write(gen)
        return
    with open(WGSL, encoding="utf-8", newline="") as f:
        text = f.read()
    crlf = "\r\n" in text
    norm = text.replace("\r\n", "\n")
    new = splice(norm, gen)
    if "--check" in sys.argv:
        if new != norm:
            print("06_uniaxial_fullwave_complex.wgsl: generated region is stale")
            sys.exit(1)
        print("up to date")
        return
    if crlf:
        new = new.replace("\n", "\r\n")
    with open(WGSL, "w", encoding="utf-8", newline="") as f:
        f.write(new)
    print(f"wrote {WGSL}")


if __name__ == "__main__":
    main()
