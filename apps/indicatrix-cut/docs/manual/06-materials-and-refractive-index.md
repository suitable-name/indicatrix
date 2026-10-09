# 6. Adapting a Design for Another Material

## What you will do

This chapter is about a question every cutter eventually asks: "I like this
cut, but I want to cut it in a different gem material -- what do I need to
change?" It covers the built-in material list, building your own custom
material, the Design Settings panel's Material and RI Override controls,
how a material actually reaches the optimizer/tilt curve/viewport/export,
and **Retarget**, the command bar's automated proposal for re-angling a
design for a different material.

## What refractive index is, briefly

**Refractive index (RI)** measures how strongly a material bends light. It
determines a stone's **critical angle** -- the angle, measured from a
facet's normal, beyond which light hitting that facet from inside the stone
reflects internally instead of escaping:

> critical angle = arcsin(1 / RI)

A cutting design's pavilion angles are chosen so that light entering
through the crown strikes the pavilion facets steeper than the critical
angle, reflects internally (rather than leaking out the bottom, which shows
as **windowing** -- a washed-out, see-through patch) and exits back out
through the crown towards the viewer. **Birefringence** is a related but
separate property: some materials split light into two rays with slightly
different refractive indices, part of what gives certain gems (peridot,
zircon, and others) their characteristic doubling.

A cutting design that performs well in one material's RI will not
necessarily perform well in another -- a design cut for diamond (RI about
2.417) at a given pavilion angle may window badly if cut in quartz (RI
about 1.54) at the same angle, because quartz's critical angle is larger.

## The built-in material list

The renderer ships 33 built-in materials, used both by the Live Render
viewport and as starting templates in the Material Editor. Full dispersion
and color detail for every one is in Appendix C; the figures that matter
for cutting -- refractive index, birefringence, and whether the material
has real absorption bands modelled (which mostly affects render color, not
cutting) -- are:

| Material | n_D | Birefringence (Δn) | Optical character | Absorption modelled |
|---|---|---|---|---|
| Diamond | 2.417 | — | Isotropic | No |
| Sapphire | 1.768 | −0.0081 | Uniaxial (−) | Yes |
| Ruby | 1.768 | −0.0081 | Uniaxial (−) | Yes |
| Emerald | 1.579 | −0.0060 | Uniaxial (−) | Yes |
| Zircon | 1.925 | +0.0590 | Uniaxial (+) | Yes |
| Alexandrite | 1.743 | +0.0076 | Biaxial (+) | Yes |
| Topaz | 1.627 | +0.0080 | Biaxial (+) | Yes |
| Spinel | 1.716 | — | Isotropic | Yes |
| Quartz | 1.544 | +0.0091 | Uniaxial (+) | No |
| Tourmaline | 1.639 | −0.0210 | Uniaxial (−) | Yes |
| Tanzanite | 1.701 | +0.0130 | Biaxial (+) | Yes |
| Synthetic Moissanite | 2.647 | +0.0415 | Uniaxial (+) | No |
| Cubic Zirconia | 2.158 | — | Isotropic | No |
| Aquamarine | 1.577 | −0.0060 | Uniaxial (−) | Yes |
| Morganite | 1.577 | −0.0060 | Uniaxial (−) | Yes |
| Chrysoberyl (Yellow) | 1.746 | +0.0090 | Biaxial (+) | Yes |
| Amethyst | 1.544 | +0.0091 | Uniaxial (+) | Yes |
| Citrine | 1.544 | +0.0091 | Uniaxial (+) | Yes |
| Pyrope Garnet | 1.714 | — | Isotropic | Yes |
| Almandine Garnet | 1.790 | — | Isotropic | Yes |
| Spessartine Garnet | 1.800 | — | Isotropic | Yes |
| Color-Change Garnet (Pyrope-Spessartine) | 1.760 | — | Isotropic | Yes |
| Grossular Garnet (Tsavorite) | 1.734 | — | Isotropic | Yes |
| Andradite Garnet (Demantoid) | 1.887 | — | Isotropic | Yes |
| Peridot | 1.654 | +0.0360 | Biaxial (+) | Yes |
| YAG | 1.833 | — | Isotropic | No |
| GGG | 1.970 | — | Isotropic | No |
| Benitoite | 1.757 | +0.0470 | Uniaxial (+) | Yes |
| Andalusite | 1.634 | −0.0100 | Biaxial (−) | Yes |
| Opal | 1.450 | — | Isotropic | No |
| Glass (N-BK7) | 1.517 | — | Isotropic | No |
| Glass (F2) | 1.620 | — | Isotropic | No |
| Rutile | 2.616 | +0.287 | Uniaxial (+) | Yes |

