//! Integration tests for `indicatrix`'s Fresnel/Mueller polarization math,
//! `StandardGemCuts::from_database_angles` angle-parsing/classification, B-Rep
//! reconstruction, and real `.asc`-design end-to-end geometry. Split by topic
//! across the sibling modules below; `fixtures` holds helpers shared across more
//! than one topic module.

mod asc_designs;
mod brep_reconstruction;
mod facet_angle_classification;
mod fixtures;
mod fresnel_mueller;
