# Appendix C: Built-in Render Materials

This table lists every one of the app's 32 built-in render materials, as
used by the spectral renderer (Chapter 2) and the Material Editor's
templates. n_D is the refractive index at the sodium D line (589.3nm).
Birefringence (Δn) is the difference between a material's fast and slow
rays at that same wavelength; isotropic materials have none. Dispersion is
given as the Abbe number V_d where the underlying data provides one (lower
V_d means more dispersive, i.e. more "fire"); a few entries only have a
directly stated Δn(F–C) instead, shown as such.

All 32 are reachable from the viewport's **Render Material** drop-down
(Chapter 2), listed alphabetically. The entries marked with `*` were the
only ones offered by older versions of the drop-down.

| Material | Optical character | n_D | Birefringence (Δn) | Dispersion | Colour |
|---|---|---|---|---|---|
| Diamond* | Isotropic | 2.417 | — | Abbe V_d ≈ 55.3 | Colourless |
| Sapphire* | Uniaxial (−) | 1.768 | −0.0081 | Abbe V_d ≈ 72.3 | Blue (Fe²⁺–Ti⁴⁺ charge transfer) |
| Ruby* | Uniaxial (−) | 1.768 | −0.0081 | Abbe V_d ≈ 72.3 | Red (Cr³⁺) |
| Emerald* | Uniaxial (−) | 1.579 | −0.0060 | Abbe V_d ≈ 70.9 | Green (Cr³⁺/V³⁺), narrow green transmission window |
| Zircon* | Uniaxial (+) | 1.925 | +0.0590 | Abbe V_d ≈ 41.0 | Near-colourless with faint tint (trace U⁴⁺ lines) |
| Alexandrite | Biaxial (+) | 1.743 | +0.0076 | Abbe V_d ≈ 73.9 | Colour-change: green in daylight, red in incandescent light (Cr³⁺) |
| Topaz* | Biaxial (+) | 1.627 | +0.0080 | Abbe V_d ≈ 76.8 | Blue (models London/Sky Blue topaz's irradiation colour centre) |
| Spinel* | Isotropic | 1.716 | — | Abbe V_d ≈ 60.6 | Red (Cr³⁺) |
| Quartz* | Uniaxial (+) | 1.544 | +0.0091 | Abbe V_d ≈ 69.7 | Colourless (rock crystal) |
| Tourmaline | Uniaxial (−) | 1.639 | −0.0210 | Abbe V_d ≈ 64.6 | Green (elbaite), strongly dichroic — the "dark ray" runs along the c-axis |
| Tanzanite* | Biaxial (+) | 1.701 | +0.0130 | Abbe V_d ≈ 40.2 | Models unheated, genuinely trichroic zoisite: red / blue / yellow-green (the commercial heat-treated gem is blue-violet) |
| Synthetic Moissanite* | Uniaxial (+) | 2.647 | +0.0415 | Abbe V_d ≈ 25.9 | Colourless |
| Cubic Zirconia* | Isotropic | 2.158 | — | Abbe V_d ≈ 33.5 | Colourless |
| Aquamarine | Uniaxial (−) | 1.577 | −0.0060 | Abbe V_d ≈ 70.7 | Pale blue-green (Fe²⁺) |
| Morganite | Uniaxial (−) | 1.577 | −0.0060 | Abbe V_d ≈ 70.7 | Pale pink (Mn³⁺) |
| Chrysoberyl (Yellow) | Biaxial (+) | 1.746 | +0.0090 | Abbe V_d ≈ 74.2 | Yellow (Fe³⁺) |
| Amethyst | Uniaxial (+) | 1.544 | +0.0091 | Abbe V_d ≈ 69.7 | Purple/violet (irradiation-induced iron colour centre) |
| Citrine | Uniaxial (+) | 1.544 | +0.0091 | Abbe V_d ≈ 69.7 | Yellow-orange (Fe³⁺ UV-blue absorption edge) |
| Pyrope Garnet | Isotropic | 1.714 | — | Abbe V_d ≈ 56.1 | Deep red (Cr³⁺/Fe²⁺) |
| Almandine Garnet | Isotropic | 1.790 | — | Abbe V_d ≈ 56.9 | Red-brown to violet-red (Fe²⁺ triplet) |
| Spessartine Garnet | Isotropic | 1.800 | — | Abbe V_d ≈ 51.2 | Vivid orange (Mn²⁺ triplet) |
| Grossular Garnet (Tsavorite) | Isotropic | 1.734 | — | Abbe V_d ≈ 45.7 | Vivid green (V³⁺/Cr³⁺) |
| Andradite Garnet (Demantoid) | Isotropic | 1.887 | — | Abbe V_d ≈ 26.9 | Vivid, saturated green — the most dispersive of the garnets here (Cr³⁺) |
| Peridot | Biaxial (+) | 1.654 | +0.0360 | Abbe V_d ≈ 56.5 | Yellow-green (Fe²⁺ triplet) |
| YAG | Isotropic | 1.833 | — | Abbe V_d ≈ 52.0 | Colourless synthetic |
| GGG | Isotropic | 1.970 | — | Δn(F–C) ≈ 0.045 | Colourless synthetic (historic diamond simulant) |
| Benitoite | Uniaxial (+) | 1.757 | +0.0470 | Abbe V_d ≈ 29.1 | Strongly dichroic: sapphire-blue one way, near-colourless the other (Ti/Fe) |
| Andalusite | Biaxial (−) | 1.634 | −0.0100 | Abbe V_d ≈ 68.5 | Trichroic: red-brown / yellow-green / olive-yellow-brown |
| Opal | Isotropic | 1.450 | — | Δn(F–C) ≈ 0.001 (deliberately minimal) | Colourless body opal; the structural play-of-colour effect is not modelled |
| Glass (N-BK7) | Isotropic | 1.517 | — | Abbe V_d ≈ 64.0 | Colourless reference crown glass |
| Glass (F2) | Isotropic | 1.620 | — | Abbe V_d ≈ 36.4 | Colourless reference dense flint glass |
| Rutile | Uniaxial (+) | 2.616 | +0.287 | Abbe V_d ≈ 8.5 | Yellow-to-brown body colour (near-UV absorption edge); this renderer's most extreme birefringence built-in |

**Notes**

- Sapphire and Ruby share the same host-mineral optics (dispersion and
  birefringence), differing only in colour. The same is true for
  Aquamarine and Morganite (both beryl), and for Quartz, Amethyst, and
  Citrine (all three the same silica host).
- Quartz (and, sharing its optics, Amethyst and Citrine) is the one
  built-in material with a genuinely wavelength-dependent birefringence
  curve for its extraordinary ray, rather than the constant-offset
  approximation every other birefringent built-in uses. That genuine curve
  renders correctly on the CPU only; a GPU-routed render still falls back
  to the constant-offset approximation for these three (Chapter 12).
- One further species with real gemological interest, Sphene (titanite),
  is deliberately not included: it needs birefringence and dispersion
  magnitudes well outside the range this renderer's anisotropic optics
  have been verified against, so adding it would mean shipping unverified
  numbers rather than a checked material (Chapter 12). Rutile is included,
  with its anisotropic Fresnel solve verified at its birefringence magnitude —
  see the `built_in_material_rutile` doc comment in
  `crates/indicatrix/src/optics/materials.rs`.

## Next steps

This is the final appendix. Return to the manual's [overview](README.md) to
find any other chapter.
