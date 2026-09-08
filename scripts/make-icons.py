#!/usr/bin/env python3
"""Generates the Windows application icons for indicatrix's two public binaries.

Run from the workspace root:

    python scripts/make-icons.py

Writes:
    apps/indicatrix-cut/assets/icon.ico     + icon.png   (viewer, cyan)
    apps/indicatrix-worker/assets/icon.ico   + icon.png   (server, emerald)

The generated files are committed. This script exists so the icons are reproducible
and adjustable rather than opaque binaries nobody can regenerate -- rerun it after
changing a colour or the geometry below.

# Why draw the icon rather than rasterize one of `ui/icons/*.svg`

Those are 16-24px monochrome UI glyphs, drawn to read at toolbar size against a known
background. An application icon has different requirements: it needs to survive being
scaled from 256px down to 16px, carry the product's own colour rather than inheriting
`colorize`, and stay legible on an arbitrary desktop wallpaper. Drawing it here also
means each size is rendered at its own detail level (see `FACET_DETAIL_MIN_PX`) instead
of one bitmap being blurred down to 16px.

# The subject

A round brilliant seen from directly above -- the shape this whole project is about,
and the one gem outline recognizable at 16px. Geometry is the real thing rather than a
doodle: a girdle circle, an octagonal table, eight kite (bezel) facets from the table's
vertices to the girdle, and eight star facets from its edge midpoints. Facet edges are
what make a cut stone read as a cut stone, so they are drawn rather than implied.
"""

import math
import os

from PIL import Image, ImageDraw

# Rendered at this multiple of the target size, then downsampled with LANCZOS. Cheap
# supersampling: PIL's draw primitives are not antialiased, so drawing large and
# shrinking is what gives clean facet edges at every size.
SUPERSAMPLE = 8

# Below this target size the facet lines are omitted entirely. At 16px a full brilliant's
# 24 interior edges collapse into mud; the silhouette plus the table octagon still reads
# unmistakably as a gem, so small sizes get that instead of a smear.
FACET_DETAIL_MIN_PX = 32

# Every size Windows actually asks for: Explorer's list/tile/icon views, the taskbar at
# each DPI scaling, and Alt-Tab.
ICO_SIZES = (16, 24, 32, 48, 64, 128, 256)

GROUND = (15, 21, 35, 255)  # #0f1523 -- the app's own darkest surface


