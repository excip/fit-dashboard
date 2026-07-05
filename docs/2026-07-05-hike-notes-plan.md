# Hike Notes (Journal + Local LLM) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Manual journal (note + 1–5 rating on every hike and trip) plus local-LLM narrative notes generated from Rust-built fact sheets, visible in the hiking UI, regenerated weekly by content-hash staleness.

**Architecture:** Two new pure engine modules — `episodes.rs` (strain dips / strong stretches) and `factsheet.rs` (canonical-JSON fact sheets, SHA-256 hashes, prompts) — plus a web-only `notes_pipeline.rs` that calls an OpenAI-compatible endpoint (LM Studio on the Mac mini) and writes to three new DuckDB tables. A scheduler task in `main.rs` fires nightly at 03:00 and runs the pipeline if the last successful run is > 6 days old. Frontend: narrative + journal on trip detail, stars on trip rows, status + "Generate now" in the HikingTab header.

**Tech Stack:** Rust (chrono, serde_json, sha2 — already deps; **reqwest added here**), axum (existing gated `http.rs`), React + existing `src/lib/api.ts` client.

**Spec:** `docs/2026-07-05-hike-notes-design.md` (approved). Model already validated 2026-07-05: `qwen/qwen3-14b` (Q4_K_M) in LM Studio, 45 s cold including load, ~1.1k prompt tokens per trip.

**Spec-to-reality adjustments (verified against the code 2026-07-05):**
- The design says LLM endpooint/model live in "the existing hiking settings mechanism" — **there is none** (`HikingSettings` is `Default`-only, `http.rs:54`). Use env vars instead: `FIT_DASHBOARD_LLM_ENDPOINT` (e.g. `http://localhost:1234/v1`), `FIT_DASHBOARD_LLM_MODEL` (default `qwen/qwen3-14b`). Endpoint unset ⇒ LLM pipeline disabled (status reports it); the journal works regardless.
- The design puts status + "Generate now" in "hiking settings" — `SettingsPanel.tsx` has no hiking section, so they go in the **HikingTab header** next to the year switcher.
- Episode thresholds: fields on a new `EpisodeConfig` with `Default` (same pattern as `HikingSettings`), not persisted settings.

**Conventions that bind every task:**
- Engine code in `src-tauri/src/hiking/` is pure (no `crate::server`/`crate::state`, no cfg gate) and strict TDD.
- Anything importing `AppState` or reqwest sits behind `#[cfg(all(feature = "web", not(feature = "tauri-app")))]` — same gate as `http.rs`. `cargo check --features tauri-app` must stay clean after every task.
- Run cargo with `export PATH="$HOME/.cargo/bin:$PATH"` and `--manifest-path src-tauri/Cargo.toml`.
- UI text: match whatever `HikingTripDetail.tsx` / `HikingTab.tsx` currently do for labels (i18n keys vs literals) — follow the file, don't introduce a new pattern.
- All commits end with the trailer shown in each commit step.

**File structure (whole feature):**
- Create: `src-tauri/src/hiking/episodes.rs` — `EpisodeConfig`, `DayRow`, `Episode`, `detect_episodes()` + tests
- Create: `src-tauri/src/hiking/factsheet.rs` — fact-sheet structs, `trip_fact_sheet()`, `hike_fact_sheet()`, `fact_hash()`, `build_prompt()` + tests
- Create: `src-tauri/src/hiking/notes_pipeline.rs` (gated) — LLM client, `run_notes()`, scheduler loop, mock-endpoint test
- Modify: `src-tauri/src/hiking/mod.rs` — register modules
- Modify: `src-tauri/src/database.rs` — 3 tables + accessors + roundtrip test
- Modify: `src-tauri/src/hiking/http.rs` — 3 new handlers; extend `TripDetail`, `TripDay`, trips-list payload
- Modify: `src-tauri/src/server.rs` — 3 routes (after line 61)
- Modify: `src-tauri/src/main.rs` — spawn scheduler before `axum::serve` (~line 135)
- Modify: `src-tauri/Cargo.toml` — reqwest
- Modify: `src/types.ts`, `src/lib/api.ts`, `src/components/HikingTripDetail.tsx`, `src/components/HikingTab.tsx`, `src/styles.css`

