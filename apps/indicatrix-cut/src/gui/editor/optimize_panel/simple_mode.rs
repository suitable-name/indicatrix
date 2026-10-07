//! The Optimize tab in the Simple interface: which of the settings it hides are not at their
//! defaults.
//!
//! The Simple interface hides "What may change", the angle ranges, the budget, the starts, the
//! seed, the number of candidates and polish. They keep their values and the search still uses them, so
//! the tab says "Some advanced settings are in use. Switch to Advanced to see them." whenever
//! one of them is not what a fresh design starts with. The rule is here, Slint-free, so it is
//! tested without a window.

use indicatrix_editor::optimize_view::{
    DEFAULT_BUDGET, DEFAULT_CANDIDATES, DEFAULT_STARTS, RangeInput, parse_budget,
};

/// A tick box and the state it starts in.
#[derive(Debug, Clone, Copy)]
pub(super) struct Tick {
    /// The box is ticked now.
    pub on: bool,
    /// The box is ticked on a fresh design.
    pub default_on: bool,
}

impl Tick {
    /// The cutter moved the box away from where it starts.
    const fn moved(self) -> bool {
        self.on != self.default_on
    }
}

/// The values of the settings the Simple interface hides.
#[derive(Debug, Clone, Copy)]
pub(super) struct HiddenSettings<'a> {
    /// "Vary anchored tiers". Its starting state depends on the design (on when nothing else
    /// can move), so the caller passes it.
    pub vary_anchored: Tick,
    /// "Keep the girdle".
    pub keep_girdle: Tick,
    /// "Polish".
    pub polish: Tick,
    /// "Only selected tiers".
    pub only_selected: Tick,
    /// An angle range was typed.
    pub ranges_customised: bool,
    /// The budget box, as typed.
    pub budget_text: &'a str,
    /// The seed box, as typed.
    pub seed_text: &'a str,
    /// How many candidates to keep.
    pub candidates: i32,
    /// How many starting arrangements to try.
    pub starts: i32,
}

/// Whether any hidden setting is away from its default. A budget that cannot be read counts
/// as away from it: the run would refuse it, and the cutter cannot see the box.
pub(super) fn settings_in_use(settings: &HiddenSettings<'_>) -> bool {
    settings.vary_anchored.moved()
        || settings.keep_girdle.moved()
        || settings.polish.moved()
        || settings.only_selected.moved()
        || settings.ranges_customised
        || parse_budget(settings.budget_text) != Ok(DEFAULT_BUDGET)
        || !seed_is_default(settings.seed_text)
        || usize::try_from(settings.candidates) != Ok(DEFAULT_CANDIDATES)
        || usize::try_from(settings.starts) != Ok(DEFAULT_STARTS)
}

/// A blank seed box or a zero is the default seed.
fn seed_is_default(text: &str) -> bool {
    let text = text.trim();
    text.is_empty() || text.parse::<u64>() == Ok(0)
}

/// Whether the cutter typed an angle range the search will use: some row of the ranges table
/// has a non-blank minimum or maximum.
pub(super) fn ranges_customised(inputs: &[RangeInput]) -> bool {
    inputs
        .iter()
        .any(|input| !input.min_text.trim().is_empty() || !input.max_text.trim().is_empty())
}

