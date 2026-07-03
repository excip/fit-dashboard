import { useCallback, useEffect, useState } from "react";
import ReactECharts from "echarts-for-react";
import { api } from "../lib/api";
import type { RecordPoint, RecoveryDay, TripDetail } from "../types";
import { TripMap } from "./TripMap";

const KM = (m: number) => (m / 1000).toFixed(0);

type Props = {
  tripId: number;
  onBack: () => void;
  onTripChanged: (newTripId: number) => void;
  onOpenActivity?: (dashboardActivityId: number) => void;
};

function recoveryChart(
  recovery: RecoveryDay[],
  series: Array<{ key: keyof RecoveryDay; label: string }>,
  title: string,
) {
  return {
    title: { text: title, textStyle: { fontSize: 12 }, left: 4, top: 2 },
    tooltip: { trigger: "axis" },
    grid: { left: 36, right: 12, top: 30, bottom: 20 },
    xAxis: {
      type: "category",
      data: recovery.map((r) => r.date.slice(5)),
      axisLabel: { fontSize: 9 },
    },
    yAxis: { type: "value", axisLabel: { fontSize: 9 }, scale: true },
    series: series.map((s) => ({
      name: s.label,
      type: "line",
      connectNulls: true,
      showSymbol: false,
      data: recovery.map((r) => r[s.key] as number | null),
    })),
    legend: series.length > 1 ? { bottom: 0, textStyle: { fontSize: 9 } } : undefined,
  };
}

