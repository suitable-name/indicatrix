//! Shared real `.asc` fixtures for [`super`]'s topic modules, and
//! [`assert_round_trips`], the round-trip property checker.

use super::super::{parse_asc, to_asc_string};

/// A real file from the database (`pc46019.asc` / `attached_files` id 4210):
/// "For Fun" by Michiko Huyhn, RI 1.54, gear 96, symmetry order 1 with mirror.
pub(super) const REAL_SAMPLE: &str = "GemCad 5.0\r\ng 96 0.0\r\ny 1 y\r\nI 1.54\r\nH PC 46.019  For Fun\r\nH by Michiko Huyhn\r\nH Gemology Online Faceting Design Competition 2013\r\nH Vindictive Entry 3\r\na -44.864054 0.53791082 84 n P1 12 G Cut to mast depth X.\r\na -50.185680 0.48593919 71 n P2 25 G Cut to mast depth X.\r\na -90.000000 0.78956831 36 12 84 n G1 60 n G1 G Set stone size.\r\na 0.000000 0.44755829 96 n U\r\nF Also USFG Newsletter Sep 2013, Facets Jan 2014\r\nF This diagram is in the public domain, and may be reproduced freely with full credit given to the \r\nF author.\r\n";

/// Real corpus tier line (girdle tier literally named "G"): `parse_tier`'s token
/// loop must honor a pending `expect_name` from the preceding "n" marker before
/// checking for the "G" notes marker, or the name token "G" is misread
/// as the start of the notes tail and every index after it is swallowed into
/// `notes` instead. 2,296 of 5,759 corpus files hit this because "G" is a very
/// common girdle-tier name.
pub(super) const ASC_GIRDLE_NAMED_G: &str = "GemCad 5.0\n\
g 96 0.0\n\
y 1 y\n\
I 1.54\n\
H Test\n\
a -90.000000 0.78956831 36 12 84 n G 60 n G G Set stone size.\n";

/// Same "G"-named-tier corpus quirk, but with no "G <notes>" tail at all -- every
/// index after the name token must still parse as an index, not get swallowed as
/// (empty) notes text.
pub(super) const ASC_GIRDLE_NAMED_G_NO_NOTES: &str = "GemCad 5.0\n\
g 96 0.0\n\
y 1 y\n\
I 1.54\n\
H Test\n\
a -90.000000 1.02 4 n G 12 20 28 36 44 52 60 68 76 84 92\n";

/// `attached_files` id 4208 ("pc45149.asc") -- "PC 45.149 Round Trichecker-12" by
/// Fred W. Van Sant.
pub(super) const ASC_ROUND_TRICHECKER_12: &str = "GemCad 5.0\n\
g 96 0.0\n\
y 6 y\n\
I 1.72\n\
H PC 45.149  Round Trichecker-12\n\
H by Fred W. Van Sant, X 51, Extra Designs 2000\n\
H Released into the public domain in memory of Charles L. Moon\n\
a -41.000000 0.64991234 92 n 1 84 76 68 60 52 44 36 28 20 12 4\n\
a -90.000000 1.07325092 92 n 2 84 76 68 60 52 44 36 28 20 12 4\n\
a 29.730000 0.65249790 4 n A 12 20 28 36 44 52 60 68 76 84 92\n\
a 25.000000 0.59508784 96 n B 16 32 48 64 80\n\
a 10.000000 0.48799664 96 n C 16 32 48 64 80\n\
F \"For small stones\"\n";

/// `attached_files` id 4210 ("pc46019.asc") -- "PC 46.019 For Fun" by Michiko
/// Huyhn. Exercises an unsigned zero-angle culet-like tier ("U") with no explicit
/// crown/pavilion marker, and several tiers with repeated `n <name>` groups.
pub(super) const ASC_FOR_FUN: &str = "GemCad 5.0\n\
g 96 0.0\n\
y 1 y\n\
I 1.54\n\
H PC 46.019  For Fun\n\
H by Michiko Huyhn\n\
a -44.864054 0.53791082 84 n P1 12 G Cut to mast depth X.\n\
a -90.000000 0.78956831 36 12 84 n G1 60 n G1 G Set stone size.\n\
a 54.575729 0.70935195 12 n C1 84 G Set girdle width.\n\
a 0.000000 0.44755829 96 n U\n\
F Also USFG Newsletter Sep 2013, Facets Jan 2014\n";

/// `attached_files` id 4430 ("pc42060.asc") -- "PC 42.060 Large Texas Star" by
/// Charles `McCoy`. Gear=80 (not the far more common 96), symmetry order 5, plus
/// an explicit table tier at unsigned zero.
pub(super) const ASC_LARGE_TEXAS_STAR: &str = "GemCad 5.0\n\
g 80 0.0\n\
y 5 y\n\
I 1.61\n\
H PC 42.060  Large Texas Star\n\
H by Charles McCoy\n\
a -40.000000 0.54589773 76 n 1 68 60 52 44 36 28 20 12 4 G TCP\n\
a 40.000000 1.11585176 4 n A 12 20 28 36 44 52 60 68 76 G Establish girdle thickness\n\
a 0.000000 0.72641642 80 n T G Make table large enough to show all of the star\n\
F Leave #4 frosted\n";

/// `attached_files` id 4422 ("pc43001a.asc") -- "PC 43.001A Shah (Replica)". No
/// facet names anywhere (exercises the "no name at all" path), a rare
/// negative-mast tier at an unsigned zero angle, and a fractional index (`1.7`).
pub(super) const ASC_SHAH_REPLICA_NO_NAMES: &str = "GemCad 4.51\n\
g 64 64.0\n\
y 1 n\n\
I 1.54\n\
H PC 43.001A Shah (Replica)\n\
a -90.00 1.00000 16\n\
a -90.00 0.44700 0 32\n\
a 0.00 -0.36800 0\n\
a 1.87 0.34210 49 47\n\
a 24.47 0.38860 1.7\n\
F Does not agree with Barbour's 43.001. Glass replica has rounded facets on the ends.\n";

pub(super) fn assert_round_trips(content: &str) {
    let original = parse_asc(content).expect("real sample must parse");
    let serialized = to_asc_string(&original);
    let reparsed = parse_asc(&serialized).unwrap_or_else(|e| {
        panic!("serialized output must itself parse: {e}\n--- serialized ---\n{serialized}")
    });
    assert_eq!(
        original, reparsed,
        "round trip must reproduce an equal AscSchedule\n--- serialized ---\n{serialized}"
    );
}
