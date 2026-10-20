use super::{
    BTreeMap, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, Design, FacetSide, FlatRing, GIRDLE_COS_EPS,
    InvalidReason, MIN_WIDTH, ManufacturabilityWarning, SolidStatus, SolvedTier, StoneAnalysis,
    StoneProportions, build_solid_mesh, check_manufacturability, girdle_band_in,
    is_horizontal_angle_deg, mean_height, measure_solid, polygon_area, rings_by_plane,
    solid_facets_in, tier_hinges_in, tier_plane_ranges,
};

/// Solves `design` and measures the stone -- [`analyze_solved`] after [`Design::solve`].
///
/// `with_hinges` also reads the girdle corners (the hinge) of every flat crown or pavilion tier
/// (needed for the ORIGINAL stone only).
///
/// # Errors
///
/// [`InvalidReason::DoesNotSolve`] when the design does not solve, [`InvalidReason::NotClosed`]
/// when its planes do not bound a finite solid.
pub fn analyze(design: &Design, with_hinges: bool) -> Result<StoneAnalysis, InvalidReason> {
    analyze_controlled(
        design,
        with_hinges,
        &indicatrix::geometry::meet_solver::SolveControl::default(),
    )
}

/// Analysis with cancellation checked inside every authoring solve.
///
/// # Errors
///
/// As [`analyze`], plus [`InvalidReason::Preservation`] when `with_hinges` is set and the
/// meet baseline cannot be captured (or the solve was cancelled).
pub fn analyze_controlled(
    design: &Design,
    with_hinges: bool,
    control: &indicatrix::geometry::meet_solver::SolveControl<'_>,
) -> Result<StoneAnalysis, InvalidReason> {
    let solved = design
        .solve_with(control)
        .map_err(|err| InvalidReason::DoesNotSolve(err.to_string()))?;
    analyze_solved_controlled(design, solved, with_hinges, control)
}

/// Measures the stone `solved` describes for `design`.
///
/// # Errors
///
/// [`InvalidReason::NotClosed`] when the planes do not bound a finite solid.
pub fn analyze_solved(
    design: &Design,
    solved: Vec<SolvedTier>,
    with_hinges: bool,
) -> Result<StoneAnalysis, InvalidReason> {
    analyze_solved_controlled(
        design,
        solved,
        with_hinges,
        &indicatrix::geometry::meet_solver::SolveControl::default(),
    )
}

fn analyze_solved_controlled(
    design: &Design,
    solved: Vec<SolvedTier>,
    with_hinges: bool,
    control: &indicatrix::geometry::meet_solver::SolveControl<'_>,
) -> Result<StoneAnalysis, InvalidReason> {
    let preservation = if with_hinges {
        Some(std::sync::Arc::new(
            indicatrix_cut_core::meet_preservation::MeetPreservationBaseline::capture_controlled(
                design, control,
            )
            .map_err(|e| InvalidReason::Preservation(e.to_string()))?,
        ))
    } else {
        None
    };
    let planes = design.planes_from_solved(&solved);
    let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
        return Err(InvalidReason::NotClosed);
    };
    let metrics = measure_solid(&planes).ok_or(InvalidReason::NotClosed)?;
    let proportions = StoneProportions::from_solid(&metrics, &mesh, &planes);
    let facets = solid_facets_in(design, &solved, &planes, &mesh);
    let ranges = tier_plane_ranges(design, &solved);
    let girdle_thinnest_percent = girdle_band_in(design, &ranges, &planes, &mesh)
        .filter(|_| metrics.width_axis > MIN_WIDTH)
        .map(|band| 100.0 * band.min_thickness / metrics.width_axis);
    let rings = rings_by_plane(&mesh);

    let mut flats = Vec::new();
    let mut band: Option<(f64, f64)> = None;
    for (tier_index, (tier, range)) in design.tiers.iter().zip(&ranges).enumerate() {
        if is_horizontal_angle_deg(tier.angle_deg) {
            let ring = range.clone().find_map(|plane| rings.get(&plane));
            if let Some(ring) = ring {
                flats.push(FlatRing {
                    tier_index,
                    side: FacetSide::of_angle_deg(tier.angle_deg),
                    area: polygon_area(ring),
                    height: mean_height(ring),
                });
            }
        } else if tier.angle_deg.abs().to_radians().cos().abs() <= GIRDLE_COS_EPS {
            for plane in range.clone() {
                if let Some(&ring) = rings.get(&plane) {
                    for vertex in ring {
                        band = Some(band.map_or((vertex.y, vertex.y), |(lo, hi)| {
                            (lo.min(vertex.y), hi.max(vertex.y))
                        }));
                    }
                }
            }
        }
    }

    let undersized =
        check_manufacturability(design, &solved, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2)
            .into_iter()
            .filter_map(|warning| match warning {
                ManufacturabilityWarning::UndersizedFacet { tier_index, .. } => Some(tier_index),
                _ => None,
            })
            .collect();
    let hinges = if with_hinges {
        tier_hinges_in(design, &solved, &planes, &mesh)
    } else {
        BTreeMap::new()
    };
    let crown_to_pavilion = match (proportions.crown_height, proportions.pavilion_depth) {
        (Some(crown), Some(pavilion)) if pavilion > MIN_WIDTH => Some(crown / pavilion),
        _ => None,
    };
    let depth_percent = (metrics.width_axis > MIN_WIDTH)
        .then(|| 100.0 * proportions.total_depth / metrics.width_axis);
    Ok(StoneAnalysis {
        preservation,
        authored: Some(std::sync::Arc::new(design.clone())),
        girdle_percent: proportions.girdle_to_width_percent,
        girdle_thinnest_percent,
        table_percent: proportions.table_percent,
        crown_height: proportions.crown_height,
        pavilion_depth: proportions.pavilion_depth,
        crown_to_pavilion,
        depth_percent,
        facets,
        flats,
        girdle_band: band,
        undersized,
        hinges,
        ranges,
        solved,
        planes,
    })
}
