# 2. Browsing the Catalogue and Viewing a Design

## What you will do

This chapter covers searching and filtering your design library, selecting
a design, reading the 3D render viewport and its controls, and using the
tilt-performance graph to judge how a cut behaves as it tilts.

## Searching and filtering

The catalogue panel on the left lists every design matching your current
search and filters, and its header reads "Catalog (N of M)" — how many
match versus how many exist in total. If nothing matches, it shows "No
diagrams found / Try adjusting your search terms or filters."

- **Search box** (top toolbar): matches against a design's title and
  designer. Its own hint text also mentions notes, but a note is not
  actually part of the match yet — see Chapter 12. A small "×" appears once
  you've typed something, to clear it in one click.
- **Shape** and **Gear** drop-downs: filter to a specific shape
  classification or index-gear size.
- **Filters** button: opens the Advanced Filters panel (below). It turns
  amber with a dot when a filter is active but the panel is closed, so an
  active filter is never invisible.

### Advanced Filters panel

Click **Filters** to open it. From top to bottom:

1. A live count: "N of M designs match."
2. Four range sliders, each spanning the *actual* range present in your
   catalogue (not a fixed scale): **Refractive Index**, **L/W Ratio**,
   **Volume (vol/w³)**, **Facet Count**. Drag either handle; dragging one
   past the other pushes it along.
3. **RI Match (current material)** — a second, independent way to filter
   by refractive index: instead of a min/max range, this centres a
   tolerance band on a chosen value ("Centre X ± Y"). The **Use current
   material** button snaps the centre to whatever material is currently
   loaded in the 3D viewport, so you can quickly ask "what else in my
   library has roughly this stone's RI?"
4. **Show ignored designs** — a toggle to include designs you have marked
   Ignored (see the catalogue card's right-click menu, below) in your
   results. Off by default; ignored rows that are shown render visually
   distinct (dimmed, amber-bordered).
5. **Tilt Performance** filters — narrow the catalogue by how a design
   actually performs when tilted, e.g. "Windowing at most 20% (worst,
   ±45°)." Build one with the metric drop-down (Brilliance / Extinction /
   Windowing), the "at most" / "at least" choice, a threshold percentage
   slider, a tilt-radius slider, and "worst" or "mean" as the aggregate,
   then click **+ Add Filter**. If any active tilt filter is excluding
   designs that have no computed tilt curve yet, an amber notice tells you
   how many and offers a **Compute missing tilt curves** button to fill
   that gap for the whole catalogue in the background.
6. **Reset Filters** — snaps every slider back to its full, unfiltered
   range and clears the toggles above.

A slider sitting exactly on its own full range's edge counts as
unfiltered on that side — this is what makes Reset genuinely clear
everything, including designs with no recorded value for that attribute.

## Selecting and inspecting a design

Click a card in the catalogue to select it. Each card shows two small
preview thumbnails (front and top view, once generated — otherwise a
placeholder "F"/"T"), the title, an optional shape badge, the designer, and
the gear/facet count. **Right-click** a card for:

- **Generate Previews** — render this design's own front/top thumbnails.
- **Compute Tilt Curves** — compute this one design's tilt-performance data
  (see below) ahead of time.
- **Ignore** / **Un-ignore** — hide a design from ordinary search results
  (still visible in the Advanced Filters "Show ignored designs" mode).

Once selected, the detail header above the tabs shows the title, designer,
and a row of spec chips (Shape, Gear, Facets, L/W, H/W, C/W, P/W, Vol/W³,
R.I.) — each chip only appears when that value is known. Local designs get
an inline pencil to rename, an **Edit Metadata** button for the full set of
fields, and a **Delete** button (with a confirm step). Below the header are
three tabs: **3D Spectral Preview (1)**, **Cutting Schedule (2)**, and
**Files & Downloads (3)** — covered further in Chapters 1 and 3.

## The 3D viewport

Open the **3D Spectral Preview** tab's **Live Render** sub-tab.

**Camera and light controls:**

