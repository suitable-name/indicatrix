//! The named rig profiles kept in the settings file: lookup, replace-by-name and removal.
//!
//! The list lives in `SettingsFile::rig_profiles` and is written through the settings
//! persister; these are the plain operations the windows apply to it.

use indicatrix_cut_core::rough_plan::locate::RigProfile;

/// The names of `profiles`, in order.
#[must_use]
pub fn names(profiles: &[RigProfile]) -> Vec<String> {
    profiles
        .iter()
        .map(|profile| profile.name.clone())
        .collect()
}

/// The index of the profile called `name`.
#[must_use]
pub fn position(profiles: &[RigProfile], name: &str) -> Option<usize> {
    profiles.iter().position(|profile| profile.name == name)
}

/// Stores `profile`: replaces the profile of the same name, or appends it. Returns its index.
pub fn upsert(profiles: &mut Vec<RigProfile>, profile: RigProfile) -> usize {
    if let Some(index) = position(profiles, &profile.name) {
        profiles[index] = profile;
        return index;
    }
    profiles.push(profile);
    profiles.len() - 1
}

/// Removes the profile called `name`; whether there was one.
pub fn remove(profiles: &mut Vec<RigProfile>, name: &str) -> bool {
    position(profiles, name).is_some_and(|index| {
        profiles.remove(index);
        true
    })
}

/// A name for a new rig that no stored rig has: "Rig 1", "Rig 2", ...
#[must_use]
pub fn fresh_name(profiles: &[RigProfile]) -> String {
    // With n stored rigs, one of "Rig 1" to "Rig n+1" is free.
    (1..=profiles.len() + 1)
        .map(|number| format!("Rig {number}"))
        .find(|name| position(profiles, name).is_none())
        .unwrap_or_else(|| "Rig".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::rough_plan::locate::Projection;

    fn rig(name: &str, stone_n: f64) -> RigProfile {
        RigProfile::new(
            name,
            RigProfile::side_layout(
                150.0,
                30.0,
                Projection::Pinhole { focal_px: 4000.0 },
                [4000, 3000],
            ),
            stone_n,
        )
    }

    #[test]
    fn a_profile_of_the_same_name_is_replaced_not_duplicated() {
        let mut rigs = Vec::new();
        assert_eq!(upsert(&mut rigs, rig("Bench", 1.54)), 0);
        assert_eq!(upsert(&mut rigs, rig("Macro", 1.76)), 1);
        assert_eq!(upsert(&mut rigs, rig("Bench", 1.6)), 0);
        assert_eq!(rigs.len(), 2);
        assert_eq!(rigs[0].stone_n, 1.6);
        assert_eq!(names(&rigs), ["Bench", "Macro"]);
    }

    #[test]
    fn removing_finds_by_name() {
        let mut rigs = vec![rig("A", 1.5), rig("B", 1.5)];
        assert!(remove(&mut rigs, "A"));
        assert!(!remove(&mut rigs, "A"));
        assert_eq!(names(&rigs), ["B"]);
        assert_eq!(position(&rigs, "B"), Some(0));
    }

    #[test]
    fn a_fresh_name_skips_the_taken_ones() {
        let mut rigs = Vec::new();
        assert_eq!(fresh_name(&rigs), "Rig 1");
        rigs.push(rig("Rig 1", 1.5));
        rigs.push(rig("Rig 3", 1.5));
        assert_eq!(fresh_name(&rigs), "Rig 2");
    }
}
