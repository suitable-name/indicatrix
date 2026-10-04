# 13. The Solid Inspection View

## What you will do

This chapter covers the Edit tab's Solid viewport: a second, independent
view of your design's actual solved shape, shown alongside the ordinary
spectral render. You will learn the four view modes, how to orbit, zoom,
and pick facets, what hover and click do, the hatched and outlined
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
the same as clicking the Solid render. This click-to-select mapping is
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
handles are hidden in Diagram mode, while the Cut slider shows only part of
the design, and whenever the solid on screen is behind the design you are
editing; they come back when the preview catches up.

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
