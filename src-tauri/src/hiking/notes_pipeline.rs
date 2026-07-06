//! Web-only LLM notes pipeline: fact-sheet enumeration, hash-diff against stored
//! generated notes, LLM generation via an OpenAI-compatible endpoint, and orphan
//! cleanup. The whole module (and its reqwest usage) is gated web-only on the
//! `pub mod` line in `mod.rs`, so no inner cfg gate is needed here.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::Duration as StdDuration;

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime};

use crate::database::{Database, GeneratedNote, NotesRun};
use crate::hiking::baseline;
use crate::hiking::cluster::TripCategory;
use crate::hiking::episodes::{detect_episodes, DayRow, EpisodeConfig};
use crate::hiking::factsheet::{build_prompt, fact_hash, hike_fact_sheet, trip_fact_sheet};
use crate::hiking::store;
use crate::hiking::HikeActivity;
use crate::state::AppState;

const DEFAULT_MODEL: &str = "qwen/qwen3-14b";

/// LLM endpoint + model, read from the environment. `None` ⇒ pipeline disabled.
pub struct LlmConfig {
    pub endpoint: String,
    pub model: String,
}

/// Read `FIT_DASHBOARD_LLM_ENDPOINT` (unset/empty ⇒ `None`, i.e. disabled) and
/// `FIT_DASHBOARD_LLM_MODEL` (default `qwen/qwen3-14b`).
pub fn llm_config() -> Option<LlmConfig> {
    let endpoint = std::env::var("FIT_DASHBOARD_LLM_ENDPOINT")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())?;
    let model = std::env::var("FIT_DASHBOARD_LLM_MODEL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    Some(LlmConfig { endpoint, model })
}

/// Outcome of a pipeline run.
pub struct RunSummary {
    pub ok: bool,
    pub generated: usize,
    pub skipped: usize,
    pub deleted: usize,
    pub message: Option<String>,
}

// ---------------------------------------------------------------------------
// Chat client abstraction (real HTTP client + mockable in tests)
// ---------------------------------------------------------------------------

/// Something that can turn a (system, user) prompt into a note. Abstracted so the
/// reconcile loop is unit-testable without any HTTP. The `Send` bound keeps
/// `run_notes` spawnable on a multi-thread runtime (wired in Task 5).
trait ChatClient {
    fn chat(
        &self,
        system: &str,
        user: &str,
    ) -> impl std::future::Future<Output = Result<String, String>> + Send;
}

/// Strip any `<think>…</think>` block from the model reply and trim.
fn strip_think(s: &str) -> String {
    let mut out = s.to_string();
    while let (Some(start), Some(end)) = (out.find("<think>"), out.find("</think>")) {
        if end >= start {
            out.replace_range(start..end + "</think>".len(), "");
        } else {
            break;
        }
    }
    out.trim().to_string()
}

struct HttpChatClient {
    client: reqwest::Client,
    endpoint: String,
    model: String,
}

impl HttpChatClient {
    fn new(cfg: &LlmConfig) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(StdDuration::from_secs(300)) // first call bears the cold model load
            .build()
            .map_err(|e| format!("failed to build HTTP client: {e}"))?;
        Ok(Self {
            client,
            endpoint: cfg.endpoint.clone(),
            model: cfg.model.clone(),
        })
    }
}

impl ChatClient for HttpChatClient {
    async fn chat(&self, system: &str, user: &str) -> Result<String, String> {
        let url = format!("{}/chat/completions", self.endpoint.trim_end_matches('/'));
        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
            "temperature": 0.7,
            "max_tokens": 600,
        });
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("LLM returned HTTP {}", resp.status()));
        }
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("invalid JSON response: {e}"))?;
        let content = v["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| "response missing choices[0].message.content".to_string())?;
        Ok(strip_think(content))
    }
}

// ---------------------------------------------------------------------------
// Reconcile loop (factored out for unit testing without garmin.db or HTTP)
// ---------------------------------------------------------------------------

/// One thing that may need a generated note: the identity + its current fact
/// hash + the prompt pair to (re)generate it.
struct Subject {
    subject_type: String,
    subject_id: i64,
    fact_hash: String,
    system: String,
    user: String,
}

fn now_string() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

