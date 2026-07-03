use chrono::{Duration, NaiveDate};
use crate::hiking::store::RecoveryDay;

/// Per-metric at-home baseline (median of pre-trip days). None = insufficient data.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Baselines {
    pub sleep_score: Option<f64>,
    pub sleep_seconds: Option<f64>,
    pub resting_hr: Option<f64>,
    pub hrv_last_night_avg: Option<f64>,
    pub body_battery_high: Option<f64>,
    pub body_battery_low: Option<f64>,
    pub avg_stress: Option<f64>,
    pub training_readiness: Option<f64>,
}

fn parse_date(s: &str) -> NaiveDate {
    // dates come from store::load_recovery, always ISO
    NaiveDate::parse_from_str(s, "%Y-%m-%d").expect("ISO date from store")
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    Some(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 })
}

/// Median over the 28 days before trip_start (widened to 56 if < 7 usable
/// values), skipping days inside any trip range. None if still < 7 values.
fn metric_baseline(
    days: &[RecoveryDay],
    trip_ranges: &[(NaiveDate, NaiveDate)],
    trip_start: NaiveDate,
    get: &dyn Fn(&RecoveryDay) -> Option<f64>,
) -> Option<f64> {
    for window in [28i64, 56] {
        let from = trip_start - Duration::days(window);
        let vals: Vec<f64> = days
            .iter()
            .filter(|d| {
                let dt = parse_date(&d.date);
                dt >= from
                    && dt < trip_start
                    && !trip_ranges.iter().any(|(s, e)| dt >= *s && dt <= *e)
            })
            .filter_map(get)
            .collect();
        if vals.len() >= 7 {
            return median(vals);
        }
    }
    None
}

pub fn baselines(
    days: &[RecoveryDay],
    trip_ranges: &[(NaiveDate, NaiveDate)],
    trip_start: NaiveDate,
) -> Baselines {
    let f = |get: &dyn Fn(&RecoveryDay) -> Option<f64>| {
        metric_baseline(days, trip_ranges, trip_start, get)
    };
    Baselines {
        sleep_score: f(&|d| d.sleep_score.map(|v| v as f64)),
        sleep_seconds: f(&|d| d.sleep_seconds.map(|v| v as f64)),
        resting_hr: f(&|d| d.resting_hr.map(|v| v as f64)),
        hrv_last_night_avg: f(&|d| d.hrv_last_night_avg),
        body_battery_high: f(&|d| d.body_battery_high.map(|v| v as f64)),
        body_battery_low: f(&|d| d.body_battery_low.map(|v| v as f64)),
        avg_stress: f(&|d| d.avg_stress.map(|v| v as f64)),
        training_readiness: f(&|d| d.training_readiness),
    }
}

/// Strain + bounce-back for one metric. peak_deviation is signed
/// (value - baseline): positive for RHR, negative for HRV.
/// days_to_recover: None = not back within 21 days of trip end.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricSummary {
    pub peak_deviation: f64,
    pub peak_trip_day: i64, // 1 = first trip day
    pub days_to_recover: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RecoverySummary {
    pub resting_hr: Option<MetricSummary>,
    pub hrv: Option<MetricSummary>,
}

fn metric_summary(
    days: &[RecoveryDay],
    baseline: Option<f64>,
    trip_start: NaiveDate,
    trip_end: NaiveDate,
    get: &dyn Fn(&RecoveryDay) -> Option<f64>,
    strains_up: bool,
    recovered: &dyn Fn(f64, f64) -> bool,
) -> Option<MetricSummary> {
    let baseline = baseline?;
    let mut peak: Option<(f64, i64)> = None; // (deviation, trip day)
    for rd in days {
        let dt = parse_date(&rd.date);
        if dt < trip_start || dt > trip_end {
            continue;
        }
        let Some(v) = get(rd) else { continue };
        let dev = v - baseline;
        let worse = match peak {
            None => true,
            Some((p, _)) => if strains_up { dev > p } else { dev < p },
        };
        if worse {
            peak = Some((dev, (dt - trip_start).num_days() + 1));
        }
    }
    let (peak_deviation, peak_trip_day) = peak?;

    let value_on = |date: NaiveDate| -> Option<f64> {
        days.iter().find(|rd| parse_date(&rd.date) == date).and_then(get)
    };
    let ok = |date: NaiveDate| value_on(date).map(|v| recovered(v, baseline)).unwrap_or(false);
    let days_to_recover = (1..=21).find(|&i| {
        let d0 = trip_end + Duration::days(i);
        ok(d0) && ok(d0 + Duration::days(1))
    });
    Some(MetricSummary { peak_deviation, peak_trip_day, days_to_recover })
}

