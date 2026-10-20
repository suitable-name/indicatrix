"""Writes Rough Planner scenario copies of the desktop ui/ with sample data as the
RoughPlanModel defaults, for slint-viewer screenshots.

Usage: python -I rough_planner.py <app_dir> <out_dir>

<app_dir> is apps/indicatrix-cut; one folder per scenario is created under <out_dir>,
each holding a patched ui/ and assets/icon.png.
Fails loudly (non-zero exit, message on stderr) when a patched property no longer
exists in ui/models/rough_plan.slint, so a renamed property cannot silently turn a
scenario into an empty screenshot.
"""
import re
import shutil
import sys
from pathlib import Path

if len(sys.argv) != 3:
    raise SystemExit("usage: rough_planner.py <app_dir> <out_dir>")

app = Path(sys.argv[1])
out = Path(sys.argv[2])

GROUP = ('{{ entry_id: {id}, name: "{name}", count: {count}, detail: "{detail}", swatch: {sw}, '
         'has_thumbnail: false, metrics: [{{ label: "Brilliance", value: "71 %" }}, {{ label: "Fire", value: "12.4" }}], '
         'status: "{status}", status_level: {lvl}, linkable: {link}, excluded: {exc} }}')


def group(i, name, count, detail, sw, status="", lvl=0, link="true", exc="false"):
    return GROUP.format(id=i, name=name, count=count, detail=detail, sw=sw, status=status, lvl=lvl, link=link, exc=exc)


CUT_PLAN = "1. Saw parallel to the top face, 2.1 mm down\\n2. Saw the left piece in half along X\\n3. Preform each piece to the outline"
RESULTS = "[" + ", ".join([
    '{ rank: 1, total_ct: "3.71 ct", yield_pct: "41.2 %", stone_count: 3, groups: ['
    + group(101, "Standard Round Brilliant 57", 2, "5.10 x 5.10 x 3.40 mm, 0.62 ct each", "#38bdf8") + ", "
    + group(202, "Oval Brilliant With A Rather Long Design Title For Elision", 1, "7.20 x 5.30 x 3.90 mm, 1.21 ct", "#f59e0b", "design changed since saved", 2)
    + f'], cut_plan: "{CUT_PLAN}", yield_weight_text: "3.71 of 9.00 ct weighed", saw_text: "5 cuts · kerf loss <= 0.21 ct", has_thumbnail: false, keep: true }}',
    '{ rank: 2, total_ct: "3.52 ct", yield_pct: "39.1 %", stone_count: 2, groups: ['
    + group(303, "Cushion Brilliant", 2, "6.00 x 6.00 x 4.10 mm, 0.88 ct each", "#a78bfa", "design deleted", 3, "false")
    + f'], cut_plan: "{CUT_PLAN}", yield_weight_text: "", saw_text: "3 cuts · kerf loss <= 0.12 ct", has_thumbnail: false, keep: false }}',
    '{ rank: 3, total_ct: "3.10 ct", yield_pct: "34.4 %", stone_count: 5, groups: ['
    + group(404, "Emerald Step Cut", 4, "4.00 x 3.00 x 2.20 mm, 0.31 ct each", "#34d399", "matched by title", 1) + ", "
    + group(505, "Pear Brilliant", 1, "6.00 x 4.00 x 2.70 mm, 0.56 ct", "#f472b6", exc="true")
    + f'], cut_plan: "{CUT_PLAN}", yield_weight_text: "", saw_text: "7 cuts · kerf loss <= 0.34 ct", has_thumbnail: false, keep: false }}',
]) + "]"

CUTS = ("[" + ", ".join([
    '{ kind: 0, title: "Edge Top-Front", hint: "", a: "1.5", b: "2.0", c: "", azimuth: "", elevation: "", depth: "", removed_text: "removes 84 mm³ (1.11 ct)", error: "", face_index: 0 }',
    '{ kind: 1, title: "Corner Top-Front-Left", hint: "", a: "1.0", b: "1.0", c: "1.2", azimuth: "", elevation: "", depth: "", removed_text: "removes 12 mm³ (0.16 ct)", error: "", face_index: 2 }',
    '{ kind: 2, title: "Face", hint: "azimuth has no effect at elevation ±90", a: "", b: "", c: "", azimuth: "35", elevation: "90", depth: "0.8", removed_text: "", error: "The face misses the rough: lower the depth.", face_index: -1 }',
]) + "]")