/// The presets the Simple interface lists: every one but the last, "Custom", which needs the
/// weight boxes. The positions match the full list.
pub(super) fn simple_labels<'a>(labels: &[&'a str]) -> Vec<&'a str> {
    labels
        .split_last()
        .map_or_else(Vec::new, |(_custom, rest)| rest.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn tick(on: bool, default_on: bool) -> Tick {
        Tick { on, default_on }
    }

    fn defaults() -> HiddenSettings<'static> {
        HiddenSettings {
            vary_anchored: tick(false, false),
            keep_girdle: tick(true, true),
            polish: tick(true, true),
            only_selected: tick(false, false),
            ranges_customised: false,
            budget_text: "800",
            seed_text: "0",
            candidates: 3,
            starts: 8,
        }
    }

    #[test]
    fn a_fresh_tab_has_nothing_in_use() {
        assert!(!settings_in_use(&defaults()));
    }

    #[test]
    fn the_vary_tick_is_judged_against_the_designs_default() {
        // An imported design starts with the tick on: that is the default, not a choice.
        let imported = HiddenSettings {
            vary_anchored: tick(true, true),
            ..defaults()
        };
        assert!(!settings_in_use(&imported));
        // Unticking it there is a choice, and so is ticking it on a design that has free tiers.
        let unticked = HiddenSettings {
            vary_anchored: tick(false, true),
            ..defaults()
        };
        assert!(settings_in_use(&unticked));
        let ticked = HiddenSettings {
            vary_anchored: tick(true, false),
            ..defaults()
        };
        assert!(settings_in_use(&ticked));
    }

    #[test]
    fn each_other_hidden_setting_counts_when_it_moves() {
        let cases = [
            HiddenSettings {
                keep_girdle: tick(false, true),
                ..defaults()
            },
            HiddenSettings {
                ranges_customised: true,
                ..defaults()
            },
            HiddenSettings {
                budget_text: "400",
                ..defaults()
            },
            HiddenSettings {
                seed_text: "7",
                ..defaults()
            },
            HiddenSettings {
                candidates: 5,
                ..defaults()
            },
            HiddenSettings {
                starts: 3,
                ..defaults()
            },
            HiddenSettings {
                starts: 1,
                ..defaults()
            },
            HiddenSettings {
                polish: tick(false, true),
                ..defaults()
            },
            HiddenSettings {
                only_selected: tick(true, false),
                ..defaults()
            },
        ];
        for case in cases {
            assert!(settings_in_use(&case), "{case:?}");
        }
    }

    #[test]
    fn a_blank_or_calculated_budget_and_a_blank_seed_read_like_the_defaults() {
        let blank_budget = HiddenSettings {
            budget_text: "  ",
            ..defaults()
        };
        assert!(!settings_in_use(&blank_budget));
        let calculated_text = format!("{} * 2", DEFAULT_BUDGET / 2);
        let calculated = HiddenSettings {
            budget_text: &calculated_text,
            ..defaults()
        };
        assert!(!settings_in_use(&calculated));
        let blank_seed = HiddenSettings {
            seed_text: "",
            ..defaults()
        };
        assert!(!settings_in_use(&blank_seed));
        let padded_zero = HiddenSettings {
            seed_text: " 00 ",
            ..defaults()
        };
        assert!(!settings_in_use(&padded_zero));
    }

    #[test]
    fn a_budget_that_cannot_be_read_counts_because_the_box_is_hidden() {
        let broken = HiddenSettings {
            budget_text: "lots",
            ..defaults()
        };
        assert!(settings_in_use(&broken));
        let negative_seed = HiddenSettings {
            seed_text: "-1",
            ..defaults()
        };
        assert!(settings_in_use(&negative_seed));
    }

    #[test]
    fn typed_ranges_count_only_when_something_was_typed() {
        assert!(!ranges_customised(&[]));
        let blank = RangeInput {
            tier: 1,
            min_text: " ".to_string(),
            max_text: String::new(),
        };
        assert!(!ranges_customised(std::slice::from_ref(&blank)));
        let typed = RangeInput {
            tier: 2,
            min_text: "35".to_string(),
            max_text: String::new(),
        };
        assert!(ranges_customised(&[blank, typed]));
    }

    #[test]
    fn the_simple_list_drops_custom_and_keeps_the_positions() {
        let all = ["Balanced", "Windowing", "Custom"];
        assert_eq!(simple_labels(&all), vec!["Balanced", "Windowing"]);
        assert_eq!(simple_labels(&[]).len(), 0);
        assert_eq!(simple_labels(&["Custom"]).len(), 0);
    }
}
