# 13. The Solid Inspection View

## What you will do

This chapter covers the Edit tab's Solid viewport: a second, independent
view of your design's actual solved shape, shown alongside the ordinary
spectral render. You will learn the four view modes, how to orbit, zoom,
and pick facets, how the Cut slider shows the stone step by step, what hover
and click do, the hatched and outlined
overlays, and what the "Not solved" banner means here versus in Chapter 5
— including the case where an edit genuinely does not solve at all.

## Why a second viewport

The Live Render viewport shows a physically based spectral render — the
gem as light actually behaves in it, in whatever material you have chosen.
It is expensive to keep exactly current, so (per Chapter 5) it only
refreshes on an explicit Solve. The Solid viewport is a much cheaper,
flat-shaded rendering of the same solved geometry, built for one job:
letting you see and pick individual facets while you edit, updated after
almost every change — not just after Solve.

## The four view modes

Four buttons sit above the Solid viewport:

- **Solid** — the flat-shaded software render, independent of the spectral
  path tracer.
- **Path-traced** — the same image the Live Render tab shows, forwarded
  here so you can compare without switching tabs. Hover and click picking
  still work here (see "Hovering and clicking a facet" below); you just
  won't see the facet tint or edges the Solid render draws underneath.
- **Both** — the path-traced image with the solid's own facet edges drawn
  on top of it, so you can see exactly which facet boundaries line up with
  which highlights in the rendered image.
- **Diagram** — a GemCAD-style flat 2D faceting diagram (crown, pavilion, and
  profile panels with the index wheel), covered in its own section below.

Your chosen mode is remembered across restarts (Chapter 1 covers where
settings are stored generally).

**The toolbar and the hint strip.** Every button above the viewport has a hover note
that says what it does, and you can reach each one with the Tab key and press it with
Space or Enter; the viewport itself shows a coloured outline while it has the keyboard.
A round **?** at the end of the toolbar opens this chapter. Simple mode (Chapter 17) hides
the **Tilt Curve** and **Save View Preset** buttons; **Preform**, **Snap** and **Slice**
stay. The thin strip under the toolbar shows what the pointer will do and carries its own
small buttons, each with a hover note too.

## Orbiting

Drag inside the Solid viewport (in Solid, Path-traced, or Both mode) to
orbit, and scroll to zoom. Both are shared with the Live Render viewport —
orbiting or zooming in one moves the other, so the two views always show
the stone from the same pose. **Front** and **Top** buttons above the
viewport snap that shared camera to the same canonical poses Chapter 2
describes. A drag that barely moves before you release the mouse is still
treated as a click, not a drag — see "Hovering and clicking a facet" below.

Diagram mode does not share this camera — its three panels are fixed
orthographic projections — but dragging and scrolling still do something
there: they pan and zoom the diagram image itself (clamped so you can't
drag it entirely off screen), and a **Reset View** button appears in
Diagram mode to snap both back to their defaults.

## The Cut slider

The small slider in the toolbar, with a short label beside it, lets you
watch the stone being cut. It works in **Solid**, **Path-traced** and **Both**
mode, and appears as soon as the design has at least one tier.

- **All the way left is the rough.** The label reads **Rough** and the
  viewport shows the uncut starting block, before any facet exists.
- **Each step to the right adds one tier.** The label names the tier you
  have just cut, for example **After P1 Pavilion Main (3 of 9)**: the first
  three tiers of the cutting steps are on the stone, the rest are not. The
  label starts with the tier's code (C1, P2, G1, T, and so on), the same
  label the tier list and the cutting sheet use, followed by the tier's own
  name when it has one. The number after "of" is the total number of
  steps, and every tier counts as one step, whether it is an ordinary
  facet tier or a concave one (a groove or a dimple).
- **All the way right is Finished.** The label reads **Finished** and you
  see the complete design, as if the slider were not there. This is where
  the slider sits whenever you start or open a design.

The slider moves one whole step at a time. While it is anywhere but
Finished, its border turns amber so you can tell at a glance that you are
looking at a part of the design.