| Action | Effect |
|---|---|
| Left-drag | Orbit the camera around the stone, freely: over the table, under the culet, all the way round |
| **Front** / **Top** buttons | Snap to the canonical poses: girdle edge-on with index 0 towards you, or straight down onto the table. Distance and lighting stay as they were |
| Right-drag, or Shift + left-drag | Move the light |
| Scroll | Zoom |
| Click the metrics readout | Copy the current metrics to the clipboard |

(Shift + left-drag exists because some laptop trackpads cannot produce a
genuine right-drag gesture.)

**Toolbar controls:**

- **Render Material** — choose the gem material the render simulates
  optically (colour, dispersion, birefringence). This changes only the
  *appearance* of the render, never the cut itself — see Chapter 6 for the
  distinction between this and any material-like control in the editor.
  (Labelled just "Material" before this control's own hover tooltip and
  caption were added to disambiguate it from the editor's unrelated Yield
  Material.) On an `editor`-enabled build, a small **"Linked to design"**
  toggle sits next to it, on by default: while on, this dropdown follows the
  Edit tab's own Design Settings material automatically (Chapter 6), so what
  you render always matches what you are editing. Turn it off to pick an
  independent render material without touching the design at all.
- A pencil button next to Render Material opens the **Material Editor**, where
  you can define a fully custom material (name, refractive index,
  dispersion, birefringence, colour swatch, crystal system, and optical
  character) starting from one of the built-in templates.
- **Lighting** — seven presets in two families:
  - **Studio** (the analytic studio rig): **D65 Daylight (6500K)**, **Incandescent
    (3200K)**, **Gem Studio Ring Lights** and **Dramatic Dark Spotlight**. A dark velvet
    backdrop with a key softbox, a fill and sixteen ring pinpoints: hard sparkle,
    clipped highlights, black everywhere else.
  - **Lit models** — what a stone looks like in a real scene. All three darken the
    facets that would reflect your own head, the way a face-up stone really shows a
    dark table:
    - **ISO hemisphere (GemRay-style)** — the whole sky above the girdle evenly lit,
      black below. The classic GemRay/GCS light model for comparing light-return
      patterns.
    - **Light tent + black cards** — the default. A jewellery light tent: grey walls,
      one broad overhead softbox, three black cards for facet contrast and a small hard
      spark light for scintillation. The softbox follows the light azimuth/elevation
      controls; the cards sit 90°, 180° and 270° around from it.
    - **Daylight sky + sun** — a clear sky, brighter towards the horizon and around the
      sun, a 2° sun for fire, dark ground.

  At exposure 1× the lit models put their ambient light near middle grey, so only a
  direct reflection of a light source clips to white; use Studio Exposure to go darker
  or brighter.
- **Backdrop** (Settings gear, under Studio Exposure) — what the camera sees behind
  the stone: **As lit** (the environment's own ground), **GemRay grey** (the default:
  the neutral canvas GemRay paints, for like-for-like comparisons) or **White**. Only
  the camera sees it; the stone's optics never do, so leakage and windows stay dark.
- **Tilt Curve** — opens the tilt-performance dialog (below).
- A reset-camera button, a "Save as preset" button (captures your full
  current lighting *and* camera pose as a named preset), a Pause/Live
  toggle, a Settings button, and an **Export** button (Chapter 9).

The Render Material drop-down lists all 32 of this app's built-in
materials in alphabetical order (Diamond, Sapphire, Tourmaline, the
various garnets, Aquamarine, Morganite, and the rest — see Appendix C's
materials table for the full list and their optical values), followed by
any custom materials you create yourself. Saved settings remember the
material by name, so the ordering never affects what is restored.

**Optical performance readout:** the on-screen metrics are Brilliance
(percentage of light returned to the eye), Fire (a unitless index of
spectral flare), Scintillation (percentage), Windowing (percentage of the
face that reads as see-through rather than reflective — turns red above
10%), and Extinction (percentage of the face that reads as dark shadow
instead of bright — turns amber above 12%).

## Quality, resolution, and GPU options

Open **Settings** from the viewport toolbar for:

- **Target Samples** — a slider controlling how many samples per pixel the
  live render converges to (8 up to 1024; default 256). More samples means
  a cleaner, less grainy image but a slower render.
