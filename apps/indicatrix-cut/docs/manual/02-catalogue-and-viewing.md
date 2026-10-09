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
  designer. Its hint text reads "Search title or designer... (Ctrl+F)", and
  Ctrl+F jumps to it. A note is not part of the match — see Chapter 12. A small
  "×" appears once you've typed something, to clear it in one click.
- **Shape** and **Gear** drop-downs: filter to a specific shape
  classification or index-gear size.
- **Filters** button: opens the Advanced Filters panel (below). It turns
  amber with a dot when a filter is active but the panel is closed, so an
  active filter is never invisible.

### Advanced Filters panel

Click **Filters** to open it. The panel takes the keyboard focus: Tab walks its
controls, Escape closes it, and the **?** in its header opens this section. From
top to bottom:

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
   how many. In the Advanced interface (Chapter 17) it also offers a **Compute
   missing tilt curves** button to fill that gap for the whole catalogue in the
   background; in the Simple interface the notice says to switch to Advanced
   for it.
6. **My designs only** and **Tags** — restrict the list to the designs you
   imported yourself, or to one tag (click a tag chip again to stop). **My
   designs only** is greyed out while a remote library is showing.
7. **Reset Filters** — snaps every slider back to its full, unfiltered
   range and clears the toggles above.
8. **Regenerate for filtered set** (Advanced interface only) — **Regenerate
   Previews** and **Regenerate Tilt Curves** redo that work for just the designs
   the filters show now, which is handy after a renderer or material change.

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
- **Exclude from planner** / **Include in planner** — keep a design out of the
  Rough Planner's candidates (Chapter 15) and nothing else. Unlike **Ignore**, it
  hides nothing: the card stays in the list, marked "not planned", and previews and
  tilt curves are made as usual. It works on the local library only.
- **Build this design** — turn the design into a step-by-step tutorial that
  rebuilds it from an empty design (Chapter 22); the steps button in the detail
  header below does the same.

The same actions are on the three-dot button that appears when you point at a
card, and in the **Library > Selected Design** menu, which works from the keyboard
too. To move through the list without the mouse, Tab into it and use Up and Down;
Enter or Space opens the card.

Previews and tilt curves are rendered from the design's own design file (its
`.asc`, else `.gem`, else `.gcs` attachment) when the record has one, and from
the angle table only when it has none or the file cannot be read.

The **Library** menu (Advanced interface only) has **Regenerate Preview Images...**,
**Regenerate Tilt Curves...** and **Regenerate Both...**. Each opens a question first:
**Missing or outdated only** (the default; it shows how many designs that is, after a
moment of "counting...") renders just the designs with no picture or curve, a missing
front or top view, or a result made with other render settings, while **All designs**
redoes the whole library. When nothing is missing the first choice reads "All designs
are up to date" and only **All designs** can be started. The two
**Regenerate for filtered set** buttons ask the same question about the filtered
designs. The offer at start-up, and **Compute missing tilt curves**, are the "missing
or outdated only" choice without the question. Ignored designs are never included.
While a preview or tilt-curve batch runs, its progress line reads "Completed 37 / 412"
followed by the time left ("about 6 min left"); it says "estimating..." until a few
seconds of finished designs give a trustworthy rate, and it counts local and remote
work together.

A batch of previews or tilt curves (several designs at once, from the library
menu or the filter panel) also renders on a configured remote coordinator when
Live Compute includes it, and keeps several pictures in flight on the remote
at the same time — how many is **Remote lanes for batches** in the Remote
Coordinator panel (Chapter 10).

After an import you are asked whether to generate previews for the new designs:
**Full render** traces them as above, **Quick (solid)** draws a flat-shaded solid
picture of each in moments on the CPU, and **Skip** generates nothing. Tick
**Remember my choice** to stop being asked (the `import_preview_choice` setting). A
solid picture counts as a stand-in: the designs that only have one are offered again for
full rendering at the next start, from the library menu's regenerate entries, or one at a
time from a design's context menu.

When several materials fit a design's refractive index, the one its previews and tilt
curves use is the one that serves the stone best on windowing, extinction and
brilliance weighted equally, measured table-up and at two tilts, so a low windowing figure
that comes with heavy extinction does not win. It is chosen once per design and kept.

Once selected, the detail header above the tabs shows the title, designer,
and a row of spec chips (Shape, Gear, Facets, L/W, H/W, C/W, P/W, Vol/W³,
R.I.) — each chip only appears when that value is known, and pointing at one
says what the figure is. Local designs get an inline pencil to rename, an
**Edit Metadata** button for the full set of fields, and a **Delete** button
(with a confirm step: the trash icon becomes "Delete? Yes No"). Every icon of the
header can be reached with Tab, and a screen reader hears each one by name. The
**Export .gcs** button (the "gcs" icon) is part of the Advanced interface
(Chapter 17). The **?** at the right opens this section of the manual. Below the
header are three tabs: **3D Spectral Preview (1)**, **Cutting Instructions (2)**,
and **Files & Downloads (3)** — covered further in Chapters 1 and 3.

