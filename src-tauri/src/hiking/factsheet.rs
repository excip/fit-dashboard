//! Canonical-JSON fact sheets, content hashes, and LLM prompts for hike notes.
//!
//! Pure module: no I/O, no AppState. Fact-sheet structs derive `serde::Serialize`
//! with a FIXED field order (serde emits fields in declaration order) so the JSON
//! is canonical without any HashMap/BTreeMap. All floats are rounded to one decimal
//! at build time so hash stability never depends on float formatting.

use serde::Serialize;
use sha2::{Digest, Sha256};

use chrono::NaiveDate;

use crate::hiking::baseline::{Baselines, RecoverySummary};
use crate::hiking::cluster::{Trip, TripCategory};
use crate::hiking::episodes::{Episode, EpisodeKind};
use crate::hiking::store::RecoveryDay;
use crate::hiking::HikeActivity;

/// Round to one decimal so hashes never depend on float-formatting drift.
fn r1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

// ---------------------------------------------------------------------------
// Fact-sheet structs (declaration order == canonical JSON key order)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct DayStat {
    pub date: NaiveDate,
    pub km: f64,
    pub gain_m: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TripDates {
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub nights: i64,
    pub hiking_days: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct TripTotals {
    pub distance_km: f64,
    pub elevation_gain_m: f64,
    pub elevation_loss_m: f64,
    pub steps: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TripSuperlatives {
    pub longest_day: Option<DayStat>,
    pub biggest_climb_day: Option<DayStat>,
    pub highest_point_m: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TripHistory {
    pub rank_by_distance_all_trips: usize,
    pub rank_by_gain_all_trips: usize,
    pub trips_total: usize,
}

/// One detected episode, pre-rendered for the prompt. `detail` always spells out
/// explicit `date: value` pairs (never a bare arrow sequence) so the model cannot
/// misdate the peak.
#[derive(Debug, Clone, Serialize)]
pub struct EpisodeFact {
    pub kind: String,
    pub trip_days: String,
    pub dates: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TripFactSheet {
    pub subject: &'static str, // "trip"
    pub category: TripCategory,
    pub name: Option<String>,
    pub dates: TripDates,
    pub totals: TripTotals,
    pub superlatives: TripSuperlatives,
    pub history: TripHistory,
    pub baselines: Baselines,
    pub recovery_summary: RecoverySummary,
    /// thru-hikes only (monthly avg resting HR), else empty. Vec, not a map.
    pub monthly_rhr: Vec<(String, f64)>,
    pub episodes: Vec<EpisodeFact>,
    pub user_rating: Option<i64>,
    pub user_note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HikeFactSheet {
    pub subject: &'static str, // "activity"
    pub date: NaiveDate,
    pub trip_name: Option<String>,
    pub trip_category: TripCategory,
    pub km: f64,
    pub gain_m: f64,
    pub loss_m: f64,
    pub duration_h: f64,
    pub max_elevation_m: Option<f64>,
    pub rank_km_in_year: usize,
    pub rank_gain_in_year: usize,
    pub activities_in_year: usize,
    pub day_rhr_dev: Option<f64>,
    pub day_hrv_dev: Option<f64>,
    pub user_rating: Option<i64>,
    pub user_note: Option<String>,
}

// ---------------------------------------------------------------------------
// Rendering helpers
// ---------------------------------------------------------------------------

fn month_day(d: NaiveDate) -> String {
    d.format("%b %-d").to_string()
}

/// Render a number without a trailing `.0` for whole values ("52", "44.9").
fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

fn day_stat(a: &HikeActivity) -> DayStat {
    DayStat {
        date: a.date,
        km: r1(a.distance_m / 1000.0),
        gain_m: r1(a.elevation_gain),
    }
}

/// Render an `Episode` as an `EpisodeFact` with explicit dated per-day values.
/// For a strain dip the strained metric is spelled out day by day with "(peak)"
/// on the worst day (max resting HR / min HRV). Never a bare arrow sequence.
fn render_episode(ep: &Episode) -> EpisodeFact {
    let kind = match ep.kind {
        EpisodeKind::StrainDip => "strain dip",
        EpisodeKind::StrongStretch => "strong stretch",
    }
    .to_string();
    let trip_days = format!("days {}\u{2013}{}", ep.start_trip_day, ep.end_trip_day);
    let dates = format!("{} \u{2013} {}", month_day(ep.start_date), month_day(ep.end_date));

    let detail = match ep.kind {
        EpisodeKind::StrainDip => {
            let dmax = ep.daily.iter().map(|(_, v)| *v).fold(f64::MIN, f64::max);
            let dmin = ep.daily.iter().map(|(_, v)| *v).fold(f64::MAX, f64::min);
            // `daily` holds resting HR when the dip is RHR-driven; then its max
            // equals peak_rhr. Otherwise it holds HRV and the worst day is the min.
            let is_rhr = ep.peak_rhr == Some(dmax);
            let label = if is_rhr { "resting HR" } else { "HRV" };
            let worst = if is_rhr { dmax } else { dmin };
            let mut marked = false;
            let parts: Vec<String> = ep
                .daily
                .iter()
                .map(|(d, v)| {
                    let cell = format!("{}: {}", month_day(*d), fmt_num(r1(*v)));
                    if !marked && *v == worst {
                        marked = true;
                        format!("{cell} (peak)")
                    } else {
                        cell
                    }
                })
                .collect();
            format!("{label} by day: {}", parts.join(", "))
        }
        EpisodeKind::StrongStretch => {
            let parts: Vec<String> = ep
                .daily
                .iter()
                .map(|(d, v)| format!("{}: {}", month_day(*d), fmt_num(r1(*v))))
                .collect();
            format!("distance in km by day: {}", parts.join(", "))
        }
    };

    EpisodeFact { kind, trip_days, dates, detail }
}

fn monthly_rhr(recovery: &[RecoveryDay], start: NaiveDate, end: NaiveDate) -> Vec<(String, f64)> {
    let (s, e) = (start.to_string(), end.to_string());
    // (month, sum, count) in first-seen (== ascending date) order.
    let mut acc: Vec<(String, f64, usize)> = Vec::new();
    for rd in recovery {
        if rd.date.as_str() < s.as_str() || rd.date.as_str() > e.as_str() {
            continue;
        }
        let Some(v) = rd.resting_hr else { continue };
        let month = rd.date[0..7].to_string();
        if let Some(entry) = acc.iter_mut().find(|(m, _, _)| *m == month) {
            entry.1 += v as f64;
            entry.2 += 1;
        } else {
            acc.push((month, v as f64, 1));
        }
    }
    acc.into_iter()
        .map(|(m, sum, n)| (m, r1(sum / n as f64)))
        .collect()
}

// ---------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------

/// Build a trip fact sheet. `activities` is the full member-activity pool (filtered
/// by `trip.activity_ids`); `all_trips` drives the history ranks; `recovery` is the
/// day series (used only for a thru-hike's monthly resting-HR aggregate).
#[allow(clippy::too_many_arguments)]
pub fn trip_fact_sheet(
    trip: &Trip,
    activities: &[HikeActivity],
    all_trips: &[Trip],
    recovery: &[RecoveryDay],
    baselines: &Baselines,
    recovery_summary: &RecoverySummary,
    episodes: &[Episode],
    user_rating: Option<i64>,
    user_note: Option<&str>,
) -> TripFactSheet {
    let members: Vec<&HikeActivity> = activities
        .iter()
        .filter(|a| trip.activity_ids.contains(&a.activity_id))
        .collect();

    let mut day_dates: Vec<NaiveDate> = members.iter().map(|m| m.date).collect();
    day_dates.sort();
    day_dates.dedup();
    let hiking_days = day_dates.len();

    let longest_day = members
        .iter()
        .copied()
        .max_by(|a, b| a.distance_m.partial_cmp(&b.distance_m).unwrap())
        .map(day_stat);
    let biggest_climb_day = members
        .iter()
        .copied()
        .max_by(|a, b| a.elevation_gain.partial_cmp(&b.elevation_gain).unwrap())
        .map(day_stat);
    let highest_point_m = members
        .iter()
        .filter_map(|m| m.max_elevation)
        .fold(None, |acc, v| Some(acc.map_or(v, |a: f64| a.max(v))))
        .map(r1);

    let rank_by_distance_all_trips = 1 + all_trips
        .iter()
        .filter(|t| t.total_distance_m > trip.total_distance_m)
        .count();
    let rank_by_gain_all_trips = 1 + all_trips
        .iter()
        .filter(|t| t.total_gain > trip.total_gain)
        .count();

    let monthly = if trip.category == TripCategory::ThruHike {
        monthly_rhr(recovery, trip.start_date, trip.end_date)
    } else {
        Vec::new()
    };

    TripFactSheet {
        subject: "trip",
        category: trip.category,
        name: trip.name.clone(),
        dates: TripDates {
            start: trip.start_date,
            end: trip.end_date,
            nights: trip.nights,
            hiking_days,
        },
        totals: TripTotals {
            distance_km: r1(trip.total_distance_m / 1000.0),
            elevation_gain_m: r1(trip.total_gain),
            elevation_loss_m: r1(trip.total_loss),
            steps: trip.total_steps,
        },
        superlatives: TripSuperlatives {
            longest_day,
            biggest_climb_day,
            highest_point_m,
        },
        history: TripHistory {
            rank_by_distance_all_trips,
            rank_by_gain_all_trips,
            trips_total: all_trips.len(),
        },
        baselines: baselines.clone(),
        recovery_summary: recovery_summary.clone(),
        monthly_rhr: monthly,
        episodes: episodes.iter().map(render_episode).collect(),
        user_rating,
        user_note: user_note.map(|s| s.to_string()),
    }
}

/// Build a per-hike fact sheet. `year_activities` are all the hike-member
/// activities in the same year (including `act`), used for the within-year ranks.
pub fn hike_fact_sheet(
    act: &HikeActivity,
    trip: &Trip,
    year_activities: &[&HikeActivity],
    recovery_day: Option<&RecoveryDay>,
    baselines: &Baselines,
    user_rating: Option<i64>,
    user_note: Option<&str>,
) -> HikeFactSheet {
    let rank_km_in_year = 1 + year_activities
        .iter()
        .filter(|a| a.distance_m > act.distance_m)
        .count();
    let rank_gain_in_year = 1 + year_activities
        .iter()
        .filter(|a| a.elevation_gain > act.elevation_gain)
        .count();

    let day_rhr_dev = match (recovery_day.and_then(|r| r.resting_hr), baselines.resting_hr) {
        (Some(v), Some(b)) => Some(r1(v as f64 - b)),
        _ => None,
    };
    let day_hrv_dev = match (
        recovery_day.and_then(|r| r.hrv_last_night_avg),
        baselines.hrv_last_night_avg,
    ) {
        (Some(v), Some(b)) => Some(r1(v - b)),
        _ => None,
    };

    HikeFactSheet {
        subject: "activity",
        date: act.date,
        trip_name: trip.name.clone(),
        trip_category: trip.category,
        km: r1(act.distance_m / 1000.0),
        gain_m: r1(act.elevation_gain),
        loss_m: r1(act.elevation_loss),
        duration_h: r1(act.duration_s / 3600.0),
        max_elevation_m: act.max_elevation.map(r1),
        rank_km_in_year,
        rank_gain_in_year,
        activities_in_year: year_activities.len(),
        day_rhr_dev,
        day_hrv_dev,
        user_rating,
        user_note: user_note.map(|s| s.to_string()),
    }
}

/// Hex-encoded SHA-256 of the canonical JSON serialization.
pub fn fact_hash<T: Serialize>(fs: &T) -> String {
    let json = serde_json::to_string(fs).expect("fact sheet serializes");
    let mut h = Sha256::new();
    h.update(json.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Build the (system, user) prompt pair. `is_thru` widens the hard word cap; the
/// user message carries the canonical JSON fact sheet.
pub fn build_prompt<T: Serialize>(fs: &T, is_trip: bool, is_thru: bool) -> (String, String) {
    let (min, max) = if is_thru { (150, 250) } else { (120, 180) };
    let subject = if !is_trip {
        "hike"
    } else if is_thru {
        "thru-hike"
    } else {
        "trip"
    };
    let system = format!(
        "/no_think\n\
You write a short, factual narrative about the reader's own {subject}, in flowing prose.\n\
\n\
Rules:\n\
- Use ONLY the facts in the JSON fact sheet. Never invent numbers, places, names, weather, or events that are not in the data.\n\
- The \"superlatives\" are records WITHIN this {subject} only. A day's all-time standing across your history is given solely by the \"history\" ranks; never call something a lifetime best or \"your biggest ever\" unless a rank of 1 says so.\n\
- Address the reader as \"you\".\n\
- Write {min}\u{2013}{max} words. This word count is a HARD limit.\n\
- Flowing prose only: two or three short paragraphs. No bullet points, no headers, no lists.\n\
- Weave any detected episodes into the story as turning points, and always name their dates.\n\
- Cover the full arc: how the {subject} built up, how it ended, and what the post-trip recovery data says about the outcome.\n\
- If a user note is present, use it to explain what the numbers show; never contradict it.\n\
- Mention unrecorded or missing metrics in at most one brief clause, if at all."
    );
    let json = serde_json::to_string(fs).expect("fact sheet serializes");
    let user = format!("Fact sheet:\n{json}");
    (system, user)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    fn act(
        id: i64,
        date: NaiveDate,
        dist_m: f64,
        gain: f64,
        loss: f64,
        steps: i64,
        dur_s: f64,
        max_elev: Option<f64>,
    ) -> HikeActivity {
        HikeActivity {
            activity_id: id,
            date,
            activity_type: "walking".into(),
            distance_m: dist_m,
            elevation_gain: gain,
            elevation_loss: loss,
            steps,
            start_lat: 0.0,
            start_lon: 0.0,
            end_lat: 0.0,
            end_lon: 0.0,
            max_elevation: max_elev,
            location_name: None,
            duration_s: dur_s,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn mk_trip(
        id: i64,
        cat: TripCategory,
        start: NaiveDate,
        end: NaiveDate,
        nights: i64,
        ids: Vec<i64>,
        dist: f64,
        gain: f64,
        loss: f64,
        steps: i64,
    ) -> Trip {
        Trip {
            id,
            category: cat,
            name: None,
            start_date: start,
            end_date: end,
            nights,
            activity_ids: ids,
            total_distance_m: dist,
            total_gain: gain,
            total_loss: loss,
            total_steps: steps,
        }
    }

    fn rs_none() -> RecoverySummary {
        RecoverySummary { resting_hr: None, hrv: None }
    }

    fn rday(date: &str, rhr: Option<i64>) -> RecoveryDay {
        RecoveryDay {
            date: date.into(),
            sleep_score: None,
            sleep_seconds: None,
            resting_hr: rhr,
            hrv_last_night_avg: None,
            body_battery_high: None,
            body_battery_low: None,
            avg_stress: None,
            training_readiness: None,
        }
    }

    #[test]
    fn hash_stable_and_changes_with_rating() {
        let acts = vec![act(1, nd(2024, 6, 1), 20_000.0, 800.0, 800.0, 25_000, 14_400.0, Some(2100.0))];
        let t = mk_trip(1, TripCategory::Weekend, nd(2024, 6, 1), nd(2024, 6, 2), 1, vec![1], 20_000.0, 800.0, 800.0, 25_000);
        let all = vec![t.clone()];
        let b = Baselines::default();
        let rs = rs_none();

        let fs1 = trip_fact_sheet(&t, &acts, &all, &[], &b, &rs, &[], None, None);
        let fs2 = trip_fact_sheet(&t, &acts, &all, &[], &b, &rs, &[], None, None);
        assert_eq!(fact_hash(&fs1), fact_hash(&fs2), "identical inputs must hash equal");

        let fs3 = trip_fact_sheet(&t, &acts, &all, &[], &b, &rs, &[], Some(5), None);
        assert_ne!(fact_hash(&fs1), fact_hash(&fs3), "flipping the rating must change the hash");
    }

    #[test]
    fn history_ranks_against_all_trips() {
        let a = mk_trip(1, TripCategory::ThruHike, nd(2022, 5, 1), nd(2022, 5, 5), 4, vec![1], 100_000.0, 5000.0, 5000.0, 1000);
        let bt = mk_trip(2, TripCategory::ThruHike, nd(2023, 5, 1), nd(2023, 5, 5), 4, vec![2], 50_000.0, 8000.0, 8000.0, 1000);
        let c = mk_trip(3, TripCategory::ThruHike, nd(2024, 5, 1), nd(2024, 5, 5), 4, vec![3], 80_000.0, 2000.0, 2000.0, 1000);
        let all = vec![a.clone(), bt, c];

        let fs = trip_fact_sheet(&a, &[], &all, &[], &Baselines::default(), &rs_none(), &[], None, None);
        assert_eq!(fs.history.rank_by_distance_all_trips, 1); // biggest distance
        assert_eq!(fs.history.rank_by_gain_all_trips, 2); // only B (8000) is bigger
        assert_eq!(fs.history.trips_total, 3);
    }

    #[test]
    fn dip_detail_has_dated_values_and_peak() {
        let ep = Episode {
            kind: EpisodeKind::StrainDip,
            start_date: nd(2022, 7, 11),
            end_date: nd(2022, 7, 14),
            start_trip_day: 11,
            end_trip_day: 14,
            daily: vec![
                (nd(2022, 7, 11), 52.0),
                (nd(2022, 7, 12), 51.0),
                (nd(2022, 7, 13), 56.0),
                (nd(2022, 7, 14), 54.0),
            ],
            peak_rhr: Some(56.0),
            min_hrv: None,
            avg_km: 0.0,
            max_gain_m: 0.0,
            severity: 1.6,
        };
        let f = render_episode(&ep);
        assert!(f.detail.contains("resting HR by day"), "detail: {}", f.detail);
        assert!(f.detail.contains("Jul 11: 52"));
        assert!(f.detail.contains("Jul 12: 51"));
        assert!(f.detail.contains("Jul 13: 56 (peak)"), "detail: {}", f.detail);
        assert!(f.detail.contains("Jul 14: 54"));
        // never a bare arrow sequence (the misdating bug)
        assert!(!f.detail.contains("->"));
        assert!(!f.detail.contains('\u{2192}'));
        // (peak) exactly once
        assert_eq!(f.detail.matches("(peak)").count(), 1);
    }

    #[test]
    fn thru_hike_has_monthly_rhr_weekend_empty() {
        let recovery = vec![
            rday("2022-07-30", Some(50)),
            rday("2022-07-31", Some(52)),
            rday("2022-08-01", Some(48)),
            rday("2022-08-02", Some(46)),
        ];
        let thru = mk_trip(1, TripCategory::ThruHike, nd(2022, 7, 30), nd(2022, 8, 2), 3, vec![1], 100_000.0, 5000.0, 5000.0, 1000);
        let fs = trip_fact_sheet(&thru, &[], &[thru.clone()], &recovery, &Baselines::default(), &rs_none(), &[], None, None);
        assert_eq!(
            fs.monthly_rhr,
            vec![("2022-07".to_string(), 51.0), ("2022-08".to_string(), 47.0)]
        );

        let wk = mk_trip(2, TripCategory::Weekend, nd(2022, 7, 30), nd(2022, 7, 31), 1, vec![2], 20_000.0, 500.0, 500.0, 1000);
        let fs2 = trip_fact_sheet(&wk, &[], &[wk.clone()], &recovery, &Baselines::default(), &rs_none(), &[], None, None);
        assert!(fs2.monthly_rhr.is_empty());
    }

    #[test]
    fn system_prompt_has_guardrails_and_word_cap() {
        let payload = serde_json::json!({ "subject": "trip" });

        let (sys, user) = build_prompt(&payload, true, true);
        assert!(sys.starts_with("/no_think"), "must start with /no_think");
        assert!(sys.contains("150\u{2013}250"), "thru-hike hard cap");
        assert!(sys.contains("HARD limit"));
        assert!(sys.contains("full arc"));
        assert!(sys.contains("how it ended"));
        assert!(sys.contains("post-trip"));
        assert!(sys.contains("Never invent numbers"));
        assert!(user.starts_with("Fact sheet:\n"));

        let (sys2, _) = build_prompt(&payload, true, false);
        assert!(sys2.contains("120\u{2013}180"), "normal hard cap");
    }

    #[test]
    fn hike_day_deviations_are_value_minus_baseline() {
        let a = act(10, nd(2024, 6, 1), 15_000.0, 600.0, 600.0, 20_000, 10_800.0, Some(1800.0));
        let t = mk_trip(1, TripCategory::DayHike, nd(2024, 6, 1), nd(2024, 6, 1), 0, vec![10], 15_000.0, 600.0, 600.0, 20_000);
        let year: Vec<&HikeActivity> = vec![&a];
        let b = Baselines {
            resting_hr: Some(48.0),
            hrv_last_night_avg: Some(60.0),
            ..Default::default()
        };
        let rd = RecoveryDay {
            date: "2024-06-01".into(),
            sleep_score: None,
            sleep_seconds: None,
            resting_hr: Some(53),
            hrv_last_night_avg: Some(54.0),
            body_battery_high: None,
            body_battery_low: None,
            avg_stress: None,
            training_readiness: None,
        };
        let fs = hike_fact_sheet(&a, &t, &year, Some(&rd), &b, None, None);
        assert_eq!(fs.day_rhr_dev, Some(5.0)); // 53 - 48
        assert_eq!(fs.day_hrv_dev, Some(-6.0)); // 54 - 60
        assert_eq!(fs.rank_km_in_year, 1);
        assert_eq!(fs.activities_in_year, 1);
        assert_eq!(fs.km, 15.0);
        assert_eq!(fs.duration_h, 3.0);
    }
}
