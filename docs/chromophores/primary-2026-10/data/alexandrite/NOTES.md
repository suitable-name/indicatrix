# Alexandrite / chrysoberyl measured spectra - NOTES

Tag key: [primary: ref] = number read from the source; [derived: ...] = my arithmetic/reading; [review: ref] = secondary statement.
Collected 2026-10-04. Shell proxy returns 403 for minerals.gps.caltech.edu, so data was fetched via the built-in browser (same-origin fetch).

## Raw files saved
| file | status |
|---|---|
| /home/claude/data/alexandrite/ch874.a | SAVED, content as fetched (only change: CRLF line ends became LF; 1293 data rows, 330.0185-1099.405 nm) |
| ch874.b, ch171.a, ch171.b | NOT saved verbatim (each ~24 kB; retyping through the Write tool was too costly). URLs and header lines recorded below; key readings in digest_caltech_chrysoberyl.txt. Re-fetch snippet: `await fetch(url).then(r=>r.text())` from a tab on minerals.gps.caltech.edu. |

Direct URLs (all http://minerals.gps.caltech.edu/FILES/Visible/CHRYSOBeryl/):
- ch874.a, ch874.b (GRR 874), ch171.a, ch171.b (GRR 171); GIFs ch171.gif, ch874.gif; CIT-1703 (Mogok) has images only, no data files.
- Reference list page: http://minerals.gps.caltech.edu/Refs/Visible%20Refs%20-%20by%20date/Chrysoberyl.html

## Sample A: Caltech GRR 874, alexandrite, Lake Manyara, Tanzania
- Source/id: Caltech Mineral Spectroscopy Server, GRR 874 (index page says color-change chrysoberyl, alexandrite variety).
- Composition: NONE given on the server (no Cr, Fe or V value). Nothing to tag. Not found.
- Path length: 0.717 mm [primary: file headers ch874.a and ch874.b, same value].
- Polarisation and mapping (as stated by file headers): ch874.a "E||a = beta, short direction, yellow"; ch874.b "E||b = gamma, long direction #2, blue". No third ray (E||c = alpha) exists on the server for any chrysoberyl. The server states no space-group symbol; the a=beta, b=gamma, c=alpha assignment only makes sense in the Pnma cell (a 9.40, b 5.48, c 4.43 A) [derived: see P4].
- Units: absorbance (log10 I0/I) apparently, normalised to the stated path; the header does not say "absorbance" and does not mention reflection correction. Treat as unknown: reflection correction not stated.
- Range: 330-1099 nm, step ~0.6 nm. Covers 380-750 nm fully.
- Readings, absorbance at 0.717 mm [derived: nearest-point reading of the file, +-3 nm]:
  - E||a (beta): peak 429 nm 2.02; 564 nm 0.457 (local max); minimum 490-500 nm 0.19; 380 nm 0.68; 600 nm 0.296; 700 nm 0.114.
  - E||b (gamma): peak 414 nm 1.25; main Cr3+ 4T2 band max ~596 nm 1.91; minimum 490 nm 0.19; 700 nm 0.175; shoulder/R-line region at 679 nm 0.64 (narrow feature).
  - Full 10 nm table: digest_caltech_chrysoberyl.txt.
- Measured color: none on server for this sample. Not found.
- Caveat: the "short/long direction" notes mean the crystal thickness along each polarisation was set by morphology; the b-ray is "long direction #2" i.e. possibly a different measurement session from the a-ray. Same crystal, but not guaranteed identical spot.

## Sample B: Caltech GRR 171, yellow chrysoberyl (Fe3+), Minas Gerais, Brazil
- Not alexandrite (Fe3+ color). Path 1.193 mm [primary: headers]. E||a = beta, E||b = gamma (headers). No composition. Readings: E||a 438 nm 0.48; E||b 442 nm 0.82 [derived]. Included only as an Fe3+ reference.

## Literature checked (no usable numeric polarised data or color)
- Farrell & Newnham 1965, Am Min 50:1972-1981 (https://msaweb.org/AmMin/AM50/AM50_1972.pdf). Read via text extraction (OCR noisy). Space group written in the Pnma setting with a 9.40, b 5.48, c 4.43 A [primary: p.1972ff, as extracted]. Spectra at 77 K for a and b directions of a natural alexandrite; c direction not measurable because of crystal morphology. Absorption coefficient alpha = (1/t) log10(I0/I) [primary]. No correction for scattering or reflection [primary]. Composition given only as an approximate formula Al1.96(Cr,Fe)0.04BeO4 [primary; per formula unit, total Cr+Fe, no separate Cr]. No thickness given in extracted text. Bands tabulated in figure/Table 1 (not read in detail). No color.
- Schmetzer 2019, G&G 55(1) "Pleochroism and color change in faceted alexandrite" (https://www.gia.edu/dam/migrated-assets/docs/doc1/GG-SP19-Schmetzer.pdf). Synthetic Czochralski/flux/HOC samples. Cr2O3 0.18-0.26 wt% (flux), 0.30-0.43 wt% (HOC) [primary, per-sample list not extracted]. Only visual colors of X, Y, Z components; no spectra, no CIELAB. Cell a 4.42, b 9.33, c 5.47 A with X||a, Y||b, Z||c (Pbnm).
- Xie et al. 2020, Sci Rep 10:6130 (https://www.nature.com/articles/s41598-020-62707-3): unpolarised transmittance of NHM London stones in a color-constancy model; no Cr content or axis information found in the main text.
- Sun, Palke, Muyal, McMurtry 2017, Am Min 102:1747 (doi 10.2138/am-2017-6011): paywalled; only abstract read (spectra along three axes, path 1-25 mm, CIE colorimetry for a synthetic Cr-chrysoberyl). Full text not accessed. Not found: composition, axis mapping, CIELAB values.
- Not reached: Schmetzer et al. alexandrite-effect papers (1980), Liu et al. 1994/95, Hassan & El-Rakhawy 1974, Powell 1985, White/Roy/Crichton 1967 (AM52_867 URL exists at msaweb.org but not read).

## P4: axis mapping for chrysoberyl / alexandrite
Facts [primary]: Farrell & Newnham use Pnma (a 9.40, b 5.48, c 4.43 A). Caltech headers: E||a = beta, E||b = gamma (so E||c = alpha by elimination). Schmetzer 2019 uses the morphological cell a 4.42, b 9.33, c 5.47 (Pbnm) with X||a, Y||b, Z||c; X, Y, Z = vibration directions of alpha, beta, gamma [convention, derived].
Relabelling [derived]: the three axes are 4.43 A (alpha), 9.40 A (beta), 5.48 A (gamma).
- Pnma (a 9.40, b 5.48, c 4.43): alpha||c, beta||a, gamma||b. Matches Caltech.
- Pbnm (a 4.43, b 9.40, c 5.48): alpha||a, beta||b, gamma||c. Matches Schmetzer (X||a, Y||b, Z||c).
- Pmnb setting (a 5.48, b 9.40, c 4.43): alpha||c, beta||b, gamma||a. This is exactly the "alpha||c, beta||b, gamma||a" report. I did not find that paper's text; the match is arithmetic, so I could not verify the setting it used.
color check [primary vs primary]: Schmetzer daylight colors X (alpha) violet-purple, Y (beta) yellow-orange, Z (gamma) blue-green; Caltech GRR 874: beta ray yellow (absorbs ~430 and 565 nm), gamma ray blue (absorbs ~600 nm). Consistent, so the physical assignment is the same; only a,b,c labels differ with cell setting. Conclusion: the alpha||c,beta||b,gamma||a versus alpha||a,beta||b,gamma||c discrepancy is a cell-setting relabelling (a<->c swap), not a physical disagreement. The Pnma statement (beta||a, gamma||b) differs from both in the b label too, again only because b is 5.48 A there.

## Not found
Cr content with explicit basis for any Caltech chrysoberyl sample; third (alpha) ray for Caltech samples; any measured CIELAB D65/A on a sample with its own spectra; reflection-correction statements for Caltech data; full text of Sun 2017.