**Which order are the steps in?** The order in which the stone is actually
cut, the same order as the cutting sheet and cutting mode (Chapter 11), so
every step looks like a real stage of the work: the pavilion and girdle
facets first, then the concave pavilion tiers, then the crown facets (the
Table excepted), then the concave crown tiers, and the Table last. The label
counts positions in that order, so a tier that sits early in the tier list
can be a late step (a Table at the top of the list is the last step), while
the tier list itself still shows the order you stored.

**Every position is a whole stone.** A step only ever removes material from
the rough, so even the rough and the earliest steps are closed solids you
can orbit and pick. Facets that belong to tiers not cut yet are not on the
stone, so hovering or clicking only finds the tiers shown, and the facet it
names is the one drawn under the pointer, also in a design with concave
tiers. If you edit
the design and it now has fewer steps than the slider's position, the
slider goes back to Finished rather than showing a position that no longer
exists. If the design does not solve at all right now, the rough is still
drawn (it needs no solved tier), with the banner naming the problem as
described under "When an edit genuinely does not solve" below. The later
positions use the last solid that closed; when there is none that still
fits the design (you added or removed a tier since), they stay blank
until the edit solves, and the banner says why.

**Both mode.** The path-traced picture is drawn from the same cut stone,
but it takes a moment to catch up with each step. Until it does, **Both**
shows the flat-shaded stone with its edges, with a small note saying so,
rather than a path-traced picture of the wrong stone. The Live Render tab
and the path-traced mode draw from the same geometry, so they follow the cut
as well; put the slider back to Finished when you want a picture of the
whole design. Over the Live Render picture a small amber badge says that the
stone is cut back and has a **Show finished** button that puts the slider
back for you. An export, the tilt video and the tilt curves never use the cut
stone; they always draw the finished gem (Chapter 9).

**Diagram mode.** The Diagram always shows the finished design, because its
panels are a reference drawing of the whole stone. Switching to Diagram
puts the slider back to Finished and hides it; when you switch back it
stays at Finished. **New**, **Open** and loading a design from the library
also put the slider back to Finished.

**The Optimize and Retarget previews** are cut to the slider too: with the
slider on step 3, a candidate shows its own first three tiers.

## Diagram view

Diagram mode draws your design the way a faceting reference diagram
usually does: three flat panels side by side.

