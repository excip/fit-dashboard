import { useEffect, useState } from "react";
import { api } from "../lib/api";
import type { HikingOverview, Trip, TripCategory } from "../types";

const KM = (m: number) => (m / 1000).toFixed(0);

export function HikingTab() {
  const [year, setYear] = useState<number | null>(null);
  const [ov, setOv] = useState<HikingOverview | null>(null);
  const [trips, setTrips] = useState<Trip[]>([]);

  useEffect(() => {
    void api.hikingOverview(year ?? undefined).then(setOv);
    void api.hikingTrips(undefined, year ?? undefined).then(setTrips);
  }, [year]);

  const years = Array.from({ length: 2026 - 2017 + 1 }, (_, i) => 2017 + i);
  const byCat = (c: TripCategory) => trips.filter((t) => t.category === c);

  return (
    <div className="hiking-tab">
      <div className="hiking-years">
        <button className={year === null ? "active" : ""} onClick={() => setYear(null)}>All</button>
        {years.map((y) => (
          <button key={y} className={year === y ? "active" : ""} onClick={() => setYear(y)}>{y}</button>
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
          {byCat(cat).map((t) => (
            <div key={t.id} className="trip-row">
              <span className="trip-dates">
                {t.start_date}{t.nights > 0 ? ` → ${t.end_date}` : ""}
              </span>
              <span className="trip-nights">{t.nights > 0 ? `${t.nights}n` : "—"}</span>
              <span className="trip-km">{KM(t.total_distance_m)} km</span>
              <span className="trip-gain">▲{Math.round(t.total_gain)}</span>
            </div>
          ))}
          {byCat(cat).length === 0 && <p className="empty">None</p>}
        </section>
      ))}
    </div>
  );
}
