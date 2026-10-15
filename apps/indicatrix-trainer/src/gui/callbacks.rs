//! Registers a handler for every callback of the Slint `TrainingController` contract.

use std::rc::Rc;

use slint::ComponentHandle;

use super::Gui;
use crate::{AppWindow, TrainingController};

impl Gui {
    /// Wires every callback of the controller global to the matching action.
    pub(super) fn wire(self: &Rc<Self>, window: &AppWindow) {
        let c = window.global::<TrainingController>();
        self.wire_run(&c);
        self.wire_model(&c);
        self.wire_data(&c);
        self.wire_files(&c);
        self.wire_misc(&c);
        self.wire_updates(&c);
    }

    fn wire_run(self: &Rc<Self>, c: &TrainingController<'_>) {
        let g = self.clone();
        c.on_start_training(move || g.start());
        let g = self.clone();
        c.on_pause_training(move || g.pause());
        let g = self.clone();
        c.on_step_training(move || g.step());
        let g = self.clone();
        c.on_reset_training(move || g.reset());
        let g = self.clone();
        c.on_restart_worker(move || g.restart_worker());
        let g = self.clone();
        c.on_toggle_device(move || g.toggle_device());
    }

    fn wire_model(self: &Rc<Self>, c: &TrainingController<'_>) {
        let g = self.clone();
        c.on_select_model_mode(move |mode| g.select_mode(mode));
        let g = self.clone();
        c.on_select_model_tier(move |tier| g.select_tier(tier.as_str()));
        let g = self.clone();
        c.on_select_target_material(move |name| g.select_material(name.as_str()));
        let g = self.clone();
        c.on_toggle_spectrum_mode(move || g.toggle_spectrum());
        let g = self.clone();
        c.on_begin_edit_hyperparams(move || g.begin_edit_hyperparams());
        let g = self.clone();
        c.on_apply_hyperparams(move || g.apply_hyperparams());
        let g = self.clone();
        c.on_cancel_edit_hyperparams(move || g.cancel_edit_hyperparams());
    }

    fn wire_data(self: &Rc<Self>, c: &TrainingController<'_>) {
        let g = self.clone();
        c.on_pick_database(move || g.pick_database());
        let g = self.clone();
        c.on_filter_designs(move |text| g.filter_designs(text.as_str()));
        let g = self.clone();
        c.on_select_design(move |index| g.select_design(index));
        let g = self.clone();
        c.on_next_design(move || g.step_design(true));
        let g = self.clone();
        c.on_prev_design(move || g.step_design(false));
        let g = self.clone();
        c.on_toggle_corpus_mode(move || g.toggle_corpus());
    }

    fn wire_files(self: &Rc<Self>, c: &TrainingController<'_>) {
        let g = self.clone();
        c.on_save_weights(move || g.save_weights(false));
        let g = self.clone();
        c.on_load_weights(move || g.load_weights());
        let g = self.clone();
        c.on_import_run(move || g.import_run());
        let g = self.clone();
        c.on_export_run(move || g.export_run());
        let g = self.clone();
        c.on_return_to_live(move || g.return_to_live());
    }

    fn wire_updates(self: &Rc<Self>, c: &TrainingController<'_>) {
        let g = self.clone();
        c.on_answer_update_prompt(move |enabled| g.answer_update_prompt(enabled));
        let g = self.clone();
        c.on_set_update_enabled(move |enabled| g.set_update_enabled(enabled));
        let g = self.clone();
        c.on_check_updates_now(move || g.check_updates_now());
        let g = self.clone();
        c.on_open_release(move || g.open_release());
        let g = self.clone();
        c.on_skip_release(move || g.skip_release());
        let g = self.clone();
        c.on_dismiss_release(move || g.dismiss_release());
    }

    fn wire_misc(self: &Rc<Self>, c: &TrainingController<'_>) {
        let g = self.clone();
        c.on_dismiss_notice(move || g.dismiss_notice());
        let g = self.clone();
        c.on_set_log_filter(move |filter| g.set_log_filter(filter));
        let g = self.clone();
        c.on_copy_logs(move || g.copy_logs());
        let g = self.clone();
        c.on_clear_logs(move || g.clear_logs());
        let g = self.clone();
        c.on_close_save(move || g.close_save());
        let g = self.clone();
        c.on_close_discard(move || g.close_discard());
        let g = self.clone();
        c.on_close_cancel(move || g.close_cancel());
    }
}