---

### Task 1: Episode detection (`episodes.rs`)

**Files:**
- Create: `src-tauri/src/hiking/episodes.rs`
- Modify: `src-tauri/src/hiking/mod.rs` (add `pub mod episodes;` after `pub mod baseline;`)

Pure function over per-day rows joined by the caller (date, km, gain, resting_hr, hrv) plus the trip's `Baselines`.

```rust
#[derive(Debug, Clone)]
pub struct EpisodeConfig {
    pub min_run_days: i64,        // 3
    pub merge_gap_days: i64,      // 1 (single okay/missing day inside a run)
    pub dip_rhr_over: f64,        // 5.0 (bpm over baseline)
    pub dip_hrv_factor: f64,      // 0.90 (at/below baseline * factor)
    pub ok_rhr_over: f64,         // 2.0  (recovery tolerance, same as baseline.rs)
    pub ok_hrv_factor: f64,       // 0.95
    pub load_quantile: f64,       // 0.75 (top quartile of the trip's hiking days)
    pub max_per_kind: usize,      // 3
}
impl Default for EpisodeConfig { /* the values above */ }

/// One trip day, joined by the caller from RecoveryDay + member activities.
pub struct DayRow { pub date: NaiveDate, pub km: f64, pub gain_m: f64,
                    pub resting_hr: Option<f64>, pub hrv: Option<f64> }

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum EpisodeKind { StrainDip, StrongStretch }

#[derive(Debug, Clone, serde::Serialize)]
pub struct Episode {
    pub kind: EpisodeKind,
    pub start_date: NaiveDate, pub end_date: NaiveDate,
    pub start_trip_day: i64, pub end_trip_day: i64,   // 1-based
    /// Per-day values inside the episode, for explicit date:value prompt rendering.
    /// StrainDip: the strained metric per day; StrongStretch: km per day.
    pub daily: Vec<(NaiveDate, f64)>,
    pub peak_rhr: Option<f64>, pub min_hrv: Option<f64>,
    pub avg_km: f64, pub max_gain_m: f64,
    pub severity: f64,
}

pub fn detect_episodes(days: &[DayRow], baselines: &Baselines, cfg: &EpisodeConfig) -> Vec<Episode>
```

Semantics (each is a test):
- **Dip day:** `resting_hr >= rhr_baseline + dip_rhr_over` OR `hrv <= hrv_baseline * dip_hrv_factor` (each clause only when that baseline AND value exist). **Stretch day:** load in the trip's top quartile (km **or** gain, quantile over days with `km > 0`) AND every *available* strain metric within the ok-tolerances AND at least one strain metric available that day.
- Runs of qualifying days become an episode when length ≥ `min_run_days` after merging runs separated by ≤ `merge_gap_days` non-qualifying days (merged-gap days stay in `daily` with their values; run length counts calendar days start..end).
- No baselines at all (both `None`) ⇒ no episodes of either kind.
- `severity`: dips = `max(rhr_dev / dip_rhr_over, hrv_shortfall_ratio / (1.0 - dip_hrv_factor))` peak within the episode (threshold-normalized so RHR- and HRV-driven dips compare); stretches = episode length in days, tie-break by `avg_km`. Sort each kind by severity desc, truncate to `max_per_kind`.

