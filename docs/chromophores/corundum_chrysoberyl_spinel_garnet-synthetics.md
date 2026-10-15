# Chromophore dataset: corundum, chrysoberyl, spinel, YAG, GGG

## READ THIS FIRST: verification status

Updated against the user-supplied report "Gem Chromophore Spectroscopy Verification" (48 pp; cited below as **Report p.N**). The report gives per-host prose (pp. 6-13), inter-element rules (pp. 30-31) and a "Comprehensive Spectroscopic Calibration Directory" (pp. 32-46). It does NOT attribute individual numbers to individual references. No source behind the report was read, so **no value is `verified`**.

- `secondary`: the value was taken from the report (page and table row in `source`), or is textbook-standard and cited by name.
- `estimate`: not in the report; recalled from the literature or derived by me in an earlier pass. The reasoning is in a comment. Treat as a starting value, not as a measurement.
- **Unknown means the field is absent** (`# unknown` comment). `peak_coeff`, `quantum_yield`, `lifetime_ms`, `fwhm_cm1`, `pol` are deleted rather than written as 0.0 or -1.0.
- Schema extension: `fluorescence` blocks now also carry `confidence` (same meaning as on bands).
- Concentration conversions, number densities (computed from the lattice):
  - Al2O3 (rho 3.99 g/cm3, M 101.96): 3.91e-2 mol/cm3 -> 2.35e22 formula units/cm3 -> **4.70e22 Al/cm3** and 1.175e23 atoms/cm3 (5 atoms per f.u.). NOTE: the earlier version of this file wrote "2.35e22 Al/cm3"; that is the formula-unit density and is wrong by a factor 2 for Al. Corrected here.
  - 1 wt% Cr2O3 in Al2O3 = 0.01 x 3.99 / 151.99 x 2 x 6.022e23 = **3.16e20 Cr/cm3** (earlier file: 1.6e20, the same factor-2 slip; the report's wt% coefficients use 1.6e20 too, see Discrepancies 1-2).
  - Report "ppma" for corundum: every report sigma/alpha pair in corundum implies 1.17e17 cm-3 per ppma (e.g. Fe3+ 2.3e-20 cm2 -> 0.0027 cm-1; V3+ 1.0e-19 -> 0.0118; Mg hole 6.0e-17 -> 7.0). 1.175e17 = 1e-6 x 1.175e23 = **1 ppm of ALL atoms**. So `conc_unit = "ppma (all atoms)"` for corundum single-ion and hole rows, and 1 ppma(all atoms) = 2.5 ppm of Al sites (4.70e16 cm-3 per site-ppm). This is inferred from the arithmetic, the report never defines ppma.
  - MgAl2O4: 1.52e22 Mg(T) and 3.03e22 Al(O)/cm3. YAG: 1.39e22 Y, 2.31e22 Al and 9.2e22 atoms/cm3. For YAG, GGG and spinel `ppm atoms` stays "ppm of the substituted cation site population"; the report's ppma basis is not defined there (Discrepancy 4).
- Convention for `ivct_pair` (proposed extension): `alpha = peak_coeff * ppm_A * ppm_B` (cm^-1 per ppm^2). The pair strength scales with the product of the two ion concentrations (statistical pairs; in ordered nearest-neighbour pairs it saturates at the smaller one).
- Biaxial labelling: `a`, `b`, `g` are used for alpha/beta/gamma vibration directions, in the order of the report's (alpha, beta, gamma) table columns. The report text says "E parallel b strongest, then a, then c", which only matches the table if alpha=a, beta=b, gamma=c. The mapping to Pbnm axes is setting-dependent and NOT confirmed.

Sources:
- [Report] "Gem Chromophore Spectroscopy Verification" (user-supplied PDF, 48 pp). Corundum pp. 6-8 and 33-34; chrysoberyl pp. 8-9 and 34-35; spinel pp. 9-10 and 35-36; YAG/GGG pp. 12-13 and 37-38; ICT/pair rules p. 30; redox, quenching p. 31; conclusions p. 46. The report's reference list (pp. 47-48; mostly GIA Gems & Gemology, e.g. the Spring 2020 corundum-chromophore paper) is not mapped to numbers.
- [Nassau83] K. Nassau, *The Physics and Chemistry of Color*, Wiley 1983.
- [Burns93] R.G. Burns, *Mineralogical Applications of Crystal Field Theory*, 2nd ed., Cambridge UP 1993.
- [Fritsch88] E. Fritsch & G.R. Rossman, "An update on color in gems", Gems & Gemology 24 (1988), Parts 1-3.
- [Emmett03] J.L. Emmett et al., "Beryllium diffusion of ruby and sapphire", Gems & Gemology 39(2) (2003) 84-135.
- [Schmetzer] K. Schmetzer & H. Bank, papers on V, Cr, Fe in corundum, chrysoberyl and spinel (1980s). Not read.
- [Krebs71] J.J. Krebs & W.G. Maisch, Phys. Rev. B 4 (1971) 757. Not read.
- [Walling80] J.C. Walling et al., IEEE J. Quantum Electron. 16 (1980) 1302. Not read.
- [Search-Cr-ruby], [Search-alex], [Search-FeTi]: earlier WebSearch snippets (2026-10): ruby 418/559 nm and 545/416 nm; alexandrite 589 nm and Cr/V site competition; Fe2+-Ti4+ near 580 nm.

---

## 1. Corundum (Al2O3), uniaxial negative, trigonal

Overview. Octahedral Al3+ site (trigonally distorted, C3; face-sharing along c, edge-sharing in the basal plane, Report p.6). Ruby = Cr3+ (4A2 -> 4T2 U band 556 nm, 4T1 Y band 410 nm; transmission windows at about 480 nm and above 620 nm). Blue sapphire = Fe2+-Ti4+ IVCT (580 nm, E perp c) plus Fe2+-Fe3+ IVCT (880 nm, E parallel c dominant; its 700-780 nm tail adds inky green/dark blue in basalt sapphire). Yellow sapphire = Fe3+ (weak spin-forbidden bands 450/388/377 nm) and/or the trapped-hole centre (Mg2+ or Be2+ exceeding Ti4+/Si4+ donors). Padparadscha = trapped hole plus Cr3+. color-change = V3+ plus Cr3+. Ti3+ gives pink-violet in reduced synthetic (Ti:sapphire) crystals.

Host edge: band gap 8.8 eV (141 nm); trace Fe3+ (O->Fe CT) puts the natural edge at 300-330 nm (Report p.6, agrees with the prior value). Pure host is colorless 380-780 nm.

Interactions:
- Fe2+-Ti4+ needs both ions; strength goes with the pair product. Reducing anneal (H2, 1200-1600 C) makes Fe3+ -> Fe2+ and deepens blue; oxidising anneal reverses it (Report p.31).
- Fe quenches Cr3+ luminescence: Fe above 0.5-1.0 wt% FeO (= 0.55-1.1 wt% Fe2O3) removes R-line emission completely (Report p.31). Isolated Cr3+ in ruby has quantum yield about 0.90 (Report p.8, p.31).
- Cr3+ plus Fe2+-Ti4+ gives purple; V3+ with Cr3+ is the color-change pair.
- Trapped hole: needs acceptor (Mg2+, Be2+) in excess of donors (Ti4+, Si4+); forms by oxidising anneal (about 1600 C, O2); bleaches at about 400 C (Report p.8, p.31). Report assigns it to a hole next to Fe3+ ([h.-Fe3+]), padparadscha adds [h.-Cr3+] orange absorption (Discrepancy 6).
- Ti3+ does not coexist in significant quantity with Fe3+ (redox); only in strongly reducing growth.
- Not offered: Co or Ni with Ti4+ pair combinations; Cu colors.

```toml
[[host]]
name = "Corundum"
formula = "Al2O3"
optical = "uniaxial"
uv_edge_nm = 150.0   # intrinsic ~141 nm (8.8 eV); trace Fe3+ moves natural stones to 300-330 nm. secondary (Report p.6; Nassau83)
materials = ["Ruby", "Sapphire", "Padparadscha", "Pink Sapphire", "Blue Sapphire", "Yellow Sapphire", "Green Sapphire", "Purple Sapphire", "color-change Sapphire"]

[[host.chromophore]]
id = "Cr3+"
kind = "ion"
elements = ["Cr"]
site = "Al octahedral (C3)"
conc_unit = "wt% Cr2O3"
conc_typical = [0.01, 2.0]   # pink 0.01-0.1, ruby 0.5-2 (natural up to ~3); estimate, not in report
conc_max = 5.0               # estimate, not in report
color = "pink (<0.1) to red (0.5-2); purple-red o-ray, orange-red e-ray (dichroic)"
requires = []
excludes = []
quenched_by = ["Fe2+", "Fe3+"]   # luminescence quench only; absorption unaffected. Report p.31: > 0.5-1.0 wt% FeO kills R lines
confidence = "secondary"
sources = ["Report p.7, p.33", "Nassau83", "Burns93", "Krebs71 (unread)"]
# U band (4A2->4T2). Report p.7 Cr3+ row: 556.0 nm = 17,985 cm-1, FWHM 2800, sigma_perp 1.62e-19 cm2, sigma_par 1.22e-19 cm2,
#   alpha 25.9 cm-1/wt%, pol {1.0, 0.75} (sigma_par/sigma_perp = 1.22/1.62 = 0.753, consistent).
# Arithmetic: report alpha = 1.62e-19 x 1.6e20 = 25.9 cm-1/wt% (the report uses 1.6e20 Cr per wt% Cr2O3).
#   Correct density is 3.16e20 Cr/cm3 per wt% Cr2O3 (see header), so sigma_perp x N = 1.62e-19 x 3.16e20 = 51 cm-1/wt%.
#   Stored value is the report's 25.9; the sigma-consistent value 51 is the more plausible one (Discrepancy 1).
# Prior value: 19.0 (= 1.2e-19 x 1.6e20 rounded; same density slip).
[[host.chromophore.band]]
centre_nm = 556.0
fwhm_cm1 = 2800.0
shape = "gaussian_energy"
peak_coeff = 25.9
pol = { o = 1.0, e = 0.75 }
confidence = "secondary"
source = "Report p.7 (Corundum table, Cr3+ row, 556 nm); directory p.33 (sigma 1.62e-19, pol {1.0,0.75})"
# Y band (4A2->4T1). Report: 410.0 nm = 24,390 cm-1, FWHM 3500, sigma 1.20e-19 cm2 (E perp), alpha 19.2 cm-1/wt% (= 1.20e-19 x 1.6e20), pol {1.0, 0.85}.
#   Density-corrected: 1.20e-19 x 3.16e20 = 38 cm-1/wt%. Prior value 14.0 (0.7 x U, recalled).
[[host.chromophore.band]]
centre_nm = 410.0
fwhm_cm1 = 3500.0
shape = "gaussian_energy"
peak_coeff = 19.2
pol = { o = 1.0, e = 0.85 }
confidence = "secondary"
source = "Report p.7 (Corundum table, Cr3+ row, 410 nm)"
# R1 zero-phonon absorption. Report p.7: 694.3 nm = 14,403 cm-1, FWHM 15 cm-1, sigma and alpha "-" (not given), pol {1.0, 1.0}.
[[host.chromophore.band]]
centre_nm = 694.3
fwhm_cm1 = 15.0
shape = "gaussian_energy"
# peak_coeff unknown (report gives "-")
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.7 (Corundum table, Cr3+ row, 694.3 nm R1; strength not given)"
[[host.chromophore.fluorescence]]
excitation_nm = [410.0, 556.0]
emission_nm = [692.9, 694.3]      # R2, R1 (2E->4A2). Report p.6-7: "intense red under UV"
# emission_fwhm_nm = 0.5 is not in the report: kept from the prior pass (estimate)
emission_fwhm_nm = 0.5
quantum_yield = 0.9               # Report p.7 (Phi ~ 0.9), p.31 (isolated Cr3+ in ruby and spinel ~0.90); falls with Fe
uv = { lw = "strong red", sw = "moderate to strong red" }   # prior pass; report only says "intense red"
lifetime_ms = 3.4                 # estimate (300 K, textbook), not in report
confidence = "secondary"
source = "Report p.7 (Luminescence column, Cr3+ row), p.31; lifetime and emission width from prior estimate"

[[host.chromophore]]
id = "V3+"
kind = "ion"
elements = ["V"]
site = "Al octahedral (C3)"
conc_unit = "ppma (all atoms)"   # report coefficient is per ppm; basis inferred = all atoms (header)
# Prior: wt% V2O3 [0.01, 1.0], max 2.0. 1 wt% V2O3 = 0.01 x 3.99 / 149.88 x 2 x 6.022e23 = 3.21e20 V/cm3 = 3.21e20 / 1.175e17 = 2730 ppma.
conc_typical = [27, 2730]    # converted from prior 0.01-1.0 wt% V2O3; not in report
conc_max = 5460              # converted from prior 2.0 wt%
color = "violet-blue by daylight, greener/greyer by incandescent when combined with Cr; alone grey-green to violet. Report p.6: blue-green daylight -> purple incandescent"
requires = []
excludes = ["Ti3+"]  # both reduced, not documented together; estimate
quenched_by = []
confidence = "secondary"
sources = ["Report p.8, p.34", "Schmetzer papers (unread)", "Fritsch88 (unread)"]
# Report p.8 V3+ row: 570.0 nm (17,543 cm-1) FWHM 3000, sigma_perp 1.00e-19 cm2 -> 0.0118 cm-1/ppm; 400.0 nm (25,000) FWHM 3500, 1.10e-19 -> 0.0130; pol {1.0, 0.8} both.
# Arithmetic: 1.00e-19 x 1.175e17 = 0.01175 (report 0.0118); 1.10e-19 x 1.175e17 = 0.01293 (report 0.0130).
[[host.chromophore.band]]
centre_nm = 570.0
fwhm_cm1 = 3000.0
shape = "gaussian_energy"
peak_coeff = 0.0118   # cm-1 per ppma (all atoms)
pol = { o = 1.0, e = 0.8 }
confidence = "secondary"
source = "Report p.8 (Corundum table, V3+ row, 570 nm)"
[[host.chromophore.band]]
centre_nm = 400.0
fwhm_cm1 = 3500.0
shape = "gaussian_energy"
peak_coeff = 0.0130   # cm-1 per ppma (all atoms)
pol = { o = 1.0, e = 0.8 }
confidence = "secondary"
source = "Report p.8 (Corundum table, V3+ row, 400 nm)"

[[host.chromophore]]
id = "Fe3+"
kind = "ion"
elements = ["Fe"]
site = "Al octahedral (C3)"
conc_unit = "ppma (all atoms)"   # report coefficient per ppm; basis inferred = all atoms (header)
# Prior: wt% Fe2O3 [0.01, 1.5], max 3.0. 1 wt% Fe2O3 = 0.01 x 3.99 / 159.69 x 2 x 6.022e23 = 3.01e20 Fe/cm3 = 2560 ppma.
# Report p.6: "several thousand atomic ppm to produce saturated yellow" (consistent with ~1 wt%).
conc_typical = [26, 3840]    # converted from prior 0.01-1.5 wt% Fe2O3; not in report
conc_max = 7680              # converted from prior 3.0 wt%
color = "pale yellow to golden yellow (sharp bands at 377, 388, 450 nm); strongly Fe-rich = greenish-yellow"
requires = []
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.6-7, p.33", "Nassau83", "Burns93", "Fritsch88 (unread)"]
# Report p.7 Fe3+ row: 450.0 / 388.0 / 377.0 nm = 22,222 / 25,773 / 26,525 cm-1; FWHM 500 / 300 / 300; sigma 2.30e-20 / 3.50e-20 / 4.00e-20 cm2;
#   alpha 0.0027 / 0.0041 / 0.0047 cm-1/ppm; pol {1.0,1.0} each. Arithmetic: x 1.175e17 -> 0.00270 / 0.00411 / 0.00470 (all match).
# Fe3+ gives no luminescence and quenches Cr3+ emission (Report p.7).
[[host.chromophore.band]]
centre_nm = 450.0
fwhm_cm1 = 500.0
shape = "gaussian_energy"
peak_coeff = 0.0027   # cm-1 per ppma (all atoms)
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.7 (Corundum table, Fe3+ row, 450 nm)"
[[host.chromophore.band]]
centre_nm = 388.0
fwhm_cm1 = 300.0
shape = "gaussian_energy"
peak_coeff = 0.0041   # cm-1 per ppma (all atoms)
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.7 (Corundum table, Fe3+ row, 388 nm)"
[[host.chromophore.band]]
centre_nm = 377.0
fwhm_cm1 = 300.0
shape = "gaussian_energy"
peak_coeff = 0.0047   # cm-1 per ppma (all atoms)
pol = { o = 1.0, e = 1.0 }
confidence = "secondary"
source = "Report p.7 (Corundum table, Fe3+ row, 377 nm)"

[[host.chromophore]]
id = "Fe2+-Ti4+"
kind = "ivct_pair"
elements = ["Fe", "Ti"]
site = "edge-sharing Al octahedra in the basal plane (Report p.6, p.30)"   # prior: face-sharing along c AND edge-sharing (Discrepancy 7)
conc_unit = "ppm atoms"   # alpha = peak_coeff * ppm_Fe2+ * ppm_Ti4+. Report says only "ppm^2"; basis (site vs all atoms) NOT defined
conc_typical = [10, 2000]   # Fe 100-2000 ppm, Ti 20-500 ppm in natural blue; estimate, not in report
conc_max = 10000            # estimate
color = "blue to violet-blue (cornflower to inky); o-ray deeper blue, e-ray greener/greyer"
requires = ["Fe2+", "Ti4+"]
excludes = ["Fe3+-only"]
quenched_by = []
confidence = "secondary"
sources = ["Report p.6-7, p.30, p.33", "Emmett03", "Search-FeTi", "Nassau83", "Burns93"]
# Report p.7: 580.0 nm = 17,241 cm-1, FWHM 3500, sigma_perp 1.94e-18 cm2 per pair, alpha 5.0e-5 cm-1/ppm^2, pol {1.0, 0.1}. Inert (quenches).
# Report alpha equals the prior estimate (5.0e-5; prior: ~5 cm-1 at Fe 1000 ppm x Ti 100 ppm).
# Pair-count check: pairs/cm3 = Z x N_Al x x_Fe x x_Ti, Z = 3 edge-sharing basal neighbours, N_Al = 4.70e22.
#   Per ppma^2 (x = 2.5e-6 per ppma): 3 x 4.70e22 x (2.5e-6)^2 = 8.8e11 pairs -> x sigma 1.94e-18 = 1.7e-6 cm-1/ppma^2, i.e. 29x BELOW the report's 5.0e-5.
#   Report alpha/sigma = 2.6e13 pairs per ppm^2 does not follow from counting (Discrepancy 8). Clustering of charge-compensating Fe2+/Ti4+ pairs could raise it.
[[host.chromophore.band]]
centre_nm = 580.0
fwhm_cm1 = 3500.0
shape = "gaussian_energy"
peak_coeff = 5.0e-5   # cm-1 per ppm^2; report value, equals prior estimate
pol = { o = 1.0, e = 0.1 }
confidence = "secondary"
source = "Report p.7 (Corundum table, Fe2+-Ti4+ row); p.30 (w_o 1.0, w_e 0.1); directory p.33"
# no fluorescence (quenches)

[[host.chromophore]]
id = "Fe2+-Fe3+"
kind = "ivct_pair"
elements = ["Fe"]
site = "face-sharing Al pairs along c"
conc_unit = "ppm atoms"   # alpha = peak_coeff * ppm_Fe2+ * ppm_Fe3+. Basis not defined in report
conc_typical = [100, 5000]   # estimate, not in report
conc_max = 20000             # estimate
color = "greenish/greyish modifier; in basalt sapphire dark blue-green to near black; e-ray stronger (visible tail 700-780 nm, Report p.6)"
requires = ["Fe2+", "Fe3+"]
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.6-7, p.30, p.33", "Burns93", "Nassau83"]
# Report p.7: 880.0 nm = 11,363 cm-1, FWHM 4500, sigma 4.50e-19 cm2 per pair, alpha 1.0e-5 cm-1/ppm^2, pol {0.3, 1.0} (w_o, w_e).
# Pair-count check: Z = 1 face-sharing neighbour: 1 x 4.70e22 x (2.5e-6)^2 = 2.9e11 pairs/ppma^2 x 4.5e-19 = 1.3e-7, i.e. 76x below the report's 1.0e-5.
[[host.chromophore.band]]
centre_nm = 880.0
fwhm_cm1 = 4500.0
shape = "gaussian_energy"
peak_coeff = 1.0e-5   # cm-1 per ppm^2
pol = { o = 0.3, e = 1.0 }
confidence = "secondary"
source = "Report p.7-8 (Corundum table, Fe2+-Fe3+ row); p.30 (w_e 1.0, w_o 0.3); directory p.33"

[[host.chromophore]]
id = "Mg-hole"
kind = "color_centre"
elements = ["Mg"]
site = "O2- hole next to Mg2+/Be2+ acceptor; report assigns [h.-Fe3+] (Report p.6, p.31)"
conc_unit = "ppma (all atoms)"   # Report p.8: cm-1/ppma. Hole fraction is what colors, and is NOT equal to Mg
# Prior: [5, 100] ppm atoms (Mg content, per cation site), max 300. x 0.4 (site ppm -> all-atom ppma) = [2, 40], 120.
# Report p.31: golden-yellow "at concentrations as low as 1.5 ppma" -> lower bound.
conc_typical = [1.5, 40]
conc_max = 120    # converted from prior 300 ppm
color = "yellow to orange-yellow, fades with light/heat; Be-diffused stones; deep golden-yellow (Report p.6); with Cr3+ [h.-Cr3+] orange gives padparadscha"
requires = []
excludes = ["Fe2+-Ti4+"]   # holes need an oxidised, charge-balanced state; they coexist only weakly. estimate (prior)
quenched_by = []
confidence = "secondary"
sources = ["Report p.6, p.8, p.31", "Emmett03 (unread detail)", "Kvapil et al. (unread)"]
# Report p.8: onset 450.0 nm (22,222 cm-1), FWHM 5000, sigma 6.0e-17 cm2, alpha 7.0 cm-1/ppma, pol {1.0, 0.7}; bleaches at 400 C.
# Arithmetic: 6.0e-17 x 1.175e17 = 7.05 (report 7.0). Absorption continuum rising from 550 nm into the UV, so centre = onset, not a peak.
[[host.chromophore.band]]
centre_nm = 450.0
fwhm_cm1 = 5000.0
shape = "gaussian_energy"
peak_coeff = 7.0   # cm-1 per ppma (all atoms)
pol = { o = 1.0, e = 0.7 }
confidence = "secondary"
source = "Report p.8 (Corundum table, [h.-Fe3+] row, 450 nm onset); shape kept from prior pass"

[[host.chromophore]]
id = "Ti3+"
kind = "ion"
elements = ["Ti"]
site = "Al octahedral (C3)"
conc_unit = "ppma (all atoms)"   # report coefficient per ppm; basis inferred = all atoms
# Prior: wt% Ti2O3 [0.01, 0.2], max 0.5. 1 wt% Ti2O3 = 0.01 x 3.99 / 143.76 x 2 x 6.022e23 = 3.34e20 Ti/cm3 = 2840 ppma.
conc_typical = [28, 570]   # converted from prior 0.01-0.2 wt%; not in report
conc_max = 1420            # converted from prior 0.5 wt% (Ti:sapphire laser crystals ~0.1-0.25 wt%)
color = "pink-violet (synthetic only, reduced atmosphere)"
requires = []
excludes = ["Fe3+", "Fe2+-Ti4+"]
quenched_by = []
confidence = "secondary"
sources = ["Report p.6, p.8, p.34", "Moulton 1986 JOSA B 3, 125 (Ti:sapphire; unread)", "Nassau83"]
# Report p.8 Ti3+ row: 490.0 nm (20,408 cm-1) FWHM 4000, sigma 4.0e-20 cm2, alpha 0.0050 cm-1/ppm; 540.0 nm (18,518) FWHM 2500, sigma 2.0e-20, alpha 0.0025; pol {0.6, 1.0}.
# Arithmetic: 4.0e-20 x 1.175e17 = 0.0047 (report 0.0050, +6 %); 2.0e-20 x 1.175e17 = 0.00235 (report 0.0025). Stored as reported.
[[host.chromophore.band]]
centre_nm = 490.0
fwhm_cm1 = 4000.0
shape = "gaussian_energy"
peak_coeff = 0.0050   # cm-1 per ppma (all atoms)
pol = { o = 0.6, e = 1.0 }   # same as prior estimate; Ti:sapphire absorption is pi (e) polarised
confidence = "secondary"
source = "Report p.8 (Corundum table, Ti3+ row, 490 nm)"
[[host.chromophore.band]]
centre_nm = 540.0
fwhm_cm1 = 2500.0
shape = "gaussian_energy"
peak_coeff = 0.0025   # cm-1 per ppma (all atoms)
pol = { o = 0.6, e = 1.0 }
confidence = "secondary"
source = "Report p.8 (Corundum table, Ti3+ row, 540 nm shoulder)"
[[host.chromophore.fluorescence]]
excitation_nm = [490.0]
emission_nm = [780.0]   # Report p.6, p.8 (broad NIR laser band)
emission_fwhm_nm = 180.0   # prior estimate, not in report
# quantum_yield unknown (near 1 in laser-grade, but Fe/Ti4+ residuals quench)
uv = { lw = "none visible", sw = "none visible" }
confidence = "secondary"
source = "Report p.8 (Luminescence column, Ti3+ row); emission width from prior estimate"
```

---

## 2. Chrysoberyl (BeAl2O4), biaxial positive, orthorhombic (Pbnm)

Overview. Two Al sites: mirror site Al(M) (Cs, 78 % of Cr3+, intermediate field Dq/B about 2.15) and inversion site Al(I) (Ci, 22 %) (Report p.8). Cr3+ 4T2 at 585-590 nm (FWHM 2800 cm-1) and 4T1 at 410-420 nm (FWHM 3500 cm-1) leave windows at 480-510 nm and above 630 nm: bluish-green by daylight, purplish-red by incandescent light. Strongly trichroic, E parallel b strongest, then a, then c. R lines 680.4 (R1) and 678.5 (R2) nm, tau about 0.26 ms. Fe3+ on the mirror site: 440 nm band plus a continuum rising below 400 nm (yellow to yellow-green), quenches Cr3+ emission. V3+: 600 and 425 nm (mint to blue-green).

Host edge: gap above 7 eV; trace Fe3+ CT puts the cutoff at 330-380 nm (Report p.8).

Interactions:
- Cr3+ and V3+ compete for sites (earlier snippet; the report is silent).
- Fe3+ on the mirror site efficiently quenches Cr3+ luminescence (Report p.8-9, table "Inert (quenches Cr3+)").
- Cr3+ with Fe3+ gives yellow-green daylight color and a weaker color change at high Fe.
- Not offered: Ti, Co, Ni, Mn.

```toml
[[host]]
name = "Chrysoberyl"
formula = "BeAl2O4"
optical = "biaxial"
uv_edge_nm = 330.0   # Fe3+ CT cutoff 330-380 nm (Report p.8); lower end kept from prior. secondary
materials = ["Chrysoberyl", "Alexandrite", "Synthetic Alexandrite"]

[[host.chromophore]]
id = "Cr3+"
kind = "ion"
elements = ["Cr"]
site = "Al mirror site Al(M, Cs, 78 %) and inversion site Al(I, Ci, 22 %)"
conc_unit = "wt% Cr2O3"   # report says "wt%"; oxide assumed
conc_typical = [0.02, 0.5]   # estimate: natural alexandrite up to ~1 wt% Cr2O3; laser rods 0.05-0.3; not in report
conc_max = 1.5               # estimate
color = "bluish-green by daylight to raspberry/purplish-red by incandescent light; strongly trichroic (E||b > E||a > E||c, Report p.8)"
requires = []
excludes = []
quenched_by = ["Fe3+"]
confidence = "secondary"
sources = ["Report p.8-9, p.34", "Walling80 (unread)", "Search-alex", "Nassau83"]
# Report p.9 Cr3+ row: 585.0 nm (17,094 cm-1) FWHM 2800, 22.0 cm-1/wt%, pol (a,b,g) {0.65,1.0,0.45};
#   420.0 nm (23,810) FWHM 3500, 18.0 cm-1/wt%, pol {0.70,1.0,0.50}; 680.4 nm R1 (14,697) FWHM 12 cm-1, coeff "-", pol {0.50,1.0,0.30}.
# Prior: 585 nm (Search-alex 589), 425 nm; coefficients unknown; pol placeholder {1,1,1}.
[[host.chromophore.band]]
centre_nm = 585.0
fwhm_cm1 = 2800.0
shape = "gaussian_energy"
peak_coeff = 22.0   # cm-1 per wt% Cr2O3
pol = { a = 0.65, b = 1.0, g = 0.45 }   # report (alpha, beta, gamma) order; axis mapping unconfirmed (header)
confidence = "secondary"
source = "Report p.9 (Chrysoberyl table, Cr3+ row, 585 nm); directory p.34-35"
[[host.chromophore.band]]
centre_nm = 420.0   # prior 425; report table 420 (prose: 410-420)
fwhm_cm1 = 3500.0
shape = "gaussian_energy"
peak_coeff = 18.0   # cm-1 per wt% Cr2O3
pol = { a = 0.70, b = 1.0, g = 0.50 }
confidence = "secondary"
source = "Report p.9 (Chrysoberyl table, Cr3+ row, 420 nm)"
[[host.chromophore.band]]
centre_nm = 680.4
fwhm_cm1 = 12.0
shape = "gaussian_energy"
# peak_coeff unknown (report gives "-")
pol = { a = 0.50, b = 1.0, g = 0.30 }
confidence = "secondary"
source = "Report p.9 (Chrysoberyl table, Cr3+ row, 680.4 nm R1; strength not given)"
[[host.chromophore.fluorescence]]
excitation_nm = [420.0, 585.0]
emission_nm = [680.4, 678.5]   # R1 and R2 of the mirror site (Report p.8-9)
emission_fwhm_nm = 1.0         # prior estimate, not in report
# quantum_yield unknown
uv = { lw = "weak to moderate red (Fe-poor stones); none if Fe-rich", sw = "weak red" }   # prior pass, not in report
lifetime_ms = 0.26   # Report p.9 (tau ~ 0.26 ms); agrees with prior recalled value
confidence = "secondary"
source = "Report p.9 (Luminescence column, Cr3+ row); uv response and line width from prior estimate"

[[host.chromophore]]
id = "Fe3+"
kind = "ion"
elements = ["Fe"]
site = "Al(M) mirror site (Cs)"
conc_unit = "wt% Fe2O3"   # report says "wt%"; oxide assumed
conc_typical = [0.05, 2.0]   # estimate, not in report
conc_max = 3.0               # estimate
color = "yellow to yellow-green (chrysoberyl); 440 nm band plus UV continuum"
requires = []
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.8-9, p.34", "Search-alex (Fe concentration snippet)", "Nassau83"]
# Report p.9 Fe3+ row: 440.0 nm (22,727 cm-1) FWHM 1500, 8.0 cm-1/wt%; 375.0 nm edge (26,667) FWHM 5000, 60.0 cm-1/wt%; pol {0.8,1.0,0.6} both.
# Plausibility: 8.0 cm-1/wt% Fe2O3 ~ 1 wt% = 3.0e20 Fe/cm3 (3.7 g/cm3, M 159.69) -> sigma 2.7e-20 cm2, same order as corundum Fe3+ 450 nm (2.3e-20).
[[host.chromophore.band]]
centre_nm = 440.0
fwhm_cm1 = 1500.0
shape = "gaussian_energy"
peak_coeff = 8.0   # cm-1 per wt% Fe2O3
pol = { a = 0.8, b = 1.0, g = 0.6 }
confidence = "secondary"
source = "Report p.9 (Chrysoberyl table, Fe3+ row, 440 nm)"
[[host.chromophore.band]]
centre_nm = 375.0
fwhm_cm1 = 5000.0
shape = "exponential_edge"   # report calls it "(edge)"; shape not stated for chrysoberyl
peak_coeff = 60.0   # cm-1 per wt% Fe2O3
pol = { a = 0.8, b = 1.0, g = 0.6 }
confidence = "secondary"
source = "Report p.9 (Chrysoberyl table, Fe3+ row, 375 nm edge)"

[[host.chromophore]]
id = "V3+"
kind = "ion"
elements = ["V"]
site = "Al octahedral"
conc_unit = "wt% V2O3"   # report says "wt%"; oxide assumed
conc_typical = [0.0, 0.5]   # estimate, not in report
conc_max = 0.5
color = "mint-green to blue-green (Report p.8); contributes to some natural color-change chrysoberyl"
requires = []
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.8-9, p.34", "Search-alex (V/Cr competition snippet)"]
# Report p.9 V3+ row: 600.0 nm (16,667 cm-1) FWHM 3000, 15.0 cm-1/wt%; 425.0 nm (23,529) FWHM 3200, 14.0 cm-1/wt%; pol {0.7,1.0,0.5} both.
# Plausibility: 1 wt% V2O3 = 0.01 x 3.7 / 149.88 x 2 x 6.022e23 = 2.97e20 V/cm3 -> sigma 5.1e-20 cm2, normal for V3+ d-d.
[[host.chromophore.band]]
centre_nm = 600.0
fwhm_cm1 = 3000.0
shape = "gaussian_energy"
peak_coeff = 15.0   # cm-1 per wt% V2O3
pol = { a = 0.7, b = 1.0, g = 0.5 }
confidence = "secondary"
source = "Report p.9 (Chrysoberyl table, V3+ row, 600 nm)"
[[host.chromophore.band]]
centre_nm = 425.0
fwhm_cm1 = 3200.0
shape = "gaussian_energy"
peak_coeff = 14.0   # cm-1 per wt% V2O3
pol = { a = 0.7, b = 1.0, g = 0.5 }
confidence = "secondary"
source = "Report p.9 (Chrysoberyl table, V3+ row, 425 nm)"
```

---

## 3. Spinel (MgAl2O4), isotropic cubic

Overview. Normal spinel: Mg on tetrahedral (T) sites, Al on octahedral (O) sites; natural spinel 5-10 % inverted; Verneuil spinel MgO.nAl2O3, n = 1.5-3.5 (Report p.9). Cr3+ (O) red: 4A2->4T2 545 nm, 4T1 390 nm, emission 685.5 nm with vibronic sidebands 675-720 nm ("organ-pipe"). Co2+ (T) vivid blue: triplet 540/580/625 nm, epsilon 400-600 L/mol/cm, 50 ppm already vivid. Fe2+ (T) weak spin-forbidden bands 460 and 480 nm; Fe2+-Fe3+ clusters add broad absorption 600-750 nm (dark denim, greenish-blue). V3+ (O) 570 nm: green to violet-blue color change in synthetics. Mn2+ (T): weak absorption 450 nm, bright green luminescence 515 nm.

Host edge: gap 7.8 eV (160 nm); trace Fe3+ puts natural edge at 330-370 nm (Report p.9).

Interactions:
- Co2+ T-site and Fe2+ T-site are both blue; Co dominates above about 10 ppm.
- Cr3+ with Co2+ gives violet to purple.
- Fe quenches Cr3+ emission (Report p.31, "ruby and spinel": isolated Cr3+ Phi about 0.90).
- Fe2+-Fe3+ clusters (600-750 nm) are described only in prose, with no centre or strength; not modelled (Open questions).
- Not offered: Ti IVCT, Ni.

```toml
[[host]]
name = "Spinel"
formula = "MgAl2O4"
optical = "isotropic"
uv_edge_nm = 330.0   # natural, Fe3+ (330-370 nm, Report p.9); intrinsic ~160 nm. secondary
materials = ["Spinel", "Red Spinel", "Blue Spinel", "Cobalt Spinel", "Synthetic Spinel"]

[[host.chromophore]]
id = "Cr3+"
kind = "ion"
elements = ["Cr"]
site = "Al octahedral (Oh)"
conc_unit = "wt% Cr2O3"   # report says "wt%"; oxide assumed
conc_typical = [0.01, 1.0]   # estimate, not in report
conc_max = 3.0
color = "pink to ruby-red (red spinel); with Fe, brownish red"
requires = []
excludes = []
quenched_by = ["Fe2+", "Fe3+"]
confidence = "secondary"
sources = ["Report p.9-10, p.35", "Nassau83", "Burns93", "Fritsch88 (unread)"]
# Report p.10 Cr3+ row: 545.0 nm (18,348 cm-1) FWHM 2600, 20.0 cm-1/wt%; 390.0 nm (25,641) FWHM 3500, 16.0 cm-1/wt%; Gaussian. Pol {1.0} (isotropic).
# Plausibility: 1 wt% Cr2O3 = 0.01 x 3.58 / 151.99 x 2 x 6.022e23 = 2.84e20 Cr/cm3 -> sigma = 20 / 2.84e20 = 7.0e-20 cm2, 0.43 of ruby's 1.62e-19: plausible for a weaker field.
[[host.chromophore.band]]
centre_nm = 545.0
fwhm_cm1 = 2600.0
shape = "gaussian_energy"
peak_coeff = 20.0   # cm-1 per wt% Cr2O3
confidence = "secondary"
source = "Report p.10 (Spinel table, Cr3+ row, 545 nm); directory p.35"
[[host.chromophore.band]]
centre_nm = 390.0
fwhm_cm1 = 3500.0
shape = "gaussian_energy"
peak_coeff = 16.0   # cm-1 per wt% Cr2O3
confidence = "secondary"
source = "Report p.10 (Spinel table, Cr3+ row, 390 nm)"
[[host.chromophore.fluorescence]]
excitation_nm = [390.0, 545.0]
emission_nm = [685.5, 686.7]   # 685.5 Report p.9-10; 686.7 (N line) recalled, prior pass
emission_fwhm_nm = 1.0         # prior estimate
quantum_yield = 0.9            # Report p.31 (isolated Cr3+ in ruby and spinel, Phi ~ 0.90)
uv = { lw = "strong red", sw = "weak to moderate red" }   # lw: Report p.10 "organ-pipe red, LW strong"; sw from prior pass
confidence = "secondary"
source = "Report p.10 (Luminescence column, Cr3+ row), p.31"

[[host.chromophore]]
id = "Co2+"
kind = "ion"
elements = ["Co"]
site = "Mg tetrahedral (Td)"
conc_unit = "ppm atoms"   # report: cm-1/ppm; basis taken as ppm of Mg sites (checked below)
conc_typical = [5, 500]    # natural cobalt spinel tens-hundreds ppm; report: 50 ppm already vivid blue (p.9); synthetic up to 0.1-0.5 wt% CoO
conc_max = 5000
color = "vivid cobalt blue (triplet 540/580/625 nm); electric blue at 50 ppm (Report p.9); deep violet-blue at higher conc."
requires = []
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.9-10, p.35", "Burns93", "Nassau83", "Shigley & Stockton 1984 G&G 20 (unread)"]
# Report p.10: 580.0 / 625.0 / 540.0 nm, FWHM 700 each, 0.020 / 0.015 / 0.012 cm-1/ppm. Identical to the prior estimates (no change in value).
# Check with report epsilon 400-600 L/mol/cm: sigma = eps x 3.82e-21 = 1.5e-18 .. 2.3e-18 cm2; 1 ppm of Mg sites = 1.52e16 cm-3
#   -> 0.023 .. 0.035 cm-1/ppm for the strongest component; report 0.020 is at the low end (basis = Mg-site ppm is consistent).
[[host.chromophore.band]]
centre_nm = 580.0
fwhm_cm1 = 700.0
shape = "gaussian_energy"
peak_coeff = 0.020
confidence = "secondary"
source = "Report p.10 (Spinel table, Co2+ row, 580 nm); directory p.35"
[[host.chromophore.band]]
centre_nm = 625.0
fwhm_cm1 = 700.0
shape = "gaussian_energy"
peak_coeff = 0.015
confidence = "secondary"
source = "Report p.10 (Spinel table, Co2+ row, 625 nm)"
[[host.chromophore.band]]
centre_nm = 540.0
fwhm_cm1 = 700.0
shape = "gaussian_energy"
peak_coeff = 0.012
confidence = "secondary"
source = "Report p.10 (Spinel table, Co2+ row, 540 nm)"

[[host.chromophore]]
id = "Fe2+"
kind = "ion"
elements = ["Fe"]
site = "Mg tetrahedral (Td)"
conc_unit = "wt% FeO"
conc_typical = [0.01, 2.0]   # estimate, not in report
conc_max = 10.0
color = "blue to violet-blue (natural blue spinel), greyish at high Fe; dark denim with Fe2+-Fe3+ clusters (Report p.10)"
requires = []
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.10, p.35", "Fritsch88 (unread)", "Burns93"]
# Report p.10 Fe2+ row: 460.0 nm (21,739 cm-1) FWHM 1500, 0.80 cm-1/wt%; 480.0 nm (20,833) FWHM 1200, 0.60 cm-1/wt%.
# Plausibility: 1 wt% FeO = 0.01 x 3.58 / 71.84 x 6.022e23 = 3.0e20 Fe/cm3 -> sigma 2.7e-21 cm2, eps 0.7 L/mol/cm: right for spin-forbidden Td Fe2+.
[[host.chromophore.band]]
centre_nm = 460.0
fwhm_cm1 = 1500.0
shape = "gaussian_energy"
peak_coeff = 0.80   # cm-1 per wt% FeO
confidence = "secondary"
source = "Report p.10 (Spinel table, Fe2+ row, 460 nm)"
[[host.chromophore.band]]
centre_nm = 480.0
fwhm_cm1 = 1200.0
shape = "gaussian_energy"
peak_coeff = 0.60   # cm-1 per wt% FeO
confidence = "secondary"
source = "Report p.10 (Spinel table, Fe2+ row, 480 nm)"

[[host.chromophore]]
id = "V3+"
kind = "ion"
elements = ["V"]
site = "Al octahedral (Oh)"
conc_unit = "wt% V2O3"
conc_typical = [0.05, 1.0]   # estimate, not in report
conc_max = 2.0
color = "synthetic color change: green to violet-blue (Report p.10)"
requires = []
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.10, p.35", "Nassau83 (unread detail)"]
# Report p.10 V3+ row: 570.0 nm (17,543 cm-1) FWHM 3000, 12.0 cm-1/wt%; 415.0 nm (24,096) FWHM 3200, 10.0 cm-1/wt%.
# Plausibility: 1 wt% V2O3 = 0.01 x 3.58 / 149.88 x 2 x 6.022e23 = 2.88e20 V/cm3 -> sigma 4.2e-20 cm2 (eps ~ 11 L/mol/cm), normal for V3+ d-d.
[[host.chromophore.band]]
centre_nm = 570.0
fwhm_cm1 = 3000.0
shape = "gaussian_energy"
peak_coeff = 12.0   # cm-1 per wt% V2O3
confidence = "secondary"
source = "Report p.10 (Spinel table, V3+ row, 570 nm)"
[[host.chromophore.band]]
centre_nm = 415.0
fwhm_cm1 = 3200.0
shape = "gaussian_energy"
peak_coeff = 10.0   # cm-1 per wt% V2O3
confidence = "secondary"
source = "Report p.10 (Spinel table, V3+ row, 415 nm; not in prose)"

[[host.chromophore]]
id = "Mn2+"
kind = "ion"
elements = ["Mn"]
site = "Mg tetrahedral (Td)"
conc_unit = "ppm atoms"
conc_typical = [10, 3000]   # estimate, not in report
conc_max = 10000
color = "no visible body color (spin-forbidden); bright green fluorescence (Report p.10)"
requires = []
excludes = []
quenched_by = ["Fe2+"]   # prior pass, not in report
confidence = "secondary"
sources = ["Report p.9-10, p.35", "standard luminescence lit., unread"]
# Report p.10: 450.0 nm (22,222 cm-1) FWHM 400, 0.001 cm-1/ppm. sigma = 0.001 / 1.52e16 = 6.6e-20 cm2 -> eps = 17 L/mol/cm,
#   about an order of magnitude high for spin-forbidden Td Mn2+ (Discrepancy C).
[[host.chromophore.band]]
centre_nm = 450.0
fwhm_cm1 = 400.0
shape = "gaussian_energy"
peak_coeff = 0.001   # cm-1 per ppm
confidence = "secondary"
source = "Report p.10 (Spinel table, Mn2+ row, 450 nm); directory p.35"
[[host.chromophore.fluorescence]]
excitation_nm = [365.0, 450.0]
emission_nm = [515.0]   # 4T1(G)->6A1, Report p.10 (bright green, SW/LW)
emission_fwhm_nm = 40.0   # prior estimate
# quantum_yield unknown
uv = { lw = "green", sw = "green" }   # Report p.10 "bright green, SW/LW"
confidence = "secondary"
source = "Report p.10 (Luminescence column, Mn2+ row); excitation and width from prior pass"
```

---

## 4. YAG (Y3Al5O12), isotropic cubic garnet (synthetic only)

Overview. Y on dodecahedral sites (D2, 24c), Al on octahedral (16a) and tetrahedral (24d) sites (Report p.12). Cr3+ (O) green (590 and 430 nm; R lines 687.0/688.5 nm); Nd3+ lilac to pink-purple (586 nm strongest; 530, 746, 808 nm); Ce3+ yellow (460 nm, 340 nm, emission 540 nm yellow-green, Phi 0.90, tau 65 ns); Er3+ pink (380, 520, 652 nm); Ho3+ yellow-orange (450, 538, 640 nm). The report has NO YAG data for Co2+ or V3+: its Co2+/Cr3+/Fe3+ table at the top of p.12 is the end of the cubic-zirconia section ("8-fold Zr"), so those two rows below keep their prior estimates.

Host edge: gap 6.5 eV, transparent to 215 nm (Report p.12).

Interactions:
- RE3+ substitute Y (Nd about 1.5 at%, Ce about 0.5 at%, Er and Ho tens of at%).
- Ce3+ and Cr3+ coexist (Ce -> Cr transfer). Ce3+ with Nd3+ possible.
- Not offered: Fe2+/Ti4+ IVCT.

Number densities: Y 1.39e22 cm-3 (1 at% = 1.39e20; 1 ppm of Y sites = 1.39e16), Al 2.31e22 cm-3, all atoms 9.2e22 cm-3. The report's ppma basis is undefined; see Discrepancy 4.

```toml
[[host]]
name = "YAG"
formula = "Y3Al5O12"
optical = "isotropic"
uv_edge_nm = 215.0   # 6.5 eV gap, transparent to 215 nm (Report p.12). secondary
materials = ["YAG", "Green YAG", "Pink YAG", "Lilac YAG", "Yellow YAG", "Blue YAG"]

[[host.chromophore]]
id = "Cr3+"
kind = "ion"
elements = ["Cr"]
site = "Al octahedral (16a)"
conc_unit = "ppm atoms"   # of Al sites; report says ppma, basis undefined
conc_typical = [100, 20000]   # estimate, not in report
conc_max = 50000   # ~5 at%; estimate
color = "green (emerald-like); yellow-green at low conc."
requires = []
excludes = []
quenched_by = ["Fe3+"]
confidence = "secondary"
sources = ["Report p.12-13, p.37", "Nassau83", "Burns93 (garnets chapter)", "laser literature (unread)"]
# Report p.13: 590.0 nm (16,949 cm-1) FWHM 2800, 8.0e-4 cm-1/ppma; 430.0 nm (23,256) FWHM 3300, 8.0e-4. Identical to prior estimates.
# Prior derivation: sigma(4T2) ~3.5e-20 cm2 x 2.3e16 cm-3 per Al-site ppm = 8e-4. If the report's ppma were all atoms (9.2e16), sigma would be 8.7e-21.
[[host.chromophore.band]]
centre_nm = 590.0
fwhm_cm1 = 2800.0
shape = "gaussian_energy"
peak_coeff = 8.0e-4
confidence = "secondary"
source = "Report p.13 (YAG/GGG table, YAG Cr3+ row, 590 nm); directory p.37"
[[host.chromophore.band]]
centre_nm = 430.0
fwhm_cm1 = 3300.0
shape = "gaussian_energy"
peak_coeff = 8.0e-4
confidence = "secondary"
source = "Report p.13 (YAG/GGG table, YAG Cr3+ row, 430 nm)"
[[host.chromophore.fluorescence]]
excitation_nm = [430.0, 590.0]
emission_nm = [687.0, 688.5]   # sharp R lines, Report p.13
emission_fwhm_nm = 0.5         # prior estimate
# quantum_yield unknown
uv = { lw = "weak red", sw = "weak red" }   # prior pass, not in report
# lifetime_ms unknown
confidence = "secondary"
source = "Report p.13 (Luminescence column, YAG Cr3+ row); width and uv from prior pass"

[[host.chromophore]]
id = "Nd3+"
kind = "ion"
elements = ["Nd"]
site = "Y dodecahedral (24c, D2)"
conc_unit = "ppm atoms"
conc_typical = [1000, 15000]   # estimate, not in report
conc_max = 20000   # ~2 at%
color = "pale lilac/violet; shifts to pink-purple under incandescent light (Report p.12)"
requires = []
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.12-13, p.37", "Nassau83", "laser literature (unread)"]
# Report p.13: 586.0 nm (17,065 cm-1) FWHM 40, 6.0e-4 cm-1/ppma; 746.0 (13,405) FWHM 40, 3.0e-4; 530.0 (18,868) FWHM 60, 2.5e-4.
#   6.0e-4 per site-ppm x 1e4 = 6.0 cm-1 per at%, plausible (808 nm line is ~9 cm-1/at%, recalled).
[[host.chromophore.band]]
centre_nm = 586.0
fwhm_cm1 = 40.0
shape = "gaussian_energy"
peak_coeff = 6.0e-4
confidence = "secondary"
source = "Report p.13 (YAG/GGG table, YAG Nd3+ row, 586 nm)"
[[host.chromophore.band]]
centre_nm = 746.0
fwhm_cm1 = 40.0
shape = "gaussian_energy"
peak_coeff = 3.0e-4
confidence = "secondary"
source = "Report p.13 (YAG/GGG table, YAG Nd3+ row, 746 nm)"
[[host.chromophore.band]]
centre_nm = 530.0
fwhm_cm1 = 60.0
shape = "gaussian_energy"
peak_coeff = 2.5e-4
confidence = "secondary"
source = "Report p.13 (YAG/GGG table, YAG Nd3+ row, 530 nm)"
# 808 nm line: named in the report prose (p.12) but not tabulated. Strength from the prior pass: ~9 cm-1 per at% (recalled) = 9e-4 per ppm.
[[host.chromophore.band]]
centre_nm = 808.0
# fwhm_cm1 unknown
shape = "gaussian_energy"
peak_coeff = 9.0e-4
confidence = "estimate"
source = "Centre: Report p.12 (prose); strength: recalled ~9 cm-1/at%, unread"
[[host.chromophore.fluorescence]]
excitation_nm = [586.0, 746.0, 808.0]
emission_nm = [1064.0]   # Report p.13 (infrared laser line)
emission_fwhm_nm = 1.0   # prior estimate
# quantum_yield unknown
uv = { lw = "none visible", sw = "none visible" }
lifetime_ms = 0.23   # 4F3/2 at ~1 at% (recalled, unread), not in report
confidence = "secondary"
source = "Report p.13 (Luminescence column, YAG Nd3+ row); lifetime from prior estimate"

[[host.chromophore]]
id = "Ce3+"
kind = "ion"
elements = ["Ce"]
site = "Y dodecahedral (24c; 4f-5d)"
conc_unit = "ppm atoms"
conc_typical = [100, 5000]   # estimate, not in report
conc_max = 10000   # segregation coefficient low (~0.1); ~1 at% practical limit; estimate
color = "yellow to yellow-green (460 nm band absorbs blue)"
requires = []
excludes = []
quenched_by = []   # thermal quenching only above ~500 K (prior pass)
confidence = "secondary"
sources = ["Report p.12-13, p.37", "luminescence lit. (YAG:Ce phosphors), unread"]
# Report p.12-13: 460.0 nm (21,739 cm-1) FWHM 2800, sigma 2.8e-18 cm2, 2.8e-2 cm-1/ppma; 340.0 nm (29,412) FWHM 2500, 1.5e-2 cm-1/ppma.
# Check: sigma x 1.39e16 (Y-site ppm) = 2.8e-18 x 1.39e16 = 0.039, report 0.028 is 28 % lower (Discrepancy 11). Prior: sigma 2e-18 -> 0.028.
# Oscillator strength f = 4.32e-9 x eps x FWHM x 1.064 with eps = 2.8e-18/3.82e-21 = 733 -> f = 0.0094: plausible for Ce3+ 4f-5d.
[[host.chromophore.band]]
centre_nm = 460.0
fwhm_cm1 = 2800.0
shape = "gaussian_energy"
peak_coeff = 0.028
confidence = "secondary"
source = "Report p.13 (YAG/GGG table, YAG Ce3+ row, 460 nm); p.12 (sigma 2.8e-18)"
[[host.chromophore.band]]
centre_nm = 340.0
fwhm_cm1 = 2500.0
shape = "gaussian_energy"
peak_coeff = 0.015
confidence = "secondary"
source = "Report p.13 (YAG/GGG table, YAG Ce3+ row, 340 nm); was unknown"
[[host.chromophore.fluorescence]]
excitation_nm = [340.0, 460.0]
emission_nm = [540.0]   # Report p.12-13 (yellow-green); prior pass had [530, 560] recalled
emission_fwhm_nm = 100.0   # prior estimate
quantum_yield = 0.9    # Report p.12 (Phi ~ 0.90)
uv = { lw = "strong yellow-green", sw = "weak (host absorbs 254 nm poorly)" }   # prior pass
lifetime_ms = 6.5e-5   # Report p.12 (tau ~ 65 ns)
confidence = "secondary"
source = "Report p.12 (prose), p.13 (Luminescence column, YAG Ce3+ row)"

[[host.chromophore]]
id = "Er3+"
kind = "ion"
elements = ["Er"]
site = "Y dodecahedral (24c)"
conc_unit = "ppm atoms"
conc_typical = [10000, 300000]   # estimate, not in report
conc_max = 500000
color = "pink (lines 380, 520, 652 nm; Report p.12)"
requires = []
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.12-13, p.37", "Nassau83"]
# Report p.13: 520.0 nm (19,231 cm-1) FWHM 100, 3.0e-4 cm-1/ppma; 652.0 nm (15,337) FWHM 80, 1.5e-4. The 380 nm line is prose only (p.12).
[[host.chromophore.band]]
centre_nm = 520.0
fwhm_cm1 = 100.0
shape = "gaussian_energy"
peak_coeff = 3.0e-4
confidence = "secondary"
source = "Report p.13 (YAG/GGG table, YAG Er3+ row, 520 nm)"
[[host.chromophore.band]]
centre_nm = 652.0
fwhm_cm1 = 80.0
shape = "gaussian_energy"
peak_coeff = 1.5e-4
confidence = "secondary"
source = "Report p.13 (YAG/GGG table, YAG Er3+ row, 652 nm)"
[[host.chromophore.band]]
centre_nm = 380.0
# fwhm_cm1 unknown
shape = "gaussian_energy"
# peak_coeff unknown
confidence = "secondary"
source = "Report p.12 (prose: absorption lines at 380, 520, 652 nm); not in table"
[[host.chromophore.fluorescence]]
emission_nm = [1540.0]   # infrared
confidence = "secondary"
source = "Report p.13 (Luminescence column, YAG Er3+ row)"

[[host.chromophore]]
id = "Ho3+"
kind = "ion"
elements = ["Ho"]
site = "Y dodecahedral (24c)"
conc_unit = "ppm atoms"
conc_typical = [10000, 300000]   # estimate, not in report
conc_max = 500000
color = "yellow-orange (lines 450, 538, 640 nm; Report p.12)"
requires = []
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.12-13, p.37", "Nassau83"]
# Report p.13: 450.0 nm (22,222 cm-1) FWHM 100, 2.5e-4 cm-1/ppma; 538.0 nm (18,587) FWHM 100, 2.0e-4. The 640 nm line is prose only (p.12).
[[host.chromophore.band]]
centre_nm = 450.0
fwhm_cm1 = 100.0
shape = "gaussian_energy"
peak_coeff = 2.5e-4
confidence = "secondary"
source = "Report p.13 (YAG/GGG table, YAG Ho3+ row, 450 nm)"
[[host.chromophore.band]]
centre_nm = 538.0
fwhm_cm1 = 100.0
shape = "gaussian_energy"
peak_coeff = 2.0e-4
confidence = "secondary"
source = "Report p.13 (YAG/GGG table, YAG Ho3+ row, 538 nm)"
[[host.chromophore.band]]
centre_nm = 640.0
# fwhm_cm1 unknown
shape = "gaussian_energy"
# peak_coeff unknown
confidence = "secondary"
source = "Report p.12 (prose: Ho3+ peaks at 450, 538, 640 nm); not in table"
[[host.chromophore.fluorescence]]
emission_nm = [2080.0]   # infrared (the usual Ho 5I7->5I8 line is nearer 2090-2100 nm)
confidence = "secondary"
source = "Report p.13 (Luminescence column, YAG Ho3+ row)"

[[host.chromophore]]
id = "Co2+"
kind = "ion"
elements = ["Co"]
site = "Al tetrahedral (24d)"
conc_unit = "ppm atoms"
conc_typical = [100, 5000]   # estimate
conc_max = 10000
color = "blue (reported for Czochralski YAG:Co; limited documentation)"
requires = []
excludes = []
quenched_by = []
confidence = "estimate"
sources = ["unread; marked low confidence. NOT in the report (its Co2+ row on p.12 is cubic zirconia)"]
[[host.chromophore.band]]
centre_nm = 590.0
fwhm_cm1 = 1500.0
shape = "gaussian_energy"
# peak_coeff unknown
confidence = "estimate"
source = "recalled; low confidence; not in report"

[[host.chromophore]]
id = "V3+"
kind = "ion"
elements = ["V"]
site = "Al octahedral"
conc_unit = "ppm atoms"
conc_typical = [500, 10000]   # estimate
conc_max = 20000
color = "green"
requires = []
excludes = []
quenched_by = []
confidence = "estimate"
sources = ["Nassau83 (unread detail); not in the report"]
[[host.chromophore.band]]
centre_nm = 610.0
fwhm_cm1 = 3000.0
shape = "gaussian_energy"
# peak_coeff unknown
confidence = "estimate"
source = "recalled; not in report"
```

---

## 5. GGG (Gd3Ga5O12), isotropic cubic garnet (synthetic only)

Overview. Gd on dodecahedral sites (24c), Ga on octahedral (16a) and tetrahedral sites. Larger lattice (12.38 A vs 12.01 A in YAG) weakens the field, so Cr3+ 4T2 moves to 620 nm and 4T1 to 450 nm (yellowish-green body color, broad NIR emission at 730 nm). Nd3+ lilac (586 nm, emission 1060 nm). Optical cutoff at 270 nm from Gd3+ f-f lines (8S -> 6I at 273 nm, 6P at 311 nm) (Report p.12).

Host edge: 270 nm (Report p.12).

Interactions:
- Cr3+ in the weaker field has 4T2 close to 2E (broad emission).
- Nd3+ on the Gd site follows the YAG pattern (586 nm).
- Not offered: Ce3+, IVCT pairs.

```toml
[[host]]
name = "GGG"
formula = "Gd3Ga5O12"
optical = "isotropic"
uv_edge_nm = 270.0   # Gd3+ f-f cutoff (Report p.12). secondary
materials = ["GGG", "Green GGG", "Lilac GGG"]

[[host.chromophore]]
id = "Cr3+"
kind = "ion"
elements = ["Cr"]
site = "Ga octahedral (16a)"
conc_unit = "ppm atoms"   # report ppma, basis undefined
conc_typical = [500, 20000]   # estimate, not in report
conc_max = 30000
color = "yellowish-green; in weaker field than YAG, band shifts red (Report p.12)"
requires = []
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.12-13, p.38", "laser literature on Cr:GGG (unread)"]
# Report p.13: 620.0 nm (16,129 cm-1) FWHM 2800, 6.0e-4 cm-1/ppma; 450.0 nm (22,222) FWHM 3300, 6.0e-4. Previously unknown.
[[host.chromophore.band]]
centre_nm = 620.0
fwhm_cm1 = 2800.0
shape = "gaussian_energy"
peak_coeff = 6.0e-4
confidence = "secondary"
source = "Report p.13 (YAG/GGG table, GGG Cr3+ row, 620 nm); directory p.38"
[[host.chromophore.band]]
centre_nm = 450.0
fwhm_cm1 = 3300.0
shape = "gaussian_energy"
peak_coeff = 6.0e-4
confidence = "secondary"
source = "Report p.13 (YAG/GGG table, GGG Cr3+ row, 450 nm)"
[[host.chromophore.fluorescence]]
excitation_nm = [450.0, 620.0]
emission_nm = [730.0]   # broadband NIR, Report p.12-13
emission_fwhm_nm = 80.0   # prior estimate
# quantum_yield unknown
uv = { lw = "none or weak", sw = "none or weak" }   # prior pass
confidence = "secondary"
source = "Report p.13 (Luminescence column, GGG Cr3+ row); width and uv from prior pass"

[[host.chromophore]]
id = "Nd3+"
kind = "ion"
elements = ["Nd"]
site = "Gd dodecahedral (24c)"
conc_unit = "ppm atoms"
conc_typical = [1000, 15000]   # estimate, not in report
conc_max = 20000
color = "lilac"
requires = []
excludes = []
quenched_by = []
confidence = "secondary"
sources = ["Report p.13, p.38"]
# Report p.13: 586.0 nm (17,065 cm-1) FWHM 40, 5.5e-4 cm-1/ppma. Prior: unknown, "assume ~6e-4 like YAG" (report is 8 % lower).
[[host.chromophore.band]]
centre_nm = 586.0
fwhm_cm1 = 40.0
shape = "gaussian_energy"
peak_coeff = 5.5e-4
confidence = "secondary"
source = "Report p.13 (YAG/GGG table, GGG Nd3+ row, 586 nm); directory p.38"
[[host.chromophore.fluorescence]]
emission_nm = [1060.0]   # infrared
confidence = "secondary"
source = "Report p.13 (Luminescence column, GGG Nd3+ row)"
```

---

## Discrepancies with the report

"Prior" is this file before the update. Items 1-9 and 11-13 exceed 10 % or differ in kind; 10 is a no-conflict note. A-I are problems inside the report.

Prior file vs report:

1. **Corundum Cr3+ U band, peak_coeff**: prior 19.0 cm-1/wt% Cr2O3, report 25.9 (+36 %). Both use 1.6e20 Cr/cm3 per wt% Cr2O3, which is half the true value (3.16e20; 0.01 x 3.99 g/cm3 / 151.99 x 2 x 6.022e23). With the report's own sigma_perp = 1.62e-19 cm2 the correct coefficient is **about 51 cm-1/wt%**. That is the more plausible value (0.05 wt% ruby then gives alpha(556, o) about 2.6 cm-1, as expected for a laser-grade rod). Stored: the report's 25.9 (task rule), flagged here.
2. **Corundum Cr3+ Y band, peak_coeff**: prior 14.0, report 19.2 (+37 %); same density slip; density-corrected sigma-based value is about 38.
3. **Own error corrected**: the prior header gave "2.35e22 Al/cm3" for corundum; it is the formula-unit density, Al is 4.70e22 cm-3. Every prior Cr/Fe wt%-to-cm-3 figure for corundum was low by 2.
4. **Meaning of "ppma"**: the prior file uses ppm of the cation-site population. In corundum the report's sigma/alpha pairs imply 1.175e17 cm-3 per ppma = 1e-6 of all atoms (= 2.5 ppm of Al sites). So report coefficients per ppma are 2.5x larger per site-ppm than a site-ppm reading of the same sigma, and converting to the old convention multiplies them by 0.4. Stored in the new unit `ppma (all atoms)` for corundum ions and holes (conc ranges converted). For YAG the same test gives a different answer (Ce: alpha/sigma = 1.0e16 cm-3/ppma, close to Y-site ppm, 1.39e16, not all atoms, 9.2e16), so the report is not consistent between hosts. YAG, GGG and spinel stay on `ppm atoms` (site population); if the report means all atoms there, coefficients are 6.6x (YAG) too large per site-ppm.
5. **Chrysoberyl polarisation**: prior placeholder {a=1, b=1, g=1} (isotropic) vs report {0.65,1.0,0.45}, {0.70,1.0,0.50}, {0.50,1.0,0.30} (Cr3+), {0.8,1.0,0.6} (Fe3+), {0.7,1.0,0.5} (V3+). A biaxial host cannot take equal weights. The report text (E||b > E||a > E||c) fits its table only if alpha=a, beta=b, gamma=c, unconfirmed.
6. **Corundum Mg-hole identity (in kind)**: prior = O- hole next to Mg2+ ("Mg-hole", Mg-limited, excludes Fe2+-Ti4+); report = trapped hole bound to Fe3+, [h.-Fe3+], with [h.-Cr3+] for padparadscha. Absorption data (onset 450 nm, FWHM 5000, pol {1.0,0.7}) agree; assignment differs. Prior coefficient unknown, report 7.0 cm-1/ppma.
7. **Fe2+-Ti4+ geometry (in kind)**: prior "face-sharing along c and edge-sharing", report edge-sharing basal pairs only (p.6, p.30). Pol {1.0,0.1} is the same in both. Face-sharing Ti-Fe pairs along c would add an E||c component, so w_e = 0.1 looks like a geometric simplification rather than a measurement.
8. **Fe2+-Ti4+ and Fe2+-Fe3+ coefficients**: values unchanged from prior (5.0e-5, 1.0e-5 cm-1/ppm^2), but see implausible item A.
9. **Bands the prior file lacked, now added (in kind)**: corundum Cr3+ R1 absorption 694.3 nm (FWHM 15 cm-1, no strength); corundum Ti3+ 540 nm; chrysoberyl Cr3+ R1 680.4 nm, Fe3+ 375 nm edge, V3+ 425 nm; spinel Fe2+ 480 nm and V3+ 415 nm (table only, not in prose); YAG Ce3+ 340 nm strength, Er3+ 380 nm and Ho3+ 640 nm (prose only, no FWHM or strength), Nd3+ 808 nm (prose only; strength from prior recall, `estimate`); GGG Cr3+ strengths.
10. **Spinel Cr3+ quantum yield, corundum Cr3+ and YAG Ce3+ yields**: prior spinel unknown, report 0.90 (p.31); YAG Ce prior 0.9 estimate, report 0.90; ruby unchanged. No conflict.
11. **YAG Ce3+ 460 nm**: prior 0.028 (from sigma 2e-18); report keeps 0.028 but states sigma = 2.8e-18, which gives 0.039 per Y-site ppm (+39 %). Emission: prior 530/560 nm, report 540 nm (now stored).
12. **YAG/GGG numbers equal the prior estimates** (Cr 8.0e-4, Nd 6.0e-4/3.0e-4/2.5e-4, Ce 0.028, spinel Co 0.020/0.015/0.012, Fe2+-Ti4+ 5.0e-5): the report's values coincide exactly with the previous pass's "recalled" estimates, so confidence `secondary` here may overstate independence. Only GGG Nd (5.5e-4 vs assumed 6e-4, -8 %) differs.
13. **YAG Co2+ and V3+ (in kind)**: the report has none. Its Co2+ (590 nm, FWHM 3000, 60 cm-1/wt%), Cr3+ (600/440 nm) and Fe3+ (CT edge 400 nm) rows at the top of p.12 belong to cubic zirconia (8-fold Zr). The prior YAG Co2+ 590 nm (FWHM 1500) is unsupported and may have been contaminated by that coincidence; kept as `estimate`.

Report-internal problems and implausible values:

A. **IVCT: sigma and alpha are not consistent (p.7)**. Fe2+-Ti4+: sigma 1.94e-18 cm2/pair would give 1.7e-6 cm-1/ppma^2 for random nearest-neighbour pairs (Z=3, 8.8e11 pairs per ppma^2), but the table says 5.0e-5 (29x higher). Fe2+-Fe3+: 4.5e-19 -> 1.3e-7 (Z=1) vs 1.0e-5 (76x). The alpha scale itself (about 5 cm-1 for Fe 1000 x Ti 100 ppm, deep blue) is realistic, so either the pair formula is not purely statistical (charge-compensating Fe2+/Ti4+ clustering is well known) or the sigma values are too small. The Fe-Ti alpha equals the old estimate digit for digit.
B. **Cr3+ per-wt% factor 2** (Discrepancies 1-2): the report's alpha does not follow from its own sigma with the correct Cr density.
C. **Mn2+ in spinel (p.10)**: 0.001 cm-1/ppm means epsilon about 17 L/mol/cm for a spin-forbidden tetrahedral Mn2+ band; expected a few at most. Probably high by 5-10x.
D. **Mg/Fe-hole (p.8)**: sigma 6.0e-17 cm2 and 7.0 cm-1/ppma imply alpha(450 nm) about 10 cm-1 at the 1.5 ppma the report calls "golden yellow" (p.31): opaque at 1 cm path. Charge-transfer-like cross sections of 1e-17 are possible, but the combination is very strong. Low confidence.
E. **Ti3+ 490 nm (p.8)**: sigma 4.0e-20 x 1.175e17 = 0.0047 vs listed 0.0050 (+6 %); minor.
F. **Ppma basis** differs between hosts (Discrepancy 4); wt% oxide is never named for chrysoberyl and spinel (oxides assumed).
G. **Polarisation vs optical class**: no mismatch found: uniaxial corundum has (o,e); biaxial chrysoberyl has (a,b,g); cubic spinel, YAG and GGG have a single weight 1.0. Corundum Y-band e = 0.85 is high for a 4A2->4T1 transition (U band 0.75), unverified.
H. **Prose vs table**: spinel Fe2+-Fe3+ clusters (600-750 nm), YAG Er 380 nm, Ho 640 nm and Nd 808 nm appear only in prose; the directory (pp. 33-34) omits corundum Cr3+ R1 and Fe3+ 377 nm that the p.7 table has. Ho emission 2080 nm vs the usual 2090-2100 nm: minor.
I. **Wavenumbers**: all 1e7/lambda conversions in the report for these five hosts check out (rounding only), and all FWHM values are physically sensible for their band types (broad d-d 2600-3500 cm-1, IVCT 3500-4500, f-f 40-100).

## Open questions

1. Verify the Cr3+ cross sections (Krebs & Maisch 1971) and settle the wt%-to-density factor in corundum (51 vs 25.9 cm-1/wt% for the U band).
2. Define the report's ppma basis (all atoms vs cation site) for YAG, GGG and spinel; ask whether the IVCT "ppm^2" is site or all-atom.
3. Resolve the IVCT sigma/alpha mismatch (Discrepancy A); clustering of Fe2+-Ti4+ pairs needs a separate model.
4. Alexandrite: map alpha/beta/gamma to Pbnm a/b/c; check the report's E||b > E||a > E||c ordering (Walling 1980; Farrell & Newnham 1965 Am. Mineral. 50, 1972).
5. Spinel Fe2+-Fe3+ clusters (600-750 nm, dark denim): no centre or strength in the report, not modelled. Fe2+ coefficients per wt% FeO need a source.
6. Padparadscha: build from the "Cr3+" and "Mg-hole" entries with low Fe; decide whether the hole is Mg- or Fe-bound (Discrepancy 6).
7. Still unknown (fields absent): R-line strengths (corundum 694.3, chrysoberyl 680.4); YAG Er 380 / Ho 640 / Nd 808 widths and strengths; all YAG Co2+ and V3+ coefficients; YAG and GGG Cr3+ quantum yield; chrysoberyl, YAG Cr and Mn2+ quantum yields.
8. Corundum Ni, Mn, Co (synthetic Verneuil dopants) are not covered by the report either.
9. Mn2+ in spinel looks high by an order of magnitude (Discrepancy C); Mg/Fe-hole strength (D) needs a check.

## Coverage table

Legend: Y = filled from the report (`secondary`), E = filled but `estimate` (not in report), A = field absent (unknown), - = not applicable.

| Host | Chromophore | Band centre | FWHM | Peak coeff | Pol ratio | Conc range | Fluorescence |
|---|---|---|---|---|---|---|---|
| Corundum | Cr3+ | Y (3 bands) | Y | Y (U, Y); A (R1) | Y | E | Y |
| Corundum | V3+ | Y | Y | Y | Y | E (converted) | - |
| Corundum | Fe3+ | Y | Y | Y | Y | E (converted) | - |
| Corundum | Fe2+-Ti4+ | Y | Y | Y | Y | E | - |
| Corundum | Fe2+-Fe3+ | Y | Y | Y | Y | E | - |
| Corundum | Mg-hole | Y | Y | Y | Y | Y (low end 1.5 ppma) | - |
| Corundum | Ti3+ | Y (2 bands) | Y | Y | Y | E (converted) | Y |
| Chrysoberyl | Cr3+ | Y (3 bands) | Y | Y (585, 420); A (R1) | Y | E | Y (no QY) |
| Chrysoberyl | Fe3+ | Y (2 bands) | Y | Y | Y | E | - |
| Chrysoberyl | V3+ | Y (2 bands) | Y | Y | Y | E | - |
| Spinel | Cr3+ | Y | Y | Y | - | E | Y |
| Spinel | Co2+ | Y | Y | Y | - | E (50 ppm vivid, report) | - |
| Spinel | Fe2+ | Y (2 bands) | Y | Y | - | E | - |
| Spinel | V3+ | Y (2 bands) | Y | Y | - | E | - |
| Spinel | Mn2+ | Y | Y | Y | - | E | Y (no QY) |
| YAG | Cr3+ | Y | Y | Y | - | E | Y (no QY) |
| YAG | Nd3+ | Y (3 + 808 E) | Y (A for 808) | Y (808 E) | - | E | Y |
| YAG | Ce3+ | Y | Y | Y | - | E | Y |
| YAG | Er3+ | Y (3) | Y (A for 380) | Y (A for 380) | - | E | Y (emission only) |
| YAG | Ho3+ | Y (3) | Y (A for 640) | Y (A for 640) | - | E | Y (emission only) |
| YAG | Co2+ | E (not in report) | E | A | - | E | - |
| YAG | V3+ | E (not in report) | E | A | - | E | - |
| GGG | Cr3+ | Y | Y | Y | - | E | Y (no QY) |
| GGG | Nd3+ | Y | Y | Y | - | E | Y (emission only) |
