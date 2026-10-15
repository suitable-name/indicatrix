# Tanzanite / zoisite measured spectra - NOTES

Tag key: [primary: ref], [derived: ...], [review: ref]. Collected 2026-10-04. Caltech server reachable only via the browser tool (shell proxy 403).

## Raw files
No raw zoisite file was saved verbatim in this pass (each is 20-84 kB; retyping through Write was too costly; one alexandrite file was saved as a test). What exists: /home/claude/data/tanzanite/digest_caltech_zoisite.txt (DERIVED readings every 20-40 nm plus headers, all 12 data files). To save raw, fetch each URL below with `await fetch(url).then(r=>r.text())` in a browser tab on minerals.gps.caltech.edu.

URLs, base http://minerals.gps.caltech.edu/FILES/Visible/zoisite/ :
- GRR 1265: zo1265.a, zo1265.b, zo1265.c
- GRR 2292 blue zone: Zoisite2292_blue_a.asc, _blue_b.asc, _blue_c.asc; yellow zone: Zoisite2292_yellow_a.asc, _b.asc, _c.asc
- GRR 2891: Zoisite2891a.asc, Zoisite2891b.asc, Zoisite2891c.asc
- Also listed: CIT-16940 (green, Pakistan: Zoisite16940_a/b/c.txt), CIT 17283 (pale yellow: Zoisite17283a/b/c.txt), synthetic thulite digitised from Langer 2002 (Thulite-Syn-X/Y/Z=Langer2002-1mm.txt). Not fetched (not tanzanite).
- GRR 3045 natural-brown tanzanite before heating (5.747 mm) and after heating (500 C), plus a loss-on-heating difference plot: ONLY JPG images on the index page (Zoisite 3045 spectrum before heating.jpg, ...after heating.jpg, ...loss on heating.jpg). No numeric data file is linked. Same-crystal before/after exists only as pictures. Faye & Nickel before/after heating is likewise a GIF (Zoisite_Faye (1971).gif).
- Index: http://minerals.gps.caltech.edu/FILES/Visible/zoisite/Index.html ; refs: http://minerals.gps.caltech.edu/Refs/Visible%20Refs%20-%20by%20date/Zoisite.html

## Samples (Caltech)
All: absorbance vs wavelength, nm, as in the files; the files do not say "absorbance" or mention reflection correction -> reflection correction not stated. Axis mapping as in the file headers and index page: E||a = gamma, E||b = alpha, E||c = beta (Pnma-type zoisite cell, a ~16.2, b ~5.55, c ~10.0 A; cell not printed on the server [derived: standard zoisite cell, not from the source]).

1. GRR 1265 (natural, Tanzania, unheated as far as stated; trichroic, greenish yellow to deep blue to red-violet)
   - Composition: V2O3 = 0.35 and FeO = 0.027 in the header [primary: file header]. Units and method not stated in the header; by Caltech convention these are wt% oxides but that is my assumption. Not recomputed to ppmw/apfu.
   - Path: 3.00 mm "normalized" [primary]; the original thickness is not given.
   - Rays: a=gamma yellow-green, b=alpha red-violet, c=beta blue (all three rays, same crystal).
   - Range 400.9-1103.8 nm (starts at 400.9, so 380-400 nm missing).
   - Readings [derived, absorbance at 3.00 mm]: E||a band max ~443 nm 0.34 and ~603 nm 0.236, min 0.152 at 500 nm; E||b max 544 nm 0.328, min 0.195 at 430 nm; E||c maxima 534 nm 0.251 and 605 nm 0.299, min 0.195 at 480 nm.
   - color: none measured. 
2. GRR 2292 (Arusha, two-zone crystal, blue zone and yellow zone)
   - Composition: none in headers. Not found.
   - Paths [primary: headers]: blue zone a and c 3.150 mm, b 5.142 mm; yellow zone a and c 3.237 mm, b 5.142 mm (so the b ray used a different path and sample region; the index page says "normalized to 3.0 mm", yet the files carry unnormalised headers, ambiguity unresolved). 
   - Range 360.2-1124.2 nm (c-blue starts at 210.6 nm). Covers 380-750 nm.
   - Readings: see digest. Blue zone a ray 603 nm 0.435 plus UV edge 1.61 at 380 nm; yellow zone a ray adds a ~446 nm band 0.748 [derived] (the band lost on heating in tanzanite).