- [ ] **Step 1: Register the module and write the failing tests** — fixture helper `row(date, km, gain, rhr, hrv)`; baselines fixture rhr=48, hrv=60. Tests: (1) 4-day RHR dip detected with correct trip-day indices and `daily` values; (2) dip merged across a single okay day, `daily` includes the gap day; (3) 2-day dip rejected (min-run); (4) HRV-only dip detected when RHR baseline is `None`; (5) strong stretch: 3 top-quartile days with RHR ≤ 50 detected, `avg_km`/`max_gain_m` correct; (6) stretch broken by one day at RHR 51 (> 48+2) — no episode; (7) both baselines `None` ⇒ empty; (8) 5 disjoint dips ⇒ only top 3 by severity; (9) PCT-shape regression: reconstruct Jul 1–16 2022 from the design doc's real values (RHR 50,52,47,47,48,48,48,46,50,50,52,51,56,54,50,50; km 36.1,36.3,28.1,44.9,43.4,38.2,28.3,36.7,51.3,41.5,42.9,36.1,0,22.8,43.6,42.0; gains from doc) with baseline rhr=48 ⇒ exactly one StrongStretch covering Jul 4–8 and one StrainDip covering Jul 11–14 (Jul 9–10 at RHR 50 are below 48+5; the dip is the 52,51,56,54 run — note the design doc's prose says "Jul 9–14", the detector's honest answer is Jul 11–14, which is fine).
- [ ] **Step 2: Run to verify compile failure** — `cargo test --features web --manifest-path src-tauri/Cargo.toml episodes::`
- [ ] **Step 3: Implement `detect_episodes()`** (day classification → run building → gap merge → severity → cap).
- [ ] **Step 4: Tests pass** — same command, `9 passed`.
- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/hiking/episodes.rs src-tauri/src/hiking/mod.rs
git commit -m "feat(hiking): deterministic episode detection (strain dips, strong stretches)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: DuckDB storage for notes

**Files:**
- Modify: `src-tauri/src/database.rs` (table creation ~line 156 after `hiking_activity_names`; accessors after `set_hiking_activity_name` ~line 835; test after `hiking_override_and_name_roundtrip` ~line 671)

- [ ] **Step 1: Add tables to the startup DDL**

```sql
CREATE TABLE IF NOT EXISTS hiking_user_notes (
    subject_type TEXT NOT NULL,        -- 'trip' | 'activity'
    subject_id   BIGINT NOT NULL,      -- trip.id (= first member GARMIN activity_id) or activity_id
    note         TEXT,
    rating       TINYINT,              -- 1..5, NULL = unrated
    updated_at   TIMESTAMP NOT NULL,
    PRIMARY KEY (subject_type, subject_id)
);
CREATE TABLE IF NOT EXISTS hiking_generated_notes (
    subject_type    TEXT NOT NULL,
    subject_id      BIGINT NOT NULL,
    note            TEXT NOT NULL,
    fact_sheet_hash TEXT NOT NULL,
    model           TEXT NOT NULL,
    generated_at    TIMESTAMP NOT NULL,
    PRIMARY KEY (subject_type, subject_id)
);
CREATE TABLE IF NOT EXISTS hiking_notes_runs (
    run_at    TIMESTAMP NOT NULL,
    ok        BOOLEAN NOT NULL,
    generated INTEGER NOT NULL,
    skipped   INTEGER NOT NULL,
    deleted   INTEGER NOT NULL,
    message   TEXT
);
```

- [ ] **Step 2: Accessors** (follow the delete-then-insert style of `set_hiking_trip_name`):

```rust
pub struct UserNote { pub subject_type: String, pub subject_id: i64,
                      pub note: Option<String>, pub rating: Option<i64> }
pub struct GeneratedNote { pub subject_type: String, pub subject_id: i64, pub note: String,
                           pub fact_sheet_hash: String, pub model: String, pub generated_at: String }
pub struct NotesRun { pub run_at: String, pub ok: bool, pub generated: i64,
                      pub skipped: i64, pub deleted: i64, pub message: Option<String> }

pub fn hiking_user_notes(&self) -> Result<Vec<UserNote>>;
pub fn set_hiking_user_note(&self, subject_type: &str, subject_id: i64,
                            note: Option<&str>, rating: Option<i64>) -> Result<()>; // both None ⇒ DELETE row
pub fn hiking_generated_notes(&self) -> Result<Vec<GeneratedNote>>;
pub fn upsert_hiking_generated_note(&self, n: &GeneratedNote) -> Result<()>;
pub fn delete_hiking_generated_notes_except(&self, keep: &[(String, i64)]) -> Result<usize>; // orphan cleanup
pub fn record_hiking_notes_run(&self, r: &NotesRun) -> Result<()>;
pub fn last_hiking_notes_runs(&self, limit: i64) -> Result<Vec<NotesRun>>;
```

