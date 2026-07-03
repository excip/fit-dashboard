import { useEffect, useState } from "react";
import { api } from "../lib/api";
import type { HikingOverview, Trip, TripCategory } from "../types";
import { HikingTripDetail } from "./HikingTripDetail";

const KM = (m: number) => (m / 1000).toFixed(0);

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
      <div className="hiking-years">
        <button className={year === null ? "active" : ""} aria-pressed={year === null} onClick={() => setYear(null)}>All</button>
        {years.map((y) => (
          <button key={y} className={year === y ? "active" : ""} aria-pressed={year === y} onClick={() => setYear(y)}>{y}</button>
        ))}
      </div>

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
              </div>
            );
          })}
          {byCat(cat).length === 0 && <p className="empty">None</p>}
        </section>
      ))}
    </div>
  );
}