(Sphene/titanite is deliberately not included -- see the Troubleshooting
chapter's Known Limitations.)

## The single material catalogue

Every material picker in the app -- the Design Settings panel's Material
combo, the New Design dialog's Starting Material combo, and the Live
Render viewport's Render Material dropdown -- shares one list: all 33
built-in materials above, in that order, followed by every custom material
you have saved, sorted alphabetically. Whichever picker you open, you see
the same materials in the same relative order (built-ins first, then your own).

## Custom materials

Click the pencil button next to the Live Render viewport's Render Material
dropdown (Chapter 2) to open the **Material Editor**. In the Edit tab, the
**New/Edit material...** button next to the Design Settings material combo
opens the same editor, so you do not have to switch tabs. Its fields are:

| Field | Control | Notes |
|---|---|---|
| Load Preset Template | combo | Starts you from Custom, or from one of eleven built-ins (Diamond, Sapphire, Ruby, Emerald, Tanzanite, Synthetic Moissanite, Zircon, Topaz, Spinel, Quartz, Cubic Zirconia) as a base to tweak. |
| Material Name | text field | Rejected with a warning if it matches a built-in material's own name. |
| Refractive index curve | switch: **Simple** / **Coefficients** | **Simple** uses the two sliders below. **Coefficients** types the curve as a Sellmeier or Cauchy fit and hides those two sliders -- see "Dispersion coefficients" below. |
| Refractive Index (nd) | slider, 1.30–3.20 | Simple mode. |
| Dispersion (Fire ΔnF-C) | slider, 0.000–0.210 | Simple mode. |
| Specific Gravity | slider, 0.0–7.0 | Shows "not recorded" at 0 -- leave it there if you don't know the figure. |
| Birefringence (Δn = ne − no) | slider, −0.050 to +0.100 | Zero means isotropic. |
| Crystal System | combo | Cubic, Tetragonal, Hexagonal, Trigonal, Orthorhombic, Monoclinic, Triclinic. |
| Optical Character | combo | Isotropic, Uniaxial (+), Uniaxial (−), Biaxial (+), Biaxial (−). |
| Biaxial nβ − nα | slider | Only shown when Optical Character is one of the two Biaxial choices. |
| Color Mode | radio toggle | **Fantasy** (unconstrained colors) or **Physics** (mineralogical host crystal + real chromophores). Only in builds with the `physical-color` feature -- see "Physics color mode" below. |
| Gem Body Color (Fantasy) | swatches + Pick... | Clear, Blue, Red, Green, Violet, Yellow, Pink, Teal, Amber, a custom kept color, or a free choice: **Pick...** opens the Tone / Saturation / Hue colour editor (see "Body Color" below) and the material keeps the full colour. |
| Chromophore Recipe (Physics) | controls & swatches | Only in builds with the `physical-color` feature. Host crystal, element rows with log sliders, locks, sources, strength + fractions or absolute view, treatments, equivalent path, D65 / 3200 K swatches, color picker with the inverse solver, and Undo. |

Buttons: **Delete**, **Cancel**, **Save**, and **Save & Apply** (saves, then
also selects it as the current render material).

### Dispersion coefficients (Sellmeier and Cauchy)

The two sliders describe a material's refractive-index curve with just two
numbers: the index at the yellow sodium line (nd) and the spread between the
blue and red lines (ΔnF−C). The program builds a smooth Cauchy curve through
them. That is enough for most gems. When you have a published fit instead --
a glass data sheet, or a paper on a synthetic crystal -- switch **Refractive
index curve** to **Coefficients** and type the fit itself. The stone is then
traced with exactly that curve, on the graphics card and on the processor
alike.

**Model.** Pick the equation your source uses. The wavelength λ is always in
micrometres (µm), so 589 nm is 0.589.

- **Sellmeier, 3 terms:** n² = 1 + B1·λ²/(λ² − C1) + B2·λ²/(λ² − C2) +
  B3·λ²/(λ² − C3). Fields B1, B2, B3, C1, C2, C3. This is the form glass
  catalogues print, and what the built-in Diamond, Sapphire and glass entries
  use.
- **Sellmeier, 1 term:** n² = 1 + B1·λ²/(λ² − C1). Fields B1 and C1. If your
  source gives two terms, use the 3-term model and enter 0 for the unused B
  and C.
- **Cauchy:** n = A + B/λ² + C/λ^4 (λ to the fourth power). Fields A, B (µm²)
  and C (µm^4); C is often 0.

Each B is a strength with no unit; each Sellmeier C is a resonance wavelength
squared, in µm². So C = 0.01 means a resonance at 0.1 µm = 100 nm, deep in
the ultraviolet. A typical glass has C values like 0.006 and 0.02 (resonances
at 77 nm and 141 nm) and one large C (about 103, an infrared resonance).

**Copy from.** For the Sellmeier 3-term and Cauchy models, the **Copy from**
combo fills the fields with the coefficients of a built-in material of that
model (Glass (N-BK7), Diamond, Sapphire, ...). It is the quickest way to a
sensible starting point: copy, then change one number and watch the readout.
Choosing a different model starts its fields empty, because the same number
means something else in each equation. Switching from **Simple** to
**Coefficients** the first time starts from the Cauchy fit of the two sliders'
values, so the curve you had is the curve you start editing.

**Examples.** N-BK7 glass as a 3-term Sellmeier:

| B1 | B2 | B3 | C1 | C2 | C3 |
|---|---|---|---|---|---|
| 1.03961212 | 0.231792344 | 1.01046945 | 0.00600069867 | 0.0200179144 | 103.560653 |

reads back nd 1.5168 and an Abbe number of about 64.2. A Cauchy fit of a
similar crown glass: A = 1.5046, B = 0.0042, C = 0 gives nd 1.5167,
ΔnF−C 0.0080 and an Abbe number of about 64.4. Use a decimal point (a decimal
comma works too when there is no point) and plain digits; 1e-3 is accepted.

**The live readout.** Under the fields you see nd (the sodium D line, 589.3
nm), nF (blue, 486.1 nm), nC (red, 656.3 nm), ΔnF−C and the **Abbe number**,
(nd − 1) divided by (nF − nC). A low Abbe number means strong dispersion and
lots of fire (diamond is 55, a crown glass about 64). Below that is a small
plot of n against wavelength from 380 to 780 nm. The vertical scale fits the
curve itself, so a nearly flat curve still looks like a curve; read the two
numbers at the left edge of the plot for the range.

**Errors (Save is disabled).** Each field that is not a number is outlined in
red with a plain message such as "'abc' is not a number". Beyond that, the
curve as a whole must be usable, and the red line under the fields says what is
wrong:

- *A C value puts a resonance at ... nm, inside the 300-800 nm range.* A
  resonance in the range the stone is rendered in makes the index jump to
  infinity there. This is nearly always a unit slip: C is the resonance
  wavelength squared, in µm², so a resonance at 600 nm is C = 0.36 and one in
  the ultraviolet at 100 nm is C = 0.01.
- *The refractive index comes out as ... at ... nm. It must stay above 1.* The
  signs or sizes of the coefficients do not describe a real material.
- *The refractive index cannot be calculated.* A division by zero or a negative
  square somewhere in the range.

The program never saves a curve it cannot render; the check runs again when
you press Save.

**Warnings (amber, Save still works).** These mark curves that are possible but
usually a typing slip:

- the index rises with wavelength somewhere between 380 and 780 nm (real gems
  fall from violet to red; check the signs);
- the Abbe number is outside 5 to 120;
- the index is the same at the blue and red lines, so the stone shows no fire.

**What is stored, and what else uses it.** The coefficients are stored with the
material in your catalogue and, when a design uses that material, in the
design's `.indicatrix` file next to the plain nd and ΔnF−C numbers. Those two
numbers are always the curve's own, so the Effective RI chip and the
cutting-sheet export agree with the readout on nd. Live Render traces with the
full curve, on the graphics card and on the processor alike. So does a remote
worker: every render request carries the whole material, including its Sellmeier
or Cauchy coefficients, so a high-resolution export, the share of a live picture
and the frames of a tilt video that a remote traces show the same colour play as
your own computer. Nothing has to be installed on the worker for that. Three
things to know:

- An **RI Override** on a design replaces the whole curve with a single-index
  Cauchy curve at the typed number, for that design only. This is how an RI
  override has always worked for any material, built-in or custom.
- Switching an existing material back to **Simple** and saving replaces the
  curve with the Cauchy fit of nd and ΔnF−C (the sliders start from the
  curve's own values). The coefficients are not kept. Picking a template, or a
  host in Physics color mode, proposes an nd and a ΔnF−C and so switches the
  section back to **Simple**.
- An older version of Indicatrix Cut, and the browser viewer, do not know the
  coefficients. They show the material with a Cauchy curve built from nd and
  ΔnF−C instead (close, but not the identical curve).

### Physics color mode

Available in builds with the `physical-color` feature. Other builds keep and
render physics colors but do not show the editor.

**In a build without the feature** (the normal build), the Material Editor
has no **Color Mode** toggle and no Physics section: it always shows the
Fantasy swatches and **Pick...**. A material whose color comes from a physics
recipe still renders exactly as before. Open it and the editor shows a note:
"This material's color comes from a physics recipe made in another build.
Picking a color here replaces it with a fixed color; the recipe stays saved
with the material." No swatch is marked until you pick one, and the crystal
system and optical character stay locked (the recipe's host fixes them) until
you do. Open it and Save without touching the color, and nothing changes: the
stored mode and recipe are written back as they were. Pick a swatch (or load a
preset template, which brings its own color) and the material becomes a
fixed-color material; the recipe is still saved inside it, so a build with the
feature can switch back to it. Such a build also never asks the "color changed
by an older version" question when you open a file (it keeps the saved recipe
and says so in the open message) and does not show the "Color data updated"
badge.

The rest of this section describes the editor in a build with the feature.

The **Color Mode** toggle switches between **Fantasy** and **Physics**. Both
colors are kept: switching never discards the other one. Going to Physics for
the first time solves your *current fantasy color* into the chosen host
(closest reachable); going back to Fantasy keeps the recipe, and saving while
Fantasy is active still stores the recipe, so physics -> fantasy -> save does
not delete it. A fantasy color that was still "Clear" is seeded from the
physics color on the first switch back.

In **Fantasy** mode the nine swatches are joined by **Pick...**, which opens
the same Tone / Saturation / Hue colour editor as the design settings' Body
Color (see "Body Color" under the design settings below): sliders, the optional
hue/saturation/brightness picker (**Use picker...**), the four size swatches
and the reachability badge. **Apply colour** makes the colour the material's
colour (the swatch row then shows the nearest swatch, or "Custom (keep)") and
**Save** writes it to the library: a custom material keeps its seven-band
colour, so the same material looks deeper in a larger stone. Picking one of the
nine swatches afterwards replaces it. The material also stores the closest
three-band colour next to it, which is what an older version of the program
shows. A material that uses the Physics colour keeps its recipe's colour; the
editor is for Fantasy colours. A library database from an older version opens
unchanged: its materials simply have no seven-band colour.

In **Physics** mode the body color comes from absorption spectroscopy:

- **Host crystal.** The list is the color data's own host list (corundum,
  beryl, chrysoberyl, spinel, quartz, topaz, tourmaline, zircon, garnets,
  peridot, diamond, and so on). Choosing a host fixes the crystal system and
  optical character (those combos are greyed out) and prefills refractive
  index, dispersion, birefringence and specific gravity from the host's
  built-in material. For a garnet the RI and SG are interpolated over the
  end-member mix and re-prefilled as you change it. Picking a built-in
  template in "Load Preset Template" preselects its host.
- **Data-confidence banner.** One banner per host, shown only when some of its
  coefficients are not verified measurements ("Strengths uncalibrated --
  approximate").
- **Elements.** Each recipe row has a log-scale amount slider bounded by the
  host's maximum for that item, the amount with its unit (ppm, wt% oxide,
  mole fraction), a **Lock** toggle (the solver keeps locked amounts), a
  remove button, an "estimated data" note when the data behind the item are an
  estimate, and a **Sources** popover (confidence, source citations, skipped
  bands). **+ Add** lists only the items the data offer for the host; ones
  that cannot be added right now (no room in a garnet's end-member budget, a
  required or excluding chromophore, a centre that only a treatment creates)
  are greyed with the reason. Garnet and olivine items are mole fractions that
  add up to at most 100 % with a colorless remainder.
- **Two views.** *Strength + fractions* shows each item's share and one
  **Strength** slider (a multiplier on every item's absorption, so the hue
  does not change); *Absolute amounts* shows the amounts themselves.
- **Treatments.** Only the treatments the data define for the host, and only
  when their required elements are present.
- **Equivalent path.** The reference light path in millimetres. It defaults to
  1.5 x the open design's girdle diameter (a rough proxy for the table-to-culet
  path plus return) and to 5 mm when no design with a size is open.
