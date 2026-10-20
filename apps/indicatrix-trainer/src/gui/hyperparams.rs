//! The hyperparameter form: parsing and validating what the user typed.
//!
//! The GUI trains by imitation only, so the form has six fields. The worker's command still
//! carries the PPO clip and GAE lambda (they are validated there); the form passes the values
//! the worker reported last, which an imitation run ignores.

use crate::trainer_worker::{TrainerCommand, runner::MAX_WORKERS};

/// PPO clip kept when no valid snapshot value is known.
const DEFAULT_CLIP: f32 = 0.2;
/// GAE lambda kept when no valid snapshot value is known.
const DEFAULT_LAMBDA: f32 = 0.95;

/// Raw text of the hyperparameter form.
pub struct HyperInput {
    pub workers: String,
    pub rollout_size: String,
    pub mini_batch: String,
    pub epochs: String,
    pub updates_per_round: String,
    pub learning_rate: String,
}

/// Largest "Updates per Round" the form accepts.
const MAX_UPDATES_PER_ROUND: usize = 100_000;

/// The PPO-only values the command carries unchanged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Retained {
    pub clip_epsilon: f32,
    pub gae_lambda: f32,
}

impl Default for Retained {
    fn default() -> Self {
        Self {
            clip_epsilon: DEFAULT_CLIP,
            gae_lambda: DEFAULT_LAMBDA,
        }
    }
}

impl Retained {
    /// The worker's values when they are valid, else the defaults.
    pub fn from_worker(clip_epsilon: f32, gae_lambda: f32) -> Self {
        if clip_epsilon > 0.0 && clip_epsilon < 1.0 && (0.0..=1.0).contains(&gae_lambda) {
            Self {
                clip_epsilon,
                gae_lambda,
            }
        } else {
            Self::default()
        }
    }
}

/// Parses a whole number of at least `min` (and at most `max`, when given).
fn parse_count(name: &str, text: &str, min: usize, max: Option<usize>) -> Result<usize, String> {
    let value: usize = text
        .trim()
        .parse()
        .map_err(|_| format!("{name}: '{}' is not a whole number", text.trim()))?;
    if value < min {
        return Err(format!("{name} must be at least {min}"));
    }
    if let Some(max) = max.filter(|&m| value > m) {
        return Err(format!("{name} must be at most {max}"));
    }
    Ok(value)
}

/// Parses a finite decimal number.
fn parse_decimal(name: &str, text: &str) -> Result<f32, String> {
    let value: f32 = text
        .trim()
        .parse()
        .map_err(|_| format!("{name}: '{}' is not a number", text.trim()))?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(format!("{name} must be a finite number"))
    }
}

/// Checks the form field by field and builds the command; the error names the first bad field.
/// The worker validates again as a whole.
pub fn parse_hyperparams(input: &HyperInput, retained: Retained) -> Result<TrainerCommand, String> {
    let workers = parse_count("Workers", &input.workers, 1, Some(MAX_WORKERS))?;
    let rollout_size = parse_count("Rollout size", &input.rollout_size, 1, None)?;
    let mini_batch = parse_count("Mini-batch", &input.mini_batch, 1, None)?;
    let ppo_epochs = parse_count("Epochs", &input.epochs, 1, None)?;
    let updates_per_round = parse_count(
        "Updates per Round",
        &input.updates_per_round,
        0,
        Some(MAX_UPDATES_PER_ROUND),
    )?;
    let learning_rate = parse_decimal("Learning rate", &input.learning_rate)?;
    if learning_rate <= 0.0 {
        return Err("Learning rate must be greater than 0".into());
    }
    Ok(TrainerCommand::SetHyperparams {
        workers,
        rollout_size,
        mini_batch,
        ppo_epochs,
        updates_per_round,
        learning_rate,
        clip_epsilon: retained.clip_epsilon,
        gae_lambda: retained.gae_lambda,
    })
}

