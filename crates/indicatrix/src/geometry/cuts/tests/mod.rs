//! Test topics for the `cuts` module: [`azimuth`] covers
//! [`super::StandardGemCuts::index_to_azimuth`] and its use in
//! [`super::StandardGemCuts::from_asc_schedule`]; [`dedup`] covers the
//! `.asc`-reconstruction plane dedup; [`zero_angle_side`] covers which side a
//! table or culet lands on.

mod azimuth;
mod dedup;
mod zero_angle_side;