pub fn recovery_summary(
    days: &[RecoveryDay],
    baselines: &Baselines,
    trip_start: NaiveDate,
    trip_end: NaiveDate,
) -> RecoverySummary {
    RecoverySummary {
        resting_hr: metric_summary(
            days, baselines.resting_hr, trip_start, trip_end,
            &|d| d.resting_hr.map(|v| v as f64), true,
            &|v, b| v <= b + 2.0,
        ),
        hrv: metric_summary(
            days, baselines.hrv_last_night_avg, trip_start, trip_end,
            &|d| d.hrv_last_night_avg, false,
            &|v, b| v >= b * 0.95,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    /// RecoveryDay fixture with only the fields these tests use.
    fn day(date: &str, rhr: Option<i64>, hrv: Option<f64>) -> RecoveryDay {
        RecoveryDay {
            date: date.into(),
            sleep_score: None,
            sleep_seconds: None,
            resting_hr: rhr,
            hrv_last_night_avg: hrv,
            body_battery_high: None,
            body_battery_low: None,
            avg_stress: None,
            training_readiness: None,
        }
    }

    #[test]
    fn median_of_28_day_window() {
        // 7 usable days, values 50..=56 -> median 53
        let days: Vec<RecoveryDay> = (0..7)
            .map(|i| day(&format!("2024-05-{:02}", 10 + i), Some(50 + i), None))
            .collect();
        let b = baselines(&days, &[], d("2024-06-01"));
        assert_eq!(b.resting_hr, Some(53.0));
        assert_eq!(b.hrv_last_night_avg, None); // no HRV data at all
    }

    #[test]
    fn even_count_averages_middle_two() {
        // 8 values 50..=57 -> (53 + 54) / 2 = 53.5
        let days: Vec<RecoveryDay> = (0..8)
            .map(|i| day(&format!("2024-05-{:02}", 10 + i), Some(50 + i), None))
            .collect();
        let b = baselines(&days, &[], d("2024-06-01"));
        assert_eq!(b.resting_hr, Some(53.5));
    }

    #[test]
    fn excludes_days_inside_trips() {
        // 7 clean days (median 53) + 3 poisoned days inside a prior trip
        let mut days: Vec<RecoveryDay> = (0..7)
            .map(|i| day(&format!("2024-05-{:02}", 10 + i), Some(50 + i), None))
            .collect();
        for i in 20..=22 {
            days.push(day(&format!("2024-05-{i}"), Some(90), None));
        }
        let trips = [(d("2024-05-20"), d("2024-05-22"))];
        let b = baselines(&days, &trips, d("2024-06-01"));
        assert_eq!(b.resting_hr, Some(53.0));
    }

    #[test]
    fn widens_to_56_days_when_sparse() {
        // Only 5 values inside the 28-day window; 4 more in days -56..-29.
        // 9 values total: [50,51,52,53,54,60,61,62,63] -> median 54
        let mut days: Vec<RecoveryDay> = (0..5)
            .map(|i| day(&format!("2024-05-{:02}", 10 + i), Some(50 + i), None))
            .collect();
        for i in 0..4 {
            days.push(day(&format!("2024-04-{:02}", 10 + i), Some(60 + i), None));
        }
        let b = baselines(&days, &[], d("2024-06-01"));
        assert_eq!(b.resting_hr, Some(54.0));
    }

    #[test]
    fn insufficient_data_gives_none() {
        // 6 values even in the 56-day window -> None
        let days: Vec<RecoveryDay> = (0..6)
            .map(|i| day(&format!("2024-05-{:02}", 10 + i), Some(50 + i), None))
            .collect();
        let b = baselines(&days, &[], d("2024-06-01"));
        assert_eq!(b.resting_hr, None);
    }

    /// 7 pre-trip days: rhr 50, hrv 60 -> baselines rhr=50.0, hrv=60.0
    fn pre_days() -> Vec<RecoveryDay> {
        (0..7)
            .map(|i| day(&format!("2024-05-{:02}", 10 + i), Some(50), Some(60.0)))
            .collect()
    }

    fn trip() -> (NaiveDate, NaiveDate) {
        (d("2024-06-01"), d("2024-06-03"))
    }

    #[test]
    fn rhr_peak_deviation_and_day() {
        let mut days = pre_days();
        days.push(day("2024-06-01", Some(55), None));
        days.push(day("2024-06-02", Some(58), None));
        days.push(day("2024-06-03", Some(56), None));
        let (start, end) = trip();
        let b = baselines(&days, &[], start);
        let s = recovery_summary(&days, &b, start, end);
        let rhr = s.resting_hr.expect("rhr summary");
        assert_eq!(rhr.peak_deviation, 8.0);
        assert_eq!(rhr.peak_trip_day, 2);
    }

    #[test]
    fn days_to_recover_needs_two_consecutive_days() {
        // tolerance: <= 50 + 2 = 52
        let mut days = pre_days();
        days.push(day("2024-06-02", Some(58), None));
        days.push(day("2024-06-04", Some(55), None)); // +1: no
        days.push(day("2024-06-05", Some(52), None)); // +2: ok, but +3 not ok
        days.push(day("2024-06-06", Some(54), None)); // +3: no
        days.push(day("2024-06-07", Some(51), None)); // +4: ok
        days.push(day("2024-06-08", Some(52), None)); // +5: ok -> recovered at 4
        let (start, end) = trip();
        let b = baselines(&days, &[], start);
        let s = recovery_summary(&days, &b, start, end);
        assert_eq!(s.resting_hr.expect("rhr summary").days_to_recover, Some(4));
    }

    #[test]
    fn not_recovered_within_21_days_is_none() {
        let mut days = pre_days();
        days.push(day("2024-06-02", Some(58), None));
        for i in 4..=26 {
            days.push(day(&format!("2024-06-{i:02}"), Some(60), None));
        }
        let (start, end) = trip();
        let b = baselines(&days, &[], start);
        let s = recovery_summary(&days, &b, start, end);
        assert_eq!(s.resting_hr.expect("rhr summary").days_to_recover, None);
    }

    #[test]
    fn hrv_strains_downward() {
        // baseline 60, tolerance >= 57 (60 * 0.95)
        let mut days = pre_days();
        days.push(day("2024-06-01", None, Some(55.0)));
        days.push(day("2024-06-02", None, Some(50.0)));
        days.push(day("2024-06-03", None, Some(57.0)));
        days.push(day("2024-06-04", None, Some(57.0))); // +1: ok
        days.push(day("2024-06-05", None, Some(58.0))); // +2: ok -> recovered at 1
        let (start, end) = trip();
        let b = baselines(&days, &[], start);
        let s = recovery_summary(&days, &b, start, end);
        let hrv = s.hrv.expect("hrv summary");
        assert_eq!(hrv.peak_deviation, -10.0);
        assert_eq!(hrv.peak_trip_day, 2);
        assert_eq!(hrv.days_to_recover, Some(1));
    }

    #[test]
    fn no_baseline_means_no_summary() {
        // no pre-trip data at all
        let days = vec![day("2024-06-02", Some(58), Some(50.0))];
        let (start, end) = trip();
        let b = baselines(&days, &[], start);
        let s = recovery_summary(&days, &b, start, end);
        assert!(s.resting_hr.is_none());
        assert!(s.hrv.is_none());
    }

    #[test]
    fn no_during_trip_data_means_no_summary() {
        let days = pre_days(); // baseline exists, but no trip-day values
        let (start, end) = trip();
        let b = baselines(&days, &[], start);
        let s = recovery_summary(&days, &b, start, end);
        assert!(s.resting_hr.is_none());
    }
}
