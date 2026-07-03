import { useEffect, useState } from "react";
import ReactECharts from "echarts-for-react";
import { api } from "../lib/api";
import type { Baselines, RecordPoint, RecoveryDay, TripDetail } from "../types";
import { TripMap } from "./TripMap";
import { useSettingsStore } from "../stores/settingsStore";

const KM = (m: number) => (m / 1000).toFixed(0);

type Props = {
  tripId: number;
  onBack: () => void;
  onTripChanged: (newTripId: number) => void;
  onOpenActivity?: (dashboardActivityId: number) => void;
};

type ChartColors = {
  axisColor: string;
  tooltipBg: string;
  tooltipBorder: string;
  tooltipText: string;
  tripBand: string;
};

function recoveryChart(
  recovery: RecoveryDay[],
  series: Array<{ key: keyof RecoveryDay; label: string }>,
  title: string,
  colors: ChartColors,
  baselines: Baselines,
  tripStart: string,
  tripEnd: string,
) {
  const startIdx = recovery.findIndex((r) => r.date === tripStart);
  const endIdx = recovery.findIndex((r) => r.date === tripEnd);
  return {
    title: { text: title, textStyle: { fontSize: 12, color: colors.axisColor }, left: 4, top: 2 },
    tooltip: {
      trigger: "axis",
      backgroundColor: colors.tooltipBg,
      borderColor: colors.tooltipBorder,
      textStyle: { color: colors.tooltipText },
    },
    grid: { left: 36, right: 12, top: 30, bottom: 20 },
    xAxis: {
      type: "category",
      data: recovery.map((r) => r.date.slice(5)),
      axisLabel: { fontSize: 9, color: colors.axisColor },
    },
    yAxis: { type: "value", axisLabel: { fontSize: 9, color: colors.axisColor }, scale: true },
    series: series.map((s, i) => {
      const base = baselines[s.key as keyof Baselines];
      return {
        name: s.label,
        type: "line",
        connectNulls: true,
        showSymbol: false,
        data: recovery.map((r) => r[s.key] as number | null),
        // dashed at-home baseline for this metric
        markLine:
          base != null
            ? {
                silent: true,
                symbol: "none",
                lineStyle: { type: "dashed", opacity: 0.6 },
                label: { show: false },
                data: [{ yAxis: base }],
              }
            : undefined,
        // shaded band over the trip days (first series only; +/-0.5 covers
        // the full category slots so single-day trips still show a band)
        markArea:
          i === 0 && startIdx >= 0 && endIdx >= 0
            ? {
                silent: true,
                itemStyle: { color: colors.tripBand },
                data: [[{ xAxis: startIdx - 0.5 }, { xAxis: endIdx + 0.5 }]],
              }
            : undefined,
      };
    }),
    legend:
      series.length > 1
        ? { bottom: 0, textStyle: { fontSize: 9, color: colors.axisColor } }
        : undefined,
  };
}

export function HikingTripDetail({ tripId, onBack, onTripChanged, onOpenActivity }: Props) {
  const theme = useSettingsStore((s) => s.theme);
  const [detail, setDetail] = useState<TripDetail | null>(null);
  const [tracks, setTracks] = useState<RecordPoint[][]>([]);
  const [editingName, setEditingName] = useState(false);
  const [nameDraft, setNameDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [reloadKey, setReloadKey] = useState(0);

  // When tripId changes reset to Loading state immediately (prevents stale-view interactions)
  useEffect(() => {
    setDetail(null);
    setTracks([]);
    setError(null);
  }, [tripId]);

  // Cancellable fetch — reruns on tripId change or explicit reload
  useEffect(() => {
    let cancelled = false;
    setError(null);
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
        if (!cancelled) setError("Failed to load trip details.");
      });
    return () => {
      cancelled = true;
    };
  }, [tripId, reloadKey]);

  const isDark = theme === "dark";
  const chartColors: ChartColors = {
    axisColor: isDark ? "#8899b8" : "#64748b",
    tooltipBg: isDark ? "rgba(14, 22, 45, 0.95)" : "rgba(255, 255, 255, 0.95)",
    tooltipBorder: isDark ? "rgba(100, 140, 220, 0.2)" : "rgba(0, 0, 0, 0.08)",
    tooltipText: isDark ? "#e2e8f4" : "#0f172a",
    tripBand: isDark ? "rgba(100, 140, 220, 0.10)" : "rgba(59, 130, 246, 0.08)",
  };

  if (!detail) {
    return (
      <div className="hiking-tab">
        <button className="hiking-back" onClick={onBack}>
          ← Back
        </button>
        {error ? (
          <>
            <div className="hiking-error">{error}</div>
            <button onClick={() => setReloadKey((k) => k + 1)}>Retry</button>
          </>
        ) : (
          <p className="empty">Loading…</p>
        )}
      </div>
    );
  }

  const t = detail.trip;
  const title =
    t.name ?? `${t.start_date}${t.nights > 0 ? ` → ${t.end_date}` : ""}`;

  async function saveName() {
    if (busy) return;
    const name = nameDraft.trim();
    setBusy(true);
    try {
      await api.hikingSetTripName(t.id, name.length > 0 ? name : null);
      setEditingName(false);
      setReloadKey((k) => k + 1);
    } catch {
      setError("Failed to save trip name.");
    } finally {
      setBusy(false);
    }
  }

  async function mergePrevious() {
    setBusy(true);
    try {
      const r = await api.hikingMergePrevious(t.id);
      onTripChanged(r.trip_id);
    } catch {
      setError("Failed to merge trip.");
    } finally {
      setBusy(false);
    }
  }

  async function splitTrip() {
    setBusy(true);
    try {
      const r = await api.hikingSplitTrip(t.id);
      onTripChanged(r.trip_id);
    } catch {
      setError("Failed to split trip.");
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
              disabled={busy}
            />
            <button onClick={() => void saveName()} disabled={busy}>Save</button>
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

      {error && <div className="hiking-error">{error}</div>}

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
              chartColors,
              detail.baselines,
              t.start_date,
              t.end_date,
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
              chartColors,
              detail.baselines,
              t.start_date,
              t.end_date,
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
              chartColors,
              detail.baselines,
              t.start_date,
              t.end_date,
            )}
            style={{ height: 180 }}
          />
        </div>
        {detail.recovery.some((r) => r.training_readiness != null) && (
          <div className="panel">
            <ReactECharts
              notMerge
              option={recoveryChart(
                detail.recovery,
                [{ key: "training_readiness", label: "Readiness" }],
                "Training readiness",
                chartColors,
                detail.baselines,
                t.start_date,
                t.end_date,
              )}
              style={{ height: 180 }}
            />
          </div>
        )}
      </div>
    </div>
  );
}