EDGES = "[" + ", ".join(f'"Edge {n}"' for n in ["Top-Front", "Top-Back", "Top-Left", "Top-Right", "Bottom-Front", "Bottom-Back", "Bottom-Left", "Bottom-Right", "Front-Left", "Front-Right", "Back-Left", "Back-Right"]) + "]"
CORNERS = "[" + ", ".join(f'"Corner {n}"' for n in ["TFL", "TFR", "TBL", "TBR", "BFL", "BFR", "BBL", "BBR"]) + "]"
SAVED = ('[{ id: 1, name: "Quartz block 18 x 11", summary: "10 layouts, best 3.71 ct", date: "2026-10-12 14:03" }, '
         '{ id: 2, name: "A saved plan with a name long enough to need eliding in the list", summary: "4 layouts, best 1.20 ct", date: "2026-10-01 09:41" }]')

COMMON = {
    "materials": '["Quartz (SG 2.65)", "Beryl (SG 2.72)", "Tourmaline (SG 3.06)"]',
    "rough_x": '"18.4"', "rough_y": '"11.0"', "rough_z": '"9.6"', "weighed_ct": '"9.00"',
    "max_designs": "3", "max_stones": "8", "filter_count": "1234", "library_count": "3299",
    "filter_count_text": '"1,234"', "library_count_text": '"3,299"',
    "model_text": '"Model 1,944 mm³ · 25.76 ct (Quartz, SG 2.65)"',
    "weight_check_text": '"Weighed 9.00 ct vs model 25.76 ct: the model is far too big"', "weight_check_level": "3",
    "edge_options": EDGES, "corner_options": CORNERS, "can_undo": "true",
}

SCENARIOS = {
    "results": {**COMMON,
                "results": RESULTS, "summary": '"10 layouts from 2,431 designs -- 3.2 s"', "skipped_count": "7",
                "results_unsaved": "true", "keep_count": "1", "selected_result": "0", "view_mode": "1",
                "expanded_index": "0", "status_text": '"The rough changed since this plan: re-plan to update it."', "status_level": "1",
                "losses_open": "true", "excluded_ids": "[505, 606]", "excluded_names": '["Pear Brilliant", "Marquise"]',
                "excluded_count": "2", "excluded_open": "true", "saved_plans": SAVED, "saved_open": "true",
                "thumbnails_pending": "true"},
    "loaded": {**COMMON, "results": RESULTS, "summary": '"10 layouts from 2,431 designs -- 3.2 s"',
               "loaded_banner": '"Showing the saved plan \\"Quartz block 18 x 11\\" (2026-10-12 14:03)"', "links_enabled": "false"},
    "cuts_mesh": {**COMMON, "cuts": CUTS, "selected_cut": "2", "base_kind": "3", "inclusions_possible": "true",
                  "inclusion_rows": '["Inclusion 1: 4.6 x 4.6 x 4.6 mm, 97.3 mm³", "Inclusion 2: 1.2 x 0.8 x 3.1 mm, 2.9 mm³"]',
                  "locate_possible": "true", "model_error": '"The cuts leave nothing of the rough."',
                  "error_text": '"Rough Y is not a number: \\"11,0\\"."'},
    "running": {**COMMON, "cuts": CUTS, "running": "true", "progress_fraction": "0.42",
                "stage": '"Planning: cut orders (3 of 6 finished)"', "mesh_busy": "true",
                "mesh_busy_text": '"Reading the OBJ file..."', "view_busy": "true"},
    "save_rows": {**COMMON, "results": RESULTS, "save_name_open": "true", "save_name": '"Quartz block 18 x 11 (2)"',
                  "confirm_replace_open": "true", "confirm_replace_count": "10", "face_from_view_armed": "true"},
}

model_rel = Path("ui/models/rough_plan.slint")
for name, values in SCENARIOS.items():
    dest = out / name
    if dest.exists():
        shutil.rmtree(dest)
    shutil.copytree(app / "ui", dest / "ui")
    (dest / "assets").mkdir(parents=True)
    shutil.copy(app / "assets" / "icon.png", dest / "assets" / "icon.png")
    path = dest / model_rel
    text = path.read_text(encoding="utf-8")
    for prop, value in values.items():
        pattern = re.compile(r"(property <[^>]+> " + re.escape(prop) + r")(?:: [^;]*)?;")
        text, n = pattern.subn(lambda m: f"{m.group(1)}: {value};", text, count=1)
        if n != 1:
            raise SystemExit(f"{name}: property {prop} not found in {model_rel}")
    path.write_text(text, encoding="utf-8")
    print(name, "ok")