fn record(db: &Database, run_at: &str, s: &RunSummary) {
    let row = NotesRun {
        run_at: run_at.to_string(),
        ok: s.ok,
        generated: s.generated as i64,
        skipped: s.skipped as i64,
        deleted: s.deleted as i64,
        message: s.message.clone(),
    };
    if let Err(e) = db.record_hiking_notes_run(&row) {
        tracing::error!(error = %e, "failed to record hiking notes run");
    }
}

/// Diff `subjects` against stored generated notes by fact hash: hash match ⇒ skip;
/// otherwise call the LLM and upsert with model + timestamp. Then orphan-clean any
/// stored note whose subject is no longer enumerated. Always records the run.
/// The first LLM/store error aborts (already-upserted notes stay), `ok = false`.
async fn reconcile<C: ChatClient>(
    subjects: &[Subject],
    client: &C,
    db: &Database,
    model: &str,
) -> RunSummary {
    let run_at = now_string();

    let stored = match db.hiking_generated_notes() {
        Ok(v) => v,
        Err(e) => {
            let s = RunSummary {
                ok: false,
                generated: 0,
                skipped: 0,
                deleted: 0,
                message: Some(format!("failed to load generated notes: {e}")),
            };
            record(db, &run_at, &s);
            return s;
        }
    };
    let stored_hash: HashMap<(String, i64), String> = stored
        .into_iter()
        .map(|g| ((g.subject_type, g.subject_id), g.fact_sheet_hash))
        .collect();

    let mut generated = 0usize;
    let mut skipped = 0usize;

    for subj in subjects {
        let key = (subj.subject_type.clone(), subj.subject_id);
        if stored_hash.get(&key).is_some_and(|h| *h == subj.fact_hash) {
            skipped += 1;
            continue;
        }
        let note = match client.chat(&subj.system, &subj.user).await {
            Ok(n) => n,
            Err(e) => {
                let s = RunSummary {
                    ok: false,
                    generated,
                    skipped,
                    deleted: 0,
                    message: Some(format!(
                        "LLM call failed for {} {}: {e}",
                        subj.subject_type, subj.subject_id
                    )),
                };
                record(db, &run_at, &s);
                return s;
            }
        };
        let gn = GeneratedNote {
            subject_type: subj.subject_type.clone(),
            subject_id: subj.subject_id,
            note,
            fact_sheet_hash: subj.fact_hash.clone(),
            model: model.to_string(),
            generated_at: run_at.clone(),
        };
        if let Err(e) = db.upsert_hiking_generated_note(&gn) {
            let s = RunSummary {
                ok: false,
                generated,
                skipped,
                deleted: 0,
                message: Some(format!("failed to store generated note: {e}")),
            };
            record(db, &run_at, &s);
            return s;
        }
        generated += 1;
    }

    let keep: Vec<(String, i64)> = subjects
        .iter()
        .map(|s| (s.subject_type.clone(), s.subject_id))
        .collect();
    let deleted = match db.delete_hiking_generated_notes_except(&keep) {
        Ok(n) => n,
        Err(e) => {
            let s = RunSummary {
                ok: false,
                generated,
                skipped,
                deleted: 0,
                message: Some(format!("orphan cleanup failed: {e}")),
            };
            record(db, &run_at, &s);
            return s;
        }
    };

    let s = RunSummary {
        ok: true,
        generated,
        skipped,
        deleted,
        message: None,
    };
    record(db, &run_at, &s);
    s
}

// ---------------------------------------------------------------------------
// Subject enumeration (the garmin.db-dependent part; kept thin, live-validated)
// ---------------------------------------------------------------------------

