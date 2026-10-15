# Fluorescence primary-literature notes (gem UV renderer)

Tag legend: [fetched: source] = read this session from the cited page (abstract/summary level, not the full paper tables unless stated);
[review: ref] = review/secondary; [recall] = from memory of the literature, NOT re-verified this session; [estimate] = my guess.
Low-effort pass (~14 searches/fetches). Several publisher pages were blocked (HAL 403, PMC captcha).

## Ruby (Cr3+ in Al2O3)
- R1/R2 positions: about 14403 and 14443 cm-1 (~694 nm) [review: NIST J. Res. 122, 043 (2017), https://nvlpubs.nist.gov/nistpubs/jres/122/jres.122.043.pdf]. Check: R1 694.3 / R2 692.9 nm RT [recall: Nelson & Sturge 1965; not re-verified].
- Lifetime: "a few ms" [review: NIST 2017 above]; 3.4-4 ms is [recall: Weber; Burns & Nathan 1964].
- Temperature shift of lines: -0.140 +/- 0.007 cm-1/K [review: NIST 2017]. Composition shift ~120 +/- 30 cm-1 per unit Cr mass fraction [review: NIST 2017].
- Excitation bands: Y (4A2->4T1, ~25000 cm-1 = ~400 nm) and U (4A2->4T2, ~18000 cm-1 = ~555 nm) [review: NIST 2017]. So 365 nm sits on the high-energy tail of the Y band plus Fe/charge-transfer/Cr-O tails; 254 nm is in the UV charge-transfer/host edge region. Relative 365 vs 254 efficiency: NOT FOUND; [estimate] 365 nm >> 254 nm for pigment-grade ruby.
- Quantum efficiency: 0.83 +/- 0.10 overall for lightly doped ruby-pigmented coating, ambient [fetched: LBL "Fluorescent cooling of objects exposed to sunlight - the ruby example", Sol. Energy Mater. Sol. Cells 157 (2016), abstract page via heatisland.lbl.gov]. This is a coating-level value (includes escape losses), not an intrinsic crystal yield. Near-unity intrinsic yield [recall: Weber / Burns & Nathan] unverified.
- Cr self-quenching: doping above ~3 wt% Cr2O3 causes concentration quenching [fetched: same LBL abstract]. Basis: wt% Cr2O3.
- Vibronic sideband: shape/peak/extent NOT FOUND this session. [estimate] weak phonon sideband on the low-energy side, extending to ~+/-(300-700) cm-1 from R lines; ruby emission is dominated by R lines at RT.
- Line widths at RT: NOT FOUND (typically ~10-15 cm-1 [recall], unverified).
- Fe quench halving concentration for ruby: NOT FOUND in primary literature.

## Emerald (Cr3+ in beryl)
- R1 ~683.8 (Colombia), 684.0 (Afghanistan), 684.2 nm (Zambia); R1 shifts correlate with Mg and Na (r2 0.83), R2 with Li (r2 0.79) [fetched: GIA Gems & Gemology Fall 2017 "Photoluminescence spectra of emeralds", https://www.gia.edu/gems-gemology/fall-2017-photoluminescence-spectra-emeralds; abstract-level].
- Fe quenching: count rate falls exponentially with Fe; intensity scales ~sqrt(Cr) and drops by 10x at 2200 ppm Fe [fetched: same GIA page]. Basis of "ppm" not stated in the summary I got (likely ppmw from LA-ICP-MS; must check the paper). Implied 1/e constant ~ 2200/ln10 = ~955 ppm; halving = ~660 ppm IF a pure exponential in Fe and ppm = ppmw [estimate derived from the above]. Fe-free, Cr-fixed assumption is mine.
- Also: Brazilian emerald/alexandrite impurity influence on Cr3+ luminescence (Cr, Fe effects) exists (Inst. Polytechnique Paris / HAL hal-01221058) but page was 403; values NOT retrieved.
- Quantum yield, lifetime, excitation 365 vs 254: NOT FOUND.

## Alexandrite / chrysoberyl (Cr3+ in BeAl2O4)
- Two Cr sites: mirror site Rm ~678-679 nm (~70% of Cr, lifetime ~2 ms in the quoted study) and inversion site Ri ~690 nm (long-lived, 25.7-43.9 ms natural; 31.8-52.8 ms synthetic) [fetched: Minerals (MDPI) 2023, 13(3), 419, https://www.mdpi.com/2075-163X/13/3/419]. The oft-quoted ~680.4 nm R line [recall: Walling et al. 1980 IEEE JQE] not re-verified.
- Fe content of natural alexandrite 600-10,000 ppm (basis not given in summary) and linked to decay-profile differences [fetched: same MDPI page]. No halving concentration given.
- Quantum efficiency (laser literature ~1 at low T, thermal depopulation to the 4T2 state at RT) [recall], NOT verified. Excitation 365/254: NOT FOUND.

## Spinel (Cr3+ in MgAl2O4)
- Not retrieved (PMC blocked by captcha). Leads: Data in Brief 2020 PMC7511808 (Cr3+ PL during order-disorder); Strathprints 51204; Latvian ZnAl2O4/MgAl2O4 Cr3+ paper. [recall] R line ~685-687 nm, N-lines 650-720 nm broad lines from inversion disorder; unverified.
- Quantum yield, excitation, Fe quench: NOT FOUND.

## Diamond NV (NV0, NV-)
- ZPLs ~575 nm (2.16 eV) NV0 and ~637 nm (1.95 eV) NV- [fetched: arXiv:1501.03714, Photophysics of single NV centers in nanodiamond].
- Quantum yield, RT, 515 nm CW excitation, 25-50 nm nanodiamonds: NV0 close to 1 at all powers; NV- near 1 at low power falling to ~0.5 at 10 mW [fetched: arXiv:1501.03714]. Radiative lifetimes 19.2 ns (NV0), 21.7 ns (NV-) [fetched: same].
- Counter-claim: yield "significantly smaller than the frequently stated 100%", as low as 5% in nanocrystals; depends on size/shape/medium; bulk radiative rate ~44 MHz vs total ~80 MHz (=> ~0.55 bulk-ish), 532 nm excitation [fetched: arXiv:1712.07877, abstract-level]. Conflicts with the previous item; treat NV- yield as uncertain, 0.5-1.
- Vibronic sideband (NV-: broad to ~800 nm, peak ~680-700 nm; NV0 to ~700 nm): [recall: Zaitsev]; shape parameters NOT retrieved. Excitation 300-420 nm: NOT FOUND (NV is mainly excited 450-600 nm; UV excitation is via weak/indirect absorption [estimate]).
- Fe quenching: not applicable/NOT FOUND.

## Diamond N3 and H3
- ZPL 415.2 nm (N3) and 503.2 nm (H3) [recall: Zaitsev, Optical Properties of Diamond, 2001]; not re-verified.
- Quantum yield: NOT FOUND (a search for H3/N3 yield returned only NV papers and a "delayed luminescence of the H3 centre" abstract, not fetched). [estimate] N3 and H3 yields are high (tens of %) but no primary value held.
- Excitation: N3 absorbs from its ZPL at 415 nm into UV (so 365 nm excites strongly, and 254 nm via the fundamental/N-related absorption); H3 absorbs ~503 nm and below, excited by 365 nm via sideband. Relative 365 vs 254: NOT FOUND.

## Mn2+ (calcite, willemite, spodumene/kunzite)
- Calcite/dolomite: luminescence activation at ~15-30 ppm Mn2+ in synthetic calcite, ~30-35 ppm Mn in dolomite; luminescence intensity set by absolute Mn, no Fe2+ quenching up to 200 ppm Fe; emission 540-675 nm [fetched: Netherlands J. Geosci. article 13195, "Cathodoluminescence activation and zonation in carbonate rocks: an experimental approach", https://njgjournal.nl/index.php/njg/article/view/13195]. Basis "ppm" (likely by mass, in the crystal) not specified in summary. Note these are cathodoluminescence, not UV PL.
- Kutnohorite (CaMn(CO3)2): single band at 630 nm with 8-10 wt% Mn and up to 2400 ppm Fe, self-quenching of Mn incomplete, so Fe did not fully quench [fetched: Mineral. Mag. 71, 493 (rruff MM71_493)].
- Calcite UV PL excitation (Mn2+ 4T1 bands; 254 nm vs 365 nm, with Pb/Ce sensitizers): NOT FOUND. Willemite and kunzite: NOT SEARCHED.
- Quantum yield for Mn2+ in any host: NOT FOUND.
- Fe halving concentration: NOT FOUND as a defined value for Mn2+ (only "no quenching up to 200 ppm Fe" in the synthetic calcite study).

## NOT FOUND list
1. Intrinsic ruby quantum yield (primary) and its temperature dependence.
2. Fe quench halving concentration for ruby, alexandrite, spinel, Mn2+ minerals.
3. Emerald Fe basis (ppmw vs ppma) and original-paper confirmation; emerald, alexandrite, spinel quantum yields.
4. Vibronic sideband peak/extent/width for all centres (only recall).
5. Excitation spectra 300-420 nm and 365:254 ratios for all.
6. N3/H3 quantum yields and primary ZPL widths.
7. Spinel, willemite, kunzite data entirely.
8. RT linewidths of R1/R2 and diamond ZPLs.
Priority follow-ups: GIA Fall 2017 paper tables (Fe basis), Walling 1980, Zaitsev book tables, Machel/Richter Mn-Fe CL papers.
