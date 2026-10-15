# Chromophore dataset: quartz, diamond, zoisite/tanzanite, moissanite, cubic zirconia, rutile, glass, opal

Status: DRAFT, merged with the user-supplied report "Gem_Chromophore_Spectroscopy_Verification.pdf" (48 pages, 23 references, mostly GIA Gems & Gemology; the report does not attribute individual numbers to individual references). Before the merge, only WebSearch summaries had been reachable (WebFetch was blocked), so no primary paper or table had been read in full. No DOI is quoted that was not seen. Report page numbers below are PDF page numbers; table values were read from page images, not from the garbled text extraction.

## Confidence rules used here
- `verified`: the number appeared in a retrieved search result that names its source (listed in the bibliography with a URL). Only a handful qualify. A value that was `verified` and is repeated unchanged by the report keeps `verified`.
- `secondary`: the number is transcribed from the user-supplied report ([Report]) or recalled from the cited primary or review literature (author, year, venue) but not re-read. It must be re-checked against the primary paper before being treated as ground truth. Where a band is `secondary` because of the report, the `source` field says "Report p.N (...)" and names any field of that band that the report does not give (these stay estimates).
- `estimate`: my own physical or color-based estimate (the reasoning is in the comments). Do not treat as measured.
- Unknown: there is no `unknown` confidence value and no placeholder number any more. An unknown field is ABSENT and carries a comment `# unknown`. (The earlier `0.0`, `[0.0, 0.0]` and `[]` placeholders were deleted in this merge.)

## Conventions
- `centre_nm` and `fwhm_cm1`: the band shape is a Gaussian in energy. Centre in cm-1 equals 1e7/nm. The report labels the rare-earth 4f lines "Lorentzian"; the schema has no Lorentzian shape, so they stay `gaussian_energy` with a comment.
- `peak_coeff`: absorption coefficient alpha in cm-1 at the band centre, per ONE unit of `conc_unit`, i.e. alpha = peak_coeff x conc. Glass uses cm-1 per wt% oxide, derived from alpha = 2.303 x eps x c with eps in L/mol/cm. Diamond conversions from cross section use 1 ppm atoms C = 1.762e17 cm-3 (3.515 g/cm3 / 12.011 x 6.022e23 x 1e-6), so alpha[cm-1/ppm] = sigma[cm2] x 1.762e17.
- `shape = "edge"`: a continuum with `onset_nm` and `slope_cm1_per_nm`. The renderer uses alpha(l) = peak_coeff x conc x exp(-(l - onset)/slope_nm) for l > onset, and peak_coeff x conc below it. Here `peak_coeff` is alpha at the onset wavelength. The report never gives a slope or an FWHM for an edge: both are absent (unknown) and the renderer must use its default decay.
- Zero-phonon lines (ZPL) are given as the broad vibronic band that carries the color (FWHM in cm-1) with the ZPL noted in a comment. NOTE: the report tabulates the ZPL wavelength as the band centre for N3, H3, H4, NV0 and NV-, with the width of the vibronic envelope; see the discrepancy list.
- `pol` for biaxial hosts is `{ alpha, beta, gamma }`, relative absorption weights. For tanzanite the report states X || a (alpha), Y || b (beta), Z || c (gamma); the prior file did not verify the mapping and a retrieved summary conflicts with it (see Discrepancies).
- Fluorescence: an absent `quantum_yield`, `excitation_nm`, `emission_nm` or `emission_fwhm_nm` means unknown.