export function HikingTripDetail({ tripId, onBack, onTripChanged, onOpenActivity }: Props) {
  const [detail, setDetail] = useState<TripDetail | null>(null);
  const [tracks, setTracks] = useState<RecordPoint[][]>([]);
  const [editingName, setEditingName] = useState(false);
  const [nameDraft, setNameDraft] = useState("");
  const [busy, setBusy] = useState(false);

  const load = useCallback(() => {
    let cancelled = false;
    api
      .hikingTrip(tripId)
      .then((d) => {
        if (cancelled) return;
        setDetail(d);
        const ids = d.days
          .map((day) => day.dashboard_activity_id)
          .filter((id): id is number => id != null);
        void Promise.all(
          ids.map((id) => api.getRecords(id, 30000).catch((): RecordPoint[] => [])),
        ).then((ts) => {
          if (!cancelled) setTracks(ts.filter((t) => t.length > 0));
        });
      })
      .catch(() => {
        if (!cancelled) setDetail(null);
      });
    return () => {
      cancelled = true;
    };
  }, [tripId]);

  useEffect(() => load(), [load]);

  if (!detail)
    return (
      <div className="hiking-tab">
        <button className="hiking-back" onClick={onBack}>
          ← Back
        </button>
        <p className="empty">Loading…</p>
      </div>
    );

  const t = detail.trip;
  const title =
    t.name ?? `${t.start_date}${t.nights > 0 ? ` → ${t.end_date}` : ""}`;

  async function saveName() {
    const name = nameDraft.trim();
    await api.hikingSetTripName(t.id, name.length > 0 ? name : null);
    setEditingName(false);
    load();
  }

  async function mergePrevious() {
    setBusy(true);
    try {
      const r = await api.hikingMergePrevious(t.id);
      onTripChanged(r.trip_id);
    } finally {
      setBusy(false);
    }
  }

  async function splitTrip() {
    setBusy(true);
    try {
      const r = await api.hikingSplitTrip(t.id);
      onTripChanged(r.trip_id);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="hiking-tab hiking-detail">
      <div className="hiking-detail-header">
        <button className="hiking-back" onClick={onBack}>
          ← Back
        </button>
        {editingName ? (
          <span className="hiking-name-edit">
            <input
              value={nameDraft}
              onChange={(e) => setNameDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") void saveName();
                if (e.key === "Escape") setEditingName(false);
              }}
              placeholder="Trip name"
              autoFocus
            />
            <button onClick={() => void saveName()}>Save</button>
            <button onClick={() => setEditingName(false)}>Cancel</button>
          </span>
        ) : (
          <h2>
            {title}
            <button
              className="hiking-rename"
              title="Rename trip"
              onClick={() => {
                setNameDraft(t.name ?? "");
                setEditingName(true);
              }}
            >
              ✎
            </button>
          </h2>
        )}
        <span className="hiking-detail-actions">
          {detail.has_previous && (
            <button
              disabled={busy}
              onClick={() => void mergePrevious()}
              title="Join this trip onto the trip before it"
            >
              Merge into previous trip
            </button>
          )}
          {detail.merged && (
            <button
              disabled={busy}
              onClick={() => void splitTrip()}
              title="Undo manual merges inside this trip"
            >
              Split merged trips
            </button>
          )}
        </span>
      </div>

      <div className="hiking-stats">
        <div className="stat-card">
          <div className="stat-value">
            {t.start_date}
            {t.nights > 0 ? ` → ${t.end_date}` : ""}
          </div>
          <div className="stat-label">
            {t.nights} nights · {t.activity_ids.length} hikes
          </div>
        </div>
        <div className="stat-card">
          <div className="stat-value">{KM(t.total_distance_m)} km</div>
          <div className="stat-label">distance</div>
        </div>
        <div className="stat-card">
          <div className="stat-value">+{Math.round(t.total_gain).toLocaleString()} m</div>
          <div className="stat-label">elev gain</div>
        </div>
        <div className="stat-card">
          <div className="stat-value">-{Math.round(t.total_loss).toLocaleString()} m</div>
          <div className="stat-label">elev loss</div>
        </div>
        <div className="stat-card">
          <div className="stat-value">{t.total_steps.toLocaleString()}</div>
          <div className="stat-label">steps</div>
        </div>
      </div>

      <div className="hiking-superlatives">
        <span>
          Longest day: <b>{(detail.superlatives.longest_day_m / 1000).toFixed(1)} km</b>
        </span>
        <span>
          Biggest climb: <b>▲{Math.round(detail.superlatives.biggest_climb_m)} m</b>
        </span>
        {detail.superlatives.highest_point_m != null && (
          <span>
            Highest point: <b>{Math.round(detail.superlatives.highest_point_m)} m</b>
          </span>
        )}
      </div>

      {tracks.length > 0 && <TripMap tracks={tracks} />}

      <section className="hiking-section panel">
        <h3>Days</h3>
        {detail.days.map((d) => (
          <div
            key={d.garmin_activity_id}
            className={`trip-row${d.dashboard_activity_id != null ? " clickable" : ""}`}
            onClick={() => {
              if (d.dashboard_activity_id != null) onOpenActivity?.(d.dashboard_activity_id);
            }}
          >
            <span className="trip-dates">{d.date}</span>
            <span className="trip-location">{d.location_name ?? ""}</span>
            <span className="trip-km">{(d.distance_m / 1000).toFixed(1)} km</span>
            <span className="trip-gain">▲{Math.round(d.elevation_gain)}</span>
          </div>
        ))}
      </section>

      <div className="hiking-recovery">
        <div className="panel">
          <ReactECharts
            notMerge
            option={recoveryChart(
              detail.recovery,
              [{ key: "sleep_score", label: "Sleep score" }],
              "Sleep score",
            )}
            style={{ height: 180 }}
          />
        </div>
        <div className="panel">
          <ReactECharts
            notMerge
            option={recoveryChart(
              detail.recovery,
              [
                { key: "resting_hr", label: "Resting HR" },
                { key: "hrv_last_night_avg", label: "HRV" },
              ],
              "Resting HR + HRV",
            )}
            style={{ height: 180 }}
          />
        </div>
        <div className="panel">
          <ReactECharts
            notMerge
            option={recoveryChart(
              detail.recovery,
              [
                { key: "body_battery_high", label: "BB high" },
                { key: "body_battery_low", label: "BB low" },
                { key: "avg_stress", label: "Stress" },
              ],
              "Body Battery + Stress",
            )}
            style={{ height: 180 }}
          />
        </div>
        <div className="panel">
          <ReactECharts
            notMerge
            option={recoveryChart(
              detail.recovery,
              [{ key: "training_readiness", label: "Readiness" }],
              "Training readiness",
            )}
            style={{ height: 180 }}
          />
        </div>
      </div>
    </div>
  );
}
