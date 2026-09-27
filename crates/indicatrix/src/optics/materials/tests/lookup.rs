//! [`GemMaterial::by_name`] lookup tests: exact match, longest-substring
//! preference, and full-roster round-tripping.

use crate::optics::materials::GemMaterial;

/// "Zircon" is a substring of "Cubic Zirconia" and is listed earlier in
/// `all_materials()`, so a first-match search silently returned the wrong stone.
#[test]
fn by_name_prefers_the_longest_match_not_the_first() {
    let cz = GemMaterial::by_name("Cubic Zirconia").expect("Cubic Zirconia must resolve");
    assert_eq!(cz.name, "Cubic Zirconia", "must not fall through to Zircon");

    let zircon = GemMaterial::by_name("Zircon").expect("Zircon must resolve");
    assert_eq!(
        zircon.name, "Zircon",
        "an exact name must still resolve to itself"
    );

    assert!(
        (cz.dispersion.evaluate(589.3) - zircon.dispersion.evaluate(589.3)).abs() > 0.1,
        "test premise: the two stones must have clearly different refractive indices"
    );
}
/// Every built-in material must resolve to itself by its own exact name.
#[test]
fn by_name_round_trips_every_builtin_material() {
    for m in GemMaterial::all_materials() {
        let found = GemMaterial::by_name(&m.name)
            .unwrap_or_else(|| panic!("{} must resolve by its own name", m.name));
        assert_eq!(
            found.name, m.name,
            "{} resolved to the wrong material",
            m.name
        );
    }
}
/// The substring fallback is what lets a diagram title carry extra words.
#[test]
fn by_name_still_matches_a_name_embedded_in_a_longer_string() {
    let m = GemMaterial::by_name("Fine Blue Sapphire, Ceylon").expect("should match Sapphire");
    assert_eq!(m.name, "Sapphire");
}