- **Render Resolution** — four fixed choices: 640×480, 800×600 (default),
  1280×720, 1920×1080.
- **Motion Preview Resolution** — Off (default), Half, or Quarter: while
  you're actively dragging the camera, the app can render at a reduced
  resolution for smoother movement, then snap back to full resolution once
  you stop.
- **Live Compute** — Local only, Remote only, or Local + Remote (default,
  once a remote worker is configured) — see Chapters 9 and 10.
- **Local Compute** — CPU, CPU + GPU (default), or GPU only. **This choice
  only appears on a build compiled with GPU support**; an ordinary build
  has no such choice and always renders on CPU. Even "GPU only" quietly
  falls back to CPU for a scene the GPU can't handle (no usable graphics
  adapter, or an HDR environment map loaded), so nothing ever fails to
  render outright.
- **Max Ray Bounces** — 4, 8, 12 (default), 24, 64, or 128. Higher values
  model more light bounces (useful for strongly dispersive or heavily
  faceted stones) at a rendering-time cost that grows faster on GPU than
  on CPU.
- Further sliders for inclusion/haze, crystal-axis orientation (only
  meaningful for a birefringent material — off renders the material's axis
  **as cut**, i.e. whatever the resolved material's own axis already is;
  switching it on exposes a tilt 0–90° from the table normal and an
  azimuth 0–360° around it), a frosted-girdle toggle, facet edge rounding,
  physical stone size in millimetres, and an HDR environment map loader
  (which, once loaded, replaces the studio lighting controls and forces
  CPU-only rendering).
- **Reset to Defaults** at the bottom of Settings now asks for
  confirmation before it wipes every rendering setting back to its
  default — click it once to see "Reset everything?", then **Confirm** (or
  **Cancel** to back out without changing anything).
- Your named lighting presets, each individually markable as usable for
  batch export (Chapter 9).

## Reading the tilt-performance graph

Click **Tilt Curve** to open **Tilt Performance Curve Analysis**. This
answers a real cutting question: how does the stone's performance hold up
as it tips away from being viewed straight-on?

The sweep covers −90° to +90° of tilt away from "table-up" (looking
straight down through the table), in 1° steps, for the material currently
selected. A caption reminds you that between the 181 measured points, the
curve and any hover readout are straight-line interpolated, not
freshly ray-traced.

**The three curves:**

- **Brilliance % (Light Return)** — the percentage of light returned to
  the eye at that tilt.
- **Windowing % (TIR Leakage)** — the percentage of the face reading as
  see-through: light is passing straight through the pavilion instead of
  reflecting back, the classic "fish-eye" look.
- **Extinction % (Shadows)** — the percentage of the face reading as dark,
  dead shadow instead of bright.

Choose a **Tilt Axis** — 0° (length), 45°, 90° (width), or 135° — to see
the sweep along one particular compass direction around the stone, or turn
on **All** to overlay all four at once. Three badges report the face-up
(0° tilt) values for whichever axis is selected: Face-Up Brilliance,
Face-Up Windowing, and Extinction Shadow.

Computing all four full sweeps takes about a second and a half; the dialog
shows an explicit "Sweeping..." message rather than a silent spinner while
it works, and the result is kept so reopening the dialog on the same
inputs doesn't repeat the wait.

**Hovering the chart** snaps to the nearest whole degree and shows a small
floating card with a live mini-render of the stone at exactly that tilt
and axis, plus the interpolated value of each visible curve at that point.

**If you change material after a curve was already computed and cached**,
the dialog shows a banner: "This cached curve was rendered with
&lt;old material&gt;, not the current material (&lt;current&gt;)." with a
**Re-render fresh** button. Clicking Re-render always computes a genuinely
fresh sweep for the current material; opening the dialog while the cached
curve belongs to a different material triggers this recompute
automatically, without you needing to notice or click anything.

A **Copy 181-Point Data Table** button at the bottom copies the full
numeric sweep (all four axes) to the clipboard for your own analysis.

## Next steps

Continue to Chapter 3 to load a design into the editor, or Chapter 9 to
export a still image of what you're viewing here.