### The Cutting Instructions tab

The **Cutting Instructions** tab (press 2) lists the selected design's steps as
a table, one row per tier: the step number, the facet name, its angle, its
index positions, and any notes. A concave tier takes two lines, the facet and the
tool line under it.

Every angle is a plain positive number; the side of the girdle comes from the
block the row belongs to, which the **Pavilion** and **Crown** pills below filter
on. The facet column shows the standard code (`P1`, `G1`, `C1`, `T`). A design
catalogued with older labels (`1`, `2`, `A`, `B`) shows the code with the
original label beside it in small type, for example `P2` with "was 3", so a
printed sheet that says 3 can still be matched to the row.

- The three pills above the table choose what it shows: **All Steps**,
  **Pavilion** (the steps below the girdle) or **Crown** (the steps above it).
  Each pill carries its own row count.
- **Copy Instructions** copies the whole table to the clipboard, ready to paste
  into a document. Clicking a single row copies just that row.
- From the keyboard, Tab into the table, then Up and Down move from row to row
  (skipping the rows the pills hide), Home and End jump to the first and last
  row, and Enter or Space copies the row you are on.
- The **?** at the top right opens this section.

If a design has no recorded cutting instructions, the tab says so.

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
  optically (color, dispersion, birefringence). This changes only the
  *appearance* of the render, never the cut itself — see Chapter 6 for the
  distinction between this and any material-like control in the editor.
  (Labelled just "Material" before this control's own hover tooltip and
  caption were added to disambiguate it from the editor's unrelated Yield
  Material.) On an `editor`-enabled build, a small **"Linked to design"**
  toggle sits next to it, on by default: while on, this dropdown follows the
  Edit tab's own Design Settings material automatically (Chapter 6), so what
  you render always matches what you are editing. Turn it off to pick an
  independent render material without touching the design at all.
- **Color** — a round dot (hollow while the material keeps its own color) that opens
  a small list: **Material default**, nine ready-made colors, and **Custom colour (tone, saturation, hue)...**
  for any color you like. It shows the stone in another color without making a custom
  material. For a design you are only browsing (a row in the catalogue list), the
  choice changes this view only: the catalogue entry is never changed, the app
  remembers your choice for next time, and **Material default** clears it. When the view
  shows the design open in the Edit tab with **Linked to design** on, the pick becomes
  that design's own color instead (Chapter 6). Chapter 9 has the details, including
  how the color carries into exports and the tilt video.
- A pencil button next to Render Material opens the **Material Editor**, where
  you can define a fully custom material (name, refractive index,
  dispersion, birefringence, color swatch, crystal system, and optical
  character) starting from one of the built-in templates.
