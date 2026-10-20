# L13/L14 Documentation and Website

DONE: Root and renderer READMEs distinguish the two engines and scope spectral
claims to Path-Traced. Desktop manual chapters cover all five Edit modes, three
Live modes, key 5, startup reset to Solid, AA/Fire, thumbnail and Compare choices,
export, format-2 frozen jobs, CLI engine overrides, approximation limits and
protocol-28 remote rendering. Export and remote reference docs were updated.

DONE: Browser READMEs cover the fixed tap target, no Ray-Traced denoising, all
material classes on Ray-Traced GPU, independent admission and four-binding
eligibility. Android README describes quality-to-tap mapping, 16-tap export,
1,048,576 Ray-Traced GPU pixels versus 262,144 Path-Traced pixels, CPU fallback
and protocol-28 peers. NET README now names version 28 and the final EngineSpec
wire field. Worker README final edits are owned by the lead.

DONE: Nine proposed website pages and a unified review diff are staged under
`site-update/`: index, editor, web, mobile, comparison, physics, gallery, project
and distributed. Targeted existing text and export/job/CLI/matrix/caption lines
were revised; each page also explains the Ray-Traced approximation. Old figures
are explicitly historical. Competitor Ray-Traced cells remain unknown. No image,
performance, device acceptance or completed GPU qualification is claimed.

DONE: After filesystem approval, the lead applied the nine reviewed replacements
to `C:/temp/mcp2/ghp`. Hash checks confirmed the copied bytes match staging.
Nothing was published. Metadata remained unchanged, so SEO generation was not
necessary. The staging copy (`site-update/`) was removed on 2026-10-20 after the site was updated.

Source verification: EngineSpec/RayOptions and WhittedPrep limits; browser worker
protocol 23, engine settings, Ray admission and minimum-four-binding init;
Android quality/options/report implementations; desktop renderer picker,
Advanced settings, view palette and Compare; render-job format_for_engine and
CLI engine parser; the lead's protocol-28 SceneState and remote routes. The final
remote implementation is being reviewed by the lead, who should reconcile any
last changes with the prose before release.

Source-only Slint operator audit found no ambiguous width-image_w, width-self,
height-self, mouse-y- or dimension-minus-number expressions in the browser,
export-dialog and parameter-panel changes. Valid hyphenated properties such as
root.height-limit are retained.

No build, test, Clippy, cargo, git, GPU probe or device run was executed. Standalone
rustfmt was authorized and used earlier. Every owned changed Rust file remains
under 800 lines; see `final-line-counts.md`. Plan performance budgets and all image,
driver, mobile and remote acceptance gates remain owner-run work.