/// Enumerate every trip, plus the member activities of day/weekend trips only,
/// building each subject's fact sheet + hash + prompt pair.
fn build_subjects(state: &AppState) -> Result<Vec<Subject>, String> {
    let data = crate::hiking::http::load_acts_and_trips(state)
        .map_err(|code| format!("failed to load hiking data: {code}"))?;
    let acts = data.acts;
    let trips = data.trips;
    let garmin_db_path = data.garmin_db_path;

    let user_notes = state
        .db
        .hiking_user_notes()
        .map_err(|e| format!("failed to load user notes: {e}"))?;
    let notes_by_subject: HashMap<(String, i64), (Option<String>, Option<i64>)> = user_notes
        .into_iter()
        .map(|n| ((n.subject_type, n.subject_id), (n.note, n.rating)))
        .collect();

    // All trip-member activities, for within-year ranks on per-hike sheets.
    let member_ids: HashSet<i64> = trips
        .iter()
        .flat_map(|t| t.activity_ids.iter().copied())
        .collect();
    let member_acts: Vec<&HikeActivity> = acts
        .iter()
        .filter(|a| member_ids.contains(&a.activity_id))
        .collect();

    let trip_ranges: Vec<(NaiveDate, NaiveDate)> =
        trips.iter().map(|t| (t.start_date, t.end_date)).collect();

    let mut subjects: Vec<Subject> = Vec::new();

    for trip in &trips {
        // Same widths as `hiking_trip_detail`: 56 pre-days feed the baseline,
        // 22 post-days let the recovery-summary reach day 21.
        let fetch_start = trip.start_date - Duration::days(56);
        let fetch_end = trip.end_date + Duration::days(22);
        let recovery = store::load_recovery(&garmin_db_path, fetch_start, fetch_end)
            .map_err(|e| format!("recovery load failed: {e}"))?;
        let baselines = baseline::baselines(&recovery, &trip_ranges, trip.start_date);
        let recovery_summary =
            baseline::recovery_summary(&recovery, &baselines, trip.start_date, trip.end_date);

        // This trip's member activities and their per-date km/gain.
        let trip_members: Vec<&HikeActivity> = acts
            .iter()
            .filter(|a| trip.activity_ids.contains(&a.activity_id))
            .collect();
        let mut per_date: HashMap<NaiveDate, (f64, f64)> = HashMap::new();
        for a in &trip_members {
            let entry = per_date.entry(a.date).or_insert((0.0, 0.0));
            entry.0 += a.distance_m / 1000.0;
            entry.1 += a.elevation_gain;
        }

        // One DayRow per calendar day in the trip span (rest days carry km=0),
        // joining recovery metrics with member km/gain.
        let mut day_rows: Vec<DayRow> = Vec::new();
        let mut d = trip.start_date;
        while d <= trip.end_date {
            let key = d.to_string();
            let rec = recovery.iter().find(|r| r.date == key);
            let (km, gain_m) = per_date.get(&d).copied().unwrap_or((0.0, 0.0));
            day_rows.push(DayRow {
                date: d,
                km,
                gain_m,
                resting_hr: rec.and_then(|r| r.resting_hr).map(|v| v as f64),
                hrv: rec.and_then(|r| r.hrv_last_night_avg),
            });
            d = d.succ_opt().unwrap();
        }
        let episodes = detect_episodes(&day_rows, &baselines, &EpisodeConfig::default());

        // Trip fact sheet.
        let (t_note, t_rating) = notes_by_subject
            .get(&("trip".to_string(), trip.id))
            .cloned()
            .unwrap_or((None, None));
        let fs = trip_fact_sheet(
            trip,
            &acts,
            &trips,
            &recovery,
            &baselines,
            &recovery_summary,
            &episodes,
            t_rating,
            t_note.as_deref(),
        );
        let is_thru = trip.category == TripCategory::ThruHike;
        let (system, user) = build_prompt(&fs, true, is_thru);
        subjects.push(Subject {
            subject_type: "trip".to_string(),
            subject_id: trip.id,
            fact_hash: fact_hash(&fs),
            system,
            user,
        });

        // Per-hike notes only for day/weekend trips.
        if matches!(trip.category, TripCategory::DayHike | TripCategory::Weekend) {
            for a in &trip_members {
                let year = a.date.year();
                let year_acts: Vec<&HikeActivity> = member_acts
                    .iter()
                    .filter(|m| m.date.year() == year)
                    .copied()
                    .collect();
                let rec_day = recovery.iter().find(|r| r.date == a.date.to_string());
                let (a_note, a_rating) = notes_by_subject
                    .get(&("activity".to_string(), a.activity_id))
                    .cloned()
                    .unwrap_or((None, None));
                let fs = hike_fact_sheet(
                    a,
                    trip,
                    &year_acts,
                    rec_day,
                    &baselines,
                    a_rating,
                    a_note.as_deref(),
                );
                let (system, user) = build_prompt(&fs, false, false);
                subjects.push(Subject {
                    subject_type: "activity".to_string(),
                    subject_id: a.activity_id,
                    fact_hash: fact_hash(&fs),
                    system,
                    user,
                });
            }
        }
    }

    Ok(subjects)
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Process-wide guard so concurrent pipeline runs are refused.
fn running_lock() -> &'static tokio::sync::Mutex<()> {
    static RUNNING: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    RUNNING.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// True while a run holds the process-wide lock (probe via `try_lock`).
pub fn is_running() -> bool {
    running_lock().try_lock().is_err()
}

/// Count subjects whose current fact-sheet hash differs from the stored generated
/// note (or that have no stored note yet). Rebuilds fact sheets, so it needs the
/// garmin db; sub-second on real data.
pub fn stale_count(state: &AppState) -> Result<usize, String> {
    let subjects = build_subjects(state)?;
    let stored: HashMap<(String, i64), String> = state
        .db
        .hiking_generated_notes()
        .map_err(|e| format!("failed to load generated notes: {e}"))?
        .into_iter()
        .map(|g| ((g.subject_type, g.subject_id), g.fact_sheet_hash))
        .collect();
    Ok(subjects
        .iter()
        .filter(|s| {
            stored
                .get(&(s.subject_type.clone(), s.subject_id))
                .map_or(true, |h| *h != s.fact_hash)
        })
        .count())
}

/// Run the pipeline once. Concurrent runs are refused via a process-wide mutex
/// (`try_lock` fail ⇒ "already running", no run recorded).
pub async fn run_notes(state: &AppState) -> RunSummary {
    let lock = running_lock();
    let _guard = match lock.try_lock() {
        Ok(g) => g,
        Err(_) => {
            return RunSummary {
                ok: false,
                generated: 0,
                skipped: 0,
                deleted: 0,
                message: Some("already running".to_string()),
            }
        }
    };

    let cfg = match llm_config() {
        Some(c) => c,
        None => {
            return RunSummary {
                ok: false,
                generated: 0,
                skipped: 0,
                deleted: 0,
                message: Some("LLM endpoint not configured".to_string()),
            }
        }
    };

    let subjects = match build_subjects(state) {
        Ok(s) => s,
        Err(msg) => {
            let s = RunSummary {
                ok: false,
                generated: 0,
                skipped: 0,
                deleted: 0,
                message: Some(msg),
            };
            record(&state.db, &now_string(), &s);
            return s;
        }
    };

    let client = match HttpChatClient::new(&cfg) {
        Ok(c) => c,
        Err(msg) => {
            let s = RunSummary {
                ok: false,
                generated: 0,
                skipped: 0,
                deleted: 0,
                message: Some(msg),
            };
            record(&state.db, &now_string(), &s);
            return s;
        }
    };

    reconcile(&subjects, &client, &state.db, &cfg.model).await
}

/// Duration from now until the next local 03:00.
fn until_next_0300() -> StdDuration {
    let now = chrono::Local::now().naive_local();
    let today_0300 = now.date().and_hms_opt(3, 0, 0).unwrap();
    let target = if now < today_0300 {
        today_0300
    } else {
        (now.date() + Duration::days(1)).and_hms_opt(3, 0, 0).unwrap()
    };
    target
        .signed_duration_since(now)
        .to_std()
        .unwrap_or(StdDuration::from_secs(0))
}

/// True if `run_at` (a stored timestamp string) is more than `days` old, or unparseable.
fn run_older_than_days(run_at: &str, days: i64) -> bool {
    let parsed = NaiveDateTime::parse_from_str(run_at, "%Y-%m-%d %H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(run_at, "%Y-%m-%d %H:%M:%S"));
    match parsed {
        Ok(dt) => {
            chrono::Local::now()
                .naive_local()
                .signed_duration_since(dt)
                .num_days()
                > days
        }
        Err(_) => true,
    }
}

/// Nightly loop: wake at the next local 03:00 and, if the LLM is configured and the
/// newest successful run is > 6 days old (or none exists), run the pipeline. The
/// spawn is wired from `main.rs` in Task 5.
pub async fn scheduler(state: AppState) {
    loop {
        tokio::time::sleep(until_next_0300()).await;

        if llm_config().is_none() {
            continue;
        }

        let due = match state.db.last_hiking_notes_runs(10) {
            Ok(runs) => match runs.iter().find(|r| r.ok) {
                Some(r) => run_older_than_days(&r.run_at, 6),
                None => true,
            },
            Err(e) => {
                tracing::warn!(error = %e, "failed to read last notes runs; running anyway");
                true
            }
        };
        if !due {
            continue;
        }

        let summary = run_notes(&state).await;
        tracing::info!(
            ok = summary.ok,
            generated = summary.generated,
            skipped = summary.skipped,
            deleted = summary.deleted,
            message = summary.message.as_deref().unwrap_or(""),
            "hiking notes scheduled run finished"
        );
    }
}

// ---------------------------------------------------------------------------
// Tests: the reconcile helper with a temp DuckDB + a mock chat client. No HTTP,
// no garmin.db. The garmin-dependent enumeration is covered by Task 8 live runs.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Counts requests; never touches the network.
    struct MockClient {
        calls: Arc<AtomicUsize>,
    }
    impl ChatClient for MockClient {
        async fn chat(&self, _system: &str, _user: &str) -> Result<String, String> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(format!("generated note #{n}"))
        }
    }

    /// Errors on every call (to exercise the abort path).
    struct FailingClient;
    impl ChatClient for FailingClient {
        async fn chat(&self, _system: &str, _user: &str) -> Result<String, String> {
            Err("boom".to_string())
        }
    }

    fn temp_db() -> Database {
        let dir = std::env::temp_dir().join(format!(
            "fitdash-pipeline-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Database::new(dir.join("t.duckdb").to_str().unwrap()).unwrap()
    }

    fn subj(t: &str, id: i64, hash: &str) -> Subject {
        Subject {
            subject_type: t.to_string(),
            subject_id: id,
            fact_hash: hash.to_string(),
            system: "sys".to_string(),
            user: "usr".to_string(),
        }
    }

    #[tokio::test]
    async fn reconcile_generates_skips_regenerates_and_cleans() {
        let db = temp_db();
        let calls = Arc::new(AtomicUsize::new(0));
        let client = MockClient { calls: calls.clone() };

        // (1) First run generates all N and records them.
        let subjects = vec![
            subj("trip", 1, "h1"),
            subj("activity", 2, "h2"),
            subj("activity", 3, "h3"),
        ];
        let s1 = reconcile(&subjects, &client, &db, "m1").await;
        assert!(s1.ok);
        assert_eq!(s1.generated, 3);
        assert_eq!(s1.skipped, 0);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(db.hiking_generated_notes().unwrap().len(), 3);

        // (2) Second run with identical hashes skips N (counter unchanged).
        let s2 = reconcile(&subjects, &client, &db, "m1").await;
        assert!(s2.ok);
        assert_eq!(s2.generated, 0);
        assert_eq!(s2.skipped, 3);
        assert_eq!(calls.load(Ordering::SeqCst), 3, "no LLM calls on a clean run");

        // (3) One subject's hash changes ⇒ regenerate exactly 1.
        let subjects2 = vec![
            subj("trip", 1, "h1"),
            subj("activity", 2, "h2-CHANGED"),
            subj("activity", 3, "h3"),
        ];
        let s3 = reconcile(&subjects2, &client, &db, "m1").await;
        assert!(s3.ok);
        assert_eq!(s3.generated, 1);
        assert_eq!(s3.skipped, 2);
        assert_eq!(calls.load(Ordering::SeqCst), 4);

        // (4) A dropped subject is orphan-cleaned.
        let subjects3 = vec![subj("trip", 1, "h1"), subj("activity", 2, "h2-CHANGED")];
        let s4 = reconcile(&subjects3, &client, &db, "m1").await;
        assert!(s4.ok);
        assert_eq!(s4.deleted, 1);
        let remaining = db.hiking_generated_notes().unwrap();
        assert_eq!(remaining.len(), 2);
        assert!(!remaining
            .iter()
            .any(|g| g.subject_type == "activity" && g.subject_id == 3));

        // Every run was recorded (4 rows), newest first.
        let runs = db.last_hiking_notes_runs(10).unwrap();
        assert_eq!(runs.len(), 4);
        assert!(runs.iter().all(|r| r.ok));
    }

    #[tokio::test]
    async fn reconcile_aborts_and_records_on_llm_error() {
        let db = temp_db();
        let subjects = vec![subj("trip", 1, "h1")];
        let s = reconcile(&subjects, &FailingClient, &db, "m1").await;
        assert!(!s.ok);
        assert_eq!(s.generated, 0);
        assert!(s.message.unwrap().contains("boom"));
        assert!(db.hiking_generated_notes().unwrap().is_empty());
        let runs = db.last_hiking_notes_runs(10).unwrap();
        assert_eq!(runs.len(), 1);
        assert!(!runs[0].ok);
    }

    #[test]
    fn strip_think_removes_block_and_trims() {
        assert_eq!(strip_think("<think>reasoning</think>  Hello."), "Hello.");
        assert_eq!(strip_think("  plain  "), "plain");
    }
}