3. GRR 2891 (Merelani Hills, surface-collected, sun-exposed, bluish, lacks brown)
   - Composition: none. Paths 5.587 mm (a, c) and 6.810 mm (b) [primary: headers]; range 390.6-4748.6 nm.
   - Not "heated" but natural. color: none measured.
4. Heated tanzanite: GRR 3045 before/after (see above, images only). No numeric heated spectrum was found on Caltech.

## Other literature
- Hurlbut 1969, Am Min 54:702 (https://msaweb.org/AmMin/AM54/AM54_702.pdf), via text extraction: orientation given as X||b red-violet, Y||c deep blue, Z||a yellow-green; positive, 2V ~53 deg [primary]. V2O3 0.20 wt%, Fe2O3 0.04 wt% [primary]; heated crystals blue, pleochroism red-violet / deep blue / deep blue [primary]. Spectra only cited from Anderson 1968 (band ~595 nm); no sample thickness.
- Schmetzer & Bank 1978/79, G&G winter 1978-79 "Bluish-green zoisite from Merelani" (https://rruff.info/uploads/GG16_121.pdf): pleochroism X (reddish purple), Y (blue), Z (yellowish brown) for blue unheated zoisite; heated: Z becomes blue. Bluish-green crystals 0.06 % V and 0.07 % Cr (basis not stated in the extracted text; probably wt% elements) [primary, basis unverified]. No spectra numerics, no cell setting stated in extraction.
- Pluthametwisute et al. 2020, Molecules 25:3743 (https://pmc.ncbi.nlm.nih.gov/articles/PMC7465713/): 5 natural stones T04-T12 measured before and after heating at 550 C (1.8 C/min, air). Same sample before/after in Fig. 4 (T04, spectra along a, b, c) but numbers only as a figure; no thickness, polarised-light statement or reflection correction in the text. EPMA, wt% oxides (Table 2, 3 spots per sample, on the c-axis face): V2O3 0.39/0.43/0.38 (T04), 0.35/0.38/0.39 (T06), 0.29/0.20/0.29 (T09), 0.25/0.25/0.25 (T11), 0.18/0.06/0.21 (T12); Cr2O3 <=0.08 wt%; TiO2 <=0.05 wt%; apfu on 13 O: V 0.004-0.027 [primary: Table 2]. Bands: ~460 nm (reduced by heating, largest along c) and 585-600 nm (unchanged) [primary]. color is Munsell-like GIA GemSet codes, not CIELAB (e.g. T04 c: yO 6/3 to V 7/4) [primary]. Their stated colors: a purple, b green/blue, c yellow.
- Not accessed: Tsang & Ghose 1971, Faye & Nickel 1970 (only the GIF on Caltech), Schmetzer & Bank full text, Olivier 2006 thesis, Bocchio et al. 2009 (Periodico di Mineralogia, URL found: rosa.uniroma1.it/.../15797/15170), Gunter/Smith, Schwarzinger 2025 "Zoisite and its many colors" (igc-gemmology.org PDF, found but not read), Mineralogical Magazine paper on V position in tanzanite (Cambridge, found not read).

## P4: tanzanite axis mapping
Reports: (i) Hurlbut 1969 and Caltech: alpha (X) || b red-violet, beta (Y) || c blue, gamma (Z) || a yellow-green; cell a 16.2, b 5.55, c 10.0 A (Pnma) [Caltech header = primary; cell values derived from standard zoisite]. (ii) Schmetzer & Bank, and Pluthametwisute 2020 citing Olivier and others: X purple along a, Y blue/green along b, Z yellow/brown along c.
Relabelling [derived]: with the Pnma cell, axis lengths 5.55 (alpha), 10.0 (beta), 16.2 (gamma) A. If the cell is permuted to a'=5.55, b'=10.0, c'=16.2 (Pmcn setting: a'=b, b'=c, c'=a), then alpha||a', beta||b', gamma||c', i.e. X||a, Y||b, Z||c, with colors purple / blue / yellow-brown: identical to Caltech/Hurlbut colors. So the two reports describe the same physical orientation; the difference is the axis labelling (Pnma vs Pmcn or optical-axis naming). I could not confirm from the text of (ii) which cell they state - I saw no cell setting in what I extracted - so this is the only reading consistent with the colors.
Reading caution: in (ii) "X axis (a)" may simply mean the optical direction X, not the crystallographic a axis.

## Not found
Measured CIELAB (D65, A) for any tanzanite sample with its own spectra; numeric same-crystal before/after heating data (only images/figures); V content with explicit basis for GRR 2292/2891; reflection correction statements; Tsang & Ghose, Faye & Nickel full text.
