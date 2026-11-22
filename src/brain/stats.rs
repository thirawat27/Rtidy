//! Small statistics: disk forecast, growth anomaly, quiet hours.

/// Exponentially-weighted least-squares line through (t_days, y).
/// Recent points matter more (half-life `half_life` days). Returns slope per day.
pub fn weighted_slope(points: &[(f64, f64)], half_life: f64) -> Option<f64> {
    if points.len() < 4 {
        return None;
    }
    let t_max = points.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let t_min = points.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    if t_max - t_min < 2.0 {
        return None; // need at least two days of history
    }
    let (mut sw, mut st, mut sy) = (0.0, 0.0, 0.0);
    let ws: Vec<f64> = points.iter().map(|p| 0.5f64.powf((t_max - p.0) / half_life)).collect();
    for (p, w) in points.iter().zip(&ws) {
        sw += w;
        st += w * p.0;
        sy += w * p.1;
    }
    let (mt, my) = (st / sw, sy / sw);
    let (mut num, mut den) = (0.0, 0.0);
    for (p, w) in points.iter().zip(&ws) {
        num += w * (p.0 - mt) * (p.1 - my);
        den += w * (p.0 - mt).powi(2);
    }
    (den > 0.0).then(|| num / den)
}

/// Days until free space hits zero, if it is shrinking.
pub fn days_until_full(free_now: f64, slope_per_day: f64) -> Option<f64> {
    (slope_per_day < 0.0).then(|| free_now / -slope_per_day)
}

/// `select_nth_unstable_by` only partitions around the index it needs, so this is
/// linear instead of the O(n log n) of a full sort. `total_cmp` is a total order on
/// floats, so unlike `partial_cmp().unwrap()` it cannot panic on a NaN.
fn median(v: &mut [f64]) -> f64 {
    let n = v.len();
    let (lo, mid, _) = v.select_nth_unstable_by(n / 2, f64::total_cmp);
    if n % 2 == 1 {
        return *mid;
    }
    // Even length: pair it with the largest value below it.
    let below = lo.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    f64::midpoint(below, *mid)
}

/// Robust anomaly test (median + MAD): is `latest` far above the usual values?
/// Returns (is_anomaly, typical value).
pub fn is_spike(history: &[f64], latest: f64, min_abs: f64) -> (bool, f64) {
    if history.len() < 5 {
        return (false, 0.0);
    }
    let mut h = history.to_vec();
    let med = median(&mut h);
    let mut dev: Vec<f64> = history.iter().map(|x| (x - med).abs()).collect();
    let mad = median(&mut dev).max(1.0);
    (latest > med + 6.0 * mad && latest > min_abs, med)
}

/// Hour of day (0..24) with the least build activity, smoothed over neighbours.
/// None until there's enough evidence.
pub fn quietest_hour(hours: &[u32], min_events: usize) -> Option<u32> {
    if hours.len() < min_events {
        return None;
    }
    let mut hist = [0f64; 24];
    for &h in hours {
        hist[(h % 24) as usize] += 1.0;
    }
    (0..24u32).min_by(|&a, &b| {
        let s = |h: u32| hist[((h + 23) % 24) as usize] + 2.0 * hist[h as usize] + hist[((h + 1) % 24) as usize];
        s(a).total_cmp(&s(b))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slope_and_forecast() {
        // 100 GB free, losing 2 GB/day
        let pts: Vec<(f64, f64)> = (0..10).map(|d| (d as f64, 100.0 - 2.0 * d as f64)).collect();
        let s = weighted_slope(&pts, 7.0).unwrap();
        assert!((s + 2.0).abs() < 1e-6);
        assert!((days_until_full(82.0, s).unwrap() - 41.0).abs() < 1e-6);
        assert!(days_until_full(82.0, 1.0).is_none());
        assert!(weighted_slope(&pts[..3], 7.0).is_none());
    }

    #[test]
    fn median_odd_and_even() {
        // Odd length: the middle value. Even: the mean of the middle pair.
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&mut [4.0, 1.0, 3.0, 2.0]), 2.5);
        assert_eq!(median(&mut [1.0]), 1.0);
        assert_eq!(median(&mut [5.0, 5.0]), 5.0);
        // Order of the input must not matter.
        assert_eq!(median(&mut [9.0, 0.0, 7.0, 1.0, 8.0]), 7.0);
        assert_eq!(median(&mut [0.0, 1.0, 7.0, 8.0, 9.0]), 7.0);
    }

    #[test]
    fn spike() {
        let normal = [1.0e8, 1.2e8, 0.9e8, 1.1e8, 1.0e8, 1.3e8];
        assert!(is_spike(&normal, 5.0e9, 1.0e9).0);
        assert!(!is_spike(&normal, 1.4e8, 1.0e9).0);
    }

    #[test]
    fn quiet_hour() {
        // builds 9..18 every day, never at night
        let hours: Vec<u32> = (0..20).flat_map(|_| 9..18).collect();
        let q = quietest_hour(&hours, 50).unwrap();
        assert!(!(8..=18).contains(&q), "{q}");
        assert!(quietest_hour(&hours[..10], 50).is_none());
    }
}