- [ ] **Step 3: Roundtrip test** `hiking_notes_roundtrip` next to the existing one: set/overwrite/clear a user note (rating validation is the handler's job, not the DB's); upsert a generated note twice (hash changes); orphan cleanup keeps only listed subjects; run rows come back newest-first.
- [ ] **Step 4: Verify both builds** — `cargo test --features web --manifest-path src-tauri/Cargo.toml database` and `cargo check --features tauri-app --manifest-path src-tauri/Cargo.toml` (database.rs is shared — table creation must compile in both).
- [ ] **Step 5: Commit** — `feat(hiking): DuckDB storage for journal + generated notes` + trailer.

---

### Task 3: Fact sheets, hashes, prompts (`factsheet.rs`)

**Files:**
- Create: `src-tauri/src/hiking/factsheet.rs`
- Modify: `src-tauri/src/hiking/mod.rs` (register)

Pure module. Structs derive `serde::Serialize` with **fixed field order** (serde emits struct fields in declaration order ⇒ canonical JSON without BTreeMap gymnastics; never put a HashMap in a fact sheet). Round all floats at build time (`(x * 10.0).round() / 10.0`) so hash stability never depends on float formatting.

```rust
pub struct TripFactSheet { /* subject:"trip", category, name, dates{start,end,nights,hiking_days},
    totals{distance_km,elevation_gain_m,elevation_loss_m,steps},
    superlatives{longest_day{date,km,gain_m}, biggest_climb_day{...}, highest_point_m},
    history{rank_by_distance_all_trips, rank_by_gain_all_trips, trips_total},
    baselines (reuse baseline::Baselines),
    recovery_summary (reuse baseline::RecoverySummary),
    monthly_rhr: Vec<(String, f64)>,        // thru-hikes only, else empty
    episodes: Vec<EpisodeFact>,             // rendered detail strings, see below
    user_rating: Option<i64>, user_note: Option<String> */ }

pub struct HikeFactSheet { /* subject:"activity", date, trip_name, trip_category, km, gain_m, loss_m,
    duration_h, max_elevation_m, rank_km_in_year, rank_gain_in_year, activities_in_year,
    day_rhr_dev: Option<f64>, day_hrv_dev: Option<f64>,
    user_rating: Option<i64>, user_note: Option<String> */ }

pub struct EpisodeFact { pub kind: String, pub trip_days: String, pub dates: String, pub detail: String }

pub fn trip_fact_sheet(trip, member_acts, all_trips, recovery_days, baselines, recovery_summary, episodes, user_note) -> TripFactSheet;
pub fn hike_fact_sheet(act, trip, year_acts, recovery_day, baselines, user_note) -> HikeFactSheet;
pub fn fact_hash<T: Serialize>(fs: &T) -> String;                    // hex sha256 of serde_json::to_string
pub fn build_prompt<T: Serialize>(fs: &T, is_trip: bool, is_thru: bool) -> (String, String); // (system, user)
```

**`EpisodeFact.detail` rendering — validation lesson #1 baked in:** always explicit `date: value` pairs, e.g. `"resting HR by day: Jul 11: 52, Jul 12: 51, Jul 13: 56 (peak), Jul 14: 54"` from `Episode.daily`. Never a bare `50 -> 52 -> 56` sequence (the model misdated the peak from exactly that during validation).

