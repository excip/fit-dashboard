# Hike Notes (Local LLM + Journal) — Design

Date: 2026-07-05
Status: approved (grilled 2026-07-05); PCT prompt validation in progress
Depends on: recovery baselines (`2026-07-03-recovery-baseline-design.md`) — baselines,
deviations, and recovery tolerances are inputs here.

## Goal

Two features that share one storage layer:

1. **Journal:** manual note + 1–5 star rating on every hike and trip.
2. **Generated notes:** a local LLM turns per-trip/per-hike **fact sheets** into short
   narrative summaries ("big climbing day for you — 3rd biggest this year; HRV took
   until Thursday to recover"), visible in the app next to the deterministic verdict.

Core principle: **the model never sees raw FIT data.** The Rust backend computes a
compact fact sheet (stats, baseline deviations, detected episodes, comparisons to the
user's own history, the user's manual note/rating); the LLM only writes prose from it.
Numbers stay honest; prompts stay small; a 4-hour day hike and the 155-day PCT differ
only in fact-sheet content. Narrative synthesis, not analysis; no coaching advice.

## Granularity

- **One LLM note per trip**, all categories.
- **Per-hike LLM notes only for day-hike and weekend-trip members** (1–3 activities).
  Thru-hike member days get none in v1 (an on-demand per-day "generate" button is
  v1.5).
- **Manual note + rating on every hike and every trip**, thru-hike days included.

Manual and generated notes are **separate fields**: the user's words are never touched
by the pipeline; generated notes are disposable and regenerate freely (never
hand-edited).

## Topology

- **CT 115 (Rust backend) orchestrates.** A web-only scheduled job plus a manual
  trigger build fact sheets and call the LLM over an OpenAI-compatible API.
- **Mac mini M4 Pro (24 GB) serves inference only**, via **LM Studio** headless
  service (`lms server start`, login item): JIT model load on first request, idle-TTL
  auto-unload after. No state, no scheduling on the Mac. Mac must not sleep (or must
  wake on network).
- **Model: Qwen3 14B Q4_K_M** (~10 GB), fallback Qwen3.5 9B — A/B on the PCT fact
  sheet before committing. GLM-5.2 was ruled out: 753B MoE, ~245 GB at 2-bit, no
  Air/Flash variant exists (verified 2026-07-05). 24 GB unified memory ⇒ ~18 GB usable
  ⇒ 7–20B class at Q4. Notes are generated in **English**.

## Staleness: content-hash regeneration

Each generated note stores the SHA-256 of the canonical-JSON fact sheet that produced
it. Every run (scheduled or manual) rebuilds all fact sheets, compares hashes, and
regenerates **only** where the hash changed or no note exists. This one rule covers:

- a new hike joining an existing trip (mid-thru-hike: note reflects "the trip so far"),
- a manual note/rating added after generation (manual note is part of the fact sheet),
- reclassification / re-clustering after settings or override changes.

Generated notes whose subject no longer exists in the current effective trip list are
deleted at the end of each run (orphan cleanup). Unchanged history ⇒ zero LLM calls.

## Episode detection (thru-hikes)

New pure module `src-tauri/src/hiking/episodes.rs` — deterministic, thresholds in
settings. The LLM narrates episodes; it does not find them. Two types over per-day
baseline deviations (RHR, HRV) and per-day load (km, gain):

- **Strain dip:** ≥ 3 consecutive days with RHR ≥ baseline + 5 bpm **or**
  HRV ≤ baseline × 0.90. Runs separated by a single okay day are merged.
  Severity = peak deviation. Reported with start/end trip-day and dates.
- **Strong stretch:** ≥ 3 consecutive days with daily load in the **trip's top
  quartile** (km or gain) **while** RHR/HRV stay within the existing recovery
  tolerances (RHR ≤ baseline + 2, HRV ≥ baseline × 0.95). Load is relative to the
  trip, not absolute (a 30 km PCT day is routine; the same on a weekend trip is huge).

Cap: top 3 per type per trip (by severity/duration). Validated against PCT 2022:
Jul 4–8 is a detectable strong stretch (36–45 km days, ≤1.9 km gain, RHR at/below
baseline 48); Jul 9–14 a strain dip (RHR 50→56, zero-day on the 13th at peak).
Single brutal days never become episodes — they surface as superlatives instead.

## Fact sheet

Canonical JSON (sorted keys — hash stability). Per trip: category, dates, nights,
totals, superlatives, baselines, recovery summary (peak deviation, days-to-recover),
episodes, history comparisons (rank by km/gain among all the user's trips and within
its year), user rating + manual note if present. Per hike (day/weekend members):
day stats, deviations, rank of the day within the user's year, user note/rating.
For thru-hikes add monthly aggregates (e.g. monthly avg RHR drift) instead of per-day
rows — prompts stay a few KB even for the PCT.

## Storage (DuckDB, additive)

```sql
CREATE TABLE hiking_user_notes (
  subject_type TEXT NOT NULL,          -- 'trip' | 'activity'
  subject_id   BIGINT NOT NULL,        -- trip.id (= first member activity_id) or activity_id
  note         TEXT,
  rating       TINYINT,                -- 1..5, NULL = unrated
  updated_at   TIMESTAMP NOT NULL,
  PRIMARY KEY (subject_type, subject_id)
);
CREATE TABLE hiking_generated_notes (
  subject_type    TEXT NOT NULL,
  subject_id      BIGINT NOT NULL,
  note            TEXT NOT NULL,
  fact_sheet_hash TEXT NOT NULL,
  model           TEXT NOT NULL,
  generated_at    TIMESTAMP NOT NULL,
  PRIMARY KEY (subject_type, subject_id)
);
```

Settings (existing hiking settings mechanism): `llm_endpoint`
(`http://<mac-mini>:1234/v1`), `llm_model`, `llm_enabled`, episode thresholds.

## Scheduling + API

- **Scheduler (web-only, cfg-gated like other HTTP wiring):** tokio task fires nightly
  at 03:00; runs the pipeline if the last *successful* run is > 6 days old. A failed
  run (LM Studio unreachable, Mac asleep) is skipped and retried the next night —
  effective cadence weekly, self-healing daily.
- `POST /api/hiking/notes/run` — manual trigger ("Generate now"), returns run summary.
- `GET  /api/hiking/notes/status` — last successful run, per-run counts, stale count.
- `PUT  /api/hiking/user-note` — upsert manual note/rating for a subject.
- Trip detail and trip list payloads gain the note/rating fields (additive).

All behind existing `X-Session` auth.

## UI

- **Trip detail:** generated narrative below the header/verdict block (verdict = exact
  numbers, narrative = story). Manual note + rating inline-editable (existing inline
  pattern, no dialogs). Day/weekend trips: per-hike generated notes under each member
  activity row; manual note fields on all member rows (thru-hikes too, sans LLM
  sibling).
- **Trip lists:** compact star rating per row; no note text.
- **Hiking settings:** last run, notes current/stale counts, "Generate now" button.
  No per-trip generation controls in v1.

## Prompting

System prompt constraints: use **only** the facts provided; never invent numbers,
places, or weather; 120–180 words per note (thru-hikes 150–250); address the user as
"you"; if the user's manual note is present, weave its content in as explanation for
the data. Prompt builder is a pure function → snapshot-testable.

## First run + failure behavior

- **Full-history backfill** on first enable (one overnight run; PCT narrative is the
  acceptance test).
- Failure: stale notes remain visible; settings panel shows last successful run and
  stale count; nightly retry. No notification channel in v1.

## Testing

- `episodes.rs`: strict TDD, synthetic fixtures — merge-across-one-day, 3-day minimum,
  quartile edges, missing HRV (pre-2022), cap at top 3.
- Fact sheet: canonical-JSON hash stability test (key order, float formatting);
  golden-file test built from PCT 2022 aggregates.
- Prompt builder: snapshot tests.
- Pipeline: mock OpenAI-endpoint test (regenerate-on-hash-change, skip-on-match,
  orphan cleanup); LM Studio integration behind an `--ignored` test.
- Existing gates stay green: `cargo test --features web`, `cargo check --features
  tauri-app` (LLM pipeline + scheduler are web-only cfg-gated).

## Explicit non-goals (v1)

- Episode annotations on the recovery charts (v2).
- Per-day on-demand notes for thru-hike members (v1.5 button).
- Coaching / training advice, multi-language notes, notifications on failure,
  hand-editing generated notes.

## Chosen defaults (overridable later)

- Episodes: 3-day minimum, merge across 1-day gaps, dip = RHR +5 bpm / HRV −10%,
  strong stretch = top-quartile load within recovery tolerances, top 3 per type.
- Schedule: nightly 03:00 check, weekly effective cadence.
- Note length 120–180 words (150–250 thru-hike), English, second person.
- Model Qwen3 14B Q4_K_M via LM Studio JIT/TTL; endpoint + model name in settings.
