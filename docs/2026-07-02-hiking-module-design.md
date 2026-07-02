# fit-dashboard Hiking Module — Design

**Date:** 2026-07-02
**Status:** Approved (design), pending implementation plan
**Context:** Feature fork of [fit-dashboard](https://github.com/arpanghosh8453/fit-dashboard)
running on CT 115 `fitness` (see `[[services/fitness]]` in the vault). Follow-up to the
Garmin analytics stack; realizes the "hiking module + customizability" from `OPEN_ITEMS` vk:12.

> **Repo note:** this spec is authored in the homelab repo for convenience; it should be
> copied into the fit-dashboard fork repo when that is set up (likely developed on the Mac —
> see the closing "Development environment" section).

## Goal

Add a **Hiking** tab to fit-dashboard that turns the user's Garmin history into hiking
analytics: yearly/overall stats, trips grouped by length (day / weekend / thru-hike), and a
thru-hike drill-down with per-day load, superlatives, and multi-day recovery trends.

## Data source & architecture (Approach A)

The rich data lives in **`garmin.db`** (SQLite, `/opt/appdata/fitness/givemydata/garmin.db`) —
its `activity` table already carries `distance_meters`, `elevation_gain`, `elevation_loss`,
`activity_steps`, `start/end_latitude/longitude`, `max_elevation`, `location_name`,
`duration_seconds`, `activity_type`; and daily health tables (`sleep`, `stress`, `body_battery`,
`hrv`, `training_readiness`, `heart_rate`) hold recovery data by `calendar_date`. fit-dashboard's
own DuckDB lacks elevation gain/steps, so the module reads `garmin.db` directly.

- **`garmin.db` is mounted read-only** into the fit-dashboard container and opened from the Rust
  backend via `rusqlite`.
- Hiking computations (classify → cluster → aggregate → recovery) run **server-side, on demand**,
  cached in memory (dataset is ~1,500 activities; sub-second).
- **Overrides & settings** are stored in fit-dashboard's **own DuckDB** (new tables) so a
  `garmin.db` re-sync never clobbers user edits.
- Individual hikes link to fit-dashboard's **existing single-activity map/telemetry view** via
  `activity_id` (embedded in the FIT filename: `garmin.db` `activity.activity_id` ↔ the fit-dashboard
  `activities.file_name` of the form `<activity_id>_ACTIVITY.fit`).

## Home location

Auto-derived once as the centroid of the **densest cluster of activity start points** (the user's
data centers on Aarau/Küttigen, ~47.4°N 8.05°E). Stored as a setting and **overridable**. Used only
to *group/label* trips (distance-from-home), **not** to classify hikes.

## Hike classification

**Candidate pool** (foot/trail sports only): `hiking`, `walking`, `snow_shoe`. Excluded:
`trail_running` (a run), `running`, `cycling`, `yoga`, swimming, etc.

An activity in the pool is a **HIKE** if **any** of (all thresholds tunable in settings):
- `activity_type == "hiking"` (trusts future hiking-baseline recordings), **or**
- `elevation_gain > 250 m` (default; the primary reliable signal — home walks are ~20–40 m), **or**
- `distance_meters > 12 km`.

Otherwise it's a **walk** (excluded from the Hiking tab). Rationale: Garmin's label is unreliable
(the entire 2022 PCT is logged as `walking`), and distance-from-home is a poor trigger (a flat
vacation walk is far from home but isn't a hike), so elevation/distance drive classification.

**Manual override:** a per-activity override can force an activity to `hike` or `walk`, overriding
the rule. Stored in the overrides table.

## Trip clustering

Sort hikes by `start_time`. Chain hike **B** into the same trip as the preceding hike **A** when
**both**:
- **spatial continuity:** `haversine(A.end, B.start) ≤ link_radius` (default ~7.5 km, tunable 5–10), **and**
- **temporal continuity:** calendar-day gap `≤ 4 rest days` (i.e. `date(B) − date(A) ≤ 5 days`) —
  so town/zero days don't fragment a thru-hike.

A trip's **nights** = `date(last hike) − date(first hike)` in calendar days. Buckets:
- **0 nights** → **Day hike** (a single day may contain multiple hikes),
- **1–2 nights** → **Weekend trip**,
- **3+ nights** → **Thru-hike**.

**Manual override:** merge two trips, or split/reassign an activity's trip membership. Stored in the
overrides table (keyed by `activity_id`).

**Trip superlatives** (from member hikes): longest day (distance), biggest-climb day
(`elevation_gain`), highest point (`max_elevation`), plus totals (km, steps, gain, loss).

## Recovery (trip view)

For each date the trip spans, show daily trends joined from the health tables:
- **Sleep** — score + duration,
- **Resting HR + HRV**,
- **Body Battery + Stress**,
- **Training Readiness**.

Displayed as daily sparklines/trends across the trip, alongside the daily hike load.

## UI

New **Hiking** tab in the existing fit-dashboard style.

**Overview:** year switcher (`All` + each year); four stat cards — **distance, steps, elevation
gain, elevation loss** — for the selected scope; hike/trip counts; a small distance-by-year bar.
Below, three category sections (**Thru-hikes**, **Weekend trips**, **Day hikes**), each a list of
trip rows (name/location, dates, nights, km, gain) that link to a detail view.

**Thru-hike / trip detail:** header totals (dates, nights, km, gain/loss, steps); superlatives row;
stitched multi-day route map (MapLibre, reused); per-day load list (km + gain) where each day links
to the existing single-activity telemetry view; and the four recovery trends across the trip.

(Wireframes captured in the brainstorming session; fidelity is "structure to start with".)

## Backend units (isolation)

- **`classify`** — pure function `activity → HikeClass` given settings + overrides.
- **`cluster`** — pure function `[hike] → [Trip]` given link-radius + rest-day tolerance + overrides.
- **`aggregate`** — year/overall stat rollups + superlatives from `[Trip]`/`[hike]`.
- **`recovery`** — joins trip date-range to daily health tables.
- **`store`** — rusqlite read layer over `garmin.db`; DuckDB read/write for overrides + settings.

**Endpoints:** `GET /api/hiking/overview?year=`, `GET /api/hiking/trips?category=`,
`GET /api/hiking/trip/{trip_id}`, `POST /api/hiking/overrides`, `GET|PUT /api/hiking/settings`.
All behind the existing `X-Session` auth.

## Phasing (each phase independently useful)

1. **Engine + overview** — `garmin.db` read-only mount + rusqlite; `classify`/`cluster`/`aggregate`
   with unit tests; `overview` + `trips` endpoints; Hiking tab with year switcher, stat cards, three
   category lists. No drill-down.
2. **Drill-down + recovery** — `trip/{id}` endpoint; trip detail view (per-day load, superlatives,
   the four recovery trends); day → single-activity link.
3. **Overrides + settings + map** — overrides table + reclassify/merge/split UI; settings panel
   (home, thresholds, link-radius, rest-days); stitched multi-day route map.

## Testing

The `classify` + `cluster` engine is the highest-risk logic and is pure/testable. Pin fixtures to
known truths from the real data:
- **2022 PCT** (all `walking`, 30+ km/day, 500–1,300 m gain, California) → **one ~50-night thru-hike**.
- Aarau/Küttigen home walks (~4–5 km, 20–40 m gain) → **excluded** (walks).
- **Andermatt 2026-06-28** (13 km, ▲724 m) → **day hike**.
- **Blenio 2026-06-26→27** (consecutive, ▲2.6 k) → **weekend trip**.
- A manual override flips a borderline activity and the aggregates update accordingly.

Integration test uses a sampled subset of the real `garmin.db` as a fixture.

## Out of scope (v1)

- Sub-sport granularity beyond hiking (e.g. distinguishing yoga vs strength elsewhere in the app).
- Editing/writing back to Garmin or `garmin.db` (module is read-only against it).
- Route/GPX-based trail matching (uses recorded activities only).

## Development environment

Likely developed on the **Mac** (Rust + Node toolchain, real IDE). The fork needs a copy of
`garmin.db` (copy from CT 115 over Tailscale) for realistic testing, plus optionally the
fit-dashboard DuckDB for the single-activity drill-down link. Deploy path: build the fork's image
and swap `image:` in CT 115's compose from the upstream tag to the local build. A tailored
project-bootstrap prompt for the Mac will be provided after this spec is approved.
