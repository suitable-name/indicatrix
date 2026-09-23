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

The renderer ships 32 built-in materials, used both by the Live Render
viewport and as starting templates in the Material Editor. Full dispersion
and colour detail for every one is in Appendix C; the figures that matter
for cutting -- refractive index, birefringence, and whether the material
has real absorption bands modelled (which mostly affects render colour, not
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
Render viewport's Render Material dropdown -- shares one list: all 32
built-in materials above, in that order, followed by every custom material
you have saved, sorted alphabetically. Whichever picker you open, you see
the same materials in the same relative order (built-ins first, then your own).

## Custom materials

Click the pencil button next to the Live Render viewport's Render Material
dropdown (Chapter 2) to open the **Material Editor**. Its fields are:

| Field | Control | Notes |
|---|---|---|
| Load Preset Template | combo | Starts you from Custom, or from one of eleven built-ins (Diamond, Sapphire, Ruby, Emerald, Tanzanite, Synthetic Moissanite, Zircon, Topaz, Spinel, Quartz, Cubic Zirconia) as a base to tweak. |
| Material Name | text field | Rejected with a warning if it matches a built-in material's own name. |
| Refractive Index (nd) | slider, 1.30–3.20 | |
| Dispersion (Fire ΔnF-C) | slider, 0.000–0.120 | |
| Specific Gravity | slider, 0.0–7.0 | Shows "not recorded" at 0 -- leave it there if you don't know the figure. |
| Birefringence (Δn = ne − no) | slider, −0.050 to +0.100 | Zero means isotropic. |
| Crystal System | combo | Cubic, Tetragonal, Hexagonal, Trigonal, Orthorhombic, Monoclinic, Triclinic. |
| Optical Character | combo | Isotropic, Uniaxial (+), Uniaxial (−), Biaxial (+), Biaxial (−). |
| Biaxial nβ − nα | slider | Only shown when Optical Character is one of the two Biaxial choices. |
| Gem Body Color & Transmission | swatches | Clear, Blue, Red, Green, Violet, Yellow, Pink, Teal, Amber. |

Buttons: **Delete**, **Cancel**, **Save**, and **Save & Apply** (saves, then
also selects it as the current render material).

**Where a custom material lives.** Saving writes it to your catalogue's own
database -- this is the copy every material picker across the app actually
reads from, and it is what makes the material available the next time you
open the app or select a different design. Separately, if a design's own
Material is set to a custom material, saving that design with **Save
Native** also writes a small snapshot of that material's numbers
(`CustomMaterialSnapshot`: RI, dispersion, birefringence, specific gravity
if set, crystal system, and optical character) into the design's own
`.indicatrix.toml` sidecar file. This snapshot exists purely so the design
still opens with its real optics if it is ever loaded on a machine whose
catalogue database has no row for that material name -- without it, a design
would silently reload as plain Diamond. You never edit this snapshot directly;
it is written and read automatically alongside Save Native and Open Native
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

**Export agrees with the on-screen figure.** Export Edited .asc, Save
Native, and the cutting-sheet export all resolve the design's Material the
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
once (there is nothing left to guess), and the name reaches Save
Native's sidecar and Export Edited .asc the normal way. If nothing built in
is within tolerance, no badge is shown at all -- the app never forces a
guess onto a material it cannot place.

## The tier list's MARGIN column

Each pavilion tier (a tier with a negative angle) shows how many degrees
its authored angle sits above the design's own effective critical angle,
colour-coded:

- **Green** (Safe) -- 2 degrees or more of margin.
- **Amber** (Marginal) -- less than 2 degrees, but still past the critical
  angle. A small edit, a manufacturing tolerance, or a different material
  could tip this into windowing.
- **Red** (Windows) -- already below the critical angle for the design's
  current effective RI. Light leaks straight through this facet.

A **crown** tier (angle positive) shows a margin too, suffixed **"(est.)"**
-- an ESTIMATE of whether light entering through that crown facet (rather
than straight down through the table) would still reflect off the
design's own main pavilion facet, derived from Snell's law at the crown
facet plus the plain pavilion margin above. It follows exactly one ray in
one cross-section and ignores every other path light actually takes
through a real stone, so treat it as a rough guide, not a certainty --
never the same weight as a pavilion row's own plain margin. A girdle tier
(angle exactly zero), or a crown tier in a design with no pavilion tier at
all to estimate against, shows a plain dash.

The same colour-coded bar appears live in the inspector's Tier tab, next
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
| Export Edited .asc / Save Native / cutting sheet | Catalogue-aware (see above) |
| Tier list's MARGIN badges | Catalogue-aware |
| Optimize | Catalogue-aware |
| Tilt curve | Catalogue-aware |
| Live Render viewport | Catalogue-aware |
| Retarget dialog | Catalogue-aware |
| Solid-view facet-map hatch overlay | Catalogue-aware (built-in, custom, or override) |

## Loading a design: the material suggestion

Loading a catalogue design never changes the schedule's own recorded RI on
its own. If the schedule's RI is within 0.01 of a built-in preset's own n_D,
the status strip's Log (Chapter 5) offers **"Set material to X (RI ...)?"**
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
- **Mode** --
  - **Shift** (the default) -- a deterministic move that keeps every
    pavilion tier's own margin over the critical angle fixed at what it
    already was. Always available, and fast.
  - **Optimize** -- seeds from the same shift, then runs the same
    coordinate search Chapter 8 describes over the design's free tiers, to
    find the target material's own best score for windowing, extinction,
    and tilt brilliance. Runs in the background with a progress readout
    that names its current stage (e.g. "Optimizing... 12 of ~200
    evaluations, 1.4s elapsed" -- see Chapter 8 for what each stage means);
    **Cancel** stops the search without closing the dialog.
- **Crown Handling** -- how much of the pavilion's shift the crown tiers
  follow: a **Fraction of shift** slider (0% leaves the crown untouched,
  which is common faceting practice and the default), or a **Scale crown
  by ratio instead** checkbox, which scales every crown tier's own angle
  by the ratio of the two materials' critical angles rather than following
  the pavilion's shift.
- **The proposal table** -- one row per affected tier: its block, name,
  old angle, new angle, margin, and risk badge, so you can see exactly
  what would change and whether it still reads Safe before committing to
  anything.

Two things stop a proposal from being built at all, shown in place of the
table: **"Design does not solve: ..."** when the design does not currently
close, and a list of tiers to **Adopt** first when Optimize mode needs a
tier that is still pinned as an imported scale reference (Chapter 8) --
Retarget's Optimize mode can only move free tiers, exactly like the
standalone Optimize.

Click **Apply** to commit the whole proposal as **one** undoable edit --
the material change and every tier's angle shift together, so a single
Undo reverts both at once. Applying re-solves the design; check the status
strip afterward the same as any other edit.

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
path uses the effective value. The native `.indicatrix.toml` sidecar keeps
the authored value separate, so it survives a Save Native / reopen round trip
on its own, independent of any paired `.asc` file (see Chapter 11).

## Next steps

Continue to Chapter 7 for a full worked example of building a design from
nothing, using the New Design dialog and putting the tier form, constraints,
and Solve together in practice.
