# Appendix A: Glossary

Terms used throughout this manual, gathered in one place. Cutting-schedule
terms are introduced fully in Chapter 3; optical terms in Chapters 2 and 6.

**Anchor** — A tier whose scale is stated directly (Meets: **Exact scale
value**) rather than derived from meeting other facets. Every block (crown,
pavilion, girdle) needs at least one anchor, or its overall size has
nothing to be measured against.

**Birefringence** — The property of some materials that splits light
entering them into two rays travelling at slightly different refractive
indices. Reported in this manual as Δn, the numeric difference between
those two indices at the sodium D line. Zero for an isotropic material.

**Brilliance** — The percentage of light returned to the eye by a rendered
stone; one of the app's optical performance readouts.

**Chromophore** — The trace element or defect in a gem material responsible
for its colour (for example, Cr3+ in ruby, Fe2+ in aquamarine).

**Critical angle** — The angle, measured from a facet's normal, beyond
which light hitting that facet from inside the stone reflects internally
rather than escaping. Set by the material's refractive index: a higher RI
gives a smaller critical angle.

**Crown** — The block of a stone above the girdle, culminating in the
table.

**Culet** — The lowest point of the pavilion on many cuts, where the
pavilion facets meet at (or near) a point.

**Deep Solve** — A slow, read-only diagnostic that checks a solved design's
geometry against the catalogue's own printed proportions. Never modifies
the design (Chapter 8).

**Detach / Reattach** — The toggle that exempts a tier's index positions
from automatic orbit-consistency handling, for a deliberately asymmetric
design (Chapter 4).

**Dispersion** — How much a material's refractive index varies across the
visible spectrum, which is what produces "fire" (spectral flare). Reported
in this manual as the Abbe number V_d (lower means more dispersive) or, for
a few entries, directly as Δn(F–C).

**Extinction** — The percentage of a stone's face reading as dark, dead
shadow rather than bright; one of the app's optical performance readouts.

**Fire** — A unitless index of spectral flare (the rainbow-coloured flashes
caused by dispersion); one of the app's optical performance readouts.

**Free tier** — A tier whose Meets constraint is **Unspecified vertex** or
**Named facet(s)** — its depth is derived by the solver from where its
plane meets its neighbours, rather than stated directly.

**Girdle** — The narrow band at a stone's widest point, separating crown
from pavilion.

**Index** — The position on the cutting machine's dividing head (its
"gear") where a facet is cut.

**Mast** — The solved distance of a facet's plane from the centre, along
its own normal — the number a cutting machine's mast gauge would read.
Always a solver output, never something you type in directly.

**Meet point** — The point where three or more facet planes intersect. A
facet's depth is usually determined by where its plane meets its
neighbours, rather than set as a direct number.

**Optical character** — Whether a material's index behaviour is isotropic
(one index in every direction), uniaxial (positive or negative, one
special axis), or biaxial (positive or negative, two special axes). See
Appendix C.

**Optimize** — The coordinate search that adjusts a design's free tiers'
angles to improve windowing, extinction, and tilt brilliance (Chapter 8).

**Orbit** — The complete set of index positions a symmetric tier occupies.

**Pavilion** — The block of a stone below the girdle, usually culminating
in a culet or point.

**Pinned tier** — A tier whose Meets constraint is **Exact scale value** —
its size is a stated number, not derived. Every tier loaded from a real
`.asc` file starts pinned.

**Pleochroism** — A material showing different colours (dichroism for two,
trichroism for three) depending on the direction light travels through it,
relative to its crystal axes.

**Preform** — The rough shape a design is cut from before faceting (for
example, a cylinder).

**Refractive index (RI)** — A measure of how strongly a material bends
light, expressed as n at a reference wavelength (this manual, like the
app's render materials, uses the sodium D line, 589.3nm, written n_D).

**Scintillation** — The percentage measure of sparkle as light and stone
move relative to each other; one of the app's optical performance
readouts.

**Solved / stale** — A design is "solved" once Solve has successfully run
against its current tier list; it becomes "stale" the moment any further
edit changes that tier list, until Solve runs again (Chapter 5).

**Table** — The large, flat facet at the top of the crown.

**Tier** — One row of the cutting schedule: one facet, or one symmetric
family of identical facets cut at the same angle and depth.

**Tilt curve / tilt axis** — The app's sweep of a design's optical
performance as it tips away from face-up viewing, along a chosen compass
direction around the stone (Chapter 2).

**Windowing** — The percentage of a stone's face reading as see-through
rather than reflective, caused by light leaking straight through the
pavilion instead of totally internally reflecting; one of the app's
optical performance readouts.

## Next steps

Appendix B lists the app's keyboard shortcuts; Appendix C tables every
built-in render material's optical properties.
