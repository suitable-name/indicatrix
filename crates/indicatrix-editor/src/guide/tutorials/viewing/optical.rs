//! Camera-aligned optical diagnostics; manual checkpoints keep interpretation with the learner.
use crate::guide::{Group, Guide, GuideCategory, GuideStep, StartingState};
pub(super) fn guide() -> Guide {
    const ALLOWED: &[Group] = &[
        Group::History,
        Group::FileOps,
        Group::ViewTabs,
        Group::Advanced,
    ];
    Guide::new(
        "viewing-optical-diagnostics",
        "Optical Diagnostics at a Viewing Angle",
        "Inspect analytical leakage, extinction, light return and dispersion potential without changing the beauty render.",
        GuideCategory::Viewing,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Open Live Render",
            "Open Live Render and keep Path-traced mode selected.",
        )
        .actions(["Choose Live Render above the picture."])
        .allow(ALLOWED),
    )
    .step(
        GuideStep::new(
            "Enable Optical Diagnostics",
            "Open Optical Diagnostics beside the picture and enable the master switch.",
        )
        .actions([
            "Select Enable Optical Diagnostics. Wait for a matching Draft or Ready map.",
            "Cyan diagonal lines show Windowing: a pavilion exit, possibly after reflection.",
        ])
        .check(
            "The separate colored layer follows the current picture. The persistent legend says analytical light paths.",
        )
        .allow(ALLOWED),
    )
    .step(
        GuideStep::new(
            "Choose an Effect",
            "Switch from Windowing to Extinction, then try Brilliance and Fire individually.",
        )
        .actions([
            "Clear Windowing and select Extinction. Magenta crosses mark paths that did not qualify as returned.",
            "Brilliance means an illuminated upward return outside the model observer shadow. Fire is dispersion potential in weighted degrees, and can coexist with Brilliance.",
        ])
        .why(
            "Visible-area shares count hit samples. The normal HUD uses a different disc/aperture ray fan. Neither is a percentage of rendered radiance.",
        )
        .allow(ALLOWED),
    )
    .step(
        GuideStep::new(
            "Orbit and Inspect a Region",
            "Close the panel, orbit the stone, then reopen the panel and hover a stone region.",
        )
        .actions([
            "The old layer disappears during movement. Wait for the map for the new pose.",
            "Read the centre-ray entry surface, exit surface, reflection count and reason. Mixed subpixel surfaces are labelled.",
        ])
        .why(
            "Budget exhaustion is an estimate, not proof of permanent darkness. Advanced material transport is not reproduced.",
        )
        .allow(ALLOWED),
    )
    .step(
        GuideStep::new(
            "Export With Its Legend",
            "Open Export and explicitly include Optical Diagnostics.",
        )
        .actions([
            "Choose Include Optical Diagnostics (4 Samples/Pixel); the export then adds a companion legend. Set the desired output size and samples, then Start Export.",
            "The PNG uses a separate four-sample diagnostic pass at the output dimensions. Its optical.txt companion records pose, material, lighting, selection, fixed Fire scale and limitations.",
            "Tilt videos have their own opt-in and a model caption. Ordinary exports remain clean when the checkbox is clear.",
        ])
        .allow(ALLOWED),
    )
}

#[cfg(test)]
mod tests {
    use super::guide;

    /// The tutorial is written in American English ("color"), and its step titles are the
    /// ones the tutorial-video script keys by position.
    #[test]
    fn the_guide_text_says_color_and_keeps_its_five_steps() {
        let guide = guide();
        let mut texts = vec![guide.title.clone(), guide.summary.clone()];
        for step in &guide.steps {
            texts.push(step.title.clone());
            texts.push(step.intro.clone());
            texts.extend(step.actions.iter().cloned());
            texts.push(step.check.clone());
            texts.push(step.why.clone());
        }
        for text in &texts {
            assert!(
                !text.to_lowercase().contains("colour"),
                "British spelling in the optical guide: {text}"
            );
        }
        let titles: Vec<&str> = guide.steps.iter().map(|step| step.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Open Live Render",
                "Enable Optical Diagnostics",
                "Choose an Effect",
                "Orbit and Inspect a Region",
                "Export With Its Legend",
            ]
        );
    }
}