def _lerp(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def _rgb(hex_str):
    h = hex_str.lstrip("#")
    return tuple(int(h[i : i + 2], 16) for i in (0, 2, 4))


def _ring(cx, cy, r, n, phase=0.0):
    """`n` points evenly spaced on a circle, starting at `phase` radians."""
    return [
        (cx + r * math.cos(phase + i * 2 * math.pi / n),
         cy + r * math.sin(phase + i * 2 * math.pi / n))
        for i in range(n)
    ]


def draw_gem(size, accent_hex, facets=True):
    """Renders one square RGBA icon of `size` pixels."""
    s = size * SUPERSAMPLE
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    accent = _rgb(accent_hex)
    # A brilliant's crown is brightest at the table and darkens toward the girdle, so
    # the fill runs light-centre to dark-edge rather than top-to-bottom: it reads as a
    # faceted dome instead of a flat disc.
    highlight = _lerp(accent, (255, 255, 255), 0.55)
    shadow = _lerp(accent, GROUND[:3], 0.55)

    pad = s * 0.06
    r = (s - 2 * pad) / 2
    cx = cy = s / 2

    # Rounded-square ground plate, so the icon has a body on a light wallpaper rather
    # than floating facet lines.
    d.rounded_rectangle([0, 0, s - 1, s - 1], radius=s * 0.18, fill=GROUND)

    # Radial fill, drawn as concentric circles from the rim inward. Crude but exact
    # enough at 8x supersampling, and avoids a numpy dependency for one gradient.
    steps = max(48, int(r / 2))
    for i in range(steps, 0, -1):
        t = i / steps
        d.ellipse(
            [cx - r * t, cy - r * t, cx + r * t, cy + r * t],
            fill=_lerp(highlight, shadow, t) + (255,),
        )

    table_r = r * 0.40
    table = _ring(cx, cy, table_r, 8, phase=math.pi / 8)
    girdle16 = _ring(cx, cy, r, 16, phase=math.pi / 8)

    if facets:
        line = max(1, int(s * 0.006))
        edge = _lerp(GROUND[:3], accent, 0.25) + (255,)
        # The real crown of a 57-facet round brilliant, from directly above. Three rings,
        # not one set of spokes -- spokes of equal length read as a wheel, which is what
        # makes a naive gem icon look wrong:
        #
        #   8 bezel (kite) facets  -- quadrilateral: table vertex, star apex, girdle,
        #                             previous star apex. Reaches the girdle.
        #   8 star facets          -- triangle sitting on each table EDGE, apex pointing
        #                             outward, stopping WELL SHORT of the girdle.
        #   16 upper-girdle halves -- the small triangles between a star apex and the
        #                             girdle, two per bezel.
        #
        # `star_r` is where the star apexes land: the girdle-break circle. Everything
        # below is just the edges implied by those three rings.
        star_r = r * 0.72
        # Star apexes sit at the table EDGE midpoint angles, i.e. offset half a step.
        star = _ring(cx, cy, star_r, 8, phase=math.pi / 8 + math.pi / 8)
        # The eight points where a bezel touches the girdle, aligned with table vertices.
        bezel_tip = _ring(cx, cy, r, 8, phase=math.pi / 8)

        for i in range(8):
            # Star facet's two sides: table edge's endpoints up to its apex.
            d.line([table[i][0], table[i][1], star[i][0], star[i][1]], fill=edge, width=line)
            d.line(
                [table[(i + 1) % 8][0], table[(i + 1) % 8][1], star[i][0], star[i][1]],
                fill=edge,
                width=line,
            )
            # Upper-girdle halves: apex out to the girdle either side of it.
            d.line(
                [star[i][0], star[i][1], bezel_tip[i][0], bezel_tip[i][1]],
                fill=edge,
                width=line,
            )
            d.line(
                [
                    star[i][0],
                    star[i][1],
                    bezel_tip[(i + 1) % 8][0],
                    bezel_tip[(i + 1) % 8][1],
                ],
                fill=edge,
                width=line,
            )
        # Table on top of the radial lines so its outline stays unbroken.
        d.polygon(table, fill=_lerp(highlight, (255, 255, 255), 0.35) + (255,))
        d.line(table + [table[0]], fill=edge, width=line)
    else:
        # Small sizes: silhouette plus a filled table. No interior edges.
        d.polygon(table, fill=_lerp(highlight, (255, 255, 255), 0.35) + (255,))

    # Girdle rim last, so nothing overlaps the silhouette.
    rim = max(1, int(s * 0.012))
    d.ellipse([cx - r, cy - r, cx + r, cy + r], outline=_lerp(accent, (255, 255, 255), 0.3) + (255,), width=rim)

    return img.resize((size, size), Image.LANCZOS)


def build(out_dir, accent_hex):
    os.makedirs(out_dir, exist_ok=True)
    frames = [
        draw_gem(n, accent_hex, facets=n >= FACET_DETAIL_MIN_PX) for n in ICO_SIZES
    ]
    ico_path = os.path.join(out_dir, "icon.ico")
    # `append_images` is what makes this a genuine multi-resolution .ico -- each size is
    # its own rendered frame, not one bitmap Windows rescales badly.
    frames[-1].save(
        ico_path,
        format="ICO",
        sizes=[(n, n) for n in ICO_SIZES],
        append_images=frames[:-1],
    )
    png_path = os.path.join(out_dir, "icon.png")
    draw_gem(256, accent_hex, facets=True).save(png_path, format="PNG")
    print(f"wrote {ico_path} and {png_path}")


if __name__ == "__main__":
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    # Two accents so the viewer and the server are told apart at a glance in a taskbar
    # or an output directory full of PGO build variants.
    build(os.path.join(root, "apps", "indicatrix-cut", "assets"), "#38bdf8")
    build(os.path.join(root, "apps", "indicatrix-worker", "assets"), "#10b981")
