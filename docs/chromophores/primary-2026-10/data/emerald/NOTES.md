# Emerald notes

## Candidate A: Caltech GRR 3570, emerald, Afghanistan (NOT saved to disk, see below)
- Index: http://minerals.gps.caltech.edu/FILES/Visible/Beryl/Index.html (revised 3-Jul-2022)
- Files fetched and inspected in the browser tab:
  - E perp c: http://minerals.gps.caltech.edu/FILES/Visible/Beryl/Emerald_3570_a.asc  (header "0.859 mm, E perp c")
  - E parallel c: http://minerals.gps.caltech.edu/FILES/Visible/Beryl/Emerald_3570_c.asc  (header "0.859 mm, E \\ c")
- Columns: nm, value; 350.34-1124.22 nm, about 1290 rows each (about 21 KB each). Units not stated in header (absorbance for 0.859 mm presumed, UNVERIFIED). Reflection correction not stated.
- Path length 0.859 mm [primary: file header]. Polarisation per header; axis mapping E perp c / E parallel c explicit.
- Composition (Cr, V): NOT FOUND for this sample. color: none.
- Why not saved: files lack composition so they fail the same-sample requirement, and the 21 KB files could not be written verbatim without retyping by hand (no shell route to the server); an unverified retype would not be "verbatim". Re-fetch from the URLs above if needed.

## Other Caltech emerald files seen (URLs only, not fetched in full)
- Taran contributed, Zambia Miku, light green, 2.148 mm: .../Beryl/taran/Emerald_Zambia.a and .c (header of .a: "E perp c, RT"; tab-separated 1 nm steps from 330 nm). Index says Cr and Fe features; no composition.
- Nigeria Jos 0.95 mm: .../Beryl/taran/Emerald_Nigeria.a/.c ; GRR 3040 Yemen 2.043 mm: .../Beryl/beryl_3040_a.asc/_c.asc ; Emer3 Ural (normalised to 1 cm): .../Beryl/Emerald3.a/.c ; synthetic Cr-9-67 and A-587b (Taran): .../Beryl/Emerald_Cr-9-67.a/.c and Emerald_a-587b.a/.c ; RDS 65674 Chatham flux synthetic 0.76 mm: .../Beryl/B65674.a/.c.
- None list Cr/V analyses on the index page.

## Lead with composition and color but no digital spectra
- Kamakanga (Zambia) emeralds, MDPI Crystals 13(11) 1605 (https://www.mdpi.com/2073-4352/13/11/1605), as summarised by a fetch tool [review: tool summary, not verified in the PDF]: 13 samples ZAKA-1..13; LA-ICP-MS Cr 76-2934 ppmw (mean 1435), V 78-183 ppmw (mean 128); CIELAB ranges reported (L* 33.76-57.45, a* -29.17 to -1.36, b* -1.50 to 12.81); O-ray and E-ray UV-Vis-NIR for ZAKA-7 given as figures with band positions (420 and 610-680 nm Cr3+). Per-sample Cr/V table, thickness and illuminant not extracted; spectra not digital. Next step: open the paper's Table 2 and Figure for ZAKA-7 and digitise, or ask authors.

## Not found
- No emerald with Cr and V analysed separately AND digital E perp c / E parallel c spectra AND path length from a single source was located.
