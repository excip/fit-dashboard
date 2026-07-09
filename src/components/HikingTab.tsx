import { useEffect, useState } from "react";
import { api } from "../lib/api";
import type { HikingOverview, NotesStatus, Trip, TripCategory } from "../types";
import { HikingTripDetail } from "./HikingTripDetail";
import { HikingAtlas } from "./HikingAtlas";

const KM = (m: number) => (m / 1000).toFixed(0);

const fmtRunDate = (iso: string | null | undefined): string => {
  if (!iso) return "never";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "never";
  return d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
};

function ReadonlyStars({ rating }: { rating: number }) {
  return (
    <span className="hiking-stars readonly small" aria-label={`${rating} of 5 stars`}>
      {[1, 2, 3, 4, 5].map((n) => (
        <span key={n} className={n <= rating ? "on" : ""}>{n <= rating ? "★" : "☆"}</span>
      ))}
    </span>
  );
}

type Props = { onOpenActivity?: (dashboardActivityId: number) => void };

export function HikingTab({ onOpenActivity }: Props) {
  const [year, setYear] = useState<number | null>(null);
  const [ov, setOv] = useState<HikingOverview | null>(null);
  const [trips, setTrips] = useState<Trip[]>([]);
  const [selectedTripId, setSelectedTripId] = useState<number | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const [editingTripId, setEditingTripId] = useState<number | null>(null);
  const [tripNameDraft, setTripNameDraft] = useState("");
  const [savingName, setSavingName] = useState(false);
  const [notesStatus, setNotesStatus] = useState<NotesStatus | null>(null);
  const [generating, setGenerating] = useState(false);
  const [atlasOpen, setAtlasOpen] = useState(false);

  const running = (notesStatus?.running ?? false) || generating;

  async function generateNotes() {
    if (running) return;
    setGenerating(true);
    try {
      await api.hikingNotesRun();
    } catch {
      setGenerating(false);
    }
  }

  async function saveTripName(tripId: number) {
    if (savingName) return;
    const name = tripNameDraft.trim();
    setSavingName(true);
    try {
      await api.hikingSetTripName(tripId, name.length > 0 ? name : null);
      setEditingTripId(null);
      setRefreshKey((k) => k + 1);
    } finally {
      setSavingName(false);
    }
  }

  useEffect(() => {
    let cancelled = false;
    Promise.all([
      api.hikingOverview(year ?? undefined).catch(() => null),
      api.hikingTrips(undefined, year ?? undefined).catch((): Trip[] => []),
    ]).then(([o, ts]) => {
      if (!cancelled) {
        setOv(o);
        setTrips(ts);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [year, refreshKey]);

  useEffect(() => {
    let cancelled = false;
    api.hikingNotesStatus().then((s) => { if (!cancelled) setNotesStatus(s); }).catch(() => {});
    return () => { cancelled = true; };
  }, []);

  useEffect(() => {
    if (!running) return;
    const id = setInterval(() => {
      api.hikingNotesStatus().then((s) => {
        setNotesStatus(s);
        if (!s.running) {
          setGenerating(false);
          setRefreshKey((k) => k + 1);
        }
      }).catch(() => {});
    }, 5000);
    return () => clearInterval(id);
  }, [running]);

  const years = Array.from({ length: 2026 - 2017 + 1 }, (_, i) => 2017 + i);
  const byCat = (c: TripCategory) => trips.filter((t) => t.category === c);

  if (selectedTripId != null) {
    return (
      <HikingTripDetail
        tripId={selectedTripId}
        onBack={() => { setSelectedTripId(null); setRefreshKey((k) => k + 1); }}
        onTripChanged={(id) => { setSelectedTripId(id); setRefreshKey((k) => k + 1); }}
        onOpenActivity={onOpenActivity}
      />
    );
  }

  return (
    <div className="hiking-tab">
      <div className="hiking-header">
        <div className="hiking-years">
          <button className={year === null ? "active" : ""} aria-pressed={year === null} onClick={() => setYear(null)}>All</button>
          {years.map((y) => (
            <button key={y} className={year === y ? "active" : ""} aria-pressed={year === y} onClick={() => setYear(y)}>{y}</button>
          ))}
        </div>

        {notesStatus && (
          <div className="hiking-notes-control">
            {notesStatus.enabled ? (
              <>
                <span className="hiking-notes-status">
                  Notes: {notesStatus.stale} need update · last run {fmtRunDate(notesStatus.last_runs[0]?.run_at)}
                </span>
                <button className="hiking-notes-run" onClick={() => void generateNotes()} disabled={running}>
                  {running ? "Generating…" : "Generate now"}
                </button>
              </>
            ) : (
              <span className="hiking-notes-status">Notes: disabled — set FIT_DASHBOARD_LLM_ENDPOINT</span>
            )}
          </div>
        )}
      </div>

      <button className="atlas-door" onClick={() => setAtlasOpen(true)}>
        <svg className="atlas-door-trail" viewBox="0 0 340 64" preserveAspectRatio="none" aria-hidden="true">
          <path d="M4 54 C 58 50, 86 18, 132 24 S 214 56, 256 34 S 316 10, 336 8" fill="none" />
          <circle cx="336" cy="8" r="2.5" />
        </svg>
        <span className="atlas-door-text">
          <span className="atlas-door-eyebrow">The Atlas</span>
          <span className="atlas-door-title">Every trail you've walked, on one map</span>
        </span>
        <span className="atlas-door-arrow" aria-hidden="true">→</span>
      </button>

      {atlasOpen && (
        <HikingAtlas
          onClose={() => setAtlasOpen(false)}
          onOpenTrip={(id) => {
            setAtlasOpen(false);
            setSelectedTripId(id);
          }}
        />
      )}

      {ov && (
        <>
          <div className="stats-row">
            <div className="stat-card">
              <div className="stat-value">{KM(ov.total_distance_m)} <small>km</small></div>
              <div className="stat-label">distance</div>
            </div>
            <div className="stat-card">
              <div className="stat-value">{ov.total_steps.toLocaleString()}</div>
              <div className="stat-label">steps</div>
            </div>
            <div className="stat-card">
              <div className="stat-value">+{Math.round(ov.total_gain).toLocaleString()} <small>m</small></div>
              <div className="stat-label">elev gain</div>
            </div>
            <div className="stat-card">
              <div className="stat-value">-{Math.round(ov.total_loss).toLocaleString()} <small>m</small></div>
              <div className="stat-label">elev loss</div>
            </div>
          </div>
          <p className="stat-meta">{ov.hike_count} hikes · {ov.trip_count} trips</p>
        </>
      )}

      {(["thru_hike", "weekend", "day_hike"] as TripCategory[]).map((cat) => (
        <section key={cat} className="panel hiking-section">
          <h3>
            {cat === "thru_hike"
              ? "Thru-hikes (3+ nights)"
              : cat === "weekend"
              ? "Weekend trips (1–2 nights)"
              : "Day hikes"}
          </h3>
          {byCat(cat).map((t) => {
            const editing = editingTripId === t.id;
            return (
              <div
                key={t.id}
                className={`trip-row${editing ? "" : " clickable"}`}
                onClick={() => { if (!editing) setSelectedTripId(t.id); }}
              >
                {editing ? (
                  <span className="hiking-name-edit trip-name" onClick={(e) => e.stopPropagation()}>
                    <input
                      value={tripNameDraft}
                      onChange={(e) => setTripNameDraft(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") void saveTripName(t.id);
                        if (e.key === "Escape") setEditingTripId(null);
                      }}
                      placeholder="Trip name"
                      autoFocus
                      disabled={savingName}
                    />
                    <button onClick={() => void saveTripName(t.id)} disabled={savingName}>Save</button>
                    <button onClick={() => setEditingTripId(null)}>Cancel</button>
                  </span>
                ) : (
                  <span className="trip-name">
                    {t.name ?? "—"}
                    <button
                      className="hiking-rename"
                      title="Rename trip"
                      onClick={(e) => {
                        e.stopPropagation();
                        setTripNameDraft(t.name ?? "");
                        setEditingTripId(t.id);
                      }}
                    >
                      ✎
                    </button>
                  </span>
                )}
                <span className="trip-dates">
                  {t.start_date}{t.nights > 0 ? ` → ${t.end_date}` : ""}
                </span>
                <span className="trip-nights">{t.nights > 0 ? `${t.nights}n` : "—"}</span>
                <span className="trip-km">{KM(t.total_distance_m)} km</span>
                <span className="trip-gain">▲{Math.round(t.total_gain)}</span>
                {t.user_rating != null && <ReadonlyStars rating={t.user_rating} />}
              </div>
            );
          })}
          {byCat(cat).length === 0 && <p className="empty">None</p>}
        </section>
      ))}
    </div>
  );
}
