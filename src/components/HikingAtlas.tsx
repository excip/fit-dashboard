import { useEffect, useMemo, useRef, useState } from "react";
import maplibregl from "maplibre-gl";
import { format, parseISO } from "date-fns";
import { api } from "../lib/api";
import type { AtlasTrack, Trip } from "../types";

type Props = { onClose: () => void; onOpenTrip: (tripId: number) => void };

const INK = "#0a0d13";
const CARTO_ATTR = "© OpenStreetMap contributors © CARTO";
const cartoTiles = (variant: string) =>
  ["a", "b", "c", "d"].map((s) => `https://${s}.basemaps.cartocdn.com/${variant}/{z}/{x}/{y}@2x.png`);

const CAT_LABEL: Record<string, string> = {
  thru_hike: "Thru-hike",
  weekend: "Weekend trip",
  day_hike: "Day hike",
};

const fmtKm = (m: number) => Math.round(m / 1000).toLocaleString();

/** True while the reveal should still be considered "warm" (drives the cooling ramp). */
type PlayState = "idle" | "playing" | "paused" | "done";

export function HikingAtlas({ onClose, onOpenTrip }: Props) {
  const overlayRef = useRef<HTMLDivElement | null>(null);
  const mapContainerRef = useRef<HTMLDivElement | null>(null);
  const mapRef = useRef<maplibregl.Map | null>(null);
  const barRef = useRef<HTMLDivElement | null>(null);
  const fillRef = useRef<HTMLDivElement | null>(null);
  const headRef = useRef<HTMLDivElement | null>(null);
  const dateRef = useRef<HTMLSpanElement | null>(null);
  const kmRef = useRef<HTMLSpanElement | null>(null);
  const hoverCardRef = useRef<HTMLDivElement | null>(null);

  const [phase, setPhase] = useState<"loading" | "ready" | "empty" | "error">("loading");
  const [tracks, setTracks] = useState<AtlasTrack[]>([]);
  const [trips, setTrips] = useState<Trip[]>([]);
  const [playState, setPlayState] = useState<PlayState>("idle");
  const [hoverTrip, setHoverTrip] = useState<Trip | null>(null);
  const [selectedTrip, setSelectedTrip] = useState<Trip | null>(null);
  const [retryKey, setRetryKey] = useState(0);

  const clockRef = useRef(0);
  const durationRef = useRef(10);
  const rafRef = useRef(0);
  const lastTsRef = useRef(0);
  const playStateRef = useRef<PlayState>("idle");
  const hoverIdRef = useRef<number | null>(null);
  const selectedIdRef = useRef<number | null>(null);
  const mapReadyRef = useRef(false);

  const reducedMotion = useMemo(
    () => window.matchMedia("(prefers-reduced-motion: reduce)").matches,
    [],
  );

  const tripById = useMemo(() => new Map(trips.map((t) => [t.id, t])), [trips]);

  // Chronological timeline: track i ignites at t = i / N of the playback.
  const timeline = useMemo(() => {
    const n = tracks.length;
    const ts = tracks.map((_, i) => i / Math.max(n, 1));
    const cumKm: number[] = [];
    let acc = 0;
    for (const tr of tracks) {
      acc += tr.distance_m;
      cumKm.push(acc);
    }
    return { ts, cumKm, totalM: acc };
  }, [tracks]);

  const yearTicks = useMemo(() => {
    const seen = new Set<number>();
    const ticks: { year: number; t: number }[] = [];
    tracks.forEach((tr, i) => {
      const y = parseISO(tr.date).getFullYear();
      if (!seen.has(y)) {
        seen.add(y);
        ticks.push({ year: y, t: i / Math.max(tracks.length, 1) });
      }
    });
    // drop labels that would collide on the bar (~5% of its width apart)
    const spaced: { year: number; t: number }[] = [];
    for (const tk of ticks) {
      if (spaced.length === 0 || tk.t - spaced[spaced.length - 1].t >= 0.05) spaced.push(tk);
    }
    return spaced;
  }, [tracks]);

  const totals = useMemo(() => {
    if (tracks.length === 0) return null;
    const firstYear = parseISO(tracks[0].date).getFullYear();
    const lastYear = parseISO(tracks[tracks.length - 1].date).getFullYear();
    const tripCount = new Set(tracks.map((t) => t.trip_id)).size;
    return { firstYear, lastYear, tripCount, km: timeline.totalM / 1000, hikes: tracks.length };
  }, [tracks, timeline]);

  // ---- data ----------------------------------------------------------------
  useEffect(() => {
    let cancelled = false;
    setPhase("loading");
    Promise.all([api.hikingTracks(), api.hikingTrips()])
      .then(([tks, tps]) => {
        if (cancelled) return;
        setTrips(tps);
        setTracks(tks.filter((t) => t.coords.length > 1));
        setPhase(tks.length === 0 ? "empty" : "ready");
      })
      .catch(() => {
        if (!cancelled) setPhase("error");
      });
    return () => {
      cancelled = true;
    };
  }, [retryKey]);

  // ---- paint expressions ----------------------------------------------------
  // "age" = seconds since a track ignited; drives the hot-white → ember cooling.
  const applyPaint = (clock: number) => {
    const map = mapRef.current;
    if (!map || !mapReadyRef.current) return;
    const d = durationRef.current;
    const age: any = ["*", ["-", clock, ["get", "t"]], d];
    const lit: any = [
      "any",
      ["boolean", ["feature-state", "hover"], false],
      ["boolean", ["feature-state", "selected"], false],
    ];
    const focusActive = hoverIdRef.current != null || selectedIdRef.current != null;
    const dim = focusActive ? 0.3 : 1;

    const coreOpacity: any = [
      "case", lit, 1,
      ["*", ["interpolate", ["linear"], age, -0.001, 0, 0.02, 0.4, 0.4, 0.95], dim],
    ];
    const coreColor: any = [
      "case", lit, "#ffe4b3",
      ["interpolate", ["linear"], age, 0, "#fff7e4", 0.5, "#ffc078", 2.6, "#f09246"],
    ];
    const glowOpacity: any = [
      "case", lit, 0.45,
      ["*", ["interpolate", ["linear"], age, -0.001, 0, 0.02, 0.6, 0.8, 0.34, 2.6, 0.26], dim],
    ];
    const glowColor: any = [
      "case", lit, "#ffd9a0",
      ["interpolate", ["linear"], age, 0, "#fff3d6", 0.5, "#ffb763", 2.6, "#ff9a45"],
    ];
    map.setPaintProperty("atlas-core", "line-opacity", coreOpacity);
    map.setPaintProperty("atlas-core", "line-color", coreColor);
    map.setPaintProperty("atlas-glow", "line-opacity", glowOpacity);
    map.setPaintProperty("atlas-glow", "line-color", glowColor);
  };

  const updateHud = (clock: number) => {
    const n = tracks.length;
    if (n === 0) return;
    const count = Math.min(n, Math.floor(clock * n + 1e-9) + (clock >= 1 ? 0 : 1));
    const idx = Math.max(0, Math.min(n - 1, count - 1));
    if (fillRef.current) fillRef.current.style.width = `${clock * 100}%`;
    if (headRef.current) headRef.current.style.left = `${clock * 100}%`;
    if (dateRef.current)
      dateRef.current.textContent = count === 0 ? "—" : format(parseISO(tracks[idx].date), "MMM yyyy");
    if (kmRef.current)
      kmRef.current.textContent =
        count === 0 ? "0 km" : `${fmtKm(timeline.cumKm[idx])} km · ${count} ${count === 1 ? "hike" : "hikes"}`;
  };

  const setClock = (c: number) => {
    clockRef.current = Math.max(0, Math.min(1, c));
    applyPaint(clockRef.current);
    updateHud(clockRef.current);
  };

  const setPlay = (s: PlayState) => {
    playStateRef.current = s;
    setPlayState(s);
  };

  const tick = (ts: number) => {
    if (playStateRef.current !== "playing") return;
    const dt = lastTsRef.current === 0 ? 0 : (ts - lastTsRef.current) / 1000;
    lastTsRef.current = ts;
    const next = clockRef.current + dt / durationRef.current;
    if (next >= 1) {
      setClock(1);
      setPlay("done");
      return;
    }
    setClock(next);
    rafRef.current = requestAnimationFrame(tick);
  };

  const play = () => {
    if (clockRef.current >= 1) setClock(0);
    setPlay("playing");
    lastTsRef.current = 0;
    cancelAnimationFrame(rafRef.current);
    rafRef.current = requestAnimationFrame(tick);
  };

  const pause = () => {
    setPlay(clockRef.current >= 1 ? "done" : "paused");
    cancelAnimationFrame(rafRef.current);
  };

  // ---- map -------------------------------------------------------------------
  useEffect(() => {
    if (phase !== "ready" || tracks.length === 0 || !mapContainerRef.current) return;

    durationRef.current = Math.max(6, Math.min(14, tracks.length * 0.03));

    const bounds = new maplibregl.LngLatBounds();
    for (const tr of tracks) for (const c of tr.coords) bounds.extend(c);

    const map = new maplibregl.Map({
      container: mapContainerRef.current,
      style: {
        version: 8,
        sources: {
          base: { type: "raster", tiles: cartoTiles("dark_nolabels"), tileSize: 256, attribution: CARTO_ATTR },
          labels: { type: "raster", tiles: cartoTiles("dark_only_labels"), tileSize: 256 },
        },
        layers: [
          { id: "bg", type: "background", paint: { "background-color": INK } },
          {
            id: "base",
            type: "raster",
            source: "base",
            paint: { "raster-opacity": 0.9, "raster-saturation": -0.35, "raster-brightness-max": 0.82 },
          },
        ],
      },
      bounds,
      fitBoundsOptions: { padding: { top: 150, bottom: 170, left: 90, right: 90 } },
      attributionControl: { compact: true },
    });
    mapRef.current = map;
    if (import.meta.env.DEV) (window as any).__atlasMap = map;

    const features = tracks.map((tr, i) => ({
      type: "Feature" as const,
      properties: { trip_id: tr.trip_id, t: i / tracks.length },
      geometry: { type: "LineString" as const, coordinates: tr.coords },
    }));

    map.on("load", () => {
      map.addSource("trails", {
        type: "geojson",
        data: { type: "FeatureCollection", features },
        promoteId: "trip_id",
      });
      const lit: any = [
        "any",
        ["boolean", ["feature-state", "hover"], false],
        ["boolean", ["feature-state", "selected"], false],
      ];
      // zoom interpolation must stay outermost; lit-vs-idle width branches per stop
      const widthCore: any = [
        "interpolate", ["exponential", 1.5], ["zoom"],
        2, ["case", lit, 2.6, 1.7],
        6, ["case", lit, 3.2, 2.2],
        10, ["case", lit, 4, 2.8],
        14, ["case", lit, 5.5, 4],
      ];
      map.addLayer({
        id: "atlas-glow",
        type: "line",
        source: "trails",
        layout: { "line-cap": "round", "line-join": "round" },
        paint: {
          "line-color": "#ff9a45",
          "line-opacity": 0,
          "line-width": ["interpolate", ["exponential", 1.5], ["zoom"], 2, 6, 6, 10, 10, 14, 14, 20] as any,
          "line-blur": ["interpolate", ["linear"], ["zoom"], 2, 3, 10, 6] as any,
        },
      });
      map.addLayer({
        id: "atlas-core",
        type: "line",
        source: "trails",
        layout: { "line-cap": "round", "line-join": "round" },
        paint: {
          "line-color": "#e2873f",
          "line-opacity": 0,
          "line-width": widthCore,
        },
      });
      map.addLayer({
        id: "atlas-hit",
        type: "line",
        source: "trails",
        paint: { "line-color": "#000", "line-opacity": 0.001, "line-width": 16 },
      });
      map.addLayer({
        id: "labels",
        type: "raster",
        source: "labels",
        paint: { "raster-opacity": 0.55 },
      });

      mapReadyRef.current = true;

      if (reducedMotion) {
        setClock(1);
        setPlay("done");
      } else {
        setClock(0);
        // slow settle-in, then the chronology begins
        map.easeTo({ zoom: map.getZoom() + 0.35, duration: 2600, essential: false });
        window.setTimeout(() => {
          if (playStateRef.current === "idle") play();
        }, 900);
      }
    });

    // ---- hover / click -------------------------------------------------------
    const pick = (p: maplibregl.Point) => {
      if (!mapReadyRef.current) return null;
      const feats = map.queryRenderedFeatures(
        [
          [p.x - 6, p.y - 6],
          [p.x + 6, p.y + 6],
        ],
        { layers: ["atlas-hit"] },
      );
      const f = feats.find((f) => ((f.properties?.t as number) ?? 1) <= clockRef.current + 1e-9);
      return f ? (f.properties!.trip_id as number) : null;
    };

    const setHoverId = (id: number | null) => {
      if (hoverIdRef.current === id) return;
      if (hoverIdRef.current != null)
        map.setFeatureState({ source: "trails", id: hoverIdRef.current }, { hover: false });
      hoverIdRef.current = id;
      if (id != null) map.setFeatureState({ source: "trails", id }, { hover: true });
      map.getCanvas().style.cursor = id != null ? "pointer" : "";
      applyPaint(clockRef.current);
    };

    map.on("mousemove", (e) => {
      const id = pick(e.point);
      setHoverId(id);
      setHoverTrip(id != null ? tripById.get(id) ?? null : null);
      const card = hoverCardRef.current;
      if (card && id != null) {
        const pad = 18;
        const w = card.offsetWidth || 260;
        const x = Math.min(e.originalEvent.clientX + pad, window.innerWidth - w - 12);
        const y = Math.max(e.originalEvent.clientY - card.offsetHeight - pad, 12);
        card.style.transform = `translate(${x}px, ${y}px)`;
      }
    });
    map.on("mouseout", () => {
      setHoverId(null);
      setHoverTrip(null);
    });

    map.on("click", (e) => {
      const id = pick(e.point);
      if (selectedIdRef.current != null)
        map.setFeatureState({ source: "trails", id: selectedIdRef.current }, { selected: false });
      selectedIdRef.current = id;
      if (id != null) {
        map.setFeatureState({ source: "trails", id }, { selected: true });
        const b = new maplibregl.LngLatBounds();
        for (const tr of tracks) if (tr.trip_id === id) for (const c of tr.coords) b.extend(c);
        map.fitBounds(b, {
          padding: { top: 150, bottom: 190, left: 110, right: 110 },
          maxZoom: 11.5,
          duration: 1500,
          essential: true,
        });
      }
      setSelectedTrip(id != null ? tripById.get(id) ?? null : null);
      applyPaint(clockRef.current);
    });

    return () => {
      cancelAnimationFrame(rafRef.current);
      mapReadyRef.current = false;
      map.remove();
      mapRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phase, tracks]);

  // ---- keyboard / scroll lock -----------------------------------------------
  useEffect(() => {
    overlayRef.current?.focus();
    const prev = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => {
      document.body.style.overflow = prev;
    };
  }, []);

  const deselect = () => {
    const map = mapRef.current;
    if (map && selectedIdRef.current != null)
      map.setFeatureState({ source: "trails", id: selectedIdRef.current }, { selected: false });
    selectedIdRef.current = null;
    setSelectedTrip(null);
    applyPaint(clockRef.current);
  };

  const fitAll = () => {
    const map = mapRef.current;
    if (!map || tracks.length === 0) return;
    const b = new maplibregl.LngLatBounds();
    for (const tr of tracks) for (const c of tr.coords) b.extend(c);
    map.fitBounds(b, {
      padding: { top: 150, bottom: 170, left: 90, right: 90 },
      duration: 1500,
      essential: true,
    });
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      if (selectedTrip) deselect();
      else onClose();
    } else if (e.key === " " && phase === "ready") {
      e.preventDefault();
      playState === "playing" ? pause() : play();
    } else if (e.key === "ArrowRight" && phase === "ready") {
      pause();
      setClock(clockRef.current + 0.02);
    } else if (e.key === "ArrowLeft" && phase === "ready") {
      pause();
      setClock(clockRef.current - 0.02);
    }
  };

  // ---- scrubber ---------------------------------------------------------------
  const scrubTo = (clientX: number) => {
    const bar = barRef.current;
    if (!bar) return;
    const r = bar.getBoundingClientRect();
    setClock((clientX - r.left) / r.width);
  };

  const onBarPointerDown = (e: React.PointerEvent) => {
    (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
    const wasPlaying = playStateRef.current === "playing";
    pause();
    scrubTo(e.clientX);
    const move = (ev: PointerEvent) => scrubTo(ev.clientX);
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      if (wasPlaying && clockRef.current < 1) play();
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  const playGlyph = playState === "playing" ? "❚❚" : clockRef.current >= 1 ? "↻" : "▶";

  return (
    <div className="atlas-overlay" ref={overlayRef} tabIndex={-1} onKeyDown={onKeyDown} role="dialog" aria-label="Hiking atlas">
      <div className="atlas-map" ref={mapContainerRef} />
      <div className="atlas-vignette" aria-hidden="true" />

      <header className="atlas-top">
        <div className="atlas-title-block">
          <span className="atlas-eyebrow">
            {totals ? `Every trail · ${totals.firstYear}–${totals.lastYear}` : "Every trail"}
          </span>
          <h1 className="atlas-title">The Atlas</h1>
          {totals && (
            <span className="atlas-subtitle">
              {fmtKm(timeline.totalM)} km on foot · {totals.hikes} hikes · {totals.tripCount} trips
            </span>
          )}
        </div>
        <button className="atlas-close" onClick={onClose} aria-label="Close atlas">✕</button>
      </header>

      {hoverTrip && !selectedTrip && (
        <div className="atlas-hovercard" ref={hoverCardRef}>
          <span className="atlas-card-eyebrow">{CAT_LABEL[hoverTrip.category]}</span>
          <span className="atlas-card-name">{hoverTrip.name ?? "Unnamed trip"}</span>
          <span className="atlas-card-meta">
            {format(parseISO(hoverTrip.start_date), "d MMM yyyy")} · {fmtKm(hoverTrip.total_distance_m)} km · ▲{Math.round(hoverTrip.total_gain).toLocaleString()} m
          </span>
        </div>
      )}

      {selectedTrip && (
        <aside className="atlas-tripcard">
          <button className="atlas-card-close" onClick={deselect} aria-label="Deselect trip">✕</button>
          <span className="atlas-card-eyebrow">{CAT_LABEL[selectedTrip.category]}{selectedTrip.nights > 0 ? ` · ${selectedTrip.nights} nights` : ""}</span>
          <span className="atlas-card-name big">{selectedTrip.name ?? "Unnamed trip"}</span>
          <span className="atlas-card-meta">
            {format(parseISO(selectedTrip.start_date), "d MMM yyyy")}
            {selectedTrip.nights > 0 ? ` – ${format(parseISO(selectedTrip.end_date), "d MMM yyyy")}` : ""}
          </span>
          <span className="atlas-card-meta">
            {fmtKm(selectedTrip.total_distance_m)} km · ▲{Math.round(selectedTrip.total_gain).toLocaleString()} m
          </span>
          <button className="atlas-open-trip" onClick={() => onOpenTrip(selectedTrip.id)}>
            Open trip →
          </button>
        </aside>
      )}

      {phase === "ready" && tracks.length > 0 && (
        <div className="atlas-controls">
          <button
            className="atlas-play"
            onClick={() => (playState === "playing" ? pause() : play())}
            aria-label={playState === "playing" ? "Pause playback" : "Play your hiking history"}
          >
            {playGlyph}
          </button>
          <div className="atlas-timeline" ref={barRef} onPointerDown={onBarPointerDown}>
            <div className="atlas-timeline-fill" ref={fillRef} />
            <div className="atlas-timeline-head" ref={headRef} />
            {yearTicks.map((tk) => (
              <span key={tk.year} className="atlas-tick" style={{ left: `${tk.t * 100}%` }}>
                <i />
                {tk.year}
              </span>
            ))}
          </div>
          <div className="atlas-readout">
            <span className="atlas-readout-date" ref={dateRef}>—</span>
            <span className="atlas-readout-km" ref={kmRef}>0 km</span>
          </div>
        </div>
      )}

      {phase === "ready" && (
        <button className="atlas-fitall" onClick={fitAll} title="View all trails" aria-label="Zoom to all trails">
          ◎
        </button>
      )}

      {phase === "loading" && (
        <div className="atlas-loading">
          <h1 className="atlas-title">The Atlas</h1>
          <span className="atlas-loading-line" />
          <span className="atlas-loading-text">Gathering every trail…</span>
        </div>
      )}
      {phase === "empty" && (
        <div className="atlas-loading">
          <h1 className="atlas-title">The Atlas</h1>
          <span className="atlas-loading-text">No GPS tracks found for your hikes.</span>
          <button className="atlas-open-trip" onClick={onClose}>Back to hiking</button>
        </div>
      )}
      {phase === "error" && (
        <div className="atlas-loading">
          <h1 className="atlas-title">The Atlas</h1>
          <span className="atlas-loading-text">Couldn't load your trails.</span>
          <button className="atlas-open-trip" onClick={() => setRetryKey((k) => k + 1)}>Try again</button>
        </div>
      )}
    </div>
  );
}