**System prompt** (validated wording, lesson #2 included): starts with `/no_think`; rules — use ONLY facts in the JSON, never invent numbers/places/weather/events; address the user as "you"; word budget 120–180 (trip/hike) or 150–250 (thru-hike) as a hard limit; flowing prose, 2–3 paragraphs, no bullets/headers; weave episodes in as turning points with dates; **cover the full arc including how the trip ended and what post-trip data says about the outcome**; if a user note is present, use it to explain the data; at most one brief clause about unrecorded metrics. User message = `"Fact sheet:\n" + canonical JSON`.

- [ ] **Step 1: Failing tests** — (1) `fact_hash` stable across two builds from identical inputs and changes when `user_rating` flips `None→Some(5)`; (2) rank correctness against three synthetic trips; (3) `EpisodeFact.detail` for a dip contains every `date: value` pair and `(peak)` on the max; (4) thru-hike sheet has `monthly_rhr`, weekend sheet has it empty; (5) system prompt contains `/no_think`, the hard word cap for the right category, and the full-arc rule; (6) `HikeFactSheet` day deviations = value − baseline.
- [ ] **Step 2: Verify compile failure.** `cargo test --features web --manifest-path src-tauri/Cargo.toml factsheet::`
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Tests pass** (6 passed).
- [ ] **Step 5: Commit** — `feat(hiking): canonical fact sheets, hashes, prompts for hike notes` + trailer.

---

### Task 4: LLM client + pipeline (`notes_pipeline.rs`, web-only)

**Files:**
- Modify: `src-tauri/Cargo.toml` — add `reqwest = { version = "0.12", default-features = false, features = ["json"] }` (plain-HTTP LAN endpoint; no TLS features needed)
- Create: `src-tauri/src/hiking/notes_pipeline.rs`
- Modify: `src-tauri/src/hiking/mod.rs` — `#[cfg(all(feature = "web", not(feature = "tauri-app")))] pub mod notes_pipeline;`
- Modify: `src-tauri/src/hiking/http.rs` — make `load_acts_and_trips` `pub(crate)` (pipeline reuses it; same cfg gate)

```rust
pub struct LlmConfig { pub endpoint: String, pub model: String }   // from env, None ⇒ disabled
pub fn llm_config() -> Option<LlmConfig>;                          // FIT_DASHBOARD_LLM_ENDPOINT/_MODEL

pub struct RunSummary { pub ok: bool, pub generated: usize, pub skipped: usize,
                        pub deleted: usize, pub message: Option<String> }

pub async fn run_notes(state: &AppState) -> RunSummary;
pub async fn scheduler(state: AppState);   // spawned from main.rs
```

`run_notes` (guarded by a `static RUNNING: OnceLock<tokio::sync::Mutex<()>>` — `try_lock` fail ⇒ return "already running"):
1. Load acts + effective trips (`load_acts_and_trips`), user notes, generated notes.
2. Enumerate subjects: every trip; plus member activities of day/weekend trips only.
3. Per trip: fetch recovery `start−56 .. end+22` (same widths as `hiking_trip_detail`), compute `baselines`, `recovery_summary`, `DayRow`s (join recovery days with per-date member km/gain), `detect_episodes`, then the fact sheet + hash. Per member hike: `hike_fact_sheet` + hash.
4. Diff against stored notes: hash match ⇒ skip; else call the LLM (sequential; request timeout **300 s** — first call bears the cold model load, ~45 s measured; strip any `<think>…</think>` block from the reply; trim) and upsert with model + timestamp.
5. Orphan cleanup: `delete_hiking_generated_notes_except(current subjects)`.
6. Any LLM/store error aborts the run (notes already upserted stay), `ok=false` with the error message. Always `record_hiking_notes_run`.

`scheduler`: loop — sleep until the next local 03:00 (`chrono::Local`, `tokio::time::sleep`); if `llm_config()` is `Some` and the newest `ok` run is older than 6 days (or none exists), `run_notes`. Log the summary via `tracing`.

- [ ] **Step 1: Failing pipeline test (mock endpoint)** — `#[tokio::test] async fn pipeline_generates_skips_and_cleans`: spawn a tiny axum server on an ephemeral port answering `/chat/completions` with a canned completion (count requests via `Arc<AtomicUsize>`); temp-dir DuckDB + the same synthetic garmin.db fixture approach as the existing `--ignored` store tests is NOT needed — instead give the test a tiny real sqlite file built inline with rusqlite (3 activities: one 2-day weekend trip). Assert: first run generates 3 notes (1 trip + 2 hikes); second run generates 0 / skips 3 (request counter unchanged); after `set_hiking_user_note` on the trip, third run regenerates exactly 1; after deleting one activity from the fixture db, orphan cleanup removes its note.
- [ ] **Step 2: Verify failure**, **Step 3: implement**, **Step 4: pass** — `cargo test --features web --manifest-path src-tauri/Cargo.toml notes_pipeline::`; then `cargo check --features tauri-app --manifest-path src-tauri/Cargo.toml` (reqwest + module must not leak into the tauri build).
- [ ] **Step 5: Commit** — `feat(hiking): LLM notes pipeline (hash-diff, orphan cleanup, mock-endpoint test)` + trailer.

---

### Task 5: Endpoints, payloads, scheduler wiring

**Files:**
- Modify: `src-tauri/src/hiking/http.rs`, `src-tauri/src/server.rs` (routes after line 61), `src-tauri/src/main.rs` (before `axum::serve`, ~line 135)

- [ ] **Step 1: Handlers in `http.rs`**
  - `PUT /api/hiking/user-note` — body `{ subject_type: "trip"|"activity", subject_id, note: string|null, rating: number|null }`; reject rating outside 1..=5 and unknown subject_type with 422; upsert via Task 2 accessor.
  - `POST /api/hiking/notes/run` — 409 if disabled (`llm_config() == None`); `tokio::spawn(run_notes)` and return `202 {"started": true}` (a run takes minutes on backfill; the UI polls status).
  - `GET /api/hiking/notes/status` — `{ enabled, model, running, last_runs: [NotesRun; ≤5], stale: usize }`; `stale` = rebuild fact sheets and count hash mismatches (sub-second; reuses the pipeline's enumeration fn — factor it so status and run share it); `running` from the `try_lock` probe.
- [ ] **Step 2: Extend read payloads (additive)** — `TripDetail`: `generated_note: Option<String>`, `user_note: Option<String>`, `user_rating: Option<i64>`; `TripDay` (per-day list): same three fields per member activity (generated only present for day/weekend categories). Trips-list rows: `user_rating: Option<i64>`.
- [ ] **Step 3: Routes in `server.rs`** for the three handlers; **scheduler spawn in `main.rs`**: `tokio::spawn(hiking::notes_pipeline::scheduler(state.clone()));` immediately before the listener bind (web path only — that block already is).
- [ ] **Step 4: Verify** — `cargo test --features web --manifest-path src-tauri/Cargo.toml hiking`, `cargo check --features tauri-app --manifest-path src-tauri/Cargo.toml`.
- [ ] **Step 5: Commit** — `feat(hiking): notes endpoints, payload fields, nightly scheduler` + trailer.

---

### Task 6: Frontend — trip detail journal + narrative

**Files:**
- Modify: `src/types.ts` (extend `TripDetail`, `TripDay`, `Trip`), `src/lib/api.ts` (`hikingSetUserNote`, `hikingNotesRun`, `hikingNotesStatus`), `src/components/HikingTripDetail.tsx`, `src/styles.css`

- [ ] **Step 1: Types + api methods** (follow `hikingSetTripName`'s shape in `api.ts`).
- [ ] **Step 2: Narrative block** — below the verdict block, above the recovery charts: `.hiking-note-generated` panel rendering `detail.generated_note` as paragraphs, with a muted caption line (model + generated date). Render nothing when `null`.
- [ ] **Step 3: Journal block** — under the narrative: five-star row (filled up to `user_rating`, click sets, click same value clears) + auto-growing textarea for `user_note`, saved on blur via `hikingSetUserNote` (inline pattern, no dialogs — same feel as trip rename). Optimistic update, revert on error.
- [ ] **Step 4: Per-day notes** — in the per-day list, when a `TripDay` carries `generated_note`/`user_note`/`user_rating`, render the generated note as collapsible text under the row plus a compact star+note affordance (same components, small variant). Manual entry available on **all** member days (thru-hikes included); generated only appears where the pipeline wrote one.
- [ ] **Step 5: CSS** — `.hiking-note-generated`, `.hiking-journal`, `.hiking-stars` (+ `.small` variant) near the existing `.hiking-verdict` rules; muted panel, no new colors beyond CSS vars.
- [ ] **Step 6: Verify** — `npx tsc --noEmit && npm run build`.
- [ ] **Step 7: Commit** — `feat(hiking): journal + generated-note UI on trip detail` + trailer.

---

### Task 7: Frontend — list stars + HikingTab controls

**Files:**
- Modify: `src/components/HikingTab.tsx`, `src/styles.css`

- [ ] **Step 1: Stars on trip rows** — compact read-only stars (from `user_rating`) on each trip row across all three category sections; nothing when unrated. No inline editing here (design decision: rate from detail only).
- [ ] **Step 2: Notes control in the header** — next to the year switcher: one muted status line from `hikingNotesStatus` ("Notes: 42 current · 3 stale · last run Jul 5" / "Notes: disabled — set FIT_DASHBOARD_LLM_ENDPOINT") and a "Generate now" button (hidden when disabled, spinner + disabled while `running`, poll status every 5 s while a run is active).
- [ ] **Step 3: Verify** — `npx tsc --noEmit && npm run build`.
- [ ] **Step 4: Commit** — `feat(hiking): trip-row ratings + notes status/generate controls` + trailer.

---

### Task 8: Full verification + live validation

**Files:** none (verification only)

- [ ] **Step 1: All gates**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --features web --manifest-path src-tauri/Cargo.toml hiking
GARMIN_DB_TEST=/tmp/garmin.db cargo test --features web --manifest-path src-tauri/Cargo.toml store:: -- --ignored
cargo check --features tauri-app --manifest-path src-tauri/Cargo.toml
npx tsc --noEmit && npm run build
```

- [ ] **Step 2: Live run against LM Studio** — dev happens on the same Mac that serves inference, so the endpoint is local:

```bash
~/.lmstudio/bin/lms server start   # if not already up
FIT_DASHBOARD_GARMIN_DB=/tmp/garmin.db FIT_DASHBOARD_DATA_DIR=/tmp/fit-notes-dev \
  FIT_DASHBOARD_LLM_ENDPOINT=http://localhost:1234/v1 FIT_DASHBOARD_LLM_MODEL=qwen/qwen3-14b \
  cargo run --features web --manifest-path src-tauri/Cargo.toml
```

Trigger "Generate now" in the UI (throwaway data dir ⇒ full backfill; note wall time and per-note latency). Verify: a weekend trip shows trip narrative + per-hike notes; PCT fragments (no merge overrides locally) each get a trip narrative with episodes and NO per-day generated notes; add a rating + manual note to one trip, re-run, confirm exactly that note regenerates and mentions the manual note's content; confirm zero invented numbers on 3 spot-checked notes.
- [ ] **Step 3: A/B `qwen3.5-9b` (optional but recommended)** — `lms get qwen3.5-9b`, flip `FIT_DASHBOARD_LLM_MODEL`, regenerate one PCT narrative (delete its row from `hiking_generated_notes` to force it), compare quality/speed, record the verdict in the design doc's model section.
- [ ] **Step 4: Report.** No deploy in this plan. Deployment to CT 115 is a separate decision and needs two env vars in `/opt/appdata/fitness/docker-compose.yml` pointing at the Mac mini's **Tailscale address**, plus confirming CT 115 → Mac :1234 reachability and Mac sleep settings (`caffeinate` / Energy Saver). Runbook in memory: `fit-hiking-dashboard-project.md`.
