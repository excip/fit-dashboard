use chrono::NaiveDate;
use crate::hiking::baseline::Baselines;

/// Thresholds for episode detection. Not persisted; built-in defaults.
#[derive(Debug, Clone)]
pub struct EpisodeConfig {
    pub min_run_days: i64,   // 3
    pub merge_gap_days: i64, // 1 (single okay/missing day inside a run)
    pub dip_rhr_over: f64,   // 5.0 (bpm over baseline)
    pub dip_hrv_factor: f64, // 0.90 (at/below baseline * factor)
    pub ok_rhr_over: f64,    // 2.0  (recovery tolerance, same as baseline.rs)
    pub ok_hrv_factor: f64,  // 0.95
    pub load_quantile: f64,  // 0.75 (top quartile of the trip's hiking days)
    pub max_per_kind: usize, // 3
}

impl Default for EpisodeConfig {
    fn default() -> Self {
        Self {
            min_run_days: 3,
            merge_gap_days: 1,
            dip_rhr_over: 5.0,
            dip_hrv_factor: 0.90,
            ok_rhr_over: 2.0,
            ok_hrv_factor: 0.95,
            load_quantile: 0.75,
            max_per_kind: 3,
        }
    }
}

/// One trip day, joined by the caller from RecoveryDay + member activities.
pub struct DayRow {
    pub date: NaiveDate,
    pub km: f64,
    pub gain_m: f64,
    pub resting_hr: Option<f64>,
    pub hrv: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum EpisodeKind {
    StrainDip,
    StrongStretch,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Episode {
    pub kind: EpisodeKind,
    pub start_date: NaiveDate,
    pub end_date: NaiveDate,
    pub start_trip_day: i64,
    pub end_trip_day: i64,
    /// Per-day values inside the episode, for explicit date:value prompt rendering.
    /// StrainDip: the strained metric per day; StrongStretch: km per day.
    pub daily: Vec<(NaiveDate, f64)>,
    pub peak_rhr: Option<f64>,
    pub min_hrv: Option<f64>,
    pub avg_km: f64,
    pub max_gain_m: f64,
    pub severity: f64,
}

/// Linear-interpolation quantile (numpy "type 7") over an ascending-sorted slice.
fn quantile(sorted: &[f64], q: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    if sorted.len() == 1 {
        return Some(sorted[0]);
    }
    let pos = q * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let frac = pos - lo as f64;
    if lo + 1 < sorted.len() {
        Some(sorted[lo] + frac * (sorted[lo + 1] - sorted[lo]))
    } else {
        Some(sorted[lo])
    }
}

/// Maximal runs of consecutive `flag` days, merging runs separated by
/// <= merge_gap_days non-flag days, then keeping runs whose calendar span
/// (start..=end) is >= min_len days. Returns inclusive (start_idx, end_idx).
fn runs(days: &[DayRow], flags: &[bool], merge_gap: i64, min_len: i64) -> Vec<(usize, usize)> {
    let mut raw: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < flags.len() {
        if flags[i] {
            let s = i;
            while i + 1 < flags.len() && flags[i + 1] {
                i += 1;
            }
            raw.push((s, i));
        }
        i += 1;
    }
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for r in raw {
        if let Some(last) = merged.last_mut() {
            let gap = r.0 as i64 - last.1 as i64 - 1;
            if gap <= merge_gap {
                last.1 = r.1;
                continue;
            }
        }
        merged.push(r);
    }
    merged
        .into_iter()
        .filter(|&(s, e)| (days[e].date - days[s].date).num_days() + 1 >= min_len)
        .collect()
}

pub fn detect_episodes(days: &[DayRow], baselines: &Baselines, cfg: &EpisodeConfig) -> Vec<Episode> {
    let rhr_b = baselines.resting_hr;
    let hrv_b = baselines.hrv_last_night_avg;
    if rhr_b.is_none() && hrv_b.is_none() {
        return Vec::new();
    }

    let trip_day = |i: usize| -> i64 { (days[i].date - days[0].date).num_days() + 1 };

    // --- Strain dips -------------------------------------------------------
    // Membership = strained beyond the recovery tolerance (strict). A run only
    // qualifies if it also reaches the dip threshold on at least one day.
    let dip_member = |d: &DayRow| -> bool {
        let rhr_hit = matches!((rhr_b, d.resting_hr), (Some(b), Some(v)) if v > b + cfg.ok_rhr_over);
        let hrv_hit = matches!((hrv_b, d.hrv), (Some(b), Some(v)) if v < b * cfg.ok_hrv_factor);
        rhr_hit || hrv_hit
    };
    let dip_core = |d: &DayRow| -> bool {
        let rhr_hit = matches!((rhr_b, d.resting_hr), (Some(b), Some(v)) if v >= b + cfg.dip_rhr_over);
        let hrv_hit = matches!((hrv_b, d.hrv), (Some(b), Some(v)) if v <= b * cfg.dip_hrv_factor);
        rhr_hit || hrv_hit
    };

    let dip_flags: Vec<bool> = days.iter().map(&dip_member).collect();
    let mut dips: Vec<Episode> = Vec::new();
    for (s, e) in runs(days, &dip_flags, cfg.merge_gap_days, cfg.min_run_days) {
        let span = &days[s..=e];
        if !span.iter().any(&dip_core) {
            continue;
        }
        // Which metric drove this dip -> rendered per-day in `daily`.
        let rhr_drove = rhr_b.is_some()
            && span.iter().any(|d| matches!((rhr_b, d.resting_hr), (Some(b), Some(v)) if v >= b + cfg.dip_rhr_over));
        let hrv_drove = hrv_b.is_some()
            && span.iter().any(|d| matches!((hrv_b, d.hrv), (Some(b), Some(v)) if v <= b * cfg.dip_hrv_factor));
        let use_rhr = if rhr_drove {
            true
        } else if hrv_drove {
            false
        } else {
            rhr_b.is_some()
        };
        let daily: Vec<(NaiveDate, f64)> = span
            .iter()
            .filter_map(|d| {
                let v = if use_rhr { d.resting_hr } else { d.hrv };
                v.map(|v| (d.date, v))
            })
            .collect();
        let mut severity = 0.0f64;
        for d in span {
            if let (Some(b), Some(v)) = (rhr_b, d.resting_hr) {
                let t = (v - b) / cfg.dip_rhr_over;
                if t > severity {
                    severity = t;
                }
            }
            if let (Some(b), Some(v)) = (hrv_b, d.hrv) {
                let t = ((b - v) / b) / (1.0 - cfg.dip_hrv_factor);
                if t > severity {
                    severity = t;
                }
            }
        }
        dips.push(Episode {
            kind: EpisodeKind::StrainDip,
            start_date: days[s].date,
            end_date: days[e].date,
            start_trip_day: trip_day(s),
            end_trip_day: trip_day(e),
            daily,
            peak_rhr: span.iter().filter_map(|d| d.resting_hr).reduce(f64::max),
            min_hrv: span.iter().filter_map(|d| d.hrv).reduce(f64::min),
            avg_km: span.iter().map(|d| d.km).sum::<f64>() / span.len() as f64,
            max_gain_m: span.iter().map(|d| d.gain_m).fold(0.0, f64::max),
            severity,
        });
    }
    dips.sort_by(|a, b| b.severity.partial_cmp(&a.severity).unwrap());
    dips.truncate(cfg.max_per_kind);

    // --- Strong stretches --------------------------------------------------
    // Load quantiles are taken over the trip's hiking days (km > 0).
    let mut kms: Vec<f64> = days.iter().filter(|d| d.km > 0.0).map(|d| d.km).collect();
    let mut gains: Vec<f64> = days.iter().filter(|d| d.km > 0.0).map(|d| d.gain_m).collect();
    kms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    gains.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let km_q = quantile(&kms, cfg.load_quantile);
    let gain_q = quantile(&gains, cfg.load_quantile);

    let stretch_member = |d: &DayRow| -> bool {
        if d.km <= 0.0 {
            return false;
        }
        let load_top =
            km_q.map_or(false, |q| d.km >= q) || gain_q.map_or(false, |q| d.gain_m >= q);
        if !load_top {
            return false;
        }
        let mut any = false;
        if let Some(b) = rhr_b {
            if let Some(v) = d.resting_hr {
                any = true;
                if !(v < b + cfg.ok_rhr_over) {
                    return false;
                }
            }
        }
        if let Some(b) = hrv_b {
            if let Some(v) = d.hrv {
                any = true;
                if !(v > b * cfg.ok_hrv_factor) {
                    return false;
                }
            }
        }
        any
    };

    let stretch_flags: Vec<bool> = days.iter().map(&stretch_member).collect();
    let mut stretches: Vec<Episode> = Vec::new();
    // No gap-merging for stretches: a broken day (elevated strain) genuinely
    // ends a strong stretch, unlike a single okay recovery day inside a dip.
    for (s, e) in runs(days, &stretch_flags, 0, cfg.min_run_days) {
        let span = &days[s..=e];
        stretches.push(Episode {
            kind: EpisodeKind::StrongStretch,
            start_date: days[s].date,
            end_date: days[e].date,
            start_trip_day: trip_day(s),
            end_trip_day: trip_day(e),
            daily: span.iter().map(|d| (d.date, d.km)).collect(),
            peak_rhr: span.iter().filter_map(|d| d.resting_hr).reduce(f64::max),
            min_hrv: span.iter().filter_map(|d| d.hrv).reduce(f64::min),
            avg_km: span.iter().map(|d| d.km).sum::<f64>() / span.len() as f64,
            max_gain_m: span.iter().map(|d| d.gain_m).fold(0.0, f64::max),
            severity: (days[e].date - days[s].date).num_days() as f64 + 1.0,
        });
    }
    stretches.sort_by(|a, b| {
        b.severity
            .partial_cmp(&a.severity)
            .unwrap()
            .then(b.avg_km.partial_cmp(&a.avg_km).unwrap())
    });
    stretches.truncate(cfg.max_per_kind);

    dips.into_iter().chain(stretches).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn row(date: &str, km: f64, gain: f64, rhr: Option<f64>, hrv: Option<f64>) -> DayRow {
        DayRow { date: d(date), km, gain_m: gain, resting_hr: rhr, hrv }
    }

    /// Baselines fixture: rhr = 48, hrv = 60.
    fn base() -> Baselines {
        Baselines {
            resting_hr: Some(48.0),
            hrv_last_night_avg: Some(60.0),
            ..Default::default()
        }
    }

    #[test]
    fn dip_four_day_rhr_run() {
        let days = vec![
            row("2024-06-01", 0.0, 0.0, Some(48.0), None),
            row("2024-06-02", 0.0, 0.0, Some(55.0), None),
            row("2024-06-03", 0.0, 0.0, Some(55.0), None),
            row("2024-06-04", 0.0, 0.0, Some(55.0), None),
            row("2024-06-05", 0.0, 0.0, Some(55.0), None),
            row("2024-06-06", 0.0, 0.0, Some(48.0), None),
        ];
        let eps = detect_episodes(&days, &base(), &EpisodeConfig::default());
        assert_eq!(eps.len(), 1);
        let e = &eps[0];
        assert_eq!(e.kind, EpisodeKind::StrainDip);
        assert_eq!(e.start_trip_day, 2);
        assert_eq!(e.end_trip_day, 5);
        assert_eq!(e.start_date, d("2024-06-02"));
        assert_eq!(e.end_date, d("2024-06-05"));
        assert_eq!(e.daily.len(), 4);
        assert!(e.daily.iter().all(|&(_, v)| v == 55.0));
        assert_eq!(e.peak_rhr, Some(55.0));
        assert_eq!(e.min_hrv, None);
    }

    #[test]
    fn dip_merged_across_single_okay_day() {
        let days = vec![
            row("2024-06-01", 0.0, 0.0, Some(48.0), None),
            row("2024-06-02", 0.0, 0.0, Some(55.0), None),
            row("2024-06-03", 0.0, 0.0, Some(55.0), None),
            row("2024-06-04", 0.0, 0.0, Some(49.0), None), // okay gap day
            row("2024-06-05", 0.0, 0.0, Some(55.0), None),
            row("2024-06-06", 0.0, 0.0, Some(55.0), None),
            row("2024-06-07", 0.0, 0.0, Some(48.0), None),
        ];
        let eps = detect_episodes(&days, &base(), &EpisodeConfig::default());
        assert_eq!(eps.len(), 1);
        let e = &eps[0];
        assert_eq!(e.start_date, d("2024-06-02"));
        assert_eq!(e.end_date, d("2024-06-06"));
        assert_eq!(e.daily.len(), 5);
        // the merged-gap day stays in `daily` with its value
        assert!(e.daily.contains(&(d("2024-06-04"), 49.0)));
    }

    #[test]
    fn dip_two_days_rejected() {
        let days = vec![
            row("2024-06-01", 0.0, 0.0, Some(48.0), None),
            row("2024-06-02", 0.0, 0.0, Some(55.0), None),
            row("2024-06-03", 0.0, 0.0, Some(55.0), None),
            row("2024-06-04", 0.0, 0.0, Some(48.0), None),
        ];
        let eps = detect_episodes(&days, &base(), &EpisodeConfig::default());
        assert_eq!(eps.len(), 0);
    }

    #[test]
    fn dip_hrv_only_when_rhr_baseline_missing() {
        let b = Baselines { hrv_last_night_avg: Some(60.0), ..Default::default() };
        let days = vec![
            row("2024-06-01", 0.0, 0.0, None, Some(60.0)),
            row("2024-06-02", 0.0, 0.0, None, Some(52.0)),
            row("2024-06-03", 0.0, 0.0, None, Some(50.0)),
            row("2024-06-04", 0.0, 0.0, None, Some(53.0)),
            row("2024-06-05", 0.0, 0.0, None, Some(60.0)),
        ];
        let eps = detect_episodes(&days, &b, &EpisodeConfig::default());
        assert_eq!(eps.len(), 1);
        let e = &eps[0];
        assert_eq!(e.kind, EpisodeKind::StrainDip);
        assert_eq!(e.daily, vec![
            (d("2024-06-02"), 52.0),
            (d("2024-06-03"), 50.0),
            (d("2024-06-04"), 53.0),
        ]);
        assert_eq!(e.min_hrv, Some(50.0));
        assert_eq!(e.peak_rhr, None);
    }

    #[test]
    fn stretch_top_quartile_within_tolerance() {
        let days = vec![
            row("2024-06-01", 10.0, 100.0, Some(48.0), None),
            row("2024-06-02", 10.0, 100.0, Some(48.0), None),
            row("2024-06-03", 30.0, 500.0, Some(47.0), None),
            row("2024-06-04", 30.0, 500.0, Some(48.0), None),
            row("2024-06-05", 30.0, 500.0, Some(49.0), None),
            row("2024-06-06", 10.0, 100.0, Some(48.0), None),
            row("2024-06-07", 10.0, 100.0, Some(48.0), None),
            row("2024-06-08", 10.0, 100.0, Some(48.0), None),
        ];
        let eps = detect_episodes(&days, &base(), &EpisodeConfig::default());
        assert_eq!(eps.len(), 1);
        let e = &eps[0];
        assert_eq!(e.kind, EpisodeKind::StrongStretch);
        assert_eq!(e.start_trip_day, 3);
        assert_eq!(e.end_trip_day, 5);
        assert_eq!(e.avg_km, 30.0);
        assert_eq!(e.max_gain_m, 500.0);
        assert_eq!(e.daily.len(), 3);
    }

    #[test]
    fn stretch_broken_by_high_rhr_day() {
        let days = vec![
            row("2024-06-01", 10.0, 100.0, Some(48.0), None),
            row("2024-06-02", 10.0, 100.0, Some(48.0), None),
            row("2024-06-03", 30.0, 500.0, Some(47.0), None),
            row("2024-06-04", 30.0, 500.0, Some(51.0), None), // > 48 + 2
            row("2024-06-05", 30.0, 500.0, Some(49.0), None),
            row("2024-06-06", 10.0, 100.0, Some(48.0), None),
            row("2024-06-07", 10.0, 100.0, Some(48.0), None),
            row("2024-06-08", 10.0, 100.0, Some(48.0), None),
        ];
        let eps = detect_episodes(&days, &base(), &EpisodeConfig::default());
        assert_eq!(eps.len(), 0);
    }

    #[test]
    fn no_baselines_no_episodes() {
        let days = vec![
            row("2024-06-01", 30.0, 500.0, Some(90.0), Some(10.0)),
            row("2024-06-02", 30.0, 500.0, Some(90.0), Some(10.0)),
            row("2024-06-03", 30.0, 500.0, Some(90.0), Some(10.0)),
            row("2024-06-04", 30.0, 500.0, Some(90.0), Some(10.0)),
        ];
        let eps = detect_episodes(&days, &Baselines::default(), &EpisodeConfig::default());
        assert_eq!(eps.len(), 0);
    }

    #[test]
    fn five_disjoint_dips_keep_top_three() {
        // five 3-day dips, separated by 2 okay days so they never merge,
        // with increasing peak RHR (53, 55, 57, 59, 61).
        let mut days: Vec<DayRow> = Vec::new();
        let mut day = 1;
        let mut push = |days: &mut Vec<DayRow>, day: &mut i32, rhr: f64| {
            days.push(row(&format!("2024-06-{:02}", *day), 0.0, 0.0, Some(rhr), None));
            *day += 1;
        };
        for peak in [53.0, 55.0, 57.0, 59.0, 61.0] {
            for _ in 0..3 {
                push(&mut days, &mut day, peak);
            }
            for _ in 0..2 {
                push(&mut days, &mut day, 48.0);
            }
        }
        let eps = detect_episodes(&days, &base(), &EpisodeConfig::default());
        assert_eq!(eps.len(), 3);
        assert!(eps.iter().all(|e| e.kind == EpisodeKind::StrainDip));
        // sorted by severity desc -> peaks 61, 59, 57
        assert_eq!(eps[0].peak_rhr, Some(61.0));
        let peaks: Vec<f64> = eps.iter().map(|e| e.peak_rhr.unwrap()).collect();
        assert_eq!(peaks, vec![61.0, 59.0, 57.0]);
        assert!(!peaks.contains(&53.0));
        assert!(!peaks.contains(&55.0));
    }

    #[test]
    fn pct_shape_regression_jul_2022() {
        // Real PCT Jul 1-16 2022. baseline rhr = 48, no HRV.
        // Gains: Jul 4-8 are the high-gain stretch (1800), everything else 500.
        let rhr = [50.0, 52.0, 47.0, 47.0, 48.0, 48.0, 48.0, 46.0, 50.0, 50.0, 52.0, 51.0, 56.0, 54.0, 50.0, 50.0];
        let km = [36.1, 36.3, 28.1, 44.9, 43.4, 38.2, 28.3, 36.7, 51.3, 41.5, 42.9, 36.1, 0.0, 22.8, 43.6, 42.0];
        let days: Vec<DayRow> = (0..16)
            .map(|i| {
                let gain = if (3..=7).contains(&i) { 1800.0 } else { 500.0 };
                row(&format!("2022-07-{:02}", i + 1), km[i], gain, Some(rhr[i]), None)
            })
            .collect();
        let b = Baselines { resting_hr: Some(48.0), ..Default::default() };
        let eps = detect_episodes(&days, &b, &EpisodeConfig::default());
        assert_eq!(eps.len(), 2, "expected exactly one stretch and one dip");

        let stretch = eps.iter().find(|e| e.kind == EpisodeKind::StrongStretch).expect("stretch");
        assert_eq!(stretch.start_date, d("2022-07-04"));
        assert_eq!(stretch.end_date, d("2022-07-08"));
        assert_eq!(stretch.start_trip_day, 4);
        assert_eq!(stretch.end_trip_day, 8);

        let dip = eps.iter().find(|e| e.kind == EpisodeKind::StrainDip).expect("dip");
        assert_eq!(dip.start_date, d("2022-07-11"));
        assert_eq!(dip.end_date, d("2022-07-14"));
        assert_eq!(dip.start_trip_day, 11);
        assert_eq!(dip.end_trip_day, 14);
        assert_eq!(dip.peak_rhr, Some(56.0));
    }
}
