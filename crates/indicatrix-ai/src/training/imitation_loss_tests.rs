use super::*;

#[test]
fn sensitivity_mse_is_the_masked_mean_over_labelled_entries() {
    let mut hat_values = vec![1.0; 2 * N_COMPONENTS];
    hat_values[N_COMPONENTS + 2] = 7.0;
    let mut target_values = vec![0.0; 2 * N_COMPONENTS];
    target_values[0] = 0.5;
    target_values[N_COMPONENTS + 2] = 4.0;
    let mut mask_values = vec![0.0; 2 * N_COMPONENTS];
    mask_values[0] = 1.0;
    mask_values[N_COMPONENTS + 2] = 1.0;
    let hat = cube(hat_values, [1, 2, N_COMPONENTS]);
    let target = cube(target_values, [1, 2, N_COMPONENTS]);
    let mask = cube(mask_values, [1, 2, N_COMPONENTS]);
    let mse = host_scalar(masked_sens_mse(hat.clone(), target.clone(), mask)).unwrap();
    assert!((mse - 4.625).abs() < 1e-6, "{mse}");

    let none = masked_sens_mse(
        hat,
        target,
        cube(vec![0.0; 2 * N_COMPONENTS], [1, 2, N_COMPONENTS]),
    );
    assert_eq!(host_scalar(none).unwrap(), 0.0);
}

#[test]
fn cosine_averages_over_the_samples_that_have_a_direction() {
    let hat = matrix(vec![1.0, 0.0, 99.0, 5.0, 5.0, 5.0, 2.0, -2.0, 0.0], 3, 3);
    let target = matrix(vec![1.0, 1.0, 7.0, 1.0, 2.0, 3.0, 1.0, -1.0, 0.0], 3, 3);
    let mask = matrix(vec![1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0], 3, 3);
    let (cosine, loss) = masked_cosine(hat, target, mask);
    let want = f32::midpoint(FRAC_1_SQRT_2, 1.0);
    let (cosine, loss) = (host_scalar(cosine).unwrap(), host_scalar(loss).unwrap());
    assert!((cosine - want).abs() < 1e-5, "{cosine} vs {want}");
    assert!((loss - (1.0 - want)).abs() < 1e-5, "{loss}");
}

#[test]
fn cosine_of_opposite_directions_is_minus_one_and_an_empty_mask_is_exactly_zero() {
    let target = matrix(vec![1.0, 2.0], 1, 2);
    let opposite = matrix(vec![-1.0, -2.0], 1, 2);
    let both = matrix(vec![1.0; 2], 1, 2);
    let (cosine, loss) = masked_cosine(opposite.clone(), target.clone(), both);
    assert!((host_scalar(cosine).unwrap() + 1.0).abs() < 1e-5);
    assert!((host_scalar(loss).unwrap() - 2.0).abs() < 1e-5);
    let (cosine, loss) = masked_cosine(opposite, target, matrix(vec![0.0; 2], 1, 2));
    assert_eq!(host_scalar(cosine).unwrap(), 0.0);
    assert_eq!(host_scalar(loss).unwrap(), 0.0);
}

#[test]
fn huber_is_quadratic_below_the_knee_linear_above_and_masked() {
    let hat = matrix(vec![0.5, 3.0, 10.0, 7.0, -1.5], 5, 1);
    let target = matrix(vec![0.0, 0.5, 2.0, 0.0, 0.0], 5, 1);
    let mask = matrix(vec![1.0, 1.0, 0.0, 1.0, 1.0], 5, 1);
    let loss = host_scalar(masked_huber(hat, target, mask)).unwrap();
    assert!((loss - 2.406_25).abs() < 1e-5, "{loss}");
    let one = |v: f32| matrix(vec![v], 1, 1);
    let knee = masked_huber(one(1.0), one(0.0), one(1.0));
    assert!((host_scalar(knee).unwrap() - 0.5).abs() < 1e-6);
    let none = masked_huber(one(4.0), one(0.0), one(0.0));
    assert_eq!(host_scalar(none).unwrap(), 0.0);
}
