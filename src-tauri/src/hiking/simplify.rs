/// Ramer–Douglas–Peucker polyline simplification for GPS tracks.
///
/// Points are (lon, lat) in degrees. Longitude is scaled by cos(mean latitude)
/// so the tolerance is isotropic in metres-ish terms; `epsilon_deg` is the
/// perpendicular tolerance in latitude degrees (1e-5 deg ≈ 1.1 m).
pub fn simplify_track(points: &[(f64, f64)], epsilon_deg: f64) -> Vec<(f64, f64)> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mean_lat = points.iter().map(|p| p.1).sum::<f64>() / points.len() as f64;
    let lon_scale = mean_lat.to_radians().cos().max(0.01);
    let scaled: Vec<(f64, f64)> = points.iter().map(|p| (p.0 * lon_scale, p.1)).collect();

    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    rdp(&scaled, 0, points.len() - 1, epsilon_deg, &mut keep);

    points
        .iter()
        .zip(keep.iter())
        .filter(|(_, k)| **k)
        .map(|(p, _)| *p)
        .collect()
}

fn rdp(pts: &[(f64, f64)], first: usize, last: usize, eps: f64, keep: &mut [bool]) {
    if last <= first + 1 {
        return;
    }
    let (mut max_d, mut idx) = (0.0_f64, first);
    for i in (first + 1)..last {
        let d = perp_dist(pts[i], pts[first], pts[last]);
        if d > max_d {
            max_d = d;
            idx = i;
        }
    }
    if max_d > eps {
        keep[idx] = true;
        rdp(pts, first, idx, eps, keep);
        rdp(pts, idx, last, eps, keep);
    }
}

fn perp_dist(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len_sq = dx * dx + dy * dy;
    if len_sq == 0.0 {
        return ((p.0 - a.0).powi(2) + (p.1 - a.1).powi(2)).sqrt();
    }
    // distance from p to the infinite line through a-b (segments share endpoints
    // in RDP recursion, so clamping to the segment is unnecessary here)
    ((dy * p.0 - dx * p.1 + b.0 * a.1 - b.1 * a.0).abs()) / len_sq.sqrt()
}

/// Round to 5 decimal places (~1.1 m) so JSON payloads stay compact.
pub fn round5(v: f64) -> f64 {
    (v * 1e5).round() / 1e5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_endpoints_and_corners() {
        // L-shaped track with redundant collinear points
        let pts = vec![
            (8.0, 47.0),
            (8.0, 47.001),
            (8.0, 47.002),
            (8.0, 47.003),
            (8.001, 47.003),
            (8.002, 47.003),
        ];
        let out = simplify_track(&pts, 0.0001);
        assert_eq!(out.first(), Some(&(8.0, 47.0)));
        assert_eq!(out.last(), Some(&(8.002, 47.003)));
        // the corner must survive
        assert!(out.contains(&(8.0, 47.003)), "corner dropped: {out:?}");
        assert!(out.len() <= 4, "too many points kept: {out:?}");
    }

    #[test]
    fn short_tracks_untouched() {
        let pts = vec![(8.0, 47.0), (8.1, 47.1)];
        assert_eq!(simplify_track(&pts, 0.001), pts);
    }

    #[test]
    fn straight_line_collapses_to_two_points() {
        let pts: Vec<(f64, f64)> = (0..100).map(|i| (8.0 + i as f64 * 1e-4, 47.0)).collect();
        assert_eq!(simplify_track(&pts, 1e-5).len(), 2);
    }

    #[test]
    fn zigzag_survives_small_epsilon() {
        let pts: Vec<(f64, f64)> = (0..20)
            .map(|i| (8.0 + i as f64 * 1e-3, 47.0 + if i % 2 == 0 { 0.0 } else { 1e-3 }))
            .collect();
        let out = simplify_track(&pts, 1e-5);
        assert_eq!(out.len(), pts.len(), "zigzag should keep every vertex");
    }

    #[test]
    fn round5_rounds() {
        assert_eq!(round5(8.123456789), 8.12346);
        assert_eq!(round5(-47.000004), -47.0);
    }
}
