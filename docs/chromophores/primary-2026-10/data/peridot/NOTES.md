# Peridot / forsterite notes

## Sample 1: Caltech GRR 418 (San Carlos, Arizona), room temperature
- Source: Caltech Mineral Spectroscopy Server, olivine index http://minerals.gps.caltech.edu/FILES/Visible/olivine/Index.html (page updated 15-Mar-2023). Origin of data: server hosts it without a citation on the index page.
- Files (saved here, text identical except CRLF line endings converted to LF; header lines kept):
  - ol418.a http://minerals.gps.caltech.edu/FILES/Visible/olivine/ol418.a  (header: 1.141 mm, alpha)
  - ol418.b http://minerals.gps.caltech.edu/FILES/Visible/olivine/ol418.b  (header: 1.1381 mm, beta)
  - ol418.g http://minerals.gps.caltech.edu/FILES/Visible/olivine/ol418.g  (header: 0.66 mm, gamma)
- Columns: wavelength nm, value. Range of files about 347-1993 nm (alpha), 347-1991 (beta), 328-1993 (gamma); covers 380-750 nm.
- Path length: 1.141 / 1.1381 / 0.66 mm per file header [primary: server file headers]. Note the gamma slab is a different, thinner section.
- Polarisation / axis mapping: stated on index page: 10 A axis = alpha, 6 A = beta, 4.7 A = gamma [primary: Caltech index page]. (Standard olivine: alpha = a, beta = c, gamma = b is not stated there; use only the page's wording.)
- Units: column header absent. Values are consistent with decadic absorbance for the stated thickness (not alpha in cm-1), but the files do not say so. UNVERIFIED. Reflection-loss correction: not stated.
- Composition: NOT FOUND. No Fa mol% analysed on this sample was located. Do not assign a San Carlos typical value; that would be a cross-sample number.
- Measured color (CIELAB/illuminant) of this sample: none found.
- Suitability: spectra usable only as an uncalibrated-composition reference; fails the "Fa analysed on that sample" requirement.

## Looked for and not found / leads
- Runciman, Sengupta, Gourley 1973, Am. Min. 58, 451 (https://msaweb.org/AmMin/AM58/AM58_451.pdf): Burmese peridot, microprobe wt% reported (SiO2 40.68, FeO 9.97, MgO 49.22 oxide wt%, as summarised by a fetch tool [review: tool summary, not checked against the PDF]); polarised spectra at 295 K and 4.2 K; sample 5.00 x 3.24 x 3.24 mm. Spectra appear in figures only; no digital table of visible absorbance found; no color. Would need figure digitising. Not downloaded as a data file.
- Burns 1970, Am. Min. 55, 1608 (https://msaweb.org/AmMin/AM55/AM55_1608.pdf): polarised olivine spectra vs Fe content; not examined for per-sample numbers.
- Gem-quality peridot with CIELAB: citations in a Scientific Reports reference list (refs 24, 25) were seen but not opened.
- Not opened: GIA Sardinia peridot PDF, RRUFF, USGS spectral library (reflectance, not absorbance).