/// The learning rate as the form shows it (`0.0003`).
pub fn format_learning_rate(rate: f32) -> String {
    format!("{rate}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> HyperInput {
        HyperInput {
            workers: "8".into(),
            rollout_size: "128".into(),
            mini_batch: "64".into(),
            epochs: "4".into(),
            updates_per_round: "64".into(),
            learning_rate: "0.0003".into(),
        }
    }

    #[test]
    fn valid_form_builds_the_command() {
        let Ok(TrainerCommand::SetHyperparams {
            workers,
            ppo_epochs,
            updates_per_round,
            clip_epsilon,
            gae_lambda,
            ..
        }) = parse_hyperparams(&input(), Retained::default())
        else {
            panic!("a valid form must parse");
        };
        assert_eq!(workers, 8);
        assert_eq!(ppo_epochs, 4);
        assert_eq!(updates_per_round, 64);
        assert!((clip_epsilon - DEFAULT_CLIP).abs() < 1e-9);
        assert!((gae_lambda - DEFAULT_LAMBDA).abs() < 1e-9);
    }

    #[test]
    fn each_bad_field_is_named() {
        let retained = Retained::default();
        let err = |edit: &dyn Fn(&mut HyperInput)| {
            let mut bad = input();
            edit(&mut bad);
            parse_hyperparams(&bad, retained).unwrap_err()
        };
        assert!(err(&|i| i.workers = "0".into()).contains("Workers"));
        assert!(err(&|i| i.workers = (MAX_WORKERS + 1).to_string()).contains("at most"));
        assert!(err(&|i| i.rollout_size = "0".into()).contains("Rollout size"));
        assert!(err(&|i| i.mini_batch = "x".into()).contains("Mini-batch"));
        assert!(err(&|i| i.epochs = "0".into()).contains("Epochs"));
        assert!(err(&|i| i.updates_per_round = "x".into()).contains("Updates per Round"));
        assert!(err(&|i| i.updates_per_round = "-1".into()).contains("Updates per Round"));
        assert!(err(&|i| i.updates_per_round = "100001".into()).contains("at most 100000"));
        assert!(err(&|i| i.learning_rate = "abc".into()).contains("Learning rate"));
        assert!(err(&|i| i.learning_rate = "0".into()).contains("greater than 0"));
        assert!(err(&|i| i.learning_rate = "-1".into()).contains("greater than 0"));
        assert!(err(&|i| i.learning_rate = "NaN".into()).contains("finite"));
        assert!(err(&|i| i.learning_rate = "inf".into()).contains("finite"));
    }

    #[test]
    fn updates_per_round_accepts_zero_up_to_the_limit() {
        for (text, expected) in [("0", 0), (" 1 ", 1), ("100000", 100_000)] {
            let mut form = input();
            form.updates_per_round = text.into();
            let Ok(TrainerCommand::SetHyperparams {
                updates_per_round, ..
            }) = parse_hyperparams(&form, Retained::default())
            else {
                panic!("'{text}' must parse");
            };
            assert_eq!(updates_per_round, expected);
        }
    }

    #[test]
    fn whitespace_around_numbers_is_accepted() {
        let mut padded = input();
        padded.workers = "  3 ".into();
        assert!(parse_hyperparams(&padded, Retained::default()).is_ok());
    }

    #[test]
    fn retained_values_follow_the_worker_when_valid() {
        let kept = Retained::from_worker(0.3, 0.9);
        assert!((kept.clip_epsilon - 0.3).abs() < 1e-9);
        assert_eq!(Retained::from_worker(0.0, 0.9), Retained::default());
        assert_eq!(Retained::from_worker(0.3, 1.5), Retained::default());
        assert_eq!(Retained::from_worker(1.0, 0.5), Retained::default());
    }

    #[test]
    fn learning_rate_formats_without_exponent() {
        assert_eq!(format_learning_rate(0.0003), "0.0003");
        assert_eq!(format_learning_rate(1.0), "1");
    }
}