## Bibliography
- [Report] "Quantitative Optical Spectroscopy and Comprehensive Chromophore Catalogue for Natural, Synthetic, and Amorphous Gemstone Hosts" (title as in the supplied PDF, file Gem_Chromophore_Spectroscopy_Verification.pdf, 48 pp.). User-supplied; 23 references on pp. 47-48 (GIA Spring 2020 corundum chromophores, Diamond Handbook 2024, Gems & Gemology issues, MDPI Gem Characterisation, Winter 2024 UV-Vis-NIR review and others); no number is traced to a reference.
- [Zaitsev2001] A.M. Zaitsev, Optical Properties of Diamond: A Data Handbook, Springer, Berlin 2001 (not retrieved; cited as handbook for centre positions).
- [Collins1982] A.T. Collins, The color of diamond and how it may be changed, J. Gemmology 18, 1982; and [Collins2001] Collins, Diamond Relat. Mater. / Phil. Trans. R. Soc. A reviews (not retrieved).
- [Woods1976] Woods, Collins, van Wyk, Proc. R. Soc. A / J. Phys. C, C-centre 1130 cm-1 calibration, quoted as "25 +/- 2 ppm per cm-1" in the search results for query "C centre 1130 cm-1" (snippets from arXiv:2112.07260 and arXiv:1806.09816 and a Mat. Res. Soc. Japan paper, https://www.mrs-j.org/pub/tmrsj/vol14B/vol14B_1553.pdf).
- [Collins1971-B] Collins & Williams 1971 boron calibration; updated coefficients as summarised in arXiv:2310.12587 "Accurate FTIR determination of boron concentration in CVD homoepitaxial diamond layers", https://arxiv.org/pdf/2310.12587 (search snippet only).
- [Wee2007] Wee et al., J. Phys. Chem. A 111, 9379 (2007), NV0/NV- absorption cross sections (not retrieved).
- [Davies1992] Davies, Collins et al., GR1 calibration, Diamond Relat. Mater. 1, 1992 (not retrieved).
- [Fisher2009] Fisher, Sibley, Kelly, J. Phys.: Condens. Matter 21, 364213 (2009), brown/pink plastic deformation (not retrieved).
- [Hainschwang2005] Hainschwang et al., Gems & Gemology 41(1), chameleon diamonds (not retrieved).
- [Eaton2007] Eaton-Magana et al., Gems & Gemology 43(4), IIb phosphorescence (not retrieved).
- [Cox1977] R.T. Cox, Optical absorption of the d4 ion Fe4+ in pleochroic amethyst quartz, J. Phys. C 10, 4631 (1977); bands 545, 357, ~950 nm as quoted in the retrieved Springer/MSA/PMC results (https://www.ncbi.nlm.nih.gov/pmc/articles/PMC7483767/, https://link.springer.com/article/10.1007/s00269-016-0864-z).
- [Lehmann1973] Lehmann & Bambauer, Angew. Chem. 12, 1973, quartz color centres (not retrieved).
- [NuttallWeil1981] Nuttall & Weil, Can. J. Phys. 59, 1981, Al-hole centre (not retrieved).
- [Goreva2001] Goreva, Ma, Rossman, Am. Mineral. 86, 270 (2001), rose quartz and dumortierite (not retrieved).
- [Rossman] G.R. Rossman, Caltech mineral spectroscopy server, http://minerals.gps.caltech.edu (not retrieved).
- [TanzCause2020] Cause of Color Modification in Tanzanite after Heat Treatment, 2020, PMC7465713, https://pmc.ncbi.nlm.nih.gov/articles/PMC7465713/ (search summary only: 450-460 nm and 520 nm bands fall along c after heating; no change below 250 C; 550 C optimum).
- [Weingartner2004] Weingaertner, Wellmann, Winnacker 2004 and "On the origin of the below band-gap absorption bands in n-type (N) 4H- and 6H-SiC", https://cris.fau.de/publications/122709224 (search summary: N band ~620-630 nm in 6H, ~460-470 nm in 4H; Al shoulder 410-420 nm).
- [BamfordWeyl] C.R. Bamford, color Generation and Control in Glass, Elsevier 1977; W.A. Weyl, colored Glasses, Society of Glass Technology 1951 (not retrieved; eps values from memory of these books).
- [CoGlass] Co2+ in alkali silicate glasses: three bands at 537, 595, 640 nm (535 -> 526 nm soda to potash), British Glass digital library abstract, https://www.britglass.org.uk/knowledge-base/digital-library-and-information-services/effect-alkali-ions-optical-absorption (search summary).
- [Nassau1980] K. Nassau, Gems Made by Man, Chilton 1980; Nassau, The Physics and Chemistry of Color, Wiley 2001 (not retrieved).
- [CZspectra] Chinese J. Geochem./J. Gems & Gemmology study of colored CZ, https://jogg.cug.edu.cn/article/id/95388769-5a3e-4668-b0f0-1a0ec24e773f (search summary: Nd lilac, Er pink, Ce red, Fe deep yellow; Pr3+ lines 449/474/485/583/612 nm, Pr4+ band below 450 nm).
- [HoCZ2017] Gem News International, Gems & Gemology 53(2) 2017, holmium synthetic CZ, ~7780 ppma Ho, https://gia.edu/gems-gemology/summer-2017-gemnews-holmium-synthetic-cz (search summary).
- [Cronemeyer1959] Cronemeyer, Phys. Rev. 113, 1222 (1959), rutile absorption (not retrieved).
- [Gaillou2012] Gaillou et al., Opal: Gems & Gemology 48(1) 2012 and Fritsch et al., Rossman opal spectroscopy (not retrieved).
- [SchottN-BK7] Schott optical glass datasheet N-BK7 and F2 (not retrieved).

## Quartz
Alpha-quartz (SiO2) is uniaxial positive (report p.26: trigonal P3_121, birefringence +0.009, band gap 8.5 eV, transmits to 150 nm). colors arise from (i) Fe4+ (amethyst, formed by ionising radiation from substitutional Fe3+, with Li+/Na+/H+ charge compensation), (ii) Fe3+ in nano-inclusion form (citrine, heated amethyst; the report names about 100 nm hematite particles and a 560 nm charge-transfer edge), (iii) the Al hole centre [AlO4]0 (smoky), (iv) Fe2+/Ti4+ IVCT in dumortierite nanofibres (rose; also a rarer phosphate-related crystalline pink). Heating amethyst above about 400-500 C destroys Fe4+ and gives citrine (yellow to orange-brown); partial heating plus irradiation gives prasiolite (green; the report says about 550 C anneal gives an interstitial Fe2+ band at 720 nm). Ametrine is natural sector-zoned amethyst/citrine from one crystal (Anahi, Bolivia), so it is not a separate chromophore, only both centres present in different growth sectors. Rose quartz color is not scattering-free: the milky look and asterism are scattering and are not absorption.

All quartz numbers in the report equal the earlier values of this file, so the merge changed confidence and sources, not numbers (see Discrepancies). The report's `ppm` is not specified as atoms or weight; for Fe in SiO2 the two differ by only 7 % (1 ppm atoms per Si = 2.66e16 cm-3, 1 ppm wt Fe = 2.86e16 cm-3), so the existing `ppm atoms` units are kept.

Interactions: amethyst Fe4+ requires Fe3+ precursor and radiation; heating quenches it (report p.27: bleaches at 400 C; p.31: 250-500 C range for trapped-hole polarons). Smoky Al-hole requires Al plus irradiation and is bleached at 350 C (report p.27; prior text 300-400 C). Citrine (natural) and amethyst centre are mutually exclusive in the same volume after heating. Ametrine requires both in separate sectors only.

```toml
[[host]]
name = "Quartz"
formula = "SiO2"
optical = "uniaxial"
uv_edge_nm = 150.0            # secondary: Report p.26 (band gap 8.5 eV, transmits down to 150 nm); prior estimate 150 nm unchanged
materials = ["Amethyst", "Citrine", "Quartz"]

[[host.chromophore]]
id = "Fe4+ (amethyst centre)"
kind = "color_centre"
elements = ["Fe"]
treatment = "irradiation"
site = "substitutional Si4+ site, Fe3+ -> Fe4+ by gamma irradiation (d4), formal [FeO4]0 trapped-hole centre"
conc_unit = "ppm atoms Fe as Fe4+"
conc_typical = [0.5, 20.0]        # estimate; total Fe in amethyst ~ 5-100 ppm, only a fraction is Fe4+
conc_max = 50.0
color = "violet to purple"
requires = ["Fe3+ (interstitial Fe precursor)"]
excludes = ["Fe3+ nano-inclusions (citrine, heated)"]
quenched_by = ["heating > ~400 C (Report p.27: bleaches at 400 C; p.26: 400-500 C)"]
confidence = "secondary"
sources = ["Report p.26-27, p.31", "Cox 1977 J. Phys. C 10, 4631 (bands verified in search result)", "Lehmann & Bambauer 1973"]
[[host.chromophore.band]]
centre_nm = 545.0
fwhm_cm1 = 3600.0             # Report p.27 (Amethyst row); prior estimate 3600, unchanged
shape = "gaussian_energy"
peak_coeff = 0.08             # Report p.27: 0.080 cm-1/ppm; prior estimate 0.08, unchanged. Implied sigma = 0.08 / 2.66e16 = 3.0e-18 cm2
pol = { o = 1.0, e = 0.7 }    # Report p.26-27: w_o = 1.0, w_e = 0.7 (ordinary ray favoured); prior estimate identical
confidence = "secondary"      # prior: "verified" for the centre only (Cox 1977 via search result); the report now supplies all fields
source = "Report p.27 (Amethyst, Fe4+ centre, 545 nm row); centre also verified from Cox 1977 search result"
[[host.chromophore.band]]
centre_nm = 950.0             # near-IR band, outside the visible window; kept for tail contribution
fwhm_cm1 = 4500.0             # Report p.26-27
shape = "gaussian_energy"
peak_coeff = 0.04             # Report p.27: 0.040 cm-1/ppm; prior estimate 0.04, unchanged
pol = { o = 1.0, e = 0.5 }    # Report p.27 (1.0, 0.5)
confidence = "secondary"      # prior: "verified" (centre quoted with 545 and 350 nm in PMC7483767)
source = "Report p.27 (Amethyst, 950 nm row); centre also in Cohen 1956; Lehmann & Bambauer 1973; Cox 1977 as quoted in PMC7483767"
[[host.chromophore.band]]
centre_nm = 357.0
fwhm_cm1 = 4000.0             # Report p.27
shape = "gaussian_energy"
peak_coeff = 0.15             # Report p.27: 0.150 cm-1/ppm; prior estimate 0.15, unchanged
pol = { o = 1.0, e = 1.0 }    # Report p.27 (1.0, 1.0)
confidence = "secondary"      # prior: "verified" (centre only)
source = "Report p.27 (Amethyst, 357 nm row); centre also Cox 1977. NOTE the report directory p.43 prints 25,000 cm-1 for 357 nm; 1e7/357 = 28,011 cm-1 as in the prose table"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }   # Report p.27 and p.43: Inert (bleaches at 400 C)
source = "Report p.27 (Amethyst, luminescence column)"

[[host.chromophore]]
id = "Fe3+ (citrine, heated amethyst)"
kind = "ion"
elements = ["Fe"]
treatment = "heating"
site = "dispersed iron oxide nano-inclusions: Report p.26 says ~100 nm alpha-Fe2O3 (hematite) after heating amethyst above 450 C (prior: interstitial / nano-inclusion Fe3+)"
conc_unit = "ppm atoms Fe as Fe3+"
conc_typical = [5.0, 100.0]       # estimate
conc_max = 300.0                  # estimate
color = "pale yellow to orange-brown (heated amethyst); natural citrine paler yellow; Report: yellow, orange, reddish-brown"
requires = ["Fe"]
excludes = ["Fe4+ (amethyst centre)"]
# quenched_by: none known (empty)
confidence = "secondary"
sources = ["Report p.26-27, p.31 (Citrine Quartz: thermal anneal 450 C forms 560 nm charge-transfer edge)", "Czaja, Mossbauer spectra of prasiolite and amethyst (rebus.us.edu.pl)", "PMC7483767 heat treatment of amethyst (search summary)"]
[[host.chromophore.band]]
centre_nm = 400.0                 # Report p.27: "400.0 (onset 560)"
shape = "edge"
onset_nm = 560.0                  # Report p.27: onset 560 nm; prior estimate 560, unchanged
# unknown: fwhm_cm1, slope_cm1_per_nm (edge; prior note: renderer should use onset + 80 nm decay as default, estimate)
peak_coeff = 0.02                 # Report p.27: 0.020 cm-1/ppm at onset; prior estimate 0.02, unchanged
pol = { o = 1.0, e = 1.0 }        # Report p.27
confidence = "secondary"          # prior: "estimate"
source = "Report p.27 (Citrine, Fe3+ nanoparticles, edge row)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }
source = "Report p.27 (Citrine, luminescence column: Inert)"

[[host.chromophore]]
id = "Al hole centre (smoky)"
kind = "color_centre"
elements = ["Al"]
treatment = "irradiation"
site = "[AlO4/h+]0, hole on O adjacent to substitutional Al3+, charge-compensated by Li+ or H+"
conc_unit = "ppm atoms Al as hole centres"
conc_typical = [1.0, 30.0]        # estimate
conc_max = 100.0                  # estimate
color = "grey to brown to near-black"
requires = ["Al"]
excludes = []
quenched_by = ["heating > ~300-400 C (Report p.27: bleaches at 350 C; p.26: 300-400 C)"]
confidence = "secondary"
sources = ["Report p.26-27", "Nuttall & Weil 1981 Can. J. Phys. 59", "Nassau 2001"]
[[host.chromophore.band]]
centre_nm = 470.0                 # Report p.26-27; prior estimate 470, unchanged (earlier note: two components reported)
fwhm_cm1 = 6500.0                 # Report p.27; prior estimate 6500, unchanged
shape = "gaussian_energy"
peak_coeff = 0.05                 # Report p.27: 0.050 cm-1/ppm; prior estimate 0.05, unchanged
pol = { o = 1.0, e = 0.8 }        # Report p.27 and p.43: (1.0, 0.8); p.31 only states w_o = 1.0
confidence = "secondary"          # prior: "estimate"
source = "Report p.27 (Smoky, [AlO4/h+]0 row)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }
source = "Report p.27 (Smoky, luminescence column: Inert, bleaches at 350 C)"

[[host.chromophore]]
id = "Fe2+-Ti4+ IVCT in dumortierite nanofibres (rose)"
kind = "ivct_pair"
elements = ["Fe", "Ti"]
treatment = "natural"
site = "dumortierite inclusions (Al7BSi3O18), not in quartz lattice"
conc_unit = "ppm atoms Fe in inclusions (bulk)"
conc_typical = [10.0, 200.0]      # estimate
conc_max = 500.0                  # estimate
color = "pink (cool, blue-tinged)"
requires = ["Fe", "Ti"]
excludes = []
quenched_by = ["heating > ~575 C (dumortierite pink fades; unverified)"]
confidence = "secondary"
sources = ["Report p.26-27", "Goreva, Ma, Rossman 2001 Am. Mineral. 86, 270"]
[[host.chromophore.band]]
centre_nm = 510.0                 # Report: 500-510 nm; table 510. Earlier note: the true spectrum is two transmission windows (violet ~400 nm and red) with an IVCT near 500 nm
fwhm_cm1 = 4500.0                 # Report p.26-27; prior estimate 4500, unchanged
shape = "gaussian_energy"
peak_coeff = 0.01                 # Report p.27: 0.010 cm-1/ppm; prior estimate 0.01, unchanged
pol = { o = 1.0, e = 1.0 }        # Report p.27
confidence = "secondary"          # prior: "estimate"
source = "Report p.27 (Rose, Fe2+-Ti4+ IVCT row)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }
source = "Report p.27 (Rose, luminescence column: Inert)"

[[host.chromophore]]
id = "Fe2+ (prasiolite)"
kind = "ion"
elements = ["Fe"]
treatment = "heating"
site = "interstitial Fe2+ after heating amethyst (~550 C, Report p.26); some natural Montezuma prasiolite"
conc_unit = "ppm atoms Fe as Fe2+"
conc_typical = [5.0, 60.0]        # estimate
conc_max = 150.0                  # estimate
color = "pale leek green"
requires = ["Fe"]
excludes = ["Fe4+ (amethyst centre)"]
quenched_by = ["strong heating (> ~700 C turns it citrine/brown, unverified)"]
confidence = "secondary"          # prior: "estimate" (the report supplies the band)
sources = ["Report p.26-27", "Czaja (rebus.us.edu.pl) Mossbauer of prasiolite"]
[[host.chromophore.band]]
centre_nm = 720.0                 # Report p.26-27; prior estimate 720, unchanged
fwhm_cm1 = 3500.0                 # Report p.26-27; prior estimate 3500, unchanged
shape = "gaussian_energy"
peak_coeff = 0.03                 # Report p.27: 0.030 cm-1/ppm; prior estimate 0.03, unchanged
pol = { o = 1.0, e = 1.0 }        # Report p.27
confidence = "secondary"          # prior: "estimate"
source = "Report p.27 (Prasiolite, Fe2+ interstitial crystal field row)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }
source = "Report p.27 (Prasiolite, luminescence column: Inert)"
```

## Diamond
Isotropic (cubic), n = 2.417. Intrinsic edge 225 nm (5.47 eV indirect gap) for type IIa; N-rich type Ia/Ib stones show an edge 300-400 nm because of A-aggregate, B-aggregate and C-centre absorption (the 270 nm band and the continuum; Report p.2 states the same 225 nm and 300-400 nm). The renderer should switch `uv_edge_nm` by composition: 225 for N below ~1 ppm, rising toward 300-400 for N above ~100 ppm (estimate, no calibration retrieved).

Verified numbers: C-centre calibration is [N_s0] = (25 +/- 2) ppm x alpha(1130 cm-1) (Woods/Collins/van Wyk, quoted widely; Report p.2 repeats it, "1.0 cm-1 corresponds to 25 +/- 2 atomic ppm"). Boron: the earlier file had two conflicting numbers, Collins [NA-ND] = 0.0350 x H2800 from the arXiv:2310.12587 snippet (units ambiguous) and a recalled 0.37 ppm per cm-1. The report gives alpha about 2.7 cm-1/ppm for boron, i.e. 1/2.7 = 0.37 ppm per cm-1, which supports the recalled value (see Discrepancies D2); neither is a verified primary value.

Interactions and impossible combinations:
- Type IIb (B) cannot coexist with uncompensated N: donor N compensates acceptor B 1:1, so show blue only when [B] > [N] (net NA-ND > 0). In the UI offer N and B jointly only with B_ppm > N_ppm. (Report p.2-3: N donors, E_D = 1.7 eV, compensate B acceptors, 0.37 eV.)
- N-vacancy centres (NV, H3, H4, N3) need N aggregates plus vacancies; irradiation without N gives GR1 only. H3 needs A-aggregate, so it needs type Ia (N in A form), not type Ib alone: type Ib + irradiation + 800 C gives NV, not H3 (H3 appears after higher-temperature annealing creates A aggregates).
- GR1 (V0) is stable until about 600-700 C; heating removes it (so GR1 and H3/NV from the same irradiation are mutually exclusive stages). Report p.3: annealing above 600-700 C converts vacancies to NV.
- 550 nm plastic-deformation band and brown vacancy clusters are natural or HPHT-annealed-away; HPHT above 2000 C removes them (treatment `HPHT` quenches).
- 480 nm hydrogen band appears only in H-rich natural (chameleon, some Argyle-type) stones; not offered for B or for N-free stones.

```toml
[[host]]
name = "Diamond"
formula = "C"
optical = "isotropic"
uv_edge_nm = 225.0            # secondary: Report p.2 (type IIa 5.47 eV = 225 nm; type Ia/Ib N-rich edge 300-400 nm)
materials = ["Diamond"]

[[host.chromophore]]
id = "C centre / Ns0 (type Ib yellow)"
kind = "color_centre"
elements = ["N"]
treatment = "natural"          # also HPHT growth (synthetic Ib)
site = "isolated substitutional nitrogen N_s0 (donor 1.7 eV below the conduction band, Report p.2)"
conc_unit = "ppm atoms N"
conc_typical = [10.0, 300.0]   # secondary; HPHT Ib 10-300 ppm, natural Ib up to ~500
conc_max = 600.0               # estimate
color = "yellow to orange-yellow (canary)"
requires = []
excludes = ["Boron (type IIb blue)"]    # uncompensated B cannot coexist, see overview
quenched_by = ["aggregation by HPHT/annealing > ~1700 C (-> A/B)"]
confidence = "secondary"
sources = ["Report p.2-3, p.32", "Woods, Collins, van Wyk (Proc. R. Soc. A, 1976) calibration quoted in arXiv:2112.07260", "Zaitsev 2001"]
[[host.chromophore.band]]
centre_nm = 270.0               # UV band of N_s0, outside the window; kept so the renderer sees the UV tail
fwhm_cm1 = 5500.0               # Report p.3 (C-Centre row); prior estimate 5500, unchanged
shape = "gaussian_energy"
peak_coeff = 0.18               # Report p.3: alpha = 0.18 cm-1/ppm (= 5.5 ppm per cm-1, same as the recalled Dyer/Kiflawi value). The report's own sigma = 1.5e-19 cm2 would give 1.5e-19 x 1.762e17 = 0.026 cm-1/ppm, a factor 6.8 lower (D5)
pol = { o = 1.0, e = 1.0 }      # Report: {1.0}; isotropic
confidence = "secondary"
source = "Report p.3 (C-Centre row, alpha = 0.18 cm-1/ppm); also Dyer 1965 / Kiflawi 1994 recalled"
[[host.chromophore.band]]
centre_nm = 400.0               # continuum; the report lists the continuum as "Onset 560" with FWHM "Continuum"
shape = "edge"
onset_nm = 560.0                # Report p.2-3: absorption continuum rises from 560 nm (2.2 eV) through blue and violet
# unknown: fwhm_cm1, slope_cm1_per_nm (default exponential decay constant 60 nm is an estimate)
peak_coeff = 0.0015             # estimate cm-1/ppm at onset (tuned: 100 ppm gives alpha ~0.15 cm-1 at 500 nm, saturated yellow at 5 mm). NOT in the report: the report's C-Centre row has only the 270 nm alpha; its sigma = 1.5e-19 cm2 is ambiguous (if it belongs to the continuum it implies 0.026 cm-1/ppm, 17x the estimate)
pol = { o = 1.0, e = 1.0 }
confidence = "estimate"
source = "onset 560 nm from Report p.2-3 (C-Centre continuum); coefficient tuned to yellow Ib appearance; Collins 1980 continuum coefficient not retrieved"
[[host.chromophore.band]]
centre_nm = 8850.0              # 1130 cm-1, IR, calibration only, not rendered
# unknown: fwhm_cm1 (calibration band, not rendered)
shape = "gaussian_energy"
peak_coeff = 0.04               # verified: 1 cm-1 per 25 ppm (1/25 = 0.04); Report p.2 gives the same 25 +/- 2 ppm per cm-1
pol = { o = 1.0, e = 1.0 }
confidence = "verified"
source = "Woods/Collins/van Wyk calibration 25 +/- 2 ppm per cm-1 (arXiv:2112.07260, arXiv:1806.09816, mrs-j.org vol14B_1553); Report p.2 (C-centre 1130 cm-1 / 8850 nm) agrees"
[[host.chromophore.fluorescence]]
excitation_nm = [365.0]
emission_nm = [575.0, 637.0]    # Report p.3: 575.0, 637.0 (weak NV traces); C centre itself is non-luminescent
# unknown: emission_fwhm_nm, quantum_yield
uv = { lw = "inert to weak orange", sw = "inert" }   # report column gives only "weak NV traces"
source = "Report p.3 (C-Centre, luminescence column); excitation, uv: secondary, general knowledge"

[[host.chromophore]]
id = "N3/N2 Cape series (A aggregates + vacancies)"
kind = "color_centre"
elements = ["N"]
treatment = "natural"
site = "N3 = three N around a vacancy on a {111} plane (ZPL 415.2 nm); N2 band ~478 nm"
conc_unit = "ppm atoms N (as A aggregate in host)"
conc_typical = [100.0, 1500.0]  # secondary; Cape diamonds Ia(AB) with N3 absorption
conc_max = 3500.0               # estimate
color = "pale yellow ('Cape')"
requires = ["N aggregates (A/B)"]
excludes = ["Boron (type IIb blue)"]
quenched_by = ["HPHT > ~2000 C"]
confidence = "secondary"
sources = ["Report p.3-4", "Zaitsev 2001 (ZPL 415.2 nm)", "Collins 1982 J. Gemmology 18"]
[[host.chromophore.band]]
centre_nm = 415.2               # ZPL; the vibronic band extends 380-415 nm (Report p.2)
fwhm_cm1 = 1500.0               # Report p.3-4 (N3 / N2 row, first FWHM); prior estimate 1500, unchanged
shape = "gaussian_energy"
peak_coeff = 0.0008             # Report p.3-4: 8.0e-4 cm-1/ppm; prior estimate 0.0008, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"        # prior: "estimate"
source = "Report p.3-4 (N3 / N2 row); a Gaussian centred on the ZPL is physically doubtful, see D21"
[[host.chromophore.band]]
centre_nm = 478.0               # N2 band
fwhm_cm1 = 2500.0               # Report p.3-4 (second FWHM); prior estimate 2500, unchanged
shape = "gaussian_energy"
peak_coeff = 0.0004             # Report p.3-4: 4.0e-4 cm-1/ppm; prior estimate 0.0004, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"        # prior: "estimate"
source = "Report p.3-4 (N3 / N2 row, N2 478 nm); centre also Zaitsev handbook, recalled"
[[host.chromophore.fluorescence]]
excitation_nm = [365.0, 254.0, 300.0]
emission_nm = [415.2, 430.0, 450.0]     # Report p.3-4: 415.2, 430.0, 450.0 (LW blue); N3 ZPL + vibronic sideband to ~ 500
emission_fwhm_nm = 40.0                 # estimate
# unknown: quantum_yield
uv = { lw = "blue (often stronger LW)", sw = "blue (weaker)" }
source = "Report p.3-4 (luminescence column) and p.2 (365 nm excitation); emission_fwhm_nm estimate; excitation list from Zaitsev 2001"

[[host.chromophore]]
id = "H3 / H4 (N-V-N, 2N+V; 4N+2V)"
kind = "color_centre"
elements = ["N"]
treatment = "irradiation"       # irradiation + annealing > 800 C of type Ia; also natural
site = "H3 = two N + vacancy (ZPL 503.2 nm, vibronic 440-500 nm); H4 = four N + two vacancies (ZPL 496 nm)"
conc_unit = "ppm atoms N (type Ia host)"
conc_typical = [100.0, 1500.0]  # estimate (host N), centre concentration itself usually ppb to 1 ppm
conc_max = 3500.0               # estimate
color = "yellow-green to orange-yellow (absorbs blue-green)"
requires = ["N aggregates (A/B)"]
excludes = ["Boron (type IIb blue)", "GR1 (green, radiation)"]    # annealing for H3 destroys GR1
quenched_by = ["HPHT > ~1900 C (H4/H3 destroyed; Report p.3)"]
confidence = "secondary"
sources = ["Report p.3-4", "Zaitsev 2001 (H3 503.2, H4 496.0 nm ZPL)"]
[[host.chromophore.band]]
centre_nm = 503.2               # ZPL; band spans ~ 440-503 nm
fwhm_cm1 = 1800.0               # Report p.4 (H3 / H4 row); prior estimate 1800, unchanged
shape = "gaussian_energy"
peak_coeff = 0.0006             # Report p.4: 6.0e-4 cm-1/ppm; prior estimate 0.0006, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"        # prior: "estimate"
source = "Report p.4 (H3 / H4 row, H3 503.2 nm)"
[[host.chromophore.band]]
centre_nm = 496.0               # H4 ZPL
fwhm_cm1 = 2000.0               # Report p.4; prior estimate 2000, unchanged
shape = "gaussian_energy"
peak_coeff = 0.0003             # Report p.4: 3.0e-4 cm-1/ppm; prior estimate 0.0003, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"        # prior: "estimate"
source = "Report p.4 (H3 / H4 row, H4 496.0 nm)"
[[host.chromophore.fluorescence]]
excitation_nm = [365.0, 405.0, 450.0, 480.0]    # Report p.3: 365 and 405 nm; 450 and 480 nm kept from the prior file
emission_nm = [503.2, 520.0, 550.0]    # Report p.4: 503.2, 520.0, 550.0 (green)
emission_fwhm_nm = 60.0                # estimate
# unknown: quantum_yield
uv = { lw = "green (strong)", sw = "green" }
source = "Report p.3-4 (luminescence column); Zaitsev 2001"

[[host.chromophore]]
id = "Boron acceptor (type IIb blue)"
kind = "color_centre"
elements = ["B"]
treatment = "natural"            # also HPHT/CVD growth
site = "substitutional B, acceptor level 0.37 eV above valence band"
conc_unit = "ppm atoms B (net NA-ND)"   # CHANGED from ppb: the report quotes the boron coefficient per ppm
conc_typical = [0.05, 1.0]       # was [50, 1000] ppb (estimate); natural gem IIb 0.02-1 ppm
conc_max = 10.0                  # was 10000 ppb (estimate, gem-relevant; synthetic can far exceed)
color = "blue to grey-blue to near black at high B"
requires = []
excludes = ["C centre / Ns0 (type Ib yellow)", "N3/N2 Cape series", "H3 / H4", "NV (pink)"]  # N compensates B
# quenched_by: none known (empty)
confidence = "secondary"
sources = ["Report p.2, p.4, p.32", "Collins & Williams 1971 J. Phys. C 4, 1789 (not retrieved)", "arXiv:2310.12587 (search summary)"]
[[host.chromophore.band]]
centre_nm = 700.0                # continuum from photoionisation of the 0.37 eV acceptor, rising toward red/IR
shape = "edge"
onset_nm = 700.0                 # Report p.2, p.4: continuum rises from 700 nm into the near-IR; prior estimate 700, unchanged
# unknown: fwhm_cm1, slope_cm1_per_nm (default exponential rise toward longer wavelengths, absorption increases with lambda)
peak_coeff = 1.76                # cm-1 per ppm B at 700 nm. Report p.4: sigma ~ 1.0e-17 cm2 -> 1.0e-17 x 1.762e17 = 1.76 cm-1/ppm. Prior estimate: 0.0005 cm-1/ppb = 0.5 cm-1/ppm (a factor 3.5 lower, D1). Assignment of the two numbers in the report cell (sigma to the 700 nm continuum, alpha 2.7 to the 3571 nm band) follows the row order and is an interpretation
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"         # prior: "estimate"
source = "Report p.4 (Boron row, sigma ~ 1.0e-17 cm2, 700 nm onset); onset_nm = Report; conversion to cm-1/ppm is mine"
[[host.chromophore.band]]
centre_nm = 3571.0               # 2800 cm-1 electronic acceptor band, IR; for calibration, outside 380-780 nm
# unknown: fwhm_cm1. The report lists 2800 in the FWHM column, but 2800 cm-1 is the band CENTRE (3571 nm), so it is a column mix-up and was not adopted (D7)
shape = "gaussian_energy"
peak_coeff = 2.7                 # Report p.4: alpha ~ 2.7 cm-1/ppm. Equals 1/0.37, i.e. [B] = 0.37 ppm per cm-1 of the 2800 cm-1 peak (the "recalled" value); sigma = 2.7 / 1.762e17 = 1.5e-17 cm2, consistent with the report's sigma ~ 1.0e-17 within a factor 1.5. Conflicts by 10.6x with 0.0350 ppm/cm-1 (= 28.6 cm-1/ppm, sigma = 1.6e-16 cm2) from the arXiv:2310.12587 snippet. Prior: absent (unknown), conflict unresolved
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.4 (Boron row, alpha ~ 2.7 cm-1/ppm, 3571 nm IR); recalled 0.37 ppm per cm-1 (Fisher/Lawson-type) agrees; arXiv:2310.12587 snippet (0.0350) disagrees, unresolved"
[[host.chromophore.fluorescence]]
excitation_nm = [225.0, 254.0]
emission_nm = [500.0, 660.0]     # Report p.4: 500.0, 660.0 (SW phosphorescence): blue-green and red-orange phosphorescence
emission_fwhm_nm = 80.0          # estimate
# unknown: quantum_yield
uv = { lw = "inert (weak)", sw = "blue/orange-red phosphorescence after SW exposure (seconds to minutes)" }
source = "Report p.2, p.4 (Type IIb phosphorescence under 254 nm via donor-acceptor pair recombination); Eaton-Magana et al. 2007 G&G 43(4), recalled"

[[host.chromophore]]
id = "GR1 (neutral vacancy V0)"
kind = "color_centre"
elements = []
treatment = "irradiation"
site = "neutral vacancy, ZPL 741.2 nm; vibronic band centred 700 nm, 600-740 nm"
conc_unit = "ppm vacancies"
conc_typical = [0.05, 5.0]       # estimate; green surface/volume colored stones
conc_max = 50.0                  # estimate
color = "blue-green to green (absorbs red)"
requires = []
excludes = ["H3 / H4", "NV (pink)"]
quenched_by = ["annealing > ~600-700 C (Report p.3)"]
confidence = "secondary"
sources = ["Report p.3-4", "Davies 1992 (calibration not retrieved)", "Zaitsev 2001"]
[[host.chromophore.band]]
centre_nm = 700.0                # broad centre of the GR1 envelope; ZPL 741.2 nm (FWHM ~ 45 cm-1 in the report)
fwhm_cm1 = 1600.0                # Report p.3-4; prior estimate 1600, unchanged
shape = "gaussian_energy"
peak_coeff = 0.6                 # Report p.4: alpha = 0.60 cm-1/ppm; prior estimate 0.6, unchanged. The report's own sigma = 3.4e-17 cm2 gives 3.4e-17 x 1.762e17 = 5.99 cm-1/ppm, a factor 10 higher than its alpha (D6); alpha 0.60 kept
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"         # prior: "estimate"
source = "Report p.4 (GR1 row, alpha = 0.60 cm-1/ppm; sigma = 3.4e-17 cm2 inconsistent by 10x)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_fwhm_nm, quantum_yield
emission_nm = [741.2]            # Report p.4: 741.2 (weak GR1 luminescence); prior had 741.0
uv = { lw = "inert", sw = "inert" }
source = "Report p.4 (GR1, luminescence column: 741.2 weak)"

[[host.chromophore]]
id = "NV0 / NV- (pink)"
kind = "color_centre"
elements = ["N"]
treatment = "irradiation"        # irradiation + anneal 600-800 C of type Ib/IaA; also natural pink (Argyle/Ib-like)
site = "N adjacent to vacancy; NV0 ZPL 575.0 nm, NV- ZPL 637.0 nm"
conc_unit = "ppm atoms NV"
conc_typical = [0.1, 5.0]        # estimate
conc_max = 20.0                  # estimate
color = "pink to red-purple (absorbs green-yellow)"
requires = ["C centre / Ns0 (type Ib yellow)"]    # needs substitutional N donor; Ib or IaA with mixed N
excludes = ["Boron (type IIb blue)", "GR1 (neutral vacancy V0)"]
quenched_by = ["annealing > ~1000 C"]
confidence = "secondary"
sources = ["Report p.3-5", "Wee et al. 2007 J. Phys. Chem. A 111, 9379 (cross sections; not retrieved)", "Zaitsev 2001"]
[[host.chromophore.band]]
centre_nm = 575.0                # NV0: the report table gives the ZPL 575.0 nm as the centre; the real envelope maximum lies at shorter wavelength (D21)
fwhm_cm1 = 2500.0                # Report p.4-5; prior estimate 2500, unchanged
shape = "gaussian_energy"
peak_coeff = 5.3                 # cm-1/ppm: Report p.4-5 sigma ~ 3.0e-17 cm2 x 1.762e17 = 5.29 -> 5.3. Prior estimate 3.0 (a factor 1.76 lower, D3)
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"         # prior: "estimate"
source = "Report p.4-5 (NV0 / NV- row, first sigma); conversion to cm-1/ppm is mine"
[[host.chromophore.band]]
centre_nm = 560.0                # NV- broad band centre (Report p.3 prose: 560 nm); the table lists ZPL 637.0 nm (D20)
fwhm_cm1 = 3000.0                # Report p.4-5; prior estimate 3000, unchanged
shape = "gaussian_energy"
peak_coeff = 3.5                 # cm-1/ppm: Report p.4-5 sigma ~ 2.0e-17 cm2 x 1.762e17 = 3.52 -> 3.5. Prior estimate 2.0 (a factor 1.76 lower, D4); the prior recalled 3e-17 for NV- (Wee 2007), so the report may have the two sigmas swapped
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"         # prior: "estimate"
source = "Report p.4-5 (NV0 / NV- row, second sigma) and p.3 prose (560 nm); conversion to cm-1/ppm is mine"
[[host.chromophore.fluorescence]]
excitation_nm = [532.0, 405.0]
emission_nm = [575.0, 637.0]     # Report p.5: 575.0, 637.0 (orange-red)
emission_fwhm_nm = 80.0          # estimate
# unknown: quantum_yield (NV- ~0.7-0.8 reported for isolated; reduced with proximal N, arXiv:2112.07260, not read)
uv = { lw = "orange to red (moderate to strong)", sw = "orange to red" }
source = "Report p.5 (luminescence column); Zaitsev 2001; arXiv:2112.07260 (title only)"

[[host.chromophore]]
id = "550 nm plastic-deformation band (pink/brown)"
kind = "color_centre"
elements = []
treatment = "natural"
site = "{111} dislocation-glide vacancy clusters; removed by HPHT above 2000 C"
conc_unit = "relative intensity (alpha_550 in cm-1 per unit)"
conc_typical = [0.1, 3.0]        # estimate (alpha_550 in cm-1)
conc_max = 10.0                  # estimate
color = "pink to red-pink (brown when the continuum dominates); Report: pink, purple, brown (Argyle type)"
requires = []
excludes = ["Boron (type IIb blue)"]
quenched_by = ["HPHT > ~2000 C", "annealing > ~1200 C partial"]
confidence = "secondary"
sources = ["Report p.3, p.5", "Fisher et al. 2009 J. Phys. Condens. Matter 21, 364213", "Gaillou 2010 Diamond Relat. Mater. (pink Argyle)"]
[[host.chromophore.band]]
centre_nm = 550.0
fwhm_cm1 = 3500.0                # Report p.3, p.5; prior estimate 3500, unchanged
shape = "gaussian_energy"
peak_coeff = 1.0                 # by definition: conc unit is alpha_550 itself; Report p.5: alpha = 1.0 cm-1/unit
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"         # prior: "estimate"
source = "Report p.5 (Plastic Def. row, 550 nm)"
[[host.chromophore.band]]
centre_nm = 400.0                # Report p.5: "400.0 (edge)"
shape = "edge"
onset_nm = 650.0                 # estimate: brown continuum rising to the UV. NOT in the report (it gives only the 400 nm edge position); prior value kept
# unknown: fwhm_cm1, slope_cm1_per_nm
peak_coeff = 0.15                # Report p.5: alpha = 0.15 cm-1/unit relative to alpha_550; prior estimate 0.15, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"         # prior: "estimate"; onset_nm and slope remain estimate/unknown
source = "Report p.5 (Plastic Def. row, 400 nm edge, alpha = 0.15 cm-1/unit); onset_nm estimate"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }   # prior: "inert to weak"; Report p.5: Inert
source = "Report p.5 (Plastic Def., luminescence column: Inert)"

[[host.chromophore]]
id = "Hydrogen-related 480 nm band (chameleon, olive)"
kind = "color_centre"
elements = ["N"]                 # H-related, always in N-bearing natural stones; the hydrogen itself is not an element in the app
treatment = "natural"
site = "unassigned H-related defect; accompanies 3107 cm-1 line; Report: H complex accompanying N"
conc_unit = "relative intensity (alpha_480 in cm-1 per unit)"
conc_typical = [0.1, 2.0]        # estimate
conc_max = 5.0                   # estimate
color = "yellow-green to greenish grey (with chameleon thermo/photochromism); Report: olive-green"
requires = ["N aggregates (A/B)"]
excludes = ["Boron (type IIb blue)"]
quenched_by = ["HPHT", "heating ~ 200-300 C reversible in chameleons"]
confidence = "secondary"
sources = ["Report p.3, p.5", "Hainschwang et al. 2005 G&G 41(1) chameleon diamonds (not retrieved)"]
[[host.chromophore.band]]
centre_nm = 480.0
fwhm_cm1 = 3500.0                # Report p.3, p.5; prior estimate 3500, unchanged
shape = "gaussian_energy"
peak_coeff = 1.0                 # by definition; Report p.5: alpha = 1.0 cm-1/unit
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.5 (Hydrogen row, 480 nm)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "yellow (chameleon, unverified)", sw = "weak" }   # prior kept; the report's luminescence cell says only "Thermochromic yellow", which is not a luminescence statement (D24)
source = "unknown; Report p.5 (luminescence column: Thermochromic yellow)"
```

## Zoisite / tanzanite
Orthorhombic Ca2Al3(SiO4)(Si2O7)(OH), biaxial positive (report p.17: Pnma, host edge 330 nm). The report states the indicatrix as X || a (alpha), Y || b (beta), Z || c (gamma). Tanzanite color comes from V3+ (substitutional on the octahedral Al(1) and Al(2) sites) with Ti3+/Ti4+ (or V4+) redox. Report p.17: unheated bands 595 nm (3T1g -> 3T2g), 455 nm and 520 nm; alpha (X || a) is yellow-green to brown from 455 + 520 nm, beta (Y || b) purple-violet from 520 + 595 nm, gamma (Z || c) deep sapphire-blue from 595 nm. Air annealing at 500-550 C removes the 455 and 520 nm bands "along the alpha axis" (XAS: Ti3+ -> Ti4+ or V3+/V4+ redistribution), turning the trichroic brown/green stone into a dichroic blue (gamma) / violet (beta) gem; above 600 C the blue degrades; non-luminescent under UV. A retrieved summary of the 2020 Cause-of-Color study (PMC7465713) says heating from 250 C changes nothing, the optimum is ~550 C, and the 450-460 and ~520 nm bands decrease along the c axis; this conflicts with the report on which axis is bleached (Discrepancies D12).

The report gives separate unheated and heated parameter sets, so the file now has two alternative states of the same V3+ population (natural and heated 550 C). They are mutually exclusive: offer one of them, never both.

```toml
[[host]]
name = "Tanzanite"
formula = "Ca2Al3(SiO4)(Si2O7)O(OH)"   # the catalogue carries the full zoisite formula (22 atoms per formula unit) since 2026-10-12
optical = "biaxial"
uv_edge_nm = 330.0            # secondary: Report p.17 (host transmission edge 330 nm); prior estimate 330, unchanged
materials = ["Tanzanite"]

[[host.chromophore]]
id = "V3+ (natural unheated, trichroic)"
kind = "ion"
elements = ["V"]
treatment = "natural"          # prior entry was named "V3+ (tanzanite blue)" with treatment "heating" and mixed unheated/heated numbers
site = "octahedral Al site (Al(1) corner-sharing, Al(2) edge-sharing chains)"
conc_unit = "ppm wt V"         # Report p.18 header: cm-1/ppm V
conc_typical = [100.0, 1500.0] # estimate; tanzanite V2O3 ~0.01-0.2 wt% (= 68-1360 ppm V)
conc_max = 3000.0              # estimate
color = "trichroic: blue (gamma) / purple-violet (beta) / yellow-green to brown (alpha)"
requires = ["V"]
excludes = ["V3+ / Ti4+ (heated 550 C, dichroic blue)"]
# quenched_by: heating to ~550 C converts this state to the heated state (see next chromophore)
confidence = "secondary"
sources = ["Report p.17-18, p.39, p.31", "TanzCause2020 PMC7465713", "Rossman Caltech spectra of zoisite (not retrieved)"]
[[host.chromophore.band]]
centre_nm = 595.0              # V3+ 3T1g -> 3T2g (Report p.17)
fwhm_cm1 = 3000.0              # Report p.18; prior estimate 3000, unchanged
shape = "gaussian_energy"
peak_coeff = 0.003             # Report p.18: 0.0030 cm-1/ppm V; prior estimate 0.003, unchanged. Check: 1 ppm wt V = 3.35e-6 g/cm3 / 50.94 x 6.022e23 = 3.96e16 cm-3, sigma = 0.003 / 3.96e16 = 7.6e-20 cm2, eps = sigma / 3.82e-21 = 20 L/mol/cm, plausible for d-d in a non-centrosymmetric site
pol = { alpha = 0.5, beta = 0.8, gamma = 1.0 }   # Report p.18 (natural); prior estimate identical
confidence = "secondary"       # prior: "estimate"
source = "Report p.18 (Natural (Unheated), 595 nm)"
[[host.chromophore.band]]
centre_nm = 455.0              # heating-sensitive violet-blue absorption
fwhm_cm1 = 3500.0              # Report p.18; prior estimate 3500, unchanged
shape = "gaussian_energy"
peak_coeff = 0.002             # Report p.18: 0.0020 cm-1/ppm V (unheated); prior estimate 0.002, unchanged
pol = { alpha = 1.0, beta = 0.4, gamma = 0.2 }   # Report p.18; prior estimate { alpha = 0.4, beta = 0.8, gamma = 1.0 } ("stronger along c", D8). The report is consistent with its own text (455 nm absorbs mainly along alpha)
confidence = "secondary"       # prior: "verified" (centre 450-460 nm and decrease on heating from the PMC7465713 search summary; coefficient and pol were estimates)
source = "Report p.18 (Natural (Unheated), 455 nm); centre and heating response also TanzCause2020 (PMC7465713)"
[[host.chromophore.band]]
centre_nm = 520.0              # green absorption present in unheated stones
fwhm_cm1 = 3000.0              # Report p.18; prior estimate 3000, unchanged
shape = "gaussian_energy"
peak_coeff = 0.0015            # Report p.18: 0.0015 cm-1/ppm V (unheated); prior estimate 0.0015, unchanged
pol = { alpha = 1.0, beta = 0.7, gamma = 0.3 }   # Report p.18; prior estimate { alpha = 0.3, beta = 0.5, gamma = 1.0 } (D9)
confidence = "secondary"       # prior: "verified" (centre only)
source = "Report p.18 (Natural (Unheated), 520 nm); centre also TanzCause2020 (PMC7465713)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }
source = "Report p.17, p.39 (non-luminescent under UV)"

[[host.chromophore]]
id = "V3+ / Ti4+ (heated 550 C, dichroic blue)"
kind = "ion"
elements = ["V", "Ti"]
treatment = "heating"          # air anneal 500-550 C; replaces the earlier placeholder "V4+ / Ti oxidised (heat-treated)" whose band position was unknown
site = "same V3+ on Al(1)/Al(2); trace Ti3+ oxidised to Ti4+ (or V3+/V4+ redistribution), Report p.17, p.31. No separate V4+ or Ti4+ band is given in the report"
conc_unit = "ppm wt V"
conc_typical = [100.0, 1500.0] # estimate
conc_max = 3000.0              # estimate
color = "sapphire-blue along gamma, violet along beta (dichroic)"
requires = ["V"]
excludes = ["V3+ (natural unheated, trichroic)"]
quenched_by = ["heating > ~600 C (blue degrades; Report p.17)"]
confidence = "secondary"
sources = ["Report p.17-18, p.31", "TanzCause2020 PMC7465713 (search summary)"]
[[host.chromophore.band]]
centre_nm = 595.0
fwhm_cm1 = 3000.0              # Report p.18
shape = "gaussian_energy"
peak_coeff = 0.003             # Report p.18 (Heated 550 C): 0.0030 cm-1/ppm V, unchanged by heating
pol = { alpha = 0.2, beta = 0.8, gamma = 1.0 }   # Report p.18 (heated). The report lowers the alpha weight from 0.5 to 0.2 although its text says only 455 and 520 nm are removed (D11)
confidence = "secondary"
source = "Report p.18 (Heated (550 C), 595 nm)"
[[host.chromophore.band]]
centre_nm = 455.0
fwhm_cm1 = 3500.0              # Report p.18
shape = "gaussian_energy"
peak_coeff = 0.0003            # Report p.18 (heated): 0.0003 cm-1/ppm V (unheated 0.0020, factor 0.15; prior note: "falls toward 0.0003 after 550 C")
pol = { alpha = 0.1, beta = 0.1, gamma = 0.1 }   # Report p.18 (heated). If the renderer multiplies peak_coeff by pol this double counts the heating drop and also reduces beta and gamma, which the text says are not bleached (D11)
confidence = "secondary"
source = "Report p.18 (Heated (550 C), 455 nm)"
[[host.chromophore.band]]
centre_nm = 520.0
fwhm_cm1 = 3000.0              # Report p.18
shape = "gaussian_energy"
peak_coeff = 0.0002            # Report p.18 (heated): 0.0002 cm-1/ppm V (unheated 0.0015, factor 0.13)
pol = { alpha = 0.1, beta = 0.1, gamma = 0.1 }   # Report p.18 (heated); see the 455 nm band (D11)
confidence = "secondary"
source = "Report p.18 (Heated (550 C), 520 nm)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }
source = "Report p.17 (non-luminescent under UV)"
```

## Synthetic moissanite (SiC)
6H-SiC (and some 4H/15R) is uniaxial positive, n = 2.65-2.69. Band gap 3.02 eV (6H, indirect), so the UV edge sits near 410 nm (Report p.5). color is controlled by the nitrogen donor (green; the report calls the 625 nm band (6H; 465 nm in 4H) "conduction-band free carrier absorption", leaving a window at 500 nm; w_o = 1.0, w_e = 0.4) and by aluminium acceptors (shoulder at 410-425 nm just at the edge, pale blue in p-type). N and Al co-doping gives donor-acceptor pair luminescence at 480 nm. Offer N with Al only for compensation (net donor must stay > 0 for n-type green). 1 ppma in SiC = 9.6e16 cm-3 (4.82e22 formula units/cm3 x 2 atoms x 1e-6).

```toml
[[host]]
name = "Synthetic Moissanite"
formula = "SiC"
optical = "uniaxial"
uv_edge_nm = 410.0             # secondary: Report p.5 (6H-SiC Eg 3.02 eV indirect -> 410 nm); Al shoulder 410-425 nm
materials = ["Synthetic Moissanite"]

[[host.chromophore]]
id = "N donor (green/olive)"
kind = "ion"
elements = ["N"]
treatment = "none"
site = "substitutional C site, hexagonal (h) and quasi-cubic (k1, k2) sites, donors 0.10-0.15 eV below the conduction band (Report p.5)"
conc_unit = "ppm atoms N"
conc_typical = [1.0, 50.0]     # estimate (1e17 - 5e18 cm-3; 1 ppma = 9.6e16 cm-3)
conc_max = 500.0               # estimate
color = "green to dark green (transmission window near 500 nm)"
requires = []
excludes = ["Al acceptor (blue, p-type)"]   # same crystal region compensates; offer jointly only with N > Al
# quenched_by: none known (empty)
confidence = "secondary"
sources = ["Report p.5-6, p.33", "Weingartner 2004; cris.fau.de/publications/122709224 (band position verified in summary)"]
[[host.chromophore.band]]
centre_nm = 625.0              # 6H: 620-630 nm per search summary; Report p.5-6: 625 nm
fwhm_cm1 = 2500.0              # Report p.6; prior estimate 2500, unchanged
shape = "gaussian_energy"
peak_coeff = 0.5               # Report p.6: 0.50 cm-1/ppma; prior estimate 0.5, unchanged (~5 cm-1 at 1e18 cm-3 = 10.4 ppma)
pol = { o = 1.0, e = 0.4 }     # Report p.5-6: w_o = 1.0, w_e = 0.4; prior estimate identical
confidence = "secondary"       # prior: "verified" (centre only, Weingartner 2004 summary)
source = "Report p.6 (Nitrogen donor, 625 nm row); centre also Weingartner et al. 2004 as summarised"
[[host.chromophore.band]]
centre_nm = 410.0              # Report p.6: "410.0 (edge)"
shape = "edge"
onset_nm = 520.0               # estimate: free-carrier/ionisation rise toward the band edge. NOT in the report (it gives only the 410 nm edge position); prior value kept
# unknown: fwhm_cm1, slope_cm1_per_nm
peak_coeff = 0.02              # Report p.6: 0.02 cm-1/ppma; prior estimate 0.02, unchanged
pol = { o = 1.0, e = 1.0 }     # Report p.6
confidence = "secondary"       # prior: "estimate"; onset_nm and slope remain estimate/unknown
source = "Report p.6 (Nitrogen donor, 410 nm edge row); onset_nm estimate"
[[host.chromophore.fluorescence]]
excitation_nm = [365.0]
emission_nm = [480.0]          # Report p.5-6: 480.0 nm (DAP blue) in N+Al co-doped 6H
emission_fwhm_nm = 80.0        # estimate
# unknown: quantum_yield
uv = { lw = "weak or none", sw = "weak" }
source = "Report p.6 (Nitrogen donor, luminescence column: 480 nm DAP blue); emission_fwhm_nm and uv estimate"

[[host.chromophore]]
id = "Al acceptor (blue, p-type)"
kind = "ion"
elements = ["Al"]
treatment = "none"
site = "substitutional Si site, acceptor 0.20 eV above the valence band (Report p.5)"
conc_unit = "ppm atoms Al"
conc_typical = [1.0, 50.0]     # estimate
conc_max = 500.0               # estimate
color = "pale blue to blue-grey"
requires = []
excludes = ["N donor (green/olive)"]
# quenched_by: none known (empty)
confidence = "secondary"
sources = ["Report p.5-6, p.33", "Weingartner 2004 (Al shoulder 410-420 nm, search summary)"]
[[host.chromophore.band]]
centre_nm = 415.0              # Report p.6; shoulder spans 410-425 nm (p.5)
fwhm_cm1 = 3000.0              # Report p.6; prior estimate 3000, unchanged
shape = "gaussian_energy"
peak_coeff = 0.3               # Report p.6: 0.30 cm-1/ppma; prior estimate 0.3, unchanged
pol = { o = 1.0, e = 1.0 }     # Report p.6
confidence = "secondary"       # prior: "verified" (centre only)
source = "Report p.6 (Aluminium acceptor row); centre also Weingartner 2004 as summarised"
[[host.chromophore.fluorescence]]
excitation_nm = [365.0]
emission_nm = [420.0]          # Report p.6: 420.0 nm (weak violet)
emission_fwhm_nm = 60.0        # estimate
# unknown: quantum_yield
uv = { lw = "violet (weak)", sw = "violet (weak)" }   # prior: lw "blue (weak)", sw "blue"; the report says weak violet
source = "Report p.6 (Aluminium acceptor, luminescence column: 420 nm, weak violet)"
```

## Cubic zirconia (Y- or Ca-stabilised ZrO2)
Isotropic, n = 2.15-2.18. Band gap about 5 eV (Report p.11: transmission edge 280-290 nm). Zr4+ is in 8-fold cubic coordination. The rare-earth dopants give sharp 4f lines (FWHM 200-300 cm-1); the report calls the line shape Lorentzian (kept as gaussian_energy here); transition metals give broad bands; Ce and Pr form charge-transfer edges. The report's table header says cm-1 per wt%, but it does not name the oxide; the oxides used below (Nd2O3, Er2O3, CeO2, Pr6O11, CoO, Cr2O3, Fe2O3) are the earlier file's assumption. The Ho entries are in cm-1 per ppma in the report despite the header. Ti and V need reducing growth atmosphere; Cu, V, Ti, Mn and Ni dopants are not covered by the report. Ce3+ and Ce4+ are the same element in different valences (do not offer as two elements); Pr and Ce together may form an IVCT pair (unknown).

```toml
[[host]]
name = "Cubic Zirconia"
formula = "ZrO2 (Y2O3-stabilised)"
optical = "isotropic"
uv_edge_nm = 290.0            # secondary: Report p.11 (Eg ~ 5.0 eV, edge 280-290 nm); prior estimate 290, unchanged
materials = ["Cubic Zirconia"]

[[host.chromophore]]
id = "Nd3+ (lilac)"
kind = "ion"
elements = ["Nd"]
treatment = "none"
site = "Zr4+ 8-fold cubic site (replaces Zr/Y)"
conc_unit = "wt% Nd2O3"
conc_typical = [0.1, 2.0]      # estimate
conc_max = 5.0                 # estimate
color = "lilac in daylight, lavender-pink under incandescent light (alexandrite-like color change, Report p.11)"
requires = []
excludes = []
# quenched_by: none known (empty)
confidence = "secondary"
sources = ["Report p.11, p.36", "CZspectra jogg.cug.edu.cn (Nd light purple verified); Nassau 1980"]
[[host.chromophore.band]]
centre_nm = 582.0              # 4I9/2 -> 4G5/2, 2G7/2 (strongest, hypersensitive)
fwhm_cm1 = 250.0               # Report p.11; prior estimate 250, unchanged
shape = "gaussian_energy"      # report: Lorentzian
peak_coeff = 4.0               # Report p.11: 4.0 cm-1/wt%; prior estimate 4.0, unchanged. Check: 1 wt% Nd2O3 = 10 x 1 x 5.9 / 336.5 = 0.175 M Nd2O3 = 0.35 M Nd; eps = 4.0 / (2.303 x 0.35) = 5.0 L/mol/cm, plausible
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.11 (Nd3+ row, 582 nm)"
[[host.chromophore.band]]
centre_nm = 745.0
fwhm_cm1 = 200.0               # Report p.11; prior estimate 200, unchanged
shape = "gaussian_energy"      # report: Lorentzian
peak_coeff = 1.5               # Report p.11: 1.5 cm-1/wt%; prior estimate 1.5, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.11 (Nd3+ row, 745 nm)"
[[host.chromophore.band]]
centre_nm = 520.0
fwhm_cm1 = 250.0               # Report p.11; prior estimate 250, unchanged
shape = "gaussian_energy"      # report: Lorentzian
peak_coeff = 0.8               # Report p.11: 0.8 cm-1/wt%; prior estimate 0.8, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.11 (Nd3+ row, 520 nm)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_fwhm_nm, quantum_yield
emission_nm = [1060.0]         # Report p.11, p.36: 1060.0 nm (NIR)
uv = { lw = "inert (visible)", sw = "inert (visible)" }
source = "Report p.36 (Nd3+, luminescence column: 1060 nm NIR); no visible fluorescence expected"

[[host.chromophore]]
id = "Er3+ (pink)"
kind = "ion"
elements = ["Er"]
treatment = "none"
site = "Zr4+ 8-fold cubic site"
conc_unit = "wt% Er2O3"
conc_typical = [0.2, 3.0]      # estimate
conc_max = 6.0                 # estimate
color = "pure pink"
requires = []
excludes = []
# quenched_by: none known (empty)
confidence = "secondary"
sources = ["Report p.11, p.36", "CZspectra (Er pink verified)"]
[[host.chromophore.band]]
centre_nm = 520.0              # 4I15/2 -> 2H11/2 (hypersensitive)
fwhm_cm1 = 250.0               # Report p.11; prior estimate 250, unchanged
shape = "gaussian_energy"      # report: Lorentzian
peak_coeff = 2.0               # Report p.11: 2.0 cm-1/wt%; prior estimate 2.0, unchanged. Check: 1 wt% Er2O3 = 10 x 5.9 / 382.5 = 0.154 M, 0.31 M Er; eps = 2.0 / (2.303 x 0.31) = 2.8 L/mol/cm (low-ish for a hypersensitive line)
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.11 (Er3+ row, 520 nm)"
[[host.chromophore.band]]
centre_nm = 380.0
fwhm_cm1 = 300.0               # Report p.11; prior estimate 300, unchanged
shape = "gaussian_energy"      # report: Lorentzian
peak_coeff = 2.0               # Report p.11: 2.0 cm-1/wt%; prior estimate 2.0, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.11 (Er3+ row, 380 nm)"
[[host.chromophore.band]]
centre_nm = 653.0
fwhm_cm1 = 200.0               # Report p.11; prior estimate 200, unchanged
shape = "gaussian_energy"      # report: Lorentzian
peak_coeff = 0.5               # Report p.11: 0.5 cm-1/wt%; prior estimate 0.5, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.11 (Er3+ row, 653 nm)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_fwhm_nm, quantum_yield
emission_nm = [1540.0]         # Report p.36: 1540.0 nm (NIR)
uv = { lw = "inert (visible)", sw = "inert (visible)" }
source = "Report p.36 (Er3+, luminescence column: 1540 nm NIR)"

[[host.chromophore]]
id = "Ce (Ce3+/Ce4+, yellow to red)"
kind = "ion"
elements = ["Ce"]
treatment = "none"
site = "Zr4+ site; Ce4+ O-Ce charge-transfer edge dominates color"
conc_unit = "wt% CeO2"
conc_typical = [0.1, 3.0]      # Report p.11: yellow at 0.1-0.5 wt% CeO2, deep red at 1-3 wt%; prior estimate [0.1, 3.0], unchanged
conc_max = 6.0                 # estimate
color = "yellow to orange to red (increasing with Ce)"
requires = []
excludes = []
# quenched_by: none known (empty)
confidence = "secondary"       # prior: "estimate"
sources = ["Report p.11, p.36", "CZspectra (Ce red verified in summary); Nassau 1980"]
[[host.chromophore.band]]
centre_nm = 400.0              # Report p.11: "400.0 (onset 540)"
shape = "edge"
onset_nm = 540.0               # Report p.11; prior estimate 540, unchanged
# unknown: fwhm_cm1, slope_cm1_per_nm
peak_coeff = 8.0               # Report p.11: 8.0 cm-1/wt% at onset; prior estimate 8.0, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"       # prior: "estimate"
source = "Report p.11 (Ce4+ row, CT edge)"
[[host.chromophore.fluorescence]]
excitation_nm = [365.0]        # Report p.11: Ce3+ 4f -> 5d absorption at 365 nm; prior [320.0, 365.0]
emission_nm = [520.0]          # Report p.11, p.36: 520 nm (Ce3+, weak); prior [440.0, 520.0] (the 440 nm value was an unverified estimate)
emission_fwhm_nm = 90.0        # estimate
# unknown: quantum_yield
uv = { lw = "weak yellow/greenish (unverified)", sw = "weak (unverified)" }
source = "Report p.11 (yellow-green photoluminescence, 520 nm) and p.36 (Ce3+, weak); emission_fwhm_nm estimate"

[[host.chromophore]]
id = "Pr (Pr3+/Pr4+, yellow-orange-brown)"
kind = "ion"
elements = ["Pr"]
treatment = "none"
site = "Zr4+ site"
conc_unit = "wt% Pr6O11"
conc_typical = [0.1, 2.0]      # estimate
conc_max = 5.0                 # estimate
color = "amber-orange to cognac-brown (Report p.11)"
requires = []
excludes = []
# quenched_by: none known (empty)
confidence = "secondary"
sources = ["Report p.11, p.36", "CZspectra (Pr3+ 449/474/485/583/612 nm; Pr4+ below 450 nm, verified in summary)"]
[[host.chromophore.band]]
centre_nm = 449.0
fwhm_cm1 = 250.0               # Report p.11; prior estimate 250, unchanged
shape = "gaussian_energy"      # report: Lorentzian
peak_coeff = 1.5               # Report p.11: 1.5 cm-1/wt%; prior estimate 1.5, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"       # prior: "verified" (position only, CZspectra summary)
source = "Report p.11 (Pr3+/Pr4+ row, 449 nm); position also CZspectra summary"
[[host.chromophore.band]]
centre_nm = 474.0
fwhm_cm1 = 250.0               # Report p.11; prior estimate 250, unchanged
shape = "gaussian_energy"      # report: Lorentzian
peak_coeff = 1.0               # Report p.11: 1.0 cm-1/wt%; prior estimate 1.0, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"       # prior: "verified" (position only)
source = "Report p.11 (Pr3+/Pr4+ row, 474 nm); position also CZspectra summary"
[[host.chromophore.band]]
centre_nm = 485.0              # the report names this line in its prose (p.11) but its table row lists only 449, 474, 583 and the edge
fwhm_cm1 = 250.0               # estimate (not in the report table)
shape = "gaussian_energy"
peak_coeff = 0.8               # estimate (not in the report table)
pol = { o = 1.0, e = 1.0 }
confidence = "verified"        # position only (CZspectra summary); the report prose repeats the 485 nm line; FWHM and coefficient are estimates
source = "CZspectra summary (position); Report p.11 prose lists the line, no table value"
[[host.chromophore.band]]
centre_nm = 583.0
fwhm_cm1 = 300.0               # Report p.11; prior estimate 300, unchanged
shape = "gaussian_energy"      # report: Lorentzian
peak_coeff = 0.6               # Report p.11: 0.6 cm-1/wt%; prior estimate 0.6, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"       # prior: "verified" (position only)
source = "Report p.11 (Pr3+/Pr4+ row, 583 nm); position also CZspectra summary"
[[host.chromophore.band]]
centre_nm = 612.0              # as for 485 nm: named in the report prose, not in its table row
fwhm_cm1 = 300.0               # estimate (not in the report table)
shape = "gaussian_energy"
peak_coeff = 0.4               # estimate (not in the report table)
pol = { o = 1.0, e = 1.0 }
confidence = "verified"        # position only (CZspectra summary)
source = "CZspectra summary (position); Report p.11 prose lists the line, no table value"
[[host.chromophore.band]]
centre_nm = 400.0              # Report p.11: "Onset 450" for the Pr4+ charge-transfer continuum (22,222 cm-1)
shape = "edge"
onset_nm = 450.0               # Report p.11; also verified in the CZspectra summary (Pr4+ broad band below 450 nm)
# unknown: fwhm_cm1, slope_cm1_per_nm
peak_coeff = 6.0               # Report p.11: 6.0 cm-1/wt% (edge); prior estimate 6.0, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"       # prior: "estimate"
source = "Report p.11 (Pr3+/Pr4+ row, edge, onset 450 nm)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }   # prior: "unknown"
source = "Report p.36 (Pr, luminescence column: Inert)"

[[host.chromophore]]
id = "Ho3+ (yellow-pink color change)"
kind = "ion"
elements = ["Ho"]
treatment = "none"
site = "Zr4+ 8-fold site"
conc_unit = "ppm atoms Ho"
conc_typical = [1000.0, 8000.0]   # secondary: ~7780 ppma reported in a GIA color-change Ho:CZ
conc_max = 10000.0                # estimate
color = "champagne-yellow in daylight, peach-pink under incandescent light (color change, Report p.11)"
requires = []
excludes = []
# quenched_by: none known (empty)
confidence = "secondary"
sources = ["Report p.11, p.36", "HoCZ2017 G&G 53(2) GNI (7780 ppma Ho, verified in summary)"]
[[host.chromophore.band]]
centre_nm = 450.0              # 5I8 -> 5G6/5F1
fwhm_cm1 = 250.0               # Report p.11; prior estimate 250, unchanged
shape = "gaussian_energy"      # report: Lorentzian
peak_coeff = 0.0005            # Report p.11: 0.0005 cm-1/ppma (the table header says cm-1/wt%, D23); prior estimate 0.0005, unchanged. Check: 1 ppma Ho = 5.9 / 123.2 x 6.022e23 x 1e-6 = 2.9e16 cm-3; 7780 ppma = 0.37 M; alpha = 3.9 cm-1 -> eps = 3.9 / (2.303 x 0.37) = 4.5 L/mol/cm, plausible
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"       # prior: "estimate"
source = "Report p.11 (Ho3+ row, 450 nm)"
[[host.chromophore.band]]
centre_nm = 540.0
fwhm_cm1 = 250.0               # Report p.11; prior estimate 250, unchanged
shape = "gaussian_energy"      # report: Lorentzian
peak_coeff = 0.0004            # Report p.11: 0.0004 cm-1/ppma; prior estimate 0.0004, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"       # prior: "estimate"
source = "Report p.11 (Ho3+ row, 540 nm)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }   # prior: "unknown"
source = "Report p.36 (Ho, luminescence column: Inert)"

[[host.chromophore]]
id = "Fe3+ (yellow)"
kind = "ion"
elements = ["Fe"]
treatment = "none"
site = "Zr4+ site"
conc_unit = "wt% Fe2O3"
conc_typical = [0.05, 0.5]     # estimate
conc_max = 1.0                 # estimate
color = "golden-yellow to orange-brown (Report p.11); deep yellow in CZspectra"
requires = []
excludes = []
# quenched_by: none known (empty)
confidence = "secondary"
sources = ["Report p.11-12", "CZspectra (Fe deep yellow verified in summary)"]
[[host.chromophore.band]]
centre_nm = 400.0              # Report p.12: "400.0 (onset 520)"
shape = "edge"
onset_nm = 520.0               # Report p.11-12; prior estimate 520, unchanged
# unknown: fwhm_cm1, slope_cm1_per_nm
peak_coeff = 10.0              # Report p.12: 10.0 cm-1/wt% (edge); prior estimate 10.0, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"       # prior: "estimate"
source = "Report p.12 (Fe3+ row, CT edge)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }
source = "Report p.36 (Fe, luminescence column: Inert)"

[[host.chromophore]]
id = "Co2+ (violet-blue)"
kind = "ion"
elements = ["Co"]
treatment = "none"
site = "8-fold coordinated Zr site"
conc_unit = "wt% CoO"
conc_typical = [0.01, 0.2]     # estimate
conc_max = 0.5                 # estimate
color = "royal blue (Report p.11-12); prior note: violet to blue-violet (unverified)"
requires = []
excludes = []
# quenched_by: none known (empty)
confidence = "secondary"       # prior: "estimate"
sources = ["Report p.11-12", "Nassau 1980 (not retrieved)"]
[[host.chromophore.band]]
centre_nm = 590.0
fwhm_cm1 = 3000.0              # Report p.12; prior estimate 3000, unchanged
shape = "gaussian_energy"
peak_coeff = 60.0              # Report p.12: 60.0 cm-1/wt%; prior estimate 60.0, unchanged. Check: 1 wt% CoO = 10 x 5.9 / 74.93 = 0.79 M; eps = 60 / (2.303 x 0.79) = 33 L/mol/cm (plausible for a centrosymmetric 8-fold site)
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"       # prior: "estimate"
source = "Report p.12 (Co2+ row, 590 nm)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }
source = "Report p.36 (Co, luminescence column: Inert)"

[[host.chromophore]]
id = "Cr3+ (green)"
kind = "ion"
elements = ["Cr"]
treatment = "none"
site = "Zr4+ site (the report states 8-fold coordination, see D17)"
conc_unit = "wt% Cr2O3"
conc_typical = [0.02, 0.5]     # estimate
conc_max = 1.0                 # estimate
color = "emerald green (Report p.11-12)"
requires = []
excludes = []
# quenched_by: none known (empty)
confidence = "secondary"       # prior: "estimate"
sources = ["Report p.11-12", "Nassau 1980 (not retrieved)"]
[[host.chromophore.band]]
centre_nm = 600.0
fwhm_cm1 = 3000.0              # Report p.12; prior estimate 3000, unchanged
shape = "gaussian_energy"
peak_coeff = 10.0              # Report p.12: 10.0 cm-1/wt%; prior estimate 10.0, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"       # prior: "estimate"
source = "Report p.12 (Cr3+ row, 600 nm)"
[[host.chromophore.band]]
centre_nm = 440.0
fwhm_cm1 = 3500.0              # Report p.12; prior estimate 3500, unchanged
shape = "gaussian_energy"
peak_coeff = 8.0               # Report p.12: 8.0 cm-1/wt%; prior estimate 8.0, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"       # prior: "estimate"
source = "Report p.12 (Cr3+ row, 440 nm)"
[[host.chromophore.fluorescence]]
excitation_nm = [405.0, 532.0]  # estimate (visible excitation; not a UV-lamp statement)
emission_nm = [700.0]          # Cr3+ 2E emission, position in CZ not retrieved; kept from the prior file although the report lists Cr as Inert (D15)
emission_fwhm_nm = 30.0        # estimate
# unknown: quantum_yield
uv = { lw = "inert", sw = "inert" }   # prior: "unknown"; Report p.36: Inert
source = "Report p.36 (Cr, luminescence column: Inert under UV); 700 nm emission is an unverified estimate"

[[host.chromophore]]
id = "Cu, V, Ti, Mn, Ni dopants (commercial, unverified)"
kind = "ion"
elements = ["Cu", "V", "Ti"]
treatment = "none"
site = "Zr4+ site"
conc_unit = "wt% oxide"
# unknown: conc_typical, conc_max, color, bands. The report has no data for these dopants in CZ.
requires = []
excludes = []
# quenched_by: none known (empty)
sources = ["not in Report; not retrieved"]
# no [[host.chromophore.band]]: unknown (centre, FWHM, coefficient). Do not offer in the UI until sourced.
```

## Rutile
TiO2, tetragonal, uniaxial positive, n(o) 2.61, n(e) 2.90 (Report p.13: delta n = +0.287, P4_2/mnm). Optical edge at about 413 nm (3.0 eV; polarised, E perpendicular to c at slightly longer wavelength). Nominally pure synthetic rutile is pale yellow; reduction (O vacancies/Ti3+) produces blue to black with a broad band in the red/IR; oxidising anneal in air at 600-800 C bleaches back to pale yellow; Fe3+ and Cr3+ shift the effective edge to 480-520 nm (yellow-brown to red-brown). Do not offer Ti3+ together with an oxidising treatment. All rutile numbers of the report equal the earlier estimates of this file (Cronemeyer 1959 not retrieved).

```toml
[[host]]
name = "Rutile"
formula = "TiO2"
optical = "uniaxial"
uv_edge_nm = 413.0            # secondary: Report p.13 (Eg 3.0 eV -> 413 nm)
materials = ["Rutile"]

[[host.chromophore]]
id = "Ti3+ (reduced rutile, blue-black)"
kind = "ion"
elements = ["Ti"]
treatment = "none"
site = "interstitial Ti3+ / Ti3+ on lattice site near O vacancy (small-polaron absorption)"
conc_unit = "ppm atoms Ti3+"
conc_typical = [10.0, 1000.0]  # estimate
conc_max = 10000.0             # estimate
color = "deep blue to blue-black / opaque black (absorption increasing toward red)"
requires = []
excludes = []
quenched_by = ["oxidising anneal (air, 600-800 C; Report p.13)"]
confidence = "secondary"       # prior: "estimate"
sources = ["Report p.13-14, p.34", "Cronemeyer 1959 Phys. Rev. 113, 1222 (not retrieved)"]
[[host.chromophore.band]]
centre_nm = 700.0
shape = "edge"
onset_nm = 700.0               # Report p.14 table: onset 700 nm (the report prose on p.13 says the continuum rises from 600 nm, D22)
# unknown: fwhm_cm1, slope_cm1_per_nm
peak_coeff = 0.002             # Report p.14: 0.002 cm-1/ppm; prior estimate 0.002, unchanged. Check: 1 ppm Ti = 4.25 g/cm3 / 79.87 x 6.022e23 x 1e-6 = 3.2e16 cm-3, sigma = 6e-20 cm2: low for a blue-black polaron band (D31)
pol = { o = 1.0, e = 0.7 }     # Report p.14 (1.0, 0.7); prior estimate identical
confidence = "secondary"       # prior: "estimate"
source = "Report p.14 (Reduced Ti3+/V_O row)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }
source = "Report p.14 (Inert); rutile is non-fluorescent"

[[host.chromophore]]
id = "Fe3+ / Cr3+ (yellow-brown, red-brown)"
kind = "ion"
elements = ["Fe", "Cr"]
treatment = "none"
site = "Ti4+ octahedral site (D2h)"
conc_unit = "ppm atoms"
conc_typical = [10.0, 5000.0]  # estimate
conc_max = 20000.0             # estimate
color = "yellow-brown to red-brown (Report p.13)"
requires = []
excludes = []
# quenched_by: none known (empty)
confidence = "secondary"       # prior: "estimate"
sources = ["Report p.13-14, p.34", "Nassau 1980 (not retrieved)"]
[[host.chromophore.band]]
centre_nm = 400.0              # Report p.14: "400.0 (onset 480)"
shape = "edge"
onset_nm = 480.0               # Report p.14; prior estimate 480, unchanged
# unknown: fwhm_cm1, slope_cm1_per_nm
peak_coeff = 0.002             # Report p.14: 0.002 cm-1/ppm at onset; prior estimate 0.002, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"       # prior: "estimate"
source = "Report p.14 (Fe3+/Cr3+ edge row)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }
source = "Report p.14 (Inert)"
```

## Glass (silicate, N-BK7 and F2 as built-in hosts)
Isotropic. N-BK7 transmits to about 320 nm (report p.28; prior 310-330 nm), F2 (lead flint) about 350 nm (Pb2+). colored glass uses oxide additions in wt%. The report's glass table is in cm-1 per wt% for ions and cm-1 per ppm for the Au0, Ag0, Cu0 colloids and CdS(1-x)Sex edge (the table header says wt% for all, D23). Check of the report's ion coefficients: alpha = 2.303 x eps x c, with c = 10 x wt% x rho / M and rho = 2.5 g/cm3 (the prior file's assumption); for CoO c = 10 x 1 x 2.5 / 74.93 = 0.334 M per wt%, so eps = 100, 150, 130 gives alpha = 77, 115, 100 cm-1/wt%, matching the report's table (80.0, 115.0, 100.0). The same factor reproduces Cu2+ (eps 5 -> 3.6), Mn3+ (eps 100 -> 70) and uranyl (eps 15 -> 3.0). Redox: Fe2+/Fe3+ and Mn2+/Mn3+ are set by melt redox, so the amount of the chromophoric valence is smaller than the total oxide; the UI should offer the oxide and the valence ratio. Impossible/unrealistic: Se pink and Fe2+ (decolorising pairs); Mn3+ with Fe2+ (redox couple); Cu2+ with Cu colloid (reduced); Au/Ag/Cu colloids need striking heat treatment (550-600 C) and reducing agent (Sn2+); uranyl needs oxidising melt. The report has no entries for Nd3+, Er3+ or Se in glass, so those keep their earlier values.

```toml
[[host]]
name = "Glass (silicate)"
formula = "SiO2-based glass"
optical = "isotropic"
uv_edge_nm = 320.0            # secondary: Report p.28 (N-BK7 down to 320 nm, F2 edge 350 nm)
materials = ["Glass (N-BK7)", "Glass (F2)"]

[[host.chromophore]]
id = "Co2+ (blue)"
kind = "ion"
elements = ["Co"]
treatment = "none"
site = "tetrahedral network-former-like site (T_d)"
conc_unit = "wt% CoO"
conc_typical = [0.001, 0.1]    # estimate; Report p.28: 0.05 wt% CoO gives saturated royal blue (alpha(595) = 115 x 0.05 = 5.8 cm-1)
conc_max = 0.5                 # estimate
color = "intense royal blue (pink-blue to greenish blue by host)"
requires = []
excludes = []
# quenched_by: none known (empty)
confidence = "secondary"
sources = ["Report p.28-29, p.44-45", "CoGlass British Glass abstract (537, 595, 640 nm verified in summary); Bamford 1977"]
[[host.chromophore.band]]
centre_nm = 537.0
fwhm_cm1 = 1100.0              # Report p.29; prior estimate 1100, unchanged
shape = "gaussian_energy"
peak_coeff = 80.0              # Report p.29: 80.0 cm-1/wt% (eps ~ 100; 2.303 x 100 x 0.334 = 77); prior estimate 80, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"       # prior: "verified" (centre only)
source = "Report p.29 (Co2+ row, 537 nm); centre also CoGlass"
[[host.chromophore.band]]
centre_nm = 595.0
fwhm_cm1 = 1100.0              # Report p.29
shape = "gaussian_energy"
peak_coeff = 115.0             # Report p.29: 115.0 cm-1/wt% (eps ~ 150; 2.303 x 150 x 0.334 = 115); prior estimate 115, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"       # prior: "verified" (centre only)
source = "Report p.29 (Co2+ row, 595 nm); centre also CoGlass"
[[host.chromophore.band]]
centre_nm = 640.0
fwhm_cm1 = 1100.0              # Report p.29
shape = "gaussian_energy"
peak_coeff = 100.0             # Report p.29: 100.0 cm-1/wt% (eps ~ 130; 2.303 x 130 x 0.334 = 100); prior estimate 100, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"       # prior: "verified" (centre only)
source = "Report p.29 (Co2+ row, 640 nm); centre also CoGlass"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }
source = "Report p.45 (Co2+, luminescence column: Inert)"

[[host.chromophore]]
id = "Cu2+ (blue-turquoise)"
kind = "ion"
elements = ["Cu"]
treatment = "none"
site = "distorted octahedral modifier site (O_h)"
conc_unit = "wt% CuO"
conc_typical = [0.1, 3.0]      # estimate
conc_max = 8.0                 # estimate
color = "pale blue to cyan-blue / turquoise"
requires = []
excludes = ["Cu colloid (copper ruby)"]
quenched_by = ["reducing melt (-> Cu+, Cu0)"]
confidence = "secondary"
sources = ["Report p.28-29, p.45", "Bamford 1977; Weyl 1951 (not retrieved)"]
[[host.chromophore.band]]
centre_nm = 790.0              # 2Eg -> 2T2g
fwhm_cm1 = 4500.0              # Report p.28-29; prior estimate 4500, unchanged
shape = "gaussian_energy"
peak_coeff = 3.6               # Report p.29: 3.6 cm-1/wt% (eps ~ 5; 2.303 x 5 x 0.314 = 3.6); prior estimate 3.6, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.29 (Cu2+ row)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }
source = "Report p.45 (Cu2+, luminescence column: Inert)"

[[host.chromophore]]
id = "Cr3+ (green) / Cr6+ (yellow)"
kind = "ion"
elements = ["Cr"]
treatment = "none"
site = "octahedral (Cr3+, reducing melt); chromate tetrahedra [CrO4]2- (Cr6+, oxidising melt)"
conc_unit = "wt% Cr2O3"
conc_typical = [0.01, 0.5]     # estimate
conc_max = 1.0                 # estimate
color = "emerald green (reducing) to bright yellow (oxidising, Cr6+)"
requires = []
excludes = []
# quenched_by: none known (empty)
confidence = "secondary"
sources = ["Report p.28-29, p.45", "Bamford 1977"]
[[host.chromophore.band]]
centre_nm = 650.0
fwhm_cm1 = 2500.0              # Report p.29; prior estimate 2500, unchanged
shape = "gaussian_energy"
peak_coeff = 11.0              # Report p.29: 11.0 cm-1/wt%; prior estimate 11.0 (eps ~ 15 per Cr: 2.303 x 15 x 0.33 = 11.4), unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.29 (Cr3+ row, 650 nm)"
[[host.chromophore.band]]
centre_nm = 450.0
fwhm_cm1 = 3000.0              # Report p.29; prior estimate 3000, unchanged
shape = "gaussian_energy"
peak_coeff = 11.0              # Report p.29: 11.0 cm-1/wt%; prior estimate 11.0, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.29 (Cr3+ row, 450 nm)"
[[host.chromophore.band]]
# unknown: centre_nm, fwhm_cm1, slope_cm1_per_nm, peak_coeff (Cr6+ charge-transfer edge; the report gives only the onset in prose, no table row)
shape = "edge"
onset_nm = 500.0               # Report p.29 prose: chromate charge-transfer edge rising from 500 nm into the UV (new band, no coefficient)
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.29 (prose, Cr6+ chromate edge, onset 500 nm); no table coefficient"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }
source = "Report p.45 (Cr3+, luminescence column: Inert)"

[[host.chromophore]]
id = "Fe2+/Fe3+ (green-blue / yellow-brown)"
kind = "ion"
elements = ["Fe"]
treatment = "none"
site = "octahedral (Fe2+), tetrahedral (Fe3+)"
conc_unit = "wt% Fe2O3 total"
conc_typical = [0.01, 1.0]     # estimate
conc_max = 3.0                 # estimate
color = "pale blue-green (Fe2+ dominated), yellow-brown (Fe3+ dominated)"
requires = []
excludes = []
# quenched_by: none known (empty)
confidence = "secondary"
sources = ["Report p.28-29, p.45", "Bamford 1977"]
[[host.chromophore.band]]
centre_nm = 1050.0             # Fe2+ near-IR band; its visible tail makes blue-green
fwhm_cm1 = 5000.0              # Report p.29; prior estimate 5000, unchanged
shape = "gaussian_energy"
peak_coeff = 12.0              # Report p.29: 12.0 cm-1/wt% (scale by Fe2+ fraction); prior estimate 12.0, unchanged. Check: 1 wt% Fe2O3 = 0.157 M = 0.313 M Fe; eps = 12 / (2.303 x 0.313) = 17 L/mol/cm
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.29 (Fe2+/Fe3+ row, 1050 nm)"
[[host.chromophore.band]]
centre_nm = 380.0              # estimate: the report gives only "Onset 450" for this edge, not a centre
shape = "edge"
onset_nm = 450.0               # Report p.29: Fe3+ charge-transfer edge starting at 450 nm
# unknown: fwhm_cm1, slope_cm1_per_nm
peak_coeff = 8.0               # Report p.29: 8.0 cm-1/wt% (scale by Fe3+ fraction); prior estimate 8.0, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"       # prior: "estimate"; centre_nm estimate
source = "Report p.29 (Fe2+/Fe3+ row, CT edge onset 450 nm)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }
source = "Report p.45 (Fe, luminescence column: Inert)"

[[host.chromophore]]
id = "Mn3+ (purple)"
kind = "ion"
elements = ["Mn"]
treatment = "none"
site = "distorted octahedral (d4)"
conc_unit = "wt% Mn2O3 as Mn3+"
conc_typical = [0.05, 1.0]     # estimate
conc_max = 3.0                 # estimate
color = "purple / amethyst"
requires = []
excludes = ["Fe2+/Fe3+ (green-blue / yellow-brown)"]   # redox couple; reducing melt gives Mn2+
quenched_by = ["reducing melt (-> colorless Mn2+; Report p.28)", "solarisation (Mn2+ -> Mn3+ under UV; Fe3+ yellow decolorised)"]
confidence = "secondary"
sources = ["Report p.28-29, p.45", "Weyl 1951; Bamford 1977"]
[[host.chromophore.band]]
centre_nm = 500.0
fwhm_cm1 = 3500.0              # Report p.28-29; prior estimate 3500, unchanged
shape = "gaussian_energy"
peak_coeff = 70.0              # Report p.29: 70.0 cm-1/wt% (eps ~ 100; 2.303 x 100 x 0.317 = 73); prior estimate 70, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.29 (Mn3+ row)"
[[host.chromophore.fluorescence]]
excitation_nm = [254.0]
emission_nm = [520.0]          # Mn2+ green luminescence in glass, position approximate; the report lists Mn3+ as Inert (Mn3+ itself is non-luminescent, D34)
emission_fwhm_nm = 60.0        # estimate
# unknown: quantum_yield
uv = { lw = "weak", sw = "weak green (Mn2+)" }
source = "estimate (Mn2+); Report p.45 (Mn3+, luminescence column: Inert)"

[[host.chromophore]]
id = "Ni2+ (brown / purple)"
kind = "ion"
elements = ["Ni"]
treatment = "none"
site = "octahedral (soda-lime, brown) and tetrahedral/5-coordinate (potash, purple)"
conc_unit = "wt% NiO"
conc_typical = [0.05, 0.5]     # estimate
conc_max = 1.0                 # estimate
color = "brown (soda-lime), purple-violet (potash)"
requires = []
excludes = []
# quenched_by: none known (empty)
confidence = "secondary"
sources = ["Report p.28-29, p.45", "Weyl 1951; Bamford 1977"]
[[host.chromophore.band]]
centre_nm = 440.0
fwhm_cm1 = 3000.0              # Report p.29; prior estimate 3000, unchanged
shape = "gaussian_energy"
peak_coeff = 15.0              # Report p.29: 15.0 cm-1/wt%; prior estimate 15.0 (eps ~ 20), unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"       # prior: "estimate"
source = "Report p.29 (Ni2+ row, 440 nm); strongly host-dependent"
[[host.chromophore.band]]
centre_nm = 640.0
fwhm_cm1 = 3500.0              # Report p.29; prior estimate 3500, unchanged
shape = "gaussian_energy"
peak_coeff = 10.0              # Report p.29: 10.0 cm-1/wt%; prior estimate 10.0, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"       # prior: "estimate"
source = "Report p.29 (Ni2+ row, 640 nm)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }
source = "Report p.45 (Ni2+, luminescence column: Inert)"

[[host.chromophore]]
id = "Nd3+ (lilac / dichroic)"
kind = "ion"
elements = ["Nd"]
treatment = "none"
site = "network-modifier site"
conc_unit = "wt% Nd2O3"
conc_typical = [0.5, 5.0]      # estimate
conc_max = 10.0                # estimate
color = "lilac-purple (daylight), bluer in fluorescent light"
requires = []
excludes = []
# quenched_by: none known (empty)
confidence = "secondary"
sources = ["Bamford 1977 (not in Report)"]
[[host.chromophore.band]]
centre_nm = 580.0
fwhm_cm1 = 200.0               # estimate
shape = "gaussian_energy"
peak_coeff = 3.4               # estimate: eps ~ 10
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "line position well-known; not in Report"
[[host.chromophore.band]]
centre_nm = 523.0
fwhm_cm1 = 200.0               # estimate
shape = "gaussian_energy"
peak_coeff = 1.0               # estimate
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "line position recalled; not in Report"
[[host.chromophore.band]]
centre_nm = 745.0
fwhm_cm1 = 250.0               # estimate
shape = "gaussian_energy"
peak_coeff = 1.5               # estimate
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "line position recalled; not in Report"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_fwhm_nm, quantum_yield
emission_nm = [1060.0]
uv = { lw = "inert (visible)", sw = "inert (visible)" }
source = "IR only; general knowledge"

[[host.chromophore]]
id = "Er3+ (pink)"
kind = "ion"
elements = ["Er"]
treatment = "none"
site = "network-modifier site"
conc_unit = "wt% Er2O3"
conc_typical = [0.5, 5.0]      # estimate
conc_max = 10.0                # estimate
color = "pink"
requires = []
excludes = []
# quenched_by: none known (empty)
confidence = "secondary"
sources = ["Bamford 1977 (not in Report)"]
[[host.chromophore.band]]
centre_nm = 521.0
fwhm_cm1 = 180.0               # estimate
shape = "gaussian_energy"
peak_coeff = 1.5               # estimate: eps ~ 5
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "line position recalled (2H11/2); not in Report"
[[host.chromophore.band]]
centre_nm = 379.0
fwhm_cm1 = 200.0               # estimate
shape = "gaussian_energy"
peak_coeff = 2.0               # estimate
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "recalled; not in Report"
[[host.chromophore.band]]
centre_nm = 653.0
fwhm_cm1 = 200.0               # estimate
shape = "gaussian_energy"
peak_coeff = 0.5               # estimate
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "recalled; not in Report"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_fwhm_nm, quantum_yield
emission_nm = [1540.0]
uv = { lw = "inert (visible)", sw = "inert (visible)" }
source = "IR only"

[[host.chromophore]]
id = "Au colloid (gold ruby)"
kind = "colloid"
elements = ["Au"]
treatment = "heating"          # striking heat treatment ~ 550-600 C
site = "Au0 nanoparticles 5-25 nm (Report p.29); plasmon in glass n ~ 1.5"
conc_unit = "ppm wt Au"
conc_typical = [50.0, 300.0]   # estimate; ruby glass 0.01-0.03 wt%
conc_max = 500.0               # estimate
color = "cranberry to ruby red, pink"
requires = ["Sn2+ (reducing agent)"]
excludes = ["Cu2+ (blue-turquoise)", "Mn3+ (purple)"]
quenched_by = ["oxidising melt", "overheating (particle growth -> violet/blue)"]
confidence = "secondary"
sources = ["Report p.29, p.45", "Weyl 1951; Bamford 1977; Nassau 2001"]
[[host.chromophore.band]]
centre_nm = 540.0              # plasmon in glass ~ 530-550 nm
fwhm_cm1 = 3500.0              # Report p.29; prior estimate 3500, unchanged
shape = "gaussian_energy"
peak_coeff = 0.03              # Report p.29: 0.030 cm-1/ppm; prior estimate 0.03, unchanged. Check: 1 ppm wt Au = 2.5e-6 / 196.97 x 6.022e23 = 7.6e15 cm-3, so 3.9e-18 cm2 per Au atom, the right order for 10 nm Au particles
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.29 (Au0 colloid row)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }
source = "Report p.45 (Au0, luminescence column: Inert)"

[[host.chromophore]]
id = "Ag colloid (yellow stain)"
kind = "colloid"
elements = ["Ag"]
treatment = "heating"
site = "Ag0 nanoparticles 5-20 nm, plasmon 415 nm (Report p.29)"
conc_unit = "ppm wt Ag"
conc_typical = [100.0, 2000.0] # estimate
conc_max = 5000.0              # estimate
color = "yellow to amber (silver stain)"
requires = ["reducing agent"]
excludes = ["Cu2+ (blue-turquoise)"]
quenched_by = ["oxidising melt"]
confidence = "secondary"
sources = ["Report p.29, p.45", "Weyl 1951"]
[[host.chromophore.band]]
centre_nm = 415.0
fwhm_cm1 = 3500.0              # Report p.29; prior estimate 3500, unchanged
shape = "gaussian_energy"
peak_coeff = 0.01              # Report p.29: 0.010 cm-1/ppm; prior estimate 0.01, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.29 (Ag0 colloid row)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }
source = "Report p.45 (Ag0, luminescence column: Inert)"

[[host.chromophore]]
id = "Cu colloid (copper ruby)"
kind = "colloid"
elements = ["Cu"]
treatment = "heating"
site = "Cu0 nanoparticles 10-50 nm, plasmon 570 nm (Report p.29)"
conc_unit = "ppm wt Cu"
conc_typical = [500.0, 5000.0] # estimate
conc_max = 10000.0             # estimate
color = "opaque red to brick red (copper ruby / aventurine)"
requires = ["Sn2+ (reducing agent)"]
excludes = ["Cu2+ (blue-turquoise)"]
quenched_by = ["oxidising melt"]
confidence = "secondary"
sources = ["Report p.29, p.45", "Weyl 1951"]
[[host.chromophore.band]]
centre_nm = 570.0
fwhm_cm1 = 3500.0              # Report p.29; prior estimate 3500, unchanged
shape = "gaussian_energy"
peak_coeff = 0.004             # Report p.29: 0.004 cm-1/ppm; prior estimate 0.004, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.29 (Cu0 colloid row)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }
source = "Report p.45 (Cu0, luminescence column: Inert)"

[[host.chromophore]]
id = "Se / CdSe-CdS (pink, selenium ruby)"
kind = "colloid"
elements = ["Se"]
treatment = "heating"
site = "Se0 (pink decoloriser) or CdS(1-x)Se(x) nanocrystals / quantum dots (yellow-orange-red cut-off glass, edge tunable 500-650 nm, Report p.29)"
conc_unit = "ppm wt Se"
conc_typical = [1.0, 2000.0]   # estimate; pink decoloriser 1-10 ppm; ruby/cut-off 0.1-1 wt% Se with Cd
conc_max = 10000.0             # estimate
color = "pink (Se0, low conc.), yellow to deep ruby red (CdS(Se) edge)"
requires = ["Cd (for edge glasses)"]
excludes = ["Fe2+/Fe3+ (green-blue / yellow-brown)"]   # Se pink decolorises Fe-green; combination is intentional but not additive
quenched_by = ["oxidising melt (Se volatilises as SeO2)"]
confidence = "secondary"
sources = ["Report p.29-30, p.45-46 (CdS(1-x)Se(x) edge only)", "Weyl 1951; Bamford 1977"]
[[host.chromophore.band]]
centre_nm = 500.0
fwhm_cm1 = 3000.0              # estimate
shape = "gaussian_energy"
peak_coeff = 0.02              # estimate cm-1/ppm Se0
pol = { o = 1.0, e = 1.0 }
confidence = "estimate"
source = "centre recalled (~500 nm) for pink Se0; coefficient estimate; not in Report"
[[host.chromophore.band]]
centre_nm = 550.0              # Report p.30: "550.0 (onset 580)"
shape = "edge"
onset_nm = 580.0               # Report p.30; prior value 580 (CdSe(S) cut-off tunable 500-650 nm)
# unknown: fwhm_cm1, slope_cm1_per_nm
peak_coeff = 0.02              # Report p.30: 0.020 at onset (per ppm; no unit printed in the cell); prior estimate 0.02, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"       # prior: "estimate"
source = "Report p.30 (CdS(1-x)Se(x) edge row, onset 580 nm)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }
source = "Report p.46 (CdS(1-x)Se(x), luminescence column: Inert)"

[[host.chromophore]]
id = "Uranyl UO2 2+ (uranium yellow-green)"
kind = "ion"
elements = ["U"]
treatment = "none"
site = "uranyl complex ion in network-modifier site"
conc_unit = "wt% UO3"
conc_typical = [0.1, 2.0]      # estimate; vaseline glass 0.5-2 wt%
conc_max = 5.0                 # estimate
color = "canary yellow to yellow-green (green fluorescence)"
requires = ["oxidising melt"]
excludes = ["Mn3+ (purple)"]   # not chemically impossible but rarely combined; estimate
quenched_by = ["reducing melt (-> U4+, green-brown)"]
confidence = "secondary"
sources = ["Report p.29-30, p.46", "Weyl 1951; Bamford 1977"]
[[host.chromophore.band]]
centre_nm = 420.0              # vibronic structure 400-440 nm
fwhm_cm1 = 3000.0              # Report p.30; prior estimate 3000, unchanged (unresolved)
shape = "gaussian_energy"
peak_coeff = 3.0               # Report p.30: 3.0 cm-1/wt% (eps ~ 15; 2.303 x 15 x 0.0874 = 3.0); prior estimate 3.0, unchanged
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.30 (Uranyl row)"
[[host.chromophore.fluorescence]]
excitation_nm = [365.0, 254.0, 405.0]
emission_nm = [488.0, 510.0, 533.0, 557.0]   # Report p.29, p.45-46: four vibronic lines 488, 510, 533, 557 nm (green); ~850 cm-1 spacing
emission_fwhm_nm = 15.0        # estimate per line
# unknown: quantum_yield
uv = { lw = "strong green", sw = "strong green" }
source = "Report p.29 (luminescence column: 488, 510, 533, 557 nm, green; 'under UV excitation'); strength 'strong' from the prior file"
```

## Opal
Opal-A / opal-CT (SiO2 nH2O) is amorphous to nano-crystalline, isotropic, n about 1.42-1.46 (report p.27), with 3-10 wt% water. The UV cut-off of pure hydrous silica is near 200 nm. Play-of-color is Bragg diffraction from ordered silica spheres (150-350 nm) and is not absorption; the renderer must treat it separately (not modelled here). Body color comes from Fe3+ (yellow to red, fire opal via goethite/hematite nano-inclusions; charge-transfer edge from 580 nm), octahedral Ni2+ (bands 390 and 650 nm, apple-green prase/chrysoprase opal) and Cu2+ in hydrous copper-silicate (chrysocolla) inclusions (band 700 nm, turquoise blue). The report gives no Cr3+ opal data and none for Mn or organic/carbon dark opal, so the earlier combined placeholder "Ni2+ / Cr3+ / Cu2+" was replaced by the two entries the report supports. The report's opal coefficients are per ppm without saying atoms or weight; ppm wt is kept.

```toml
[[host]]
name = "Opal"
formula = "SiO2.nH2O"
optical = "isotropic"
uv_edge_nm = 200.0            # secondary: Report p.27 (UV cut-off of pure hydrous silica near 200 nm); prior estimate 200, unchanged
materials = ["Opal"]

[[host.chromophore]]
id = "Fe3+ / Fe-oxide nanoinclusions (fire opal)"
kind = "ion"
elements = ["Fe"]
treatment = "natural"
site = "dispersed Fe3+ and goethite/hematite (iron oxyhydroxide) nano-inclusions"
conc_unit = "ppm wt Fe"
conc_typical = [50.0, 3000.0]  # estimate
conc_max = 10000.0             # estimate
color = "yellow, fiery orange to red"
requires = []
excludes = []
quenched_by = ["dehydration / heating (cracking)"]
confidence = "secondary"       # prior: "estimate"
sources = ["Report p.27-28, p.44", "Gaillou et al. 2012 G&G 48(1) (not retrieved)"]
[[host.chromophore.band]]
centre_nm = 400.0              # Report p.27-28: "400.0 (onset 580)"
shape = "edge"
onset_nm = 580.0               # Report p.27-28; prior estimate 580, unchanged
# unknown: fwhm_cm1, slope_cm1_per_nm
peak_coeff = 0.002             # Report p.28: 0.002 cm-1/ppm at onset; prior estimate 0.002, unchanged. Check: 1 ppm Fe = 2.1e-6 / 55.85 x 6.022e23 = 2.3e16 cm-3, sigma = 8.8e-20 cm2, eps = 23 L/mol/cm
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"       # prior: "estimate"
source = "Report p.27-28 (Fire Opal, Fe3+ oxyhydroxides edge row)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }
source = "Report p.44 (Opal, luminescence column: Inert)"

[[host.chromophore]]
id = "Ni2+ (prase / chrysoprase opal, apple green)"
kind = "ion"
elements = ["Ni"]
treatment = "natural"
site = "octahedral silicate network (Report p.28)"
conc_unit = "ppm wt Ni"
# unknown: conc_typical, conc_max (not in the report)
color = "apple green"
requires = []
excludes = []
# quenched_by: none known (empty)
confidence = "secondary"
sources = ["Report p.27-28, p.44"]
[[host.chromophore.band]]
centre_nm = 650.0
fwhm_cm1 = 3000.0              # Report p.28
shape = "gaussian_energy"
peak_coeff = 0.005             # Report p.28: 0.005 cm-1/ppm. Check: 1 ppm wt Ni = 2.1e-6 / 58.69 x 6.022e23 = 2.2e16 cm-3, sigma = 2.3e-19 cm2, eps = 61 L/mol/cm, about 4-6x high for octahedral Ni2+ d-d (D27)
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.28 (Prase Opal, Ni2+ row, 650 nm)"
[[host.chromophore.band]]
centre_nm = 390.0
fwhm_cm1 = 3500.0              # Report p.28
shape = "gaussian_energy"
peak_coeff = 0.004             # Report p.28: 0.004 cm-1/ppm
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.28 (Prase Opal, Ni2+ row, 390 nm)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }
source = "Report p.44 (Opal, luminescence column: Inert)"

[[host.chromophore]]
id = "Cu2+ (chrysocolla inclusions, turquoise blue opal)"
kind = "ion"
elements = ["Cu"]
treatment = "natural"
site = "hydrous Cu-silicate (chrysocolla) clusters (Report p.28)"
conc_unit = "ppm wt Cu"
# unknown: conc_typical, conc_max (not in the report)
color = "turquoise blue"
requires = []
excludes = []
# quenched_by: none known (empty)
confidence = "secondary"
sources = ["Report p.27-28, p.44"]
[[host.chromophore.band]]
centre_nm = 700.0
fwhm_cm1 = 4000.0              # Report p.28
shape = "gaussian_energy"
peak_coeff = 0.008             # Report p.28: 0.008 cm-1/ppm. Check: 1 ppm wt Cu = 2.1e-6 / 63.55 x 6.022e23 = 2.0e16 cm-3, sigma = 4.0e-19 cm2, eps = 105 L/mol/cm, 5-20x above typical Cu2+ d-d (compare glass: eps 5); possible only through cluster coupling (D26)
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.28 (Blue Opal, Cu2+ chrysocolla row, 700 nm)"
[[host.chromophore.fluorescence]]
# unknown: excitation_nm, emission_nm, emission_fwhm_nm, quantum_yield
uv = { lw = "inert", sw = "inert" }
source = "Report p.44 (Opal, luminescence column: Inert)"
```

## Discrepancies with the report
Compared are the prior file (before this merge) and the user-supplied report. Differences above 10 % or in kind are listed first (D1-D18), followed by internal inconsistencies and physically doubtful report values (D19-D34). Where I know or can derive the literature value I say which is more plausible; I have not checked any primary paper.

Agreements: all quartz numbers, all SiC numbers, rutile, glass ions and colloids, CZ centres, widths and coefficients (except the points below) and diamond N3/N2, H3/H4, 550 nm and 480 nm coefficients equal the earlier values, so the earlier file was evidently built from the same source. Only the confidence labels and sources changed for these.

Prior vs report
- D1 Boron visible continuum (diamond, onset 700 nm). Prior 0.0005 cm-1/ppb = 0.5 cm-1/ppm (estimate, tuned to color). Report: sigma ~ 1.0e-17 cm2 = 1.76 cm-1/ppm (x 3.5). The cross-section is of the right order for acceptor photoionisation, but nothing here verifies it; 0.5 ppm would give alpha(700 nm) = 0.9 cm-1. Report value adopted; it needs a check. The conc_unit was changed from ppb to ppm.
- D2 Boron 2800 cm-1 (3571 nm) band. Prior: unknown, with two conflicting calibrations: 0.0350 (arXiv:2310.12587 snippet, ambiguous units) vs about 0.37 ppm per cm-1 (recalled). Report: alpha ~ 2.7 cm-1/ppm = 1/0.37, so it sides with the recalled value and disagrees with 0.0350 by a factor 10.6 (0.0350 ppm per cm-1 = 28.6 cm-1/ppm = sigma 1.6e-16 cm2, ten times the report's own sigma 1.0e-17 cm2, while 2.7 cm-1/ppm = 1.5e-17 cm2 agrees within 1.5x). 0.37 is the more plausible; the arXiv prefactor probably has different units or uses another peak/area. Unverified.
- D3 NV0 absorption. Prior 3.0 cm-1/ppm (estimate). Report sigma ~ 3.0e-17 cm2 -> 5.3 cm-1/ppm (x 1.76).
- D4 NV- absorption. Prior 2.0 cm-1/ppm (estimate). Report sigma ~ 2.0e-17 cm2 -> 3.5 cm-1/ppm (x 1.76). The prior recalled sigma ~ 3e-17 for NV- (Wee 2007), so the report may have swapped the two cross-sections (NV0 and NV- order in the cell is by row, 575 then 637). Not resolvable without the paper.
- D5 C-centre 270 nm. Alpha = 0.18 cm-1/ppm agrees with the prior (5.5 ppm per cm-1, recalled Dyer/Kiflawi). The report's own sigma ~ 1.5e-19 cm2 corresponds to only 0.026 cm-1/ppm (x 6.8 lower). The alpha value is the more plausible one (it equals the long-established ~5.5 ppm per cm-1 calibration; the sigma was probably meant for another transition, or mis-scaled).
- D6 GR1. Alpha = 0.60 cm-1/ppm (same as prior) but the report's sigma = 3.4e-17 cm2 implies 5.99 cm-1/ppm: internal factor 10. Alpha kept; if the sigma were right a 5 ppm green stone would have alpha = 30 cm-1 (opaque).
- D7 Boron 2800 cm-1 FWHM. Report lists FWHM 2800 cm-1 for a band centred at 2800 cm-1 (3571 nm): a column mix-up (the centre wavenumber in the FWHM column). Not adopted; the field is absent. Real boron acceptor structure is far narrower.
- D8 Tanzanite 455 nm polarisation. Prior { alpha 0.4, beta 0.8, gamma 1.0 } (estimate, "stronger along c"). Report { 1.0, 0.4, 0.2 }, i.e. strongest along alpha (X || a), reversed. The report is consistent with its own text (alpha is yellow-green/brown from 455 + 520 nm). Report adopted, but see D12.
- D9 Tanzanite 520 nm polarisation. Prior { 0.3, 0.5, 1.0 }; report { 1.0, 0.7, 0.3 }. Same reversal as D8.
- D10 Tanzanite heated state. Prior had one band set (with the heated 455 nm coefficient only in a comment) and a V4+/Ti placeholder with no band. The report gives full heated values (455: 0.0003, 520: 0.0002 cm-1/ppm V; polarisation 0.1 x 3 and 0.2/0.8/1.0 for 595 nm), now a second chromophore. Prior unheated 520 nm coefficient 0.0015 equals the report; prior heated 455 estimate 0.0003 equals the report.
- D11 Report heated polarisation is inconsistent with its own text (implausible). The text says heating removes the 455 and 520 nm bands only along alpha, and 595 nm is untouched. The table lowers the 455 and 520 nm coefficients by x 0.15 and x 0.13 AND sets all three weights to 0.1 (alpha 1.0 -> 0.1, beta 0.4 -> 0.1, gamma 0.2 -> 0.1), and drops the 595 nm alpha weight from 0.5 to 0.2. If the renderer multiplies peak_coeff by weight, the alpha absorption at 455 nm falls to 0.0003 x 0.1 = 3e-5 vs 0.0020 x 1.0 unheated (x 0.015) and beta/gamma are reduced although they should not change. The values were transcribed as given; a renderer should either keep the unheated weights with the reduced coefficients or the reverse, not both.
- D12 Tanzanite axis of the heating change and axis mapping. Report: bands removed "along the alpha axis", X || a, Y || b, Z || c. The retrieved PMC7465713 summary (used in the prior file): the 450-460 and 520 nm bands fall along c. These conflict unless c is the alpha direction, i.e. unless one of the two axis assignments is wrong. I cannot settle the orientation of zoisite's indicatrix from memory; treat both the alpha/beta/gamma mapping and the report's bleaching axis as unverified.
- D13 CZ line shapes. The report tags the Nd, Er, Pr and Ho 4f lines "Lorentzian"; the file's schema (and prior values) have gaussian_energy. Kept as gaussian_energy because the schema has no Lorentzian. In disordered CZ, inhomogeneous broadening favours near-Gaussian shapes, so the choice is acceptable for rendering.
- D14 Ce:CZ luminescence. Prior excitation [320, 365], emission [440, 520]. Report: Ce3+ 365 nm absorption and 520 nm emission (weak) only. Report adopted ([365], [520]); the 440 nm value was an unverified estimate.
- D15 Cr:CZ luminescence. Prior 700 nm emission (estimate). Report: "Inert". Under UV-lamp excitation inert is plausible, but 4A2/2E Cr3+ red emission excited by visible light is normal in oxides; the 700 nm entry was kept with an unverified comment and uv set to inert.
- D16 Co:CZ color. Prior "violet to blue-violet (unverified)"; report "royal blue" (590 nm band). Numbers equal; the color text was changed to the report's.
- D17 Cr3+ (and Co2+) in 8-fold CZ (physically doubtful). The report states 8-fold cubic coordination, but its Cr3+ bands (600 and 440 nm, 4T2 and 4T1 positions of an octahedral ion with Dq about 1650 cm-1) are typical for 6-fold sites; a true cubic 8-fold field is much weaker and inverted, which would put the bands in the NIR. Cr3+ in CZ presumably sits in locally relaxed 6/7-fold sites near oxygen vacancies. The coefficients cannot be checked.
- D18 Opal. Prior: combined Ni/Cr/Cu entry with no data. Report: Ni2+ (650, 390 nm) and Cu2+ chrysocolla (700 nm) bands, now two chromophores; no Cr3+ data, so Cr3+ opal is dropped and noted as unknown.

Internal or physical issues in the report
- D19 Quartz. The report's amethyst 357 nm entry has wavenumber 28,011 cm-1 in the prose table but 25,000 cm-1 in the directory (p.43). 1e7/357 = 28,011; the directory is a transcription error. Report quartz "ppm" is not defined as atoms or weight (7 % difference only).
- D20 NV- band centre. Report prose p.3 says the NV- band is centred at 560 nm; the table lists the ZPL 637.0 nm. 560 nm was kept (real maximum about 560-570 nm). Using 637 nm as a Gaussian centre would shift the color from pink toward blue.
- D21 Zero-phonon lines as band centres (physically doubtful). N3 (415.2), H3 (503.2), H4 (496.0), NV0 (575.0) and NV- (637.0) are tabulated at the ZPL wavelength with the width of the vibronic envelope. A symmetric Gaussian on the ZPL puts half of the absorption on the wrong side (the real vibronic sidebands of N3, H3 and NV lie at shorter wavelength). The prior file had the same convention for N3/H3/H4/NV0; the NV- centre differs (see D20). color effects are probably small for N3 and H3 (absorption in the blue) but real for NV0 (maximum about 550 nm vs ZPL 575 nm).
- D22 Rutile Ti3+ onset: report prose says the continuum rises "from 600 nm", the table says onset 700 nm. The table value (= prior) is kept.
- D23 Unit confusion. The CZ table header says cm-1/wt% but the Ho3+ entries are cm-1/ppma (a factor of roughly 1e4 in the unit). The glass table header says cm-1/wt% for all rows, but the Au0, Ag0, Cu0 colloid rows and the CdS(1-x)Se(x) edge are cm-1/ppm (the directory p.45 shows cm-1/ppm). Oxide names for CZ and glass wt% values are not given in the report. Taken as ppma / ppm / oxide wt% as in the prior file.
- D24 Diamond hydrogen band. The report's luminescence cell is "Thermochromic yellow", which is not a luminescence property; the prior uv text (yellow, chameleon, unverified) was kept.
- D25 Moissanite. The report calls the 625 nm band "conduction-band free carrier absorption". True free-carrier (Drude) absorption rises monotonically with wavelength and has no maximum at 625 nm; the 6H-SiC band is attributed to interconduction-band transitions. The position and widths are unaffected; the mechanism label is wrong.
- D26 Cu2+ chrysocolla in opal. 0.008 cm-1/ppm wt Cu = sigma 4.0e-19 cm2 = eps about 105 L/mol/cm, 5-20x above octahedral Cu2+ d-d absorption (glass: eps 5). Probably high by roughly an order of magnitude unless cluster coupling enhances it.
- D27 Ni2+ in opal. 0.005 cm-1/ppm wt Ni -> eps about 61 L/mol/cm, 4-6x above typical octahedral Ni2+ d-d values (about 10-15 in glass, report p.29: alpha 10-15 cm-1/wt% NiO, i.e. eps 15-20 per Ni).
- D28 The report's conclusions (p.46) list tanzanite among the chromophores that "cannot be simulated via single-cation absorption" and need IVCT, yet the tanzanite section and table model it with V3+ d-d bands plus a Ti3+/Ti4+ redox change only; no IVCT band is given.
- D29 Smoky quartz polarisation: directory p.43 gives { 1.0, 0.8 }; rules table p.31 states only w_o = 1.0. { 1.0, 0.8 } adopted.
- D30 Citrine mechanism. Prior: interstitial or nano-inclusion Fe3+. Report: alpha-Fe2O3 (hematite) nanoparticles of about 100 nm after heating above 450 C. I cannot confirm hematite from memory; Fe3+ in other oxide or hydroxide nano-phases would give a similar edge. Numbers are identical; the site text mentions both.
- D31 Rutile Ti3+ coefficient. 0.002 cm-1/ppm = sigma 6e-20 cm2, rather low for a band called "deep blue to opaque black" (a Ti3+ density of 1e19 cm-3 = 310 ppm would give only alpha = 0.6 cm-1). Plausible only if the practical range is thousands of ppm; unverified.
- D32 Diamond directory vs prose table. The directory (p.32) drops the N2, H4 and NV0 coefficients, the boron alpha 2.7 and the GR1 alpha 0.60 that the prose tables (p.3-5) have, and shows only sigma for some centres; the prose tables were used.
- D33 Diamond C-centre sigma may belong to the visible continuum (the cell has alpha = 0.18 and sigma = 1.5e-19 with a single row); if so it implies 0.026 cm-1/ppm at 560 nm (17x the file's color-tuned estimate 0.0015), i.e. 100 ppm would have alpha = 2.6 cm-1 and look orange-brown rather than canary. Left as the color-tuned estimate.
- D34 Glass Mn3+ luminescence. Report: Inert. The prior kept a Mn2+ green emission (520 nm) for the reduced form; both statements can be true (Mn3+ is not luminescent). Kept as an estimate.

## Open questions
1. Boron calibration: the report's alpha ~ 2.7 cm-1/ppm (0.37 ppm per cm-1) supports the recalled value and disagrees with the arXiv:2310.12587 snippet (0.0350) by 10.6x; the units and the peak-height/area convention of that paper still need to be read. The report does not say whether 2.7 refers to the 2800 cm-1 peak height.
2. The visible/near-IR continuum cross-section of B-diamond per ppm: the report's sigma ~ 1.0e-17 cm2 (1.76 cm-1/ppm at 700 nm) is 3.5x the earlier color-matching estimate; no check.
3. N3/N2/H3/H4/GR1/NV per-ppm calibrations (Davies 1992, Collins, Wee 2007) are only given as report values without sources; the NV0/NV- cross-section order, the GR1 sigma-vs-alpha factor 10 and the C-centre sigma-vs-alpha factor 6.8 need the primary papers. The visible continuum coefficient of the C centre is not in the report.
4. Tanzanite: axis-to-alpha/beta/gamma mapping and which axis loses the 455/520 nm bands (report alpha vs PMC7465713 c); heated polarisation weights; the V4+/Ti4+ band position is not given by the report; V and Ti concentrations (Rossman, Schmetzer, the PMC7465713 paper).
5. Quartz: report values equal the earlier estimates, so they are not independent confirmation; polarised amethyst spectrum (Cox 1977), smoky-quartz band components, rose quartz dumortierite IVCT band (Goreva 2001), prasiolite Fe2+ band, and ppm atoms vs weight still to be pinned down.
6. SiC: B acceptor not entered; N donor cross-section per ppma and polarisation in 6H need the paper; the 410 nm edge onset, slope and any Al-N compensation are not in the report.
7. CZ: oxide forms for wt% values; band positions and coefficients for Cu, V, Ti, Mn, Ni (not in the report); Cr3+/Co2+ site coordination (D17); the Pr3+ 485 and 612 nm line coefficients (named in the report prose, absent from its table); Ce fluorescence; Nassau 1980 table.
8. Glass: all epsilon values are recalled or report magnitudes; Bamford 1977 and Weyl 1951 need to be checked; Nd3+, Er3+ and Se0 glass entries are not in the report; the Cr6+ edge has no coefficient; host-dependent shifts (e.g. Co2+ 535 to 526 nm) and redox ratios are not modelled; the Fe3+ edge centre is unknown.
9. Rutile and opal: report numbers only (no primary source); opal Ni/Cu coefficients look high (D26, D27); no Cr3+ opal data; slopes and FWHM of all edges are unknown (the report gives none).
10. Diamond UV edge as a function of N for N-rich stones (225 nm vs about 300-400 nm) is not parameterised.

## Coverage table
| Host | Chromophores listed | Bands (secondary / verified / estimate) | Main gaps |
|---|---|---|---|
| Quartz | 5 (amethyst Fe4+, Fe3+ citrine, Al smoky, rose IVCT, prasiolite Fe2+) | 7 / 0 / 0 | report values equal prior estimates (no independent check), edge slope, ppm basis |
| Diamond | 8 (C centre, N3/N2, H3/H4, B, GR1, NV, 550 nm, H 480 nm) | 13 / 1 / 1 | continuum coefficient of the C centre, onset of the 550 nm edge, boron and NV/GR1 conversions, quantum yields |
| Tanzanite | 2 (V3+ natural, V3+/Ti4+ heated) | 6 / 0 / 0 | axis mapping and bleaching axis (D12), heated polarisation (D11), V4+ band, concentrations |
| Moissanite | 2 (N, Al) | 3 / 0 / 0 | edge onset and slope, B acceptor, quantum yields |
| Cubic zirconia | 9 (one is an empty stub for Cu/V/Ti/Mn/Ni) | 17 / 2 / 0 | line shapes, Pr 485/612 coefficients, Cu/V/Ti/Mn/Ni, Cr and Co site and coefficients, quantum yields |
| Rutile | 2 | 2 / 0 / 0 | coefficient plausibility (D31), edge slopes |
| Glass | 13 | 23 / 0 / 1 | Nd3+, Er3+ and Se0 are not in the report (kept as before), Cr6+ coefficient, Fe3+ edge centre, quantum yields |
| Opal | 3 (Fe3+, Ni2+, Cu2+) | 4 / 0 / 0 | conc ranges for Ni and Cu, Cr3+ and dark-opal chromophores, edge slope; play-of-color is not absorption |
