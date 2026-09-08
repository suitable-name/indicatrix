# 13. The Solid Inspection View

## What you will do

This chapter covers the Edit tab's Solid viewport: a second, independent
view of your design's actual solved shape, shown alongside the ordinary
spectral render. You will learn the three view modes, how to orbit and
pick facets, what hover and click do, the hatched and outlined overlays,
and what the "Not solved" banner means here versus in Chapter 5.

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
  path tracer. This is where hover and click picking work.
- **Path-traced** — the same image the Live Render tab shows, forwarded
  here so you can compare without switching tabs.
- **Both** — the path-traced image with the solid's own facet edges drawn
  on top of it, so you can see exactly which facet boundaries line up with
  which highlights in the rendered image.
- **Diagram** — a GemCAD-style flat 2D facet diagram (crown, pavilion, and
  profile panels with the index wheel), covered in its own section below.

Your chosen mode is remembered across restarts (Chapter 1 covers where
settings are stored generally).

## Orbiting

Drag inside the Solid viewport to orbit. The camera is shared with the
Live Render viewport — dragging in one moves the other, so the two views
always show the stone from the same angle. Diagram mode is the one
exception: its three panels are fixed orthographic projections, not tied to
this camera, so dragging inside the viewport does nothing while Diagram is
selected.

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
outlined in the pending colour.

Hovering and clicking work exactly like the Solid view (see above), just
resolved against the diagram's own layout — click a facet in any of the
three panels and its tier is selected in the Cutting Schedule list below,
the same as clicking the Solid render.

## Hovering and clicking a facet

Move the mouse over the Solid image (mode **Solid** or **Both**) and a
small tooltip panel appears in the lower-left corner, naming:

- the tier the facet belongs to,
- its angle,
- its index position on the index wheel,
- which block it is in (Crown, Pavilion, or Girdle), and
- its margin over the critical angle at the design's current refractive
  index (positive means safely below critical angle; see Chapter 6 for
  what that number means for windowing).

**Click** a facet to select its whole tier in the Cutting Schedule list
below — the row highlights, and every facet belonging to that tier (its
whole symmetric orbit, not just the one you clicked) is tinted in the
viewport. This works in reverse too: click a row in the tier list, and its
facets tint in the Solid viewport, so you can always see exactly what a
row in the list corresponds to on the actual stone.

## The two overlays

Two visual cues appear directly on the solid, independent of hover/click:

- **Hatched facets** — diagonal stripes mark a pavilion facet whose angle
  puts it at risk of windowing (light leaking straight through instead of
  reflecting) at the design's current refractive index. This is the same
  windowing check Chapter 6 and the tier list's margin column use,
  surfaced directly on the geometry.
- **Outlined-in-a-different-colour facets** — a facet outlined rather than
  its usual dark edge colour marks a tier whose edit has not been folded
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
   facets outlined (see the overlay above) and this banner underneath the
   viewport:

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

## Next steps

Chapter 5 covers the ordinary (Live Render-facing) Solve action and its
own status banner in full; Chapter 6 covers the refractive-index margin
the hatched-facet overlay is built on.