- **Swatches.** The computed body color under D65 daylight and under 3200 K
  incandescent light, each unpolarised plus the ordinary, extraordinary and
  (biaxial hosts) beta ray, the color-change dE between the two, and, when it
  differs from the reference path, the color at this stone's size.
- **Pick color...** opens the color picker; the picked color becomes the
  solver's target. The solver runs on a background thread (the dialog shows
  "Solving..." and never freezes); if you pick again the latest request wins and
  the older result is dropped. The readout shows "dE x from your pick (D65)",
  with "closest reachable shown" when the pick cannot be matched within
  dE 1, in the warning color above dE 2.
- **Undo.** The dialog keeps its own recipe undo stack (50 entries): every
  slider release, add/remove, treatment, host change and solver result is
  undoable with **Undo**/**Redo** or Ctrl-Z / Ctrl-Y.

Edits to a recipe count as unsaved changes: closing the dialog asks before
discarding them, and every time the dialog opens its physics state is rebuilt
from the selected material (nothing carries over from a previous open).

**What is saved, and older versions.** The material stores both colors and the
recipe together with its *resolved bands* and the color-data version. Rendering
always uses the stored resolved bands: if a later release recalibrates the
color data, the material keeps its color, the Live Render toolbar shows a
"Color data updated" badge for it, and the editor offers **Update to current data**
(undoable, never automatic). The top-level color written next to a recipe is the
nearest legacy color, so an older version of Indicatrix Cut that does not know
physics color still shows an approximate color (within dE 15 for the reference
recipes). If such an older version edited that color and saved the file, this
version notices on opening it and asks whether to keep the physics recipe or use
the edited color. A physics material also takes a 7 mm default stone width when
the design has no size, so its color depth is not rendered at model scale.

**Per-design body color.** The design settings' *color* override replaces the
whole absorption, i.e. the physical color, so it is greyed out for a physics
material ("replaces the physical color"); if an override was already set when
the material was selected, a warning says it still wins until set back to
"Material default". In a build without the `physical-color` feature the same
notes avoid the word "physical": the combo is greyed out with "This material
defines its own color.", and an already-set override is reported as replacing
"this material's own color".

**How colours relate to Stone Size.** The built-in colours, the colour presets and
the nine Fantasy swatches are shown as they look face-up in a stone of 7 mm: the
swatch, the colour control's dot and the render agree, and with no Stone Size set
the stone is rendered as a 7 mm one. Setting a Stone Size in the rendering
settings scales the colour physically from there: a bigger stone is darker and
more saturated (a 10.87 mm green cubic zirconia stays green, only deeper), a
smaller one paler. Colours made with the L*C*h editor, the library band rows
and Physics recipes are per millimetre and follow the Stone Size the same way,
with 7 mm assumed when none is set.

**Where a custom material lives.** Saving writes it to your catalogue's own
database -- this is the copy every material picker across the app actually
reads from, and it is what makes the material available the next time you
open the app or select a different design. Separately, if a design's own
Material is set to a custom material, saving that design with **Save**
also writes a small snapshot of that material's numbers
(`CustomMaterialSnapshot`: RI, dispersion, birefringence, specific gravity
if set, crystal system, optical character, and the Sellmeier or Cauchy
coefficients if you typed them) into the design's own `.indicatrix` file. This snapshot exists purely so the design
still opens with its real optics if it is ever loaded on a machine whose
catalogue database has no row for that material name -- without it, a design
would silently reload as plain Diamond. You never edit this snapshot directly;
it is written and read automatically alongside Save and Open
(Chapter 11).

## RI Override vs. Material

Above the tier list, the Edit tab's Design Settings panel shows:

- **Material** -- the combo described above: `(none)`, every built-in, your
  own custom materials, and a final `Custom RI...` entry. Picking a name
  sets the design's material by name; picking `Custom RI...` (or `(none)`)
  clears the name so a typed RI override is the only thing describing this
  design's optics -- useful for a species with no built-in preset and no
  custom material of your own yet.
- **RI Override** -- blank uses the selected material's own refractive
  index; typing a number (greater than 1.0) overrides it.
- **Effective RI** / **Critical Angle** -- read-only. Effective RI resolves
  in this order: your typed override, else the selected material's own real
  RI (built-in **or custom catalogue material**), else the schedule's
  legacy recorded value. Critical Angle is `arcsin(1 / effective RI)` in
  degrees.
- **Body Color** -- the design's own colour override: *Material default* (the
  material's own colour), a short list of presets (Clear, Blue, Red and so on)
  and a final **Custom...** entry. Choosing **Custom...** opens the colour
  editor under the box with three sliders: **Tone** (how light or dark, 0 black
  to 100 white), **Saturation** (how rich, 0 grey) and **Hue** (the basic colour
  in degrees around the colour wheel). **Use picker...** offers the
  hue/saturation/brightness picker instead; the sliders then show the picked
  colour's tone, saturation and hue. Press **Apply colour** to use it (one step
  you can undo); **Cancel** changes nothing. The girdle diameter also sizes the
  face-up colour the Optimize tab predicts (Chapter 8).
  The colour of a gem depends on its size: the same absorption looks deeper in a
  larger stone. The editor therefore shows the colour you chose as four swatches,
  at 3 mm, 5 mm, 10 mm and **This stone** (the design's own size: its girdle
  width times 1.5, or 5 mm while no girdle diameter is set). The design keeps
  the colour as an absorption spectrum per millimetre, so a larger stone in the
  same design is rendered deeper and a smaller one lighter.
  Under the swatches a badge says **Reachable** with the colour difference
  (Delta E) from what you asked for, or, in amber, that the colour is **not fully
  reachable** at this size and how far the closest reachable colour is. Very
  rich or very light-and-rich colours are not all reachable; lower the
  saturation or change the tone. The presets above are unchanged and still apply
  at the size of the design.
  The same entry shows when the design holds a colour that is not one of the
  presets (one set from the Live Render toolbar or read from a file). Applying
  the settings keeps a Custom colour exactly as it is, even when you change the
  species in the same step; choosing a preset or *Material default* replaces it.
  Older versions of the program show a close three-colour version of a colour
  made here. The web app has the same editor, solves in its background worker
  and renders the full colour in its 3D view.
  The same editor opens from two more places: **Pick...** in the Gem Material
  Editor (Fantasy colour; Save keeps the colour in the material) and **Custom
  colour...** in the Live Render toolbar's Color popup. In the toolbar it
  changes the open design (one undo step) while the view is linked to the
  design; otherwise it changes only the view and stores the closest three-band
  colour, as the toolbar's other colours do.

**Simple and Advanced.** The Simple interface (Chapter 17) leaves out the panel's
RI Override, Symmetry Order, Mirror and Apply Symmetry controls, the Extra Header
Lines and Footnotes boxes, Gear Ref. Angle, and the printed-proportions row.
Hidden controls still apply: when one of them holds something other than its
default (an RI override, extra header lines, footnotes, a printed proportion, a
reference angle other than zero, Mirror on), the panel shows the line "Some
advanced settings are in use. Switch to Advanced to see them." The **?** in the
panel's header opens this section.

**Export agrees with the on-screen figure.** Export Edited .asc, Save,
and the cutting-sheet export all resolve the design's Material the
same catalogue-aware way the Effective RI chip does -- so a custom
material's own refractive index reaches the exported `I` line and matches
what you see on screen, with no separate "which RI did the export actually
use" question.

**Limitation.** The Solid-view facet-map overlay's windowing-risk hatch and
one helper function do not resolve custom catalogue materials the way the
tier list's MARGIN badge and Live Render view do. For a design whose Material
is a custom catalogue material with no explicit RI override, the Solid view's
hatch may briefly disagree with the MARGIN badge and Live Render view. Setting
an explicit RI Override matching the custom material's own value keeps every
view in agreement.

## Inferred material: a guess, never a fact

An imported `.asc` file carries only an `I` line -- a bare refractive index,
never a species name. So when a design's Material is unset (`(none)`), the
app looks for the nearest built-in preset within 0.02 of the design's own
effective RI and shows it as a labelled **guess**, never as though it were a
recorded fact:

> **Sapphire? (from RI 1.76)** — with a **Set material** button next to it.

You'll see this badge in three places, always the same wording: the Design
Settings panel (next to the Material combo), the inspector's Yield tab
(next to the Eff. RI line), and the catalogue detail card for a design with
no material name of its own. The printed cutting sheet's header shows the
same guess text in its "Material" row for an unnamed design, so a printed
sheet never states a species as fact that the app itself is only guessing
at.

Hovering the badge lists any OTHER built-in presets within the same 0.02
tolerance, so you can see what else was close before trusting the nearest
one. Clicking **Set material** writes the shown name into the design's
Material as an ordinary, undoable edit (Ctrl+Z reverts it like any other
change) -- once a name is set, the guess badge disappears everywhere at
once (there is nothing left to guess), and the name reaches the saved
`.indicatrix` file and Export Edited .asc the normal way. If nothing built in
is within tolerance, no badge is shown at all -- the app never forces a
guess onto a material it cannot place.

## The tier list's MARGIN column

Each pavilion tier (a tier whose C/P/G column reads P) shows how many degrees
its authored angle sits above the design's own effective critical angle,
color-coded:

- **Green** (Safe) -- 2 degrees or more of margin.
- **Amber** (Marginal) -- less than 2 degrees, but still past the critical
  angle. A small edit, a manufacturing tolerance, or a different material
  could tip this into windowing.
- **Red** (Windows) -- already below the critical angle for the design's
  current effective RI. Light leaks straight through this facet.

A **crown** tier (C in the C/P/G column) shows a margin too, suffixed **"(est.)"**
-- an ESTIMATE of whether light entering through that crown facet (rather
than straight down through the table) would still reflect off the
design's own main pavilion facet on the same side of the stone, derived from Snell's law at the crown
facet plus the plain pavilion margin above. It follows exactly one ray in
one cross-section and ignores every other path light actually takes
through a real stone, so treat it as a rough guide, not a certainty --
never the same weight as a pavilion row's own plain margin. A girdle tier
(angle exactly zero), or a crown tier in a design with no pavilion tier at
all to estimate against, shows a plain dash.

The same color-coded bar appears live in the inspector's Tier tab, next
to the Angle field itself, updating as you type -- so a pavilion angle you
are about to save shows red, with its margin and a one-line reason,
*before* you leave the field or click Save Tier.

## The "Linked to design" checkbox

A **"Linked to design"** toggle, on by default, appears next to the Render
Material control both in the Live Render viewport's own toolbar (Chapter 2)
and, on an `editor`-enabled build, in the Edit tab's Design Settings panel
itself. Both copies control the same underlying setting. While it is on,
the Render Material dropdown follows the design's own Material
automatically -- pick a new material in Design Settings, and the viewport
(and the optimizer, and the tilt curve cache) all update to match, so what
you render is always what you are editing. Turn it off to pick an
independent render material without touching the design at all; while off,
further design edits leave your independently-chosen render material alone
rather than snapping it back.

## Which RI each part of the app actually uses

| Consumer | RI source |
|---|---|
| Design Settings' Effective RI / Critical Angle chips | Catalogue-aware (built-in, custom, or override) |
| Export Edited .asc / Save / cutting sheet | Catalogue-aware (see above) |
| Tier list's MARGIN badges | Catalogue-aware |
| Optimize | Catalogue-aware |
| Tilt curve | Catalogue-aware |
| Live Render viewport | Catalogue-aware |
| Retarget dialog | Catalogue-aware |
| Solid-view facet-map hatch overlay | Catalogue-aware (built-in, custom, or override) |

## Loading a design: the material suggestion

Loading a catalogue design never changes the schedule's own recorded RI on
its own. If the schedule's RI is within 0.01 of a built-in preset's own n_D,
the status strip's Details (Chapter 5) offers **"Set material to X (RI ...)?"**
-- accepting it sets the design's material to that preset (for the
optimizer/tilt-curve/viewport to use its real dispersion), but if that
preset's own n_D would otherwise move the *exported* RI by more than 0.01,
the app pins an explicit RI override to the schedule's own original value
first, so accepting the suggestion never silently changes what a
subsequent export writes. Dismissing it ("No Thanks") leaves the design
exactly as loaded, with no material name set at all.

## Retarget: an automated proposal

Click **Retarget...** on the command bar's second row (Chapter 3) to open
a dialog that proposes new pavilion and crown angles for a different
material, for you to review before anything is applied.

- **Target Material** -- a combo of built-in and custom materials, plus a
  **Custom n_D** field for a typed refractive index. It starts on the
  design's own current material. A readout below shows exactly what the
  picker resolves to: the material's name, its n_D, and its critical
  angle.
- **Mode** -- the next section explains the two in plain words.
  - **Shift** (the default) -- a deterministic move that keeps every
    pavilion tier's own margin over the critical angle fixed at what it
    already was. Always available, and instant.
  - **Optimize** -- starts from the Shift result and searches every crown and
    pavilion angle, inside a range you choose, for the best score in the
    target material. It runs only when you press **Search**, in the
    background, and offers up to three valid options to pick from.
- **Crown Handling** (Advanced) -- how the crown tiers follow the
  pavilion. There are three rules. **Crown follows the pavilion** is the
  default: a steeper or shallower pavilion makes the stone deeper or
  shallower, and every crown angle is scaled by the same vertical stretch,
  so the stone keeps its silhouette (the crown height against the pavilion
  depth) and its table size. Switch it off to use a **Fraction of shift**
  slider instead (the crown moves by that share of the pavilion's change in
  critical angle; 0% leaves the crown untouched). **Scale crown by ratio
  instead** scales every crown tier's own angle by the ratio of the two
  materials' critical angles; it wins over the other two rules.
- **The proposal table** -- one row per crown and pavilion tier: its
  block, name, old angle, new angle, margin, and risk badge, so you can see
  exactly what would change and whether it still reads Safe before
  committing to anything. What the table does and does not show is
  explained in the next sections.

### Shift or Optimize?

**Shift** is a formula. Every pavilion facet moves by the change in the
critical angle between the two materials, and the crown follows the pavilion's
stretch unless you choose another rule in Crown Handling. It is instant, it always gives the same answer, and it shows
you what "keep the same margin" means for this stone. Its limit is that it
looks at one facet at a time: for a large change of material, or a stone with
little room to spare, the result can be **Not valid** (the girdle disappears,
a facet is lost). The dialog then adds one line to the verdict: **Shift alone
is not valid here. Use Optimize.**

**Optimize** starts from the Shift result and then lets every crown and
pavilion angle move on its own, looking for the best score in the target
material. It can do two things Shift cannot: find a valid stone when the
formula's result is not valid, and give you a better stone than the formula
does, because the angles Shift leaves to a rule (the crown most of all) can be
adjusted too.

*What Optimize may change.* Every crown and pavilion angle, inside the **Range**
you pick (3, 6, 10 or 15 degrees either side of its Shift angle; 6 is the
default). An angle never crosses the horizontal and never goes steeper than
89 degrees. The facet heights that go with the new angles change too, because
each facet turns about the edge it shares with the girdle, exactly as Shift
does. *What it never changes:* the angle of the table or the culet, the
girdle, and any tier whose angle follows a relation (see "Tiers that follow a
relation" below). It also keeps at least half the girdle thickness the design
has now. The table and culet heights are refitted on every option, as Shift
does, so they keep their size. A pavilion angle never drops below the target's
critical angle plus the margin the design has now (at most 2 degrees of it), so
the search cannot buy brilliance with a window.

*Keep the design's look.* In Advanced mode the Optimize settings have a switch,
on by default. While it is on, every option (and the Shift start it is compared
with) is scored with a penalty for drifting from the table size and the
crown-to-pavilion ratio the design has now, so the search prefers options that
look like your stone; the verdict quotes the stone's depth and the
crown-to-pavilion ratio next to the old ones. Switch it off and the options are
ranked by the optical score and the yield alone.

*Allow a thicker girdle.* In Advanced mode the dialog has a switch, **Allow a
thicker girdle (up to +10 %)**, on by default. When the new angles would pinch
the girdle at its corners, Retarget thickens the girdle band by 5 % and then by
10 % (never thinner) before giving up, so a change that was **Not valid** only
because of the girdle can become valid. It is only tried after a girdle
failure; a change that is valid as it is stays exactly as it was. The plan view,
the table and culet size and the crown-to-pavilion ratio are kept, and only the
total depth grows by the girdle change. The result text says which step was
used. Switch it off to keep the girdle band exactly as it is.

*The scoring light.* The Optimize search and the dialog's optical comparison
score the stone under the **Grading tray** (the evenly lit hemisphere with a head
shadow), whatever lighting the viewport shows, so the options are ranked by the
standard light-return figures.

*The settings.* **Objective** says what the search favours (Balanced weighs
windowing, extinction and brilliance equally; Brilliance, Low windowing, Low
extinction and Keep weight lean one way, and the line under the box says
which). **Range** is described above. **Effort** is how many trial stones the
search may score: Quick 100, Normal 300, Thorough 800. A trial stone is a
**step**.

*How long it takes.* Before you press Search the dialog times one step on your
own design and tells you, for example **Up to 335 steps, about 12 s at 40 ms a
step, plus a few seconds to score the results.** A small design finishes in
seconds; a large one at Thorough effort can take minutes. While it runs, a line
under the Search button names the stage (trying angles, fine-tuning, scoring
the results), the step count and, once there is enough to go on, the most time
that is left. **Cancel** stops the search without closing the dialog. The
search is seeded, not random: the same design, settings and target give the
same options every time.

*The options.* When the search ends, up to three options are listed, best
score first (lower is better). Each shows its score, windowing, brilliance,
extinction, the share of the rough the finished stone gives up (**yield
loss**), and **Valid**. Only valid options are listed: every result of the
search goes through the same checks as a Shift result (see "The verdict"
below) against the design as it is now, and a result that fails is dropped. A
note under the summary says how many were dropped and why. If the Shift result
itself was not valid, the search starts from the largest part of the Shift
change that is valid and the first note says so. If the search finds nothing
better than where it started, the note says that too, and what to try (a wider
range, another objective, more steps).

*Picking one.* Click an option and the table, the verdict, the optical
comparison and the comparison pane all switch to it, so you can look before you
decide. Nothing is applied by searching or by picking. **Apply stays disabled
until an option is picked**, and then commits that option as one undo step.

### What Retarget changes, and what it never touches

- **The table and the culet never change.** A flat facet has no slope, so
  there is nothing for a critical angle to say about it; tilting one is
  what used to push the table through zero and onto the pavilion side.
  Both are still listed, greyed out and marked **Not changed**, so you can
  see that they stay put. Their height may still be adjusted (see below),
  but never their angle.
- **The girdle is not listed at all.** Its facets are vertical and
  structural, not optical.
- **A tier that follows a relation is never changed on its own.** If a tier's
  angle is set by a relation such as `P1 - 2` (Chapter 4, "Relations between
  tiers"), its row reads **Follows** and the relation where the margin goes.
  It moves exactly as far as the tiers it reads, in both modes: Shift works
  out the new angle from the relation after the others have moved, and
  Optimize leaves it out of the search and works it out again for every
  option. If the relation cannot hold after the change (the result would not
  be a facet angle), the change is **Not valid** and the reason says so.
  Applying keeps the relation true in the same undo step.
- **A shifted angle never crosses the horizontal.** A new angle always stays
  on the side of the old one (crown stays crown, pavilion stays pavilion) and is
  held between 1 and 89.5 degrees. A row that would have gone beyond a limit
  is held at the limit, shows **(held)** after its new angle, and a note
  under the table says which row and why.

### How the facets move: they turn about the girdle

Changing a facet's angle alone is not enough: every facet is also cut to a
height (its mast), and leaving the height alone while the angle changes
swings the facet around a point deep inside the stone. A steeper crown or
pavilion then pushes out past the girdle, or eats it. That is how a retarget
could make the girdle disappear.

Retarget therefore turns each facet about the edge it shares with the
girdle, so the girdle keeps its outline and its thickness (unless the thicker-girdle
allowance had to thicken it, see above). Where one facet
sits next to another that also moves (a main next to a break facet), it
follows its neighbour's new position. The table and the culet then get a new
height so they keep their size even though the facets around them tilted.
The line under the verdict says how the heights were chosen. Only the
heights change this way; every angle is exactly the one in the table you
reviewed.

### The verdict: Valid or Not valid

Under the table the dialog judges the retargeted stone and says so in one
line, for example **Valid: girdle 2.1 % (was 2.3 %), thinnest point 0.40 %
(was 0.50 %), table 56 % (was 56 %)**. The girdle figure is its overall
thickness; the thinnest point is the narrowest the girdle band gets, which on a
faceted girdle is at its corners. The thinnest-point part appears only when it
differs from the overall figure.
It shows **Checking...** while it works (a moment, in the background; the
dialog stays usable) and re-checks every time you change the target material
or the crown handling.

A change is **Not valid** when the stone would no longer be a sound design.
The reasons are listed in plain English, and **Apply is disabled** until you
change the settings so it is valid again:

- the facets no longer close into a stone;
- the girdle disappears, or becomes less than half as thick as it was, either
  overall or at its thinnest point (a stone whose girdle closes to a knife edge
  at its corners is refused even if the overall thickness is unchanged), after
  the thicker-girdle allowance above has had its two tries;
- the table would sit below the top of the girdle, or the culet above the
  bottom of it;
- a tier loses all or some of its facets;
- facets become too small to cut;
- a tier that follows a relation cannot follow it any more.

In Shift mode the verdict is worked out for the formula's result. In Optimize
mode it is the verdict of the option you picked, and the options are always
valid. **Compare...** keeps working on a change that is not valid, so you can look at
what goes wrong; it only waits while the check is running. If the current
design itself does not solve, the check cannot compare anything: the dialog
says **Could not check this change**, and Apply stays available with the plain
angle change, as it always did.

### The Risk column and its margin

The margin is measured in degrees on the safe side of the target material's
critical angle, and it means different things per block (hover the **Margin**
header for a reminder):

- **Pavilion rows** show the plain margin for light entering through the
  table: **Safe** at 2 degrees or more, **Marginal** from 0 to 2,
  **Windows** below 0 (light leaks out of the back).
- **Crown rows** show an estimate, suffixed **est.**, of the same margin for
  light entering through that crown facet, read against the main pavilion
  angle of the *retargeted* design (the same estimate the tier list shows,
  see "The tier list's MARGIN column" above). A crown facet is never judged
  with the pavilion formula, which is what used to flag every crown row as
  Windows. The estimate follows a single ray through that crown facet and onto
  the main pavilion facet on the same side of the stone, and nothing else:
  light that wanders to other facets is not counted. Read it as a guide, not
  a measurement. The dialog repeats this in a note whenever crown estimates
  are shown.
- **The table and the culet** show **Not changed** and no margin. A row with
  nothing to measure against (a crown row in a design with no pavilion) shows
  a dash.

### The optical comparison

Below the verdict a small table compares three stones on the same three
numbers (windowing, brilliance and extinction, each a percentage of the light,
measured with the table up under the Grading tray): the **current**
design in its current material, the **current** design unchanged in the
**target** material, and the **retargeted** design in the target material. It
shows what the material change costs the stone as it is and what the
retarget wins back. It is a fast estimate for comparing these three columns
with each other, not a replacement for a full tilt sweep (Chapter 8). A column
that cannot be measured reads **n/a** (for example the first one when the
design names no material).

### Errors that stop a proposal, and Apply

A design that does not close cannot be retargeted at all: the table is replaced
by **"Design does not solve: ..."**, and a search says it needs a design that
solves first. Optimize works on imported designs whose tiers are all pinned
scale references (Chapter 8): it moves them about the girdle edge, so there
is nothing to **Adopt** first.

Click **Apply** to commit the whole proposal as **one** undoable edit --
the new angles, the adjusted facet heights and the material change
together, so a single Undo reverts all of it at once (the history entry
reads "Retarget for" and the material's name). In Optimize mode that is the
option you picked. Applying re-solves the design; check the status strip
afterward the same as any other edit. The Return key and the compare
window's **Keep after** obey the same rules as the button: a change that is
still being checked, or is not valid, or an Optimize mode with no option
picked, is refused with a message.

Chapter 14 covers two related tools that live in the same part of the
command bar: **Snapshot Design** / **Compare to Snapshot**, for diffing
your current angles against an earlier point in the same session, and the
tilt-curve persistence rule for a design that is not yet in your catalogue.

## Doing it by hand

Retarget's Shift mode already automates the mechanical part of this, but
understanding the reasoning is worth knowing, and Design Settings' MARGIN
column is what you would watch either way:

1. Load the design and note its current pavilion main angle(s) and its
   effective RI (the Design Settings panel's own readout).
2. Work out the critical angle for the target material (critical angle =
   arcsin(1 / RI), measured from the facet's normal). A higher RI gives a
   smaller critical angle; a lower RI gives a larger one.
3. Set the Design Settings panel's Material combo to the target material
   (or type a Custom RI). Watch the tier list's MARGIN column: any pavilion
   tier that turns red windows at the new RI.
4. On the inspector's Tier tab (Chapter 4), select each red or amber
   pavilion tier in turn and edit its **Angle (deg)** field so the MARGIN
   column reads green again, then **Save Tier**.
5. Click **Solve** (Chapter 5) and check the status strip still reads
   "Closed solid."
6. With "Linked to design" on, the Live Render viewport already shows your
   target material -- check the **windowing** and **extinction** readouts
   (Chapter 2) at a range of tilt angles using the tilt-performance graph.
   Iterate on the crown and pavilion angles until performance looks right.
7. Optionally, once at least one tier is a free (meet-based) tier rather
   than a pinned scale reference (see Adopt, Chapter 8), run **Optimize** to
   fine-tune angles for windowing/extinction/tilt-brilliance under the
   design's own material -- Optimize resolves the SAME material (built-ins,
   custom catalogue materials, and any RI override) the viewport and tilt
   curve use, so there is no separate "which material did Optimize actually
   score against" question.

## Authored versus effective refractive index

The app keeps two distinct notions of "the design's RI": the AUTHORED
value (the legacy `.asc` `I` line as last imported, before any material
selection or override) and the EFFECTIVE value (override, else a resolved
material's own n_D, else the authored value as a last resort —
`Design::effective_refractive_index`). Every scoring, rendering and export
path uses the effective value. The `.indicatrix` design file keeps
the authored value separate, so it survives a Save / reopen round trip
on its own, with no `.asc` beside it (see Chapter 11).

## Next steps

Continue to Chapter 7 for a full worked example of building a design from
nothing, using the New Design dialog and putting the tier form, constraints,
and Solve together in practice.