- **Crown** — looking straight down at the top of the stone.
- **Pavilion** — looking straight up at the bottom of the stone. Because
  this is the opposite side of the same view, it is mirrored left-to-right
  relative to the crown panel — the same physical index lands on the
  opposite side of the wheel (index 0 stays at the top on both panels;
  index one quarter of the way around the gear lands at 3 o'clock on the
  crown panel and 9 o'clock on the pavilion panel).
- **Profile** — a side elevation showing the stone's height, with every
  facet that isn't purely crown- or pavilion-facing drawn in relief. This
  is what shows you the crown angle, girdle, and pavilion angle stacked up
  the way a lapidary's reference cross-section does.

The crown and pavilion panels are ringed by the **index wheel**: one tick
per gear tooth, with a longer tick and the tooth number every 8th tooth (or
every 4th on a small gear of 64 teeth or fewer). Index 0 always sits at the
top of the wheel.

Large enough facets are labeled with their tier name directly on the
diagram, and the same overlays as the Solid view apply here too — a
selected tier's facets are tinted, a critical-angle-risk facet is hatched,
and a facet whose edit has not been folded into the shown solid yet is
outlined in the pending color.

Hovering and clicking work exactly like the Solid view (see above), just
resolved against the diagram's own layout — click a facet in any of the
three panels and its tier is selected in the Cutting Instructions list below,
the same as clicking the Solid render. The exact facet you clicked is
remembered the same way too, so switching between the Diagram and the Solid
view keeps the same facet picked, and the drag handles (see "Handles in the
Diagram view" below) sit on it. Hovering a facet draws it on top of the
selected tier's tint instead of replacing the tint. This click-to-select mapping is
built the moment the diagram is (re)drawn in Diagram mode, so if you switch
into Diagram mode and click immediately, before the panels have redrawn
for the current design, the click does nothing; give it a moment (or make
any edit, which redraws it) and clicking will work as described.

## Hovering and clicking a facet

Move the mouse over the viewport in **Solid**, **Path-traced**, or **Both**
mode and a small tooltip panel appears in the lower-left corner, naming:

- the tier the facet belongs to,
- its angle,
- its index position on the index wheel,
- which block it is in (Crown, Pavilion, or Girdle), and
- its margin over the critical angle at the design's current refractive
  index (positive means safely below critical angle; see Chapter 6 for
  what that number means for windowing).

**Click** a facet to select its whole tier in the Cutting Instructions list
below — the row highlights, and every facet belonging to that tier (its
whole symmetric orbit, not just the one you clicked) is tinted in the
viewport. This also works in Path-traced mode, even though the tint itself
is harder to see without the Solid render's own facet edges under it;
switch to Both if you want to see exactly which facet you picked. This
works in reverse too: click a row in the tier list, and its facets tint in
the Solid viewport, so you can always see exactly what a row in the list
corresponds to on the actual stone.

A tool facet of a concave tier (a groove or a dimple) works the same way: clicking
it, in the Solid view or the diagram, selects its concave row and opens the
concave form, and selecting a concave row tints all of its tool facets (Chapter 16).

## Dragging a facet: the angle, depth and index handles

Once a tier is selected (by clicking one of its facets, or by clicking its
row in the tier list) and the design is solved, three handles grow out of
that facet's centre in **Solid**, **Path-traced**, and **Both** mode. Each
is a colored line ending in a marker with a letter beside it, and each
drags the **whole tier** — every facet of its symmetric orbit — not just
the facet you grabbed:

- **A, the angle handle** (a cyan circle) tilts the tier. Drag it along its
  line to make the facet steeper or shallower; the angle stays on its own
  side of zero, so a crown tier stops at 0° rather than turning into a
  pavilion tier.
- **D, the depth handle** (a blue square) moves the tier in or out along
  its own normal, which changes its **mast**. This *pins* the tier to that
  mast, replacing whatever it met before (a meet against other facets, or
  "meet at any vertex"). When the drag ends, the toast says what the tier
  used to meet, and Undo restores it.
- **I, the index handle** (an amber diamond) turns the tier around the
  index wheel, a whole number of teeth at a time. A tier with no index
  positions has no index handle.

Move the pointer onto a handle and it grows and brightens; the line under
the toolbar says what dragging it does, and how many other tiers meet this
one by name (and so will follow it). Press and drag to change the value. You
see the result live: the solid re-solves as you move, and the line under the
toolbar reads, for example, "P1 -> 41.3 deg, 3 other tiers follow". Every
tier whose mast moves because of your drag is outlined in **orange** while
you drag, so you can see, before letting go, what else the change is
pushing around. If the re-solve falls behind the pointer, the last solved
solid stays on screen with the dragged tier outlined in the same
"catching up" color as after any edit; nothing is lost.

**Snapping.** By default an angle snaps to 0.1° and a mast to 0.01. Hold
**Shift** while dragging for fine steps (0.01° and 0.001). The **Snap**
pill in the toolbar turns angle and depth snapping off entirely. The index
handle always moves in whole teeth, whatever the pill says.

**One drag, one undo step.** However long you drag, and however many times
you pause, the whole gesture is a single step in the undo history: press
**Undo** once and the tier is back where it started. When you let go, a
toast names the result and reminds you that Undo restores it.

**Escape cancels.** Press **Escape** while dragging (before you let go of
the mouse button) and the design goes back exactly as it was when you
pressed. Escape with nothing being dragged clears the selection, as before.

A drag never starts on a design that is not solved yet, and a depth drag
needs the tier's solved mast, so the hint asks you to Solve first. The
handles are hidden while the Cut slider shows only part of the design, and
whenever the solid on screen is behind the design you are editing; they come
back when the preview catches up. The Diagram view has handles of its own,
described next.

## Handles in the Diagram view

The same three handles work in **Diagram** mode, on the flat panels. Select a
tier (click one of its facets in any panel, or click its row in the tier list)
and, once the design is solved, the **A**, **D** and **I** handles appear on
one of the panels. They edit the whole tier exactly as in the other views:
the same hint line, the same orange outline on the tiers that follow, one
undo step for the whole drag, a toast when you let go, and **Escape** while
dragging puts the design back as it was. The **Snap** pill is shown in
Diagram mode too and does the same thing; hold **Shift** for fine steps.

**One panel at a time.** The handles sit on the panel your pointer last
entered, as long as that panel offers handles for the facet; move the pointer
onto another panel and they move with it. They are anchored on the facet you
clicked, or, when you picked the tier from the list, on the first facet of the
tier that has handles anywhere. A tier whose facets are all drawn in a panel
where they have no handle (see below) takes its handles from another panel.

**What each panel offers.**

- **Crown and pavilion.** **A** and **D** both lie along the line from the
  centre of the wheel out through the facet, **A** nearer and **D** farther
  out so the two never sit on top of each other. Drag **A** away from the
  centre to make the facet steeper, towards the centre to make it shallower;
  drag **D** away from the centre to move the tier outward, towards the centre
  to move it inward (this pins the tier's mast, as described above). **I** lies
  along the wheel. Drag it round the wheel: the pointer's angle around the
  middle of the panel is the turn, so moving it in the direction the tooth
  numbers grow on that panel turns the tier to higher indices by the same
  number of teeth, and you can go round more than once. The pavilion panel is
  the crown's mirror image, so its numbers grow the other way round, and the
  handle follows them.
- **Profile.** Only a facet seen exactly side on, so that it shows as a line,
  has handles here (the girdle, for example): **A** runs along the line and
  **D** across it. There is no index wheel in the profile, so no **I**.

**Handles a panel leaves out.** A handle is not offered where dragging it
would barely move on the picture, because a pixel of drag would then change
the value far too much. The table, seen from above, has no **D** (its depth
points straight at you) and no **I** (it has no index positions). A facet
steeper than about 81 degrees has no **A** on the crown and pavilion panels,
and one shallower than about 9 degrees has no **D**; the profile shows the
steep ones side on. As in the other views, a tier with no index positions has
no **I**, and a tier whose angle follows another tier's through a relation has
no **A**. When no panel offers a handle for any facet of the selected tier,
no handles are shown.

**Zoom and pan.** The handles stay in step with the picture when you zoom
and pan, and keep their size on screen, so a zoomed-in diagram grabs them just
as easily as a zoomed-out one. The mouse wheel is ignored while you are
dragging a handle, so the picture holds still under your pointer.

**What is not offered here.** The **Slice** tool needs the 3D stone and is
not available in Diagram mode. While a Slice tier is waiting to be kept or
discarded, the Diagram view shows no handles; keep or discard it first.

## Slicing a new facet with the mouse

The **Slice** pill in the toolbar (or the **S** key, once you have clicked into
the viewport) lets you start a new tier by drawing on the stone instead of
typing an angle and an index. It works in **Solid**, **Path-traced**, and
**Both** mode. While Slice is on, the pill is lit, the line under the toolbar
explains what to do, and a left drag draws a line instead of orbiting the
stone.

1. **Draw the line.** Press on one side of the stone, drag across it, and
   release. A green line with an arrow head follows the pointer, with three
   faint ticks on the side that will be cut away: **the part to the right of
   your drag direction is removed** (drag left to right and everything below
   the line goes). Nothing is cut yet, and nothing enters the undo history.
2. **The provisional facet.** On release, the plane through your line and the
   eye is snapped to the index wheel (the nearest whole index, and an angle
   rounded to 0.1°) and placed so it only just touches the stone. The new
   tier appears in the viewport with a **green outline**, named with the next
   free crown or pavilion name (for example C3), and the line under the
   toolbar says how many facets it has, its angle and its index. If the side
   is the wrong way round, press **Flip** (or **F**): the same line is used, the
   other side is cut away. (In **Path-traced** mode the picture is the render
   of the committed design, which does not include the new tier until you keep
   it, so there the provisional facet shows only as its handles; use **Solid**
   or **Both** to see the cut.)
3. **Cut it in.** The provisional tier has the same three handles as any
   other tier. Drag its **depth handle inward** to make the cut deeper; the
   angle and index handles work as described above. These drags edit only
   the provisional tier: there is no toast and no undo step yet, and Escape
   while dragging puts the tier back the way it was when you pressed. A new
   facet starts exactly touching the stone, so it cuts nothing at first: the
   line under the toolbar says "Drag the depth handle inward first", and
   **Keep** refuses (with the same message) until the facet actually touches
   the stone. You can orbit, zoom, or switch between Solid, Path-traced and Both
   while you work; the provisional facet stays on screen. If the Cut slider
   is set short of the new tier, the line under the toolbar asks you to move
   it to the end.
4. **Symmetric or single.** The **Symmetric** pill (on by default) gives the
   new tier the whole symmetric set of its index under the design's symmetry
   and mirror, like any other orbit. Turn it off to cut a single index; the
   tier keeps its angle and depth when you switch.
5. **Keep or discard.** **Keep** (or **Enter**) adds the tier to the design as
   **one undo step**: the toast names it and says Undo removes it, its row is
   selected in the tier list, and Slice mode ends. **Discard** (or **Escape**)
   drops it; the design is exactly as it was. Escape with no provisional tier
   leaves Slice mode. You can also draw a second line to replace the
   provisional tier.

While a provisional tier exists, the design underneath must not change. If
anything else edits it (a nudge, an undo, opening another design), or you
select a different tier in the tier list, the provisional tier is discarded
on its own with a short message saying so, rather than being kept against a
design it was not cut from. Clicking a facet in the viewport selects nothing
until you Keep or Discard.

## The two overlays

Two visual cues appear directly on the solid, independent of hover/click:

- **Hatched facets** — diagonal stripes mark a pavilion facet whose angle
  puts it at risk of windowing (light leaking straight through instead of
  reflecting) at the design's current refractive index. This is the same
  windowing check Chapter 6 and the tier list's margin column use,
  surfaced directly on the geometry.
- **Outlined-in-a-different-color facets** — a facet outlined rather than
  its usual dark edge color marks a tier whose edit has not been folded
  into the shown solid yet (see "Live update and the 'Not solved' banner"
  below).

## Live update and the "Not solved" banner

Unlike the Live Render viewport, most edits *do* update the Solid view —
a single tier's angle, name, or indices change is normally resolved and
redrawn well within the time it takes to notice, without ever touching the
UI thread (the actual geometry solve always runs on a background worker,
so typing never stalls, no matter how large the design). Three things can
happen after an edit:

1. **Every tier pinned** (a freshly imported design, before you have
   adopted any tier back to a real meet constraint — see Chapter 8):
   instant, no solver call needed at all.
2. **A small, well-understood edit** on a design with at least one free
   tier: resolved in the background and redrawn, normally still
   effectively instant.
3. **Over budget**: on a very large or heavily interdependent design, a
   background resolve can occasionally take longer than the preview's
   short budget. When that happens, the Solid viewport keeps showing the
   **last solved** solid rather than waiting, with the edited tier's own
   facets outlined (see the overlay above — if your edit touched several
   tiers at once, only the first of them is outlined this way, not all of
   them) and this banner underneath the viewport:

   > Not solved -- showing the last solved solid. Click Solve to refresh.

   This is a different, narrower situation than Chapter 5's "Not solved"
   banner: the Solid view is not blank or stale in the sense that nothing
   has been computed — it is one edit behind. Press **Solve** to bring
   everything (Solid viewport, tier list, and Live Render viewport) fully
   current.

An edit whose effect on the rest of the design cannot be narrowed down
precisely — Undo, Redo, a gear remap, or a symmetry/mirror change — always
triggers a full background re-solve rather than guessing, so you never see
a wrong mast value flash by; it is simply slightly more likely to show the
"Not solved" banner briefly on a large design until that background solve
finishes.

## When an edit genuinely does not solve

The case above is about a background resolve simply taking a moment. A
different case is an edit that does not solve at all right now — a missing
anchor, or one that makes the design Degenerate or Unbounded (Chapter 5).
Rather than blanking the viewport or leaving a stale image on screen with
no visual sign anything is wrong, the Solid viewport keeps showing the
**last solid that did close, dimmed**, so you can still see the stone's
last good shape while you fix the edit that broke it. The banner
underneath names the actual problem (the same wording Chapter 5's status
strip shows), not the generic "showing the last solved solid" text above.

## Next steps

Chapter 5 covers the ordinary Solve action and its own status strip in
full; Chapter 6 covers the refractive-index margin the hatched-facet
overlay is built on.