- **Lighting** — thirteen presets in the list. They are listed here in the order the
  drop-down shows them, in two families:
  - **Lit models** — what a stone looks like in a real scene. They darken the facets
    that would reflect your own head, the way a face-up stone really shows a dark table
    (the Head shadow slider below sets how much), and their brilliance, windowing and
    extinction figures count every direction that is really lit, not only the brightest
    lamp:
    - **Light tent + black cards** — the default. A jewellery light tent: grey walls,
      one broad overhead softbox, three black cards for facet contrast and a small hard
      spark light for scintillation. The softbox follows the light azimuth/elevation
      controls; the cards sit 90°, 180° and 270° around from it.
    - **Grading tray (D65 hemisphere + head shadow)** (formerly "ISO hemisphere") — the
      whole sky above the girdle evenly lit, black below. The classic light-return
      model for comparing patterns, and the light the Optimize and Retarget searches
      score under. It is flat by design, so a tilt video under it shows no sparkle.
    - **White tray (lit from below)** — a bright, perfectly even white surround (the
      walls are the same brightness from the girdle to the zenith) over a bright
      ground, no cards and no spark: a stone on a white grading tray.
    - **Jewellery shop (diffuse + spots)** — a bright diffuse ceiling (walls about
      three times the tent's), the broad key, and small pinpoint spots, the light of a
      shop window or an office.
    - **Window daylight** — a dim room lit by one broad window: a single soft key
      light with a low cone and no cards or spark. The built-in view **Window daylight**
      (Views list) selects this light and puts it at 30° elevation, as a real window
      sits; with the default light elevation it would shine from high overhead instead.
    - **Daylight sky (no sun)** — a clear sky only, brighter towards the horizon, with a
      soft glow around the light direction and dark ground. There is no sun disc in
      it. (Settings saved as "Daylight sky + sun" open as this preset, which is what
      they always drew.)
    - **Daylight sky + direct sun** — the same sky plus a real sun, a hard-edged disc
      0.27° across. At the default light height it supplies about 82 % of the light,
      the rest is sky. A facet flashes, with fire, whenever it catches the sun; the
      sun follows the light azimuth/elevation controls and goes out when the light is
      below the horizon.
    - **Contrast view (ASET-style)** — a contrast view in the style of an ASET scope.
      The sky is coloured by elevation: **green** from 0° to 45°, **red** from 45°
      to 75° and **blue** from 75° to 90° overhead, **black** below the horizon.
      There is no white balance and no head shadow, so you can see where in the stone
      the light returns from. It is a diagnostic view, not a photograph, and its figures
      are scored as the Grading tray.
  - **Studio** (the analytic studio rig): **D65 Daylight (6500K)**, **Incandescent
    (3200K)**, **Incandescent A (2856K)**, **Gem Studio Ring Lights** and **Dramatic
    Dark Spotlight**. A dark velvet backdrop with a key softbox, a fill and sixteen
    ring pinpoints: hard sparkle, clipped highlights, black everywhere else.
    Incandescent A is the standard tungsten lamp (2856 K) with white balance applied.
    The brilliance figures under a Studio rig are measured against that rig's own light
    sources; the Grading tray gives the standard figure.

  At exposure 1× the lit models put their ambient light near middle grey, so only a
  direct reflection of a light source clips to white; use Studio Exposure to go darker
  or brighter.
- **Studio Exposure** (Settings gear) — the overall brightness of the picture, a
  slider from 0.4× to 2.5× (default 1×, shown next to the label). It scales the light
  of every built-in lighting preset, the lit models and the Studio rigs alike. It does
  not apply while an HDR environment map is loaded, which replaces the exposure and
  light-position controls (the map brings its own brightness). A stored value outside
  0.2× to 5× is pulled back into that range when the settings are read.
- **Light direction** (Settings gear, under Studio Exposure) — where the main light
  shines from, as two angles. **Light Azimuth** is the angle around the stone, 0° to
  360° (default 48°). **Light Elevation** is the height above the stone, 10° (low and
  raking) to 90° (straight overhead) (default 72°). Besides the two Settings sliders,
  you can move the light in the viewport itself: right-drag, or Shift + left-drag,
  changes both angles at once (the table above). The direction moves the light tent's
  overhead softbox (the black cards turn with it), the sun of **Daylight sky + direct
  sun**, the glow of **Daylight sky (no sun)**, the window of **Window daylight** and
  the key softbox of the Studio rigs. The **Grading tray** does not depend on it: the
  whole hemisphere is lit evenly, so the light direction changes nothing there. An HDR
  map ignores it too.
- **Views** (Settings gear, the **Lighting Presets** list) — one-click saved looks.
  A view is more than a lighting preset: it stores the lighting preset *and* the light
  direction (azimuth and elevation), the exposure and the camera distance, and for some
  views also the camera pose. Click a name to apply the view; applying one that stores
  no camera pose leaves your current turn around the stone alone. Seven views come
  with the app and cannot be renamed or deleted:

  | View | Lighting preset | Light azimuth / elevation | Exposure | Camera |
  |---|---|---|---|---|
  | Studio Softbox | Gem Studio Ring Lights | 48° / 54° | 1× | not set |
  | Daylight Bright | D65 Daylight (6500K) | 30° / 65° | 1.3× | not set |
  | Dramatic Spotlight | Dramatic Dark Spotlight | 300° / 25° | 0.7× | not set |
  | Light Tent | Light tent + black cards | 48° / 72° | 1× | not set |
  | Grading tray | Grading tray (D65 hemisphere + head shadow) | 48° / 72° | 1× | azimuth 20°, elevation 85° (looking almost straight down) |
  | Daylight Sun | Daylight sky + direct sun | 30° / 55° | 1× | not set |
  | Window daylight | Window daylight | 40° / 30° | 1.2× | not set |

  The **Save as preset** button on the viewport toolbar (hidden in Simple mode)
  captures your current lighting, light direction, exposure and camera pose under a
  name you type; the save button in the Lighting Presets heading saves the current
  lighting without the camera pose. Your own views are added to the same list, after
  the built-in ones, and can be renamed and deleted (the pencil and trash buttons). Every
  row, built-in or yours, has a small save toggle, the "usable for" mark: switch it on
  and the view is offered in the export dialog's list of lightings, so one export can
  render the stone under several of them (Chapter 9). New views start with it off.
- **Backdrop** (Settings gear, under Studio Exposure) — what the camera sees behind
  the stone: **As lit** (the environment's own ground), **Grey** (the default:
  a neutral grey card, the convention gem-design programs use for like-for-like
  comparisons) or **White**. Only
  the camera sees it; the stone's optics never do, so leakage and windows stay dark.
- **Surface glare** (Settings gear, under Backdrop) — a 0–100 % slider in steps of 5
  (default 100 %). It scales the white mirror image of the light that a polished
  surface reflects, which is what a cross-polarised photograph removes: at 0 % the
  bright reflection of the light on the table disappears and the inner facets stay
  visible, while the light that entered the stone is untouched. It applies to the
  built-in lighting presets (Studio and the lit models), not to an HDR environment
  map (the slider greys out while one is loaded), and it follows into the live view,
  remote and hybrid rendering and every export. The brilliance, windowing and
  extinction numbers, tilt curves and catalogue previews never use it, and the Solid
  view is unaffected. "Reset to 100 %" restores the default.
- **Head shadow** (Settings gear, under Surface glare) — a 0–30° slider (default 16°,
  0 = Off). On the lit models it is how much of the sky your own head hides: the
  dark table reflections a face-up stone shows. 16° is a head at arm's length. The
  slider now also changes the brilliance, windowing and extinction figures under those
  presets, not only the picture, so a wider head shadow lowers the face-up brilliance.
  The Studio rigs (D65 Daylight, Incandescent, Incandescent A, Ring Lights, Dark
  Spotlight) ignore it, and so does an HDR map (the slider greys out
  while one is loaded). It follows into the live view, remote and hybrid rendering
  and every export; "Reset to 16°" restores the default.
- **Tilt Curve** — opens the tilt-performance dialog (below). Simple mode hides
  this button (Chapter 17); switch to Advanced to use it.
- A reset-camera button, a "Save as preset" button (captures your full
  current lighting *and* camera pose as a named preset; Simple mode hides it), a
  Pause/Live toggle, a Settings button, and an **Export** button (Chapter 9).
- A round **?** button opens this part of the manual. Every button on the toolbar
  also has a hover note saying what it does, and you can reach each one with the
  Tab key and press it with Space or Enter.

**A stone that is cut back.** The Edit tab's Cut slider (Chapter 13) can show the
stone part-way through the cutting, and the Live Render view follows it. While it does,
a small amber badge over the picture says so and names the step, and its **Show
finished** button puts the slider back to the finished stone. The badge appears only
for the design open in the Edit tab: a catalogue row you are browsing is always drawn
whole. Exports, the tilt video and the tilt curves never use the part-cut stone; they
always draw the finished gem (Chapter 9).

The Render Material drop-down lists all 33 of this app's built-in
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

Open **Settings** (the gear) from the viewport toolbar for the list below. The round
**?** in its title bar opens this section of the manual, and every setting has a hover
note that says what it does.

In **Simple mode** (Chapter 17) the card shows the settings most designs need:
Target Samples, Render Resolution, Inclusion Haze, Crystal Axis Orientation, Frosted
Girdle, Edge Rounding, Stone Size, the Environment Map, Studio Exposure, Backdrop,
Surface glare, the light direction and the lighting presets. It hides Preview Image
Size, Preview Samples, Motion Preview Resolution, Live Compute with Live Transfer,
Local Compute, Max Ray Bounces and the Tilt Performance Curve panel; those are the
Advanced settings below. A hidden setting keeps its value and still applies. If one of
them is not at its default, the card says "Some advanced settings are in use. Switch to
Advanced to see them."

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
  once a remote coordinator is configured) — see Chapters 9 and 10. Next to it,
  **Live Transfer** (Full data or Final picture) chooses how the remote's
  contribution comes back to you — see Chapter 10.
