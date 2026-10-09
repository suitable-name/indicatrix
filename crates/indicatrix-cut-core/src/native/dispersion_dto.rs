//! A custom material's dispersion curve, converted between `indicatrix`'s
//! [`DispersionModel`] and the plain-number [`DispersionModelDto`] the native design file
//! (and the desktop's material database, as JSON) stores.
//!
//! The format crate has no dependency on `indicatrix`, so the conversion lives here with the
//! other half of the native-file story. A table that does not describe a model the renderer
//! can use ([`DispersionModel::validate`]) reads as `None`, and the caller falls back to the
//! refractive index and `n_F - n_C` pair every custom material also carries.

use indicatrix::optics::dispersion::DispersionModel;
use indicatrix_formats::native::DispersionModelDto;

/// `value` as an `f64` through its shortest round-trip decimal, so a file reads `1.0396122`
/// rather than `1.0396122051239014` (the same rule `CustomMaterialSnapshot::with_body_color`
/// follows). The decimal text identifies the `f32` uniquely, so [`dispersion_model_from_dto`]
/// gets the identical bits back.
fn widen(value: f32) -> f64 {
    value
        .to_string()
        .parse::<f64>()
        .unwrap_or_else(|_| f64::from(value))
}

/// The file form of `model`.
#[must_use]
pub fn dispersion_model_dto(model: &DispersionModel) -> DispersionModelDto {
    match *model {
        DispersionModel::Sellmeier1 { b1, c1 } => DispersionModelDto::Sellmeier1 {
            b1: widen(b1),
            c1: widen(c1),
        },
        DispersionModel::Sellmeier3 { b, c } => DispersionModelDto::Sellmeier3 {
            b: b.map(widen),
            c: c.map(widen),
        },
        DispersionModel::Cauchy { a, b, c } => DispersionModelDto::Cauchy {
            a: widen(a),
            b: widen(b),
            c: widen(c),
        },
    }
}

/// The model `dto` describes, or `None` when it is not one the renderer can use.
///
/// (A
/// coefficient that is not finite, a resonance inside the visible band, an index at or
/// below 1: see [`DispersionModel::validate`]).
#[must_use]
pub fn dispersion_model_from_dto(dto: &DispersionModelDto) -> Option<DispersionModel> {
    let model = match *dto {
        DispersionModelDto::Sellmeier1 { b1, c1 } => DispersionModel::Sellmeier1 {
            b1: b1 as f32,
            c1: c1 as f32,
        },
        DispersionModelDto::Sellmeier3 { b, c } => DispersionModel::Sellmeier3 {
            b: b.map(|v| v as f32),
            c: c.map(|v| v as f32),
        },
        DispersionModelDto::Cauchy { a, b, c } => DispersionModel::Cauchy {
            a: a as f32,
            b: b as f32,
            c: c as f32,
        },
    };
    model.validate().is_ok().then_some(model)
}

/// `model` as the JSON text the desktop's material database stores.
///
/// That is the `dispersion_model_json` column of a custom material: the same table the design
/// file holds, for example `{"kind":"cauchy","a":1.7,"b":0.006,"c":0.0}`.
#[must_use]
pub fn dispersion_model_to_json(model: &DispersionModel) -> String {
    // A plain enum of numbers and arrays always serialises; the empty string would read back
    // as `None`, the plain path, if that ever stopped being true.
    serde_json::to_string(&dispersion_model_dto(model)).unwrap_or_default()
}

/// The file form of the model stored as `json`.
///
/// That is what a design file carries when the database row holds a model. `None` for blank
/// text, text that is not a dispersion table, or a table [`dispersion_model_from_dto`]
/// refuses.
#[must_use]
pub fn dispersion_dto_from_json(json: &str) -> Option<DispersionModelDto> {
    let trimmed = json.trim();
    if trimmed.is_empty() {
        return None;
    }
    let dto = serde_json::from_str::<DispersionModelDto>(trimmed).ok()?;
    dispersion_model_from_dto(&dto).map(|_| dto)
}

/// The model stored as `json`, or `None` in every case [`dispersion_dto_from_json`] is.
#[must_use]
pub fn dispersion_model_from_json(json: &str) -> Option<DispersionModel> {
    dispersion_dto_from_json(json).and_then(|dto| dispersion_model_from_dto(&dto))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models() -> [DispersionModel; 3] {
        [
            DispersionModel::Sellmeier1 {
                b1: 1.039_612_2,
                c1: 0.006_000_699,
            },
            DispersionModel::Sellmeier3 {
                b: [1.039_612_2, 0.231_792_35, 1.010_469_4],
                c: [0.006_000_699, 0.020_017_914, 103.560_65],
            },
            DispersionModel::Cauchy {
                a: 1.7,
                b: 0.006,
                c: -0.000_01,
            },
        ]
    }

    /// The file form and back gives the identical `f32` bits for every kind, and the file
    /// holds the short decimal, not the widened `f32` digits.
    #[test]
    fn the_file_form_round_trips_every_kind_bit_for_bit() {
        for model in models() {
            let dto = dispersion_model_dto(&model);
            assert_eq!(dispersion_model_from_dto(&dto), Some(model), "{model:?}");
        }
        assert_eq!(
            dispersion_model_dto(&DispersionModel::Cauchy {
                a: 1.7,
                b: 0.006,
                c: 0.0
            }),
            DispersionModelDto::Cauchy {
                a: 1.7,
                b: 0.006,
                c: 0.0
            }
        );
    }

    /// The JSON form round-trips too, and is the same table the file holds.
    #[test]
    fn the_json_form_round_trips_every_kind() {
        for model in models() {
            let json = dispersion_model_to_json(&model);
            assert!(json.contains("\"kind\""), "{json}");
            assert_eq!(dispersion_model_from_json(&json), Some(model), "{json}");
            assert_eq!(
                dispersion_dto_from_json(&json),
                Some(dispersion_model_dto(&model))
            );
        }
        assert_eq!(
            dispersion_model_to_json(&DispersionModel::Cauchy {
                a: 1.7,
                b: 0.006,
                c: 0.0
            }),
            r#"{"kind":"cauchy","a":1.7,"b":0.006,"c":0.0}"#
        );
    }

    /// Blank, foreign and unusable JSON all read as "no model", never a half-built one.
    #[test]
    fn unusable_json_reads_as_no_model() {
        for json in [
            "",
            "   ",
            "not json",
            "{}",
            r#"{"kind":"tabulated"}"#,
            r#"{"kind":"cauchy","a":1.7}"#,
            r#"{"kind":"sellmeier1","b1":1.0,"c1":0.36}"#,
            r#"{"kind":"cauchy","a":0.5,"b":0.0,"c":0.0}"#,
        ] {
            assert_eq!(dispersion_model_from_json(json), None, "{json}");
            assert_eq!(dispersion_dto_from_json(json), None, "{json}");
        }
    }

    /// A file table that fails validation (a resonance at 600 nm) does not become a model.
    #[test]
    fn an_invalid_table_is_refused() {
        let dto = DispersionModelDto::Sellmeier1 { b1: 1.0, c1: 0.36 };
        assert_eq!(dispersion_model_from_dto(&dto), None);
    }
}
