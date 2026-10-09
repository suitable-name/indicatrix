# 24. Rough Colour and Colour Zoning

This chapter is part of builds with the `zoning` feature. Other builds do not have the **Rough colour...** button or anything else described here.

## What you will do

Photograph a piece of rough on the camera rig, then let the program work out the colour of the stone inside it. The result is a real absorption spectrum per millimetre of path, not a texture. With it the Rough Planner can show what a cut stone of any size and orientation will look like, and a design you adopt from a plan is rendered with that colour at its real size.

If the rough is colour zoned (a watermelon or bicolour tourmaline, banded amethyst, an ametrine, a zoned sapphire), you describe the zones with simple shapes and the program fits one colour for each.

The tool is the **Rough colour** wizard. It uses the same eight-view rig as **Locate inclusion from photos** (Chapter 15), so read that part of Chapter 15 first: the rig, its photos and the alignment of the mesh to the rig are all prerequisites.

## What it does and what it cannot do

- **One light gives one colour.** The photos are taken with one backlight. The result is the stone's colour for that light, plus a prediction of its colour in other light (D65 daylight and a warm lamp, "A") that comes from the fitted spectrum. For a stone whose colour changes with the light, such as an alexandrite or a colour-change garnet, the prediction under another light is a guess from three camera channels. The program says so (see "How sure is the colour?"). It does not recover a colour change.
- **No pleochroism.** The colour does not depend on the direction the light travels in the crystal. A strongly pleochroic stone gets one average colour.
- **No scattering.** Silk, milkiness and fluorescence are not fitted. The inclusion values you set elsewhere are used as they are.
- **Birefringence is ignored.** Light is traced with the ordinary index.
- **At most four zones** besides the base colour, and each zone is one of five simple shapes. A free-form zone cannot be drawn.
- **The mesh must be a scan.** The wizard needs a mesh rough in the Rough Planner and its alignment to the rig.
- **The camera is not known exactly.** Colour from three channels cannot tell some different spectra apart. How much that matters is shown next to every predicted colour.

## What you need

- A mesh rough in the Rough Planner, saved as a plan if you want to keep the result.
- A camera rig with eight fixed cameras and a backlight, as in Chapter 15, photographed with the rules below.
- For each view: a photo of the stone and a photo of the empty rig with the backlight on (the **white frame**). A photo of the empty rig with the backlight off (the **dark frame**) is strongly recommended.
- Optionally: the camera's colour sensitivity curves, a set of reference filters, or a measured spectrum of the backlight. The wizard works without them, with less certain colours.

## Capturing the photos

### The protocol, step by step

The wizard's first step shows the same list as a checklist. In order:

1. **Lock the camera.** Fixed exposure time, fixed ISO, fixed white balance, fixed focus. No HDR mode, no scene mode, no automatic anything. If the camera can shoot **RAW** (DNG or a maker's RAW), use it: RAW is linear, and the program reads it without any tone curve or white balance. A JPEG is accepted, but its colours are less certain.
2. **Set the exposure for the empty backlight.** The empty rig should be bright but not clipped. Where the backlight is clipped, the program cannot measure how much light the stone took away.
3. **White frame, one per view.** The empty rig with the backlight on. Leave the holder in place if it stays for the stone photos. This frame tells the program how bright the light is at every pixel, so lamp unevenness and lens vignetting cancel out. To lower noise you can load more than one white frame for a view; they are averaged.
4. **Dark frame, one per view.** The backlight off, the lens not capped (a cap would hide stray light that also reaches the stone photos). Several dark frames are averaged too.
5. **Stone photos, eight views.** The same camera settings as the white and dark frames, and the cameras and the stone unmoved between frames. The photo must have the size its view says and be saved upright (Chapter 15).
6. **Optional: a second, shorter exposure** of each view, about 2 EV darker. Bright, clear parts of a stone can clip in the normal exposure. Where they do, the program uses the shorter exposure instead.
7. **Optional: reference filters** for the camera calibration (see "Camera calibration").

The wizard compares the shooting data in the files (exposure time, ISO, white balance mode, aperture and picture size) and warns in plain words when the white or dark frame of a view differs from its stone photo, when a file carries no shooting data, when part of the stone photo is clipped, or when a photo is not linear (8-bit or tone-mapped).

### Immersion

If you can put the rough in a liquid whose index is close to the stone's, do it, and enter the liquid's index as the rig's surrounding index (Chapter 15, "The rig"). Light then bends and reflects very little at the stone's skin. A frosted or uneven skin is the biggest enemy of this method, because it scatters the light and hides the body colour. Immersion removes most of that, and the colour you get is much more certain.

### Do not move anything

A shift of the mesh against the rig of a fraction of a millimetre changes what a thin edge of the stone shows. The wizard therefore leaves out a band of pixels along the stone's outline (see "Masks"), but a badly aligned mesh still gives a poor fit. Align the mesh carefully in the Locate window.

## Opening the wizard

Open the Rough Planner (Library, Plan Rough...). In the rough's form, next to **Locate inclusion from photos...**, press **Rough colour...**. The button is available when the rough is a mesh and no plan is running. The wizard is a window of its own with a strip of eight steps along the top and **Back** and **Next** at the bottom. A step opens only when the earlier ones are done; a step you cannot open yet says why in its hover note. A running step (reading photos, fitting) shows a progress bar, the stage, how much is done and about how long is left, and a **Cancel** button.

| Step | What you do there |
|---|---|
| 1 Checklist | Pick the rig, check the alignment, read the capture list. |
| 2 Photos | Load the files of each view and press **Prepare photos**. |
| 3 Calibration | Say how the camera and the backlight are known and press **Apply calibration**. |
| 4 Masks | Leave out pixels that are not the stone's colour. |
| 5 Surfaces | Say how the skin of the rough is made. |
| 6 Fit | Add zones if the stone has any, then **Start fit**. |
| 7 Compare | Look at the photo, the render and the difference; read the numbers. |
| 8 Accept | Keep the result with the plan, or export a report. |

### Step 1: Checklist

Pick the **rig** from the list. **Locate...** opens Locate inclusion from photos, where you can create a rig and align the mesh to it. The line under the rig tells you whether the mesh is aligned to this rig. The wizard reads that alignment from the Locate window; it has no alignment step of its own. If it says the alignment belongs to another rig or rough, align again there (step 2 of the Locate window).

### Step 2: Photos

Each view has a row with the buttons **Stone**, **White**, **Dark**, **2nd exposure** and **Clear**. Press a button and choose a file: RAW, TIFF, PNG or JPEG. The program never copies your files; it remembers where they are. Pressing **White** or **Dark** again adds another frame, which is averaged with the first. **Clear** forgets the files of the view.

If the stone photos are already loaded in the Locate window, **Use the photos of the locate window** takes them over.

Press **Prepare photos**. For every view that has a stone photo and a white frame, the program:

1. reads the files and turns them into linear light;
2. merges the two exposures, if there are two;
3. measures the noise from the white and dark frames;
4. divides: transmittance = (stone - dark) / (white - dark), per colour channel and pixel. This is the data the fit uses. It is the share of the backlight that gets through the stone. It cancels the lamp, the vignetting and the camera gain at once;
5. shrinks the stone's area to at most about 384 pixels across, by averaging;
6. works out the pixel masks (see "Masks").

It can take a minute. Each view then shows how much of it is usable. The warnings from the shooting data appear below the rows. The fit needs at least two views.

### Step 3: Calibration

This step says how the camera sees colour and what light shines through the stone. Press **Apply calibration** when both are set; the lines it shows afterwards tell you what the program made of your choices.

#### Camera calibration

Three levels, from best to least certain:

| Choice | What it means |
|---|---|
| **Measured curves (CSV)** | You have the camera's spectral sensitivities. Choose a text file with columns `wavelength, r, g, b` (nanometres; commas, semicolons, tabs or spaces). This is the best case. |
| **Reference filter set** | You photograph filters of known spectral transmission in the rig and the program works out the camera's sensitivities from them. Needs at least six filters, more is better. |
| **sRGB fallback (least certain)** | No information about the camera. The program assumes it sees colour like an sRGB display. This is adequate for a strongly coloured stone, and the weakest choice for a pale or a very saturated one. |

**How to make a filter set.** Use six or more colour filters (gels, for example) whose transmission curves you know, and which are spread over the spectrum: reds, yellows, greens, blues, and some neutral densities are all welcome. Photograph each one in the rig, in the place of the stone, with the backlight on and the camera settings of the session, and photograph the empty rig. For each filter work out the camera value (red, green, blue, in linear light) of a patch in the middle of the filter divided by the same patch of the empty rig. The wizard does not take this measurement for you yet: it needs the three numbers in a file.

The filter set is one text file. Each line that is not empty and does not start with `#` is

```
name, r, g, b, spectrum-file
```

where `r, g, b` are those three numbers and `spectrum-file` names a table `wavelength_nm, transmission` (fractions, not percent) for that filter, given relative to the folder of the set file. Fields can be separated by commas, semicolons or tabs. One header line is skipped. The backlight you choose below must be the one the filters were photographed with: the program uses it in the calculation.

**Calibrate the camera once per camera and lens, and keep the files.** A camera calibration does not depend on the stone.

#### The backlight

| Choice | What it means |
|---|---|
| **Find from the white frame** | The program takes the CIE LED illuminant (one of nine published spectra of typical LED lamps) whose colour is closest to what the camera saw in the white frame, and bends it a little to match exactly. A good default for a white LED. |
| **A CIE LED kind** | You say which one. Choose the kind that matches your lamp's warmth. |
| **Measured spectrum (CSV)** | You measured the lamp with a spectrometer: a text file with wavelength in nm and relative power. The best choice. |

Two more numbers describe the lamp's geometry: **Panel behind stone (mm)**, how far behind the stone's centre the diffuse panel is, and **Panel size (mm)**, the side of the square panel. Measure them; the program uses them to know where the light leaves the stone towards the lamp.

### Step 4: Masks

Pixels that do not show the stone's colour are left out of the fit. The picture shows the mask flags in colour, with a legend:

| Flag | Meaning |
|---|---|
| Your brush | Pixels you excluded. |
| Inclusion | Rays that pass through a located inclusion. |
| Ghost of an inclusion | Where an inclusion's reflected copy would appear. |
| Saturated | The photo is clipped there. |
| Below the noise | The stone let through less light than the noise can resolve. |
| Edge band | A band along the outline, because the model is least exact at the rim. |
| Outside the stone | Not the stone. |

**Edge band** is the width of that band in pixels of the working picture (the default is 3). Press Enter to apply a new value. A larger band is safer if your alignment is poor.

The brush has three modes: **Brush off**, **Exclude** (click to leave out a round patch; the radius is in working pixels, default 4) and **Restore** (click to take your own exclusion off). **Clear brush** takes away all your exclusions in the view. Use the brush on reflections, stains, dust or the holder. A view's row of pills above the picture switches the view, and **1x**, **2x** and **4x** zoom.

### Step 5: Surfaces

How light meets the rough's skin matters a lot:

- **Polished** if every face is polished (a faceted preform, a cabochon).
- **Frosted, fixed** if the skin is frosted, with a roughness you type. 0.05 is lightly frosted; 0.6 is very rough. The default is 0.20.
- **Frosted, find it** if you do not know the roughness. The program tries about five values and keeps the best. It takes about five times as long.

Rough that is mostly frosted can have polished windows, faces a lapidary ground to look in. Choose **Paint window** and click on such a face, either on the photo (the brush goes through each pixel onto the mesh) or on the rough in the Rough Planner's 3D view while this step is open. Every face under the brush becomes polished. **Erase** makes faces frosted again and **Clear windows** makes every face frosted.

In immersion the choice matters much less.

### Step 6: Fit

#### The colour model

The colour of each zone is fitted as a smooth absorption spectrum, from seven control values spread across the visible range with a gentle smoothness rule. The photos of all views are fitted together.

The program also has a model built from real colouring elements of a host mineral (iron, manganese and so on). It is available with the physical colour feature and has no controls in this chapter's builds unless that feature is on as well.

**Planned stone width (mm)** is the width of the cut stone you want predicted, next to the always-present 7 mm reference. The default is 12.

#### Zones

The list shows the **Base zone** (everywhere no zone covers) and the zones you added, as "Zone 1: Cylinder" and so on, with their key numbers. Zones are applied in order; where two overlap, the later one wins. A stone without zones has just the base zone.

Add a zone with one of the five shape buttons. The new zone starts in the middle of the rough with a plain size; you then move it to fit.

| Shape | Use it for | Numbers (millimetres, angles in radians) |
|---|---|---|
| Half space | two layers, a bicolour (one flat boundary) | Offset |
| Slab | a band between two parallel planes | Offset min, Offset max |
| Cylinder | a round watermelon core or rind | Inner radius, Outer radius |
| Prism | a watermelon whose zones follow the crystal, with 3 or 6 sides | Sides, Inner radius, Outer radius, Phase (turns the prism about its axis) |
| Sector | an ametrine or a trapiche: a wedge around an axis | Angle from, Angle to |

Select a zone in the list and its numbers appear below it. Type a value and press Enter. **Lock** keeps a number where it is when the geometry is refined later. **Remove** deletes a zone. **Undo zone edit** takes back the last change; dragging a handle counts as one change, however long you drag.

You can also drag the **handles** of the selected zone in the Rough Planner's 3D view. They appear while the wizard window is open on this step, for the selected zone only, and show: a plane's offset and the tip of its normal; the radii of a cylinder or prism and its axis; the start and end of a sector. Dragging a handle edits the zone; a drag anywhere else orbits as usual. A locked number has no handle.

The numbers are in the rough's own coordinates, in millimetres. A zone's shape, size and position are those of the rough, not of a stone cut from it; the program moves them into each stone's own frame later.

**Softness (mm)** is the width over which colours blend across every zone boundary. The default 0 is a sharp boundary. Real zones are rarely razor-sharp, and a little softness (a fraction of a millimetre) often fits better and is gentler to the solver.

#### Zones from marks

Instead of guessing numbers, mark the boundary you can see:

1. Switch **Marking** on.
2. In at least two photos, click along the colour boundary: the same points, in the same order, in each of the views.
3. Choose the shape in the list under the marks and press **Fit zone**.

The program works out where each marked point lies in 3D through the stone's refraction, fits the shape to the points, and adds the zone. It reports how far the points lie from the shape ("rms 0.08 mm") and, when there are enough views, the same figure when each view is left out in turn. A small figure means the marks agree with each other; a large one means one view's marks are off. **Undo point** takes back the last point in a view and **Clear marks** all of them.

A slab and a sector take two boundaries (the two planes of the slab, the two edges of the sector). Mark the first, press **Fit zone** (the line under the marks says "First boundary kept"), then mark the second and press **Fit zone** again. A cylinder or prism fitted from marks is a core: its inner radius is 0 and a prism starts with three sides, which you can change in the zone's numbers.

#### Suggestions

**Suggest from the photos** looks for colour boundaries in the photos (or in what the last fit left unexplained) and lists up to six candidate zones with a score and the number of views that support each. Each line has an **Accept** button. A suggestion is only a starting point: nothing is added until you accept it, and a boundary seen at a slant is less exact than one seen edge-on. After accepting, check the numbers and refine.

#### Start fit and Refine geometry

**Start fit** traces light through the stone for every view and fits the colours. This is the long step: minutes for a frosted rough with all eight views, a fraction of that for a polished one or in immersion. The tracing is stored on disk, so fitting again after changing only colour settings is fast. The progress line shows the stage and the time left, and **Cancel** stops it.

**Refine geometry** moves the unlocked zone boundaries (a plane's offset, a cylinder or prism's radii) to where they fit the photos best. It needs a fit and at least one zone. **Refinement iterations** (1 to 25, default 3) is the most rounds it makes. Each round costs the number of free numbers plus one full trace-and-fit passes, and refining stops early when a step no longer improves the fit, so a high value costs time only if it keeps improving. After refining, run **Start fit** again; the traces are stored.

### Step 7: Compare

For each view three pictures sit side by side: the **photo**, the **render** (what the fitted colour predicts) and the **difference** as a heat map of CIEDE2000 colour difference. One zoom (**1x**, **2x**, **4x**) and one centre are shared; click the photo to centre. Under them the line "Colour difference (CIEDE2000)" gives the median, the 95 % level and the worst value.

A colour difference of 1 is barely visible, about 2 is visible side by side, 5 or more is a clear difference.

The lists beside the pictures:

- **How well it fits** is the leave-one-view-out table (below).
- **Models** shows the spectral model that was fitted and how well it explains the photos.
- **Predicted colours** shows a swatch for the base zone and each zone, at 7 mm and at your planned width, under D65 and under lamp A, with the uncertainty.
- Warnings in amber.

If the unexplained difference is not random noise but has a pattern, the prompt "This looks zoned" appears with **Add zones...**. It takes you back to the Fit step.

### Step 8: Accept

**Accept** stores the result with the saved plan and closes the wizard: the zones with their colours, the fit report and the working-resolution photos (about 3 MB per view). It needs a saved plan, because the colour is stored against the plan. If you have not saved yet, save the plan in the Rough Planner first. **Export report...** writes a Markdown report and the pictures into a folder you choose, with the setup, masks, zones, models, leave-one-view-out table, predicted colours, warnings and, for every view, the photo, the render and the difference.

A build without the `zoning` feature ignores the stored colour, shows the plan with its ordinary material and never deletes it.

## Reading the result

### How well it fits: leave one view out

A fit that explains the photos it saw is not yet proof that it is right. The program therefore fits again without one view, predicts that view from the others, and compares the prediction with the real photo. It does this for every view. The table lists, for each held-out view, how far (in CIEDE2000) the prediction was from the photo, and a summary line with the median and the worst. This is the honest accuracy of the colour. A row more than 3 dE off is shown in amber. A large figure in one view often means a wrong mask, an unmodelled inclusion, a badly aligned mesh or a zone nobody described.

### How sure is the colour?

Each predicted colour is shown as Lab values, then a plus-or-minus number and a second number:

```
Lab 62.1, 41.0, 55.2  ±  1.8 dE (metamer spread 3.2 dE)
```

- The **plus-or-minus** is the uncertainty of the fit given the noise in the photos, as a CIEDE2000 radius.
- The **metamer spread** is larger when different spectra look the same to the camera and the backlight but different in other light. It is the largest colour change, under the same light, that stays within the noise when the spectrum is varied in the ways the photos cannot tell apart. It does not count the smoothness the fit prefers, so it is honest where the plus-or-minus is optimistic. It is an estimate from a quadratic approximation, not a bound.

If the metamer spread is much bigger than the plus-or-minus, the camera data cannot fix the colour better than that. More views, longer paths (a larger rough or a thicker part) or better camera calibration help; a measured camera curve helps most. Under lamp A, which asks more of the spectrum than the backlight did, the spread is larger.

### The exposure factors

Every view carries one overall brightness factor, to absorb a small drift between the stone photo and the white frame. These factors are held close to 1: the average of the factors over the views is kept within about 2 %, and each view's factor within about 3 % of that average. This is on purpose. Without it, a uniform brightness error is confused with a grey absorption that does not depend on the colour, and a stone of nearly constant thickness cannot tell the two apart.

The rule assumes that you really locked the exposure. If your camera cannot hold its exposure, expect the fit to be pulled towards the wrong brightness. The solver also has a setting to pin the average factor at exactly 1, for rigs with locked exposure. This build's wizard does not expose it.

### Models

The Models list shows the spectral model fitted. This build fits the smooth spectrum model. The model made of real colouring elements is available with the physical colour feature.

## Using the colour in the planner

Once a plan with a rough colour is saved and open:

- The stones in the result scenes and thumbnails are drawn in their own colour: the colour of the rough where the stone's face-up path runs, weighted by how much of that path each zone takes. A plan has a colour only when it is saved.
- Above the results, **Stone orientation** offers **Keep planner pose** and **Best colour**. With zones, the same layout can place a stone with different parts of the rough face up. **Best colour** turns each stone, among the few poses that are equivalent for the planner, so that as much as possible of the last zone in the list is under the table; for a watermelon that is the inner pink. The choice is saved with the plan. A rough with no zones, or an exact fit, keeps its planner pose.
- **Use colour** on a design row adopts the colour for that design: the zones are moved into the stone's own frame and saved as a custom material named after the rough with "colour" added (for example "Brazil tourmaline colour"). It carries real millimetres, so the colour scales physically with the stone and no slider is involved. The editor selects it, unlinks "Linked to design" and sets the stone width to the width of the stone in the plan. If you open the material in the material dialog, a **Zones** list shows the base zone and the zones. Editing the body colour of such a material replaces the colour of its base zone only; the zones keep theirs, and a plain re-save changes nothing.
- A render job that uses a zoned material is saved with its zones; the job renders them when it runs. A remote worker renders them only when it was built with the same feature; otherwise the picture is rendered on this computer.
- GPU rendering handles zones without scattering and without free-form shells. A zoned stone with scattering renders on the processor.

## Checking against reality

The program is tested against synthetic photos, where the right answer is known. It has not yet been measured against real stones. Do this yourself once; it takes an afternoon and tells you what to trust.

**1. Reference filters as flat roughs.** Take a filter with a known spectrum, with a thick enough layer that its colour is clear (a stack of gels, or a slab of coloured glass), and model it in the Rough Planner as a block of its size. Photograph it in the rig as a rough, run the wizard with a calibration made from other filters, and compare the fitted colour with the colour of the known spectrum. This tests your camera calibration from end to end.

**2. Rough to cut pairs.** For three to five of your roughs: fit before cutting, adopt the colour for a design, and cut it. Then photograph the finished stone face-up in daylight-like light with the same camera and compare it with the render of the adopted material at the same size. The difference is what you can expect from this method for stones like yours. Record the model, the camera calibration and the metamer spread of each fit next to the result.

**3. A negative control.** Fit a colour-change stone. The report should show the colour under your backlight, and the colour under D65 and A should come with a large metamer spread. If the program claims a small uncertainty for a colour-change stone, something is wrong.

This version of the manual contains no measured agreement figures. Add your own results to your records.

## Troubleshooting

| Symptom | Cause | What to do |
|---|---|---|
| The **Photos** step will not open | No rig, or the mesh is not aligned. | Step 1: pick a rig; **Locate...** and align the mesh (Chapter 15). |
| The **Calibration** step will not open | Fewer than two views have a stone photo and a white frame. | Load them. |
| Warning "different exposure / ISO / white balance" | The frames were shot with different settings. | Reshoot with locked settings, or shoot RAW. |
| "no white frame" | The view has no white frame. | Load one; the photo cannot be calibrated without it. |
| "X % of the stone photo is clipped" | The stone photo is overexposed. | Lower the exposure, or load a second, shorter exposure. |
| "not linear (8-bit or tone-mapped)" | The photo is a JPEG or tone-mapped. | Shoot RAW. Colours stay usable but less certain. |
| Few usable pixels in a view | A large edge band, many exclusions, or a very dark stone. | Reduce the edge band; check the mask legend for what is excluded. |
| The render is too bright or too dark everywhere | The backlight or the camera is not what you chose. | Check the backlight and its panel size and distance; use measured curves or a filter set. |
| Only the outline of the stone is wrong | Poor alignment. | Realign in the Locate window; widen the edge band. |
| The difference map shows a pattern | An unmodelled zone, inclusion, or window. | Add zones (try **Suggest from the photos**), mask the inclusion, or paint the polished windows. |
| One held-out view is far off | A bad mask or alignment in that view, or a zone seen only there. | Look at that view's difference map. |
| The metamer spread is large | The camera data cannot fix the colour. | Use measured camera curves, more views, a larger or thicker piece, or immersion. |
| "These amounts ran into their limits" | A fitted value hit its allowed range. | The stone may be more saturated than the model allows, or the mask is wrong. |
| **Accept** is dimmed | The plan is not saved. | Save the plan in the Rough Planner first. |
| "Worker has no zoning support; rendering locally" | The remote worker or coordinator was built without the feature. | The picture is drawn on this computer instead. Use a worker built with the feature if you need it remote. |

## Limitations

- One backlight, one colour. No colour change, no pleochroism, no scattering, no fluorescence.
- The camera's sensitivities are a model unless you give measured curves or a filter set. The sRGB fallback is the least certain.
- A frosted or uneven skin hides the body colour. Immersion helps most.
- At most four zones plus the base, each a half space, a slab, a cylinder, a prism or a sector.
- Suggestions from the photos are exact only for boundaries seen edge-on.
- The mesh must be aligned within a fraction of a millimetre; the edge band hides part of the error.
- Refining moves plane offsets and radii. Axes, angles, prism phase and the number of sides stay as you set them.
- The planner draws its previews with the face-up colour, not a path-traced picture, and shows a colour only for a saved plan.
- Tilt-curve sweeps of a zoned stone run on this computer and may use only the base colour of the material.
- A render job that uses a zoned stone needs a worker with the same feature to render remotely; otherwise it renders here.

## Next steps

Chapter 15 explains the Rough Planner, the rig and the alignment. Chapter 6 covers custom materials and Chapter 9 covers rendering. Chapter 23 collects renders as jobs, and Chapter 12 lists what to do when something goes wrong.
