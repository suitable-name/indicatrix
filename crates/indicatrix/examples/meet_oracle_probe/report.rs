//! Console-report formatting helpers used by `main`: a distribution summary
//! (mean/median/p90/max), a rank histogram, and small percentage helpers.

/// Prints a percentile summary of the finite values.
pub fn print_dist(label: &str, vals: &[f64]) {
    let mut v: Vec<f64> = vals.iter().copied().filter(|x| x.is_finite()).collect();
    let inf = vals.len() - v.len();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if v.is_empty() {
        println!("{label}: no data");
        return;
    }
    let at = |p: f64| v[((v.len() as f64 * p) as usize).min(v.len() - 1)];
    println!(
        "{label}: n={} (+{inf} non-finite) p10={:.4} p25={:.4} med={:.4} p75={:.4} p90={:.4}",
        v.len(),
        at(0.10),
        at(0.25),
        at(0.50),
        at(0.75),
        at(0.90)
    );
}

/// Prints a histogram of rank values.
pub fn print_rank_hist(label: &str, ranks: &[usize]) {
    let n = ranks.len();
    if n == 0 {
        println!("{label}: no data");
        return;
    }
    let count = |pred: &dyn Fn(usize) -> bool| ranks.iter().filter(|&&r| pred(r)).count();
    let buckets: [(&str, Box<dyn Fn(usize) -> bool>); 7] = [
        ("0", Box::new(|r| r == 0)),
        ("1", Box::new(|r| r == 1)),
        ("2", Box::new(|r| r == 2)),
        ("3", Box::new(|r| r == 3)),
        ("4", Box::new(|r| r == 4)),
        ("5", Box::new(|r| r == 5)),
        (">5", Box::new(|r| r > 5)),
    ];
    print!("{label} (n={n}): ");
    for (name, pred) in &buckets {
        let c = count(pred.as_ref());
        print!("{name}:{c}({:.1}%) ", pct(c, n));
    }
    println!();
}

/// Fraction of values strictly below the threshold.
pub fn frac_below(vals: &[f64], thresh: f64) -> f64 {
    if vals.is_empty() {
        return 0.0;
    }
    vals.iter().filter(|&&v| v < thresh).count() as f64 / vals.len() as f64
}

/// Percentage of `n` in `total`, zero when `total` is zero.
pub fn pct(n: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        100.0 * n as f64 / total as f64
    }
}