- **Local Compute** — CPU, CPU + GPU (default), or GPU only. **This choice
  only appears on a build compiled with GPU support**; an ordinary build
  has no such choice and always renders on CPU. Even "GPU only" quietly
  falls back to CPU for a scene the GPU can't handle (no usable graphics
  adapter, or an HDR environment map too large for the graphics card's
  memory limits), so nothing ever fails to render outright.
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
  (which, once loaded, replaces the studio lighting controls; HDR maps render
  on the GPU too, and on a remote coordinator that supports them — Chapter 10).
- **Reset to Defaults** at the bottom of Settings asks for
  confirmation before it wipes every rendering setting back to its
  default — click it once to see "Reset everything?", then **Confirm** (or
  **Cancel** to back out without changing anything).
- Your named lighting presets, each individually markable as usable for
  batch export (Chapter 9).
- **Lighting for this design** — the last group in Settings. **Use this lighting for
  this design** remembers the lighting you see now (light, exposure, surface glare,
  backdrop and environment map) for the design that is open in the Edit tab. It is
  kept in your library on this computer, never inside the design file. Open that
  design in the Edit tab later and its lighting comes back; open a design with
  nothing saved and your normal lighting returns. **Forget for this design** removes
  it. A catalogue row you are only browsing does not apply its saved lighting, so you
  can flip through designs under one lighting. Chapter 9 has the details.

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
