//! Small statistics: disk forecast, growth anomaly, quiet hours.

/// Exponentially-weighted least-squares line through (t_days, y).
/// Recent points matter more (half-life `half_life` days). Returns slope per day.
pub fn weighted_slope(points: &[(f64, f64)], half_life: f64) -> Option<f64> {
    if points.len() < 4 {
        return None;
    }
    let t_max = points.iter().map(|p| p.0).fold(f64::MIN, f64::max);
    let t_min = points.iter().map(|p| p.0).fold(f64::MAX, f64::min);
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

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
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
        s(a).partial_cmp(&s(b)).unwrap()
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
