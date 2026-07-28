import { useEffect, useMemo, useRef, useState } from "react";
import { format, parseISO } from "date-fns";
import { api } from "../lib/api";
import type { WalkingLoop, WalkingOverview, WalkingYear } from "../types";

/*
 * The Riverwalk — where the hiking Atlas is fire, walking is water.
 * A nocturne in Aare-glacial cyan: every recorded day drifts through the hero
 * as a filament of current; the years pool into a river; the home loop stacks
 * into a slowly turning column of light — the same walk, hundreds of times.
 */

const fmtInt = (n: number) => Math.round(n).toLocaleString();
const fmtKm = (m: number) => Math.round(m / 1000).toLocaleString();
const fmtDate = (iso: string | null) => (iso ? format(parseISO(iso), "d MMM yyyy") : "—");

const EARTH_CIRCUMFERENCE_M = 40_075_000;

function mulberry32(seed: number) {
  let a = seed >>> 0;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function usePrefersReducedMotion() {
  return useMemo(() => window.matchMedia("(prefers-reduced-motion: reduce)").matches, []);
}

/** Eased count-up for hero numerals; snaps immediately under reduced motion. */
function useCountUp(target: number, durationMs = 2400) {
  const reduced = usePrefersReducedMotion();
  const [value, setValue] = useState(0);
  useEffect(() => {
    if (reduced || target === 0) {
      setValue(target);
      return;
    }
    let raf = 0;
    const t0 = performance.now();
    const tick = (t: number) => {
      const p = Math.min(1, (t - t0) / durationMs);
      setValue(Math.round(target * (1 - Math.pow(1 - p, 4))));
      if (p < 1) raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [target, durationMs, reduced]);
  return value;
}

/* ── Hero: the current ─────────────────────────────────────────────
   One drifting filament per recorded day. Speed, length and glow all
   follow that day's step count; the whole field flows like the Aare. */
function CurrentCanvas({ daily }: { daily: [string, number][] }) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const reduced = usePrefersReducedMotion();

  useEffect(() => {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;

    const max = Math.max(1, ...daily.map((d) => d[1]));
    const rand = mulberry32(20170608);
    const parts = daily.map(([, steps]) => {
      const n = Math.pow(steps / max, 0.65);
      return {
        x: rand() * 1.3 - 0.15,
        y: rand(),
        speed: 0.008 + 0.055 * n,
        len: 0.012 + 0.1 * n,
        alpha: 0.05 + 0.4 * n,
        phase: rand() * Math.PI * 2,
        freq: 0.25 + rand() * 0.5,
        swayPx: 5 + rand() * 14,
        color: `hsla(${168 + Math.round(26 * rand())}, ${Math.round(42 + 46 * n)}%, ${Math.round(36 + 42 * n)}%, `,
        width: 0.8 + 1.3 * n,
      };
    });

    let w = 0;
    let h = 0;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const resize = () => {
      const r = canvas.getBoundingClientRect();
      w = r.width;
      h = r.height;
      canvas.width = Math.max(1, Math.round(w * dpr));
      canvas.height = Math.max(1, Math.round(h * dpr));
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    };
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(canvas);

    let raf = 0;
    let last = 0;
    let visible = true;
    const draw = (t: number) => {
      const dt = last === 0 ? 0 : Math.min((t - last) / 1000, 0.05);
      last = t;
      ctx.clearRect(0, 0, w, h);
      ctx.globalCompositeOperation = "lighter";
      ctx.lineCap = "round";
      const ts = t / 1000;
      for (const p of parts) {
        p.x += p.speed * dt;
        if (p.x > 1.18) p.x -= 1.36;
        const y = p.y * h + Math.sin(ts * p.freq + p.phase) * p.swayPx;
        const x = p.x * w;
        ctx.strokeStyle = `${p.color}${p.alpha})`;
        ctx.lineWidth = p.width;
        ctx.beginPath();
        ctx.moveTo(x, y);
        ctx.lineTo(x - p.len * w, y + Math.cos(ts * p.freq + p.phase) * 2.5);
        ctx.stroke();
      }
      if (!reduced && visible) raf = requestAnimationFrame(draw);
    };

    const io = new IntersectionObserver(([entry]) => {
      visible = entry.isIntersecting;
      if (visible && !reduced) {
        cancelAnimationFrame(raf);
        last = 0;
        raf = requestAnimationFrame(draw);
      }
    });
    io.observe(canvas);
    raf = requestAnimationFrame(draw);

    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
      io.disconnect();
    };
  }, [daily, reduced]);

  return <canvas className="walking-current-canvas" ref={canvasRef} aria-hidden="true" />;
}

/* ── The years as a river ─────────────────────────────────────────── */
type Pt = { x: number; y: number };

function catmullRom(pts: Pt[], move = true): string {
  if (pts.length === 0) return "";
  let d = `${move ? "M" : "L"} ${pts[0].x.toFixed(1)} ${pts[0].y.toFixed(1)}`;
  for (let i = 0; i < pts.length - 1; i++) {
    const p0 = pts[Math.max(0, i - 1)];
    const p1 = pts[i];
    const p2 = pts[i + 1];
    const p3 = pts[Math.min(pts.length - 1, i + 2)];
    d += ` C ${(p1.x + (p2.x - p0.x) / 6).toFixed(1)} ${(p1.y + (p2.y - p0.y) / 6).toFixed(1)}, ${(p2.x - (p3.x - p1.x) / 6).toFixed(1)} ${(p2.y - (p3.y - p1.y) / 6).toFixed(1)}, ${p2.x.toFixed(1)} ${p2.y.toFixed(1)}`;
  }
  return d;
}

const RIVER_W = 1000;
const RIVER_H = 280;

function YearRiver({ years }: { years: WalkingYear[] }) {
  const [hover, setHover] = useState<number | null>(null);
  const n = years.length;

  const geo = useMemo(() => {
    const maxSteps = Math.max(1, ...years.map((y) => y.steps));
    const xs = years.map((_, i) => ((i + 0.5) / n) * RIVER_W);
    const centers = years.map((_, i) => RIVER_H / 2 + Math.sin(i * 0.95 + 0.6) * 20);
    const halves = years.map((y) => 7 + 96 * (y.steps / maxSteps) * 0.5);
    const pad = (arr: Pt[]): Pt[] => [
      { x: 0, y: arr[0].y },
      ...arr,
      { x: RIVER_W, y: arr[arr.length - 1].y },
    ];
    const top = pad(years.map((_, i) => ({ x: xs[i], y: centers[i] - halves[i] })));
    const bot = pad(years.map((_, i) => ({ x: xs[i], y: centers[i] + halves[i] })));
    const spine = pad(years.map((_, i) => ({ x: xs[i], y: centers[i] })));
    const ribbon = `${catmullRom(top)} ${catmullRom([...bot].reverse(), false)} Z`;
    return { ribbon, spine: catmullRom(spine), xs };
  }, [years, n]);

  return (
    <div className="walking-river-wrap">
      <svg
        className="walking-river"
        viewBox={`0 0 ${RIVER_W} ${RIVER_H}`}
        preserveAspectRatio="none"
        aria-label="Steps per year, drawn as a river"
        role="img"
      >
        <defs>
          <linearGradient id="walking-river-fill" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="rgba(111, 240, 221, 0.5)" />
            <stop offset="55%" stopColor="rgba(56, 152, 158, 0.32)" />
            <stop offset="100%" stopColor="rgba(20, 66, 92, 0.16)" />
          </linearGradient>
        </defs>
        {hover != null && (
          <rect
            x={(hover / n) * RIVER_W}
            y={0}
            width={RIVER_W / n}
            height={RIVER_H}
            className="walking-river-highlight"
          />
        )}
        <path d={geo.ribbon} fill="url(#walking-river-fill)" />
        <path d={geo.ribbon} className="walking-river-edge" />
        <path d={geo.spine} className="walking-river-spine" />
      </svg>
      <div className="walking-river-hitzones" aria-hidden="true">
        {years.map((y, i) => (
          <div key={y.year} onMouseEnter={() => setHover(i)} onMouseLeave={() => setHover(null)} />
        ))}
      </div>
      {hover != null && (
        <div
          className="walking-river-tip"
          style={{ left: `${((hover + 0.5) / n) * 100}%` }}
        >
          <span className="walking-river-tip-year">{years[hover].year}</span>
          <span>{fmtInt(years[hover].steps)} steps</span>
          <span>
            {fmtKm(years[hover].distance_m)} km · {years[hover].walks} walks
          </span>
        </div>
      )}
      <div className="walking-river-labels">
        {years.map((y) => (
          <div key={y.year} className="walking-river-label">
            <span className="walking-river-year">{String(y.year).slice(2)}</span>
            <span className="walking-river-steps">{(y.steps / 1e6).toFixed(1)}M</span>
          </div>
        ))}
      </div>
    </div>
  );
}

/* ── The loop, stacked through time ───────────────────────────────
   Every rendition of the home loop becomes one strand; strands stack
   chronologically into a column and orbit slowly. Hand-rolled 3D. */
function LoopColumn({ loop }: { loop: WalkingLoop }) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const reduced = usePrefersReducedMotion();

  const strands = useMemo(() => {
    const tracks = loop.tracks.filter((t) => t.coords.length > 2);
    if (tracks.length === 0) return null;
    let latSum = 0;
    let lonSum = 0;
    let count = 0;
    for (const t of tracks)
      for (const [lon, lat] of t.coords) {
        lonSum += lon;
        latSum += lat;
        count++;
      }
    const lat0 = latSum / count;
    const lon0 = lonSum / count;
    const kx = 111_320 * Math.cos((lat0 * Math.PI) / 180);
    const ky = 110_540;
    let maxR = 1;
    const raw = tracks.map((t) =>
      t.coords.map(([lon, lat]) => {
        const x = (lon - lon0) * kx;
        const y = (lat - lat0) * ky;
        maxR = Math.max(maxR, Math.abs(x), Math.abs(y));
        return [x, y] as [number, number];
      }),
    );
    const years = tracks.map((t) => parseISO(t.date).getFullYear());
    const y0 = Math.min(...years);
    const y1 = Math.max(...years);
    return raw.map((pts, i) => {
      const yt = y1 === y0 ? 1 : (years[i] - y0) / (y1 - y0);
      return {
        pts: pts.map(([x, y]) => [x / maxR, y / maxR] as [number, number]),
        z: (i / Math.max(1, raw.length - 1) - 0.5) * 2.3,
        // older loops sink into deep water blue, recent ones surface to glacial cyan
        r: Math.round(46 + 96 * yt),
        g: Math.round(118 + 122 * yt),
        b: Math.round(150 + 74 * yt),
      };
    });
  }, [loop.tracks]);

  useEffect(() => {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx || !strands) return;

    let w = 0;
    let h = 0;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const resize = () => {
      const r = canvas.getBoundingClientRect();
      w = r.width;
      h = r.height;
      canvas.width = Math.max(1, Math.round(w * dpr));
      canvas.height = Math.max(1, Math.round(h * dpr));
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    };
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(canvas);

    let theta = 0.6;
    let tilt = 0.88;
    let dragging = false;
    let userTilted = false;
    let entry = reduced ? 1 : 0;
    let raf = 0;
    let last = 0;
    let visible = true;

    const draw = (t: number) => {
      const dt = last === 0 ? 0 : Math.min((t - last) / 1000, 0.05);
      last = t;
      if (!dragging) theta += dt * 0.14;
      // the camera breathes between side and top view until the user takes over
      if (!dragging && !userTilted) tilt = 0.78 + Math.sin(t / 1000 * 0.16 + 2.2) * 0.34;
      if (entry < 1) entry = Math.min(1, entry + dt / 4.2);

      ctx.clearRect(0, 0, w, h);
      ctx.globalCompositeOperation = "lighter";
      ctx.lineCap = "round";
      const s = Math.min(w, h) / 3.05;
      const cx = w / 2;
      const cy = h / 2;
      const cosT = Math.cos(theta);
      const sinT = Math.sin(theta);
      const sinTilt = Math.sin(tilt);
      const cosTilt = Math.cos(tilt);
      const n = strands.length;
      const shown = Math.max(1, Math.ceil(entry * n));

      for (let i = 0; i < shown; i++) {
        const st = strands[i];
        const mid = st.pts[st.pts.length >> 1];
        const depth = mid[0] * sinT + mid[1] * cosT; // -1 back … +1 front
        const igniting = entry < 1 && i > shown - 7;
        const alpha = igniting
          ? 0.85
          : (0.16 + 0.16 * ((depth + 1) / 2)) * (0.75 + 0.25 * (i / n));
        ctx.strokeStyle = igniting
          ? `rgba(235, 255, 250, ${alpha})`
          : `rgba(${st.r}, ${st.g}, ${st.b}, ${alpha})`;
        ctx.lineWidth = i === n - 1 ? 1.8 : 1.05;
        ctx.beginPath();
        for (let j = 0; j < st.pts.length; j++) {
          const [x, y] = st.pts[j];
          const xr = x * cosT - y * sinT;
          const yr = x * sinT + y * cosT;
          const sx = cx + xr * s;
          const sy = cy + yr * sinTilt * s * 0.72 - st.z * cosTilt * s * 0.95;
          if (j === 0) ctx.moveTo(sx, sy);
          else ctx.lineTo(sx, sy);
        }
        ctx.stroke();
      }
      if (!reduced && visible) raf = requestAnimationFrame(draw);
    };

    const onPointerDown = (e: PointerEvent) => {
      dragging = true;
      canvas.setPointerCapture(e.pointerId);
      canvas.style.cursor = "grabbing";
    };
    const onPointerMove = (e: PointerEvent) => {
      if (!dragging) return;
      userTilted = true;
      theta += e.movementX * 0.006;
      tilt = Math.max(0.12, Math.min(1.35, tilt + e.movementY * 0.004));
      if (reduced) {
        last = 0;
        draw(performance.now());
      }
    };
    const onPointerUp = () => {
      dragging = false;
      canvas.style.cursor = "grab";
    };
    canvas.addEventListener("pointerdown", onPointerDown);
    canvas.addEventListener("pointermove", onPointerMove);
    canvas.addEventListener("pointerup", onPointerUp);
    canvas.addEventListener("pointercancel", onPointerUp);

    const io = new IntersectionObserver(([e]) => {
      visible = e.isIntersecting;
      if (visible && !reduced) {
        cancelAnimationFrame(raf);
        last = 0;
        raf = requestAnimationFrame(draw);
      }
    });
    io.observe(canvas);
    raf = requestAnimationFrame(draw);

    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
      io.disconnect();
      canvas.removeEventListener("pointerdown", onPointerDown);
      canvas.removeEventListener("pointermove", onPointerMove);
      canvas.removeEventListener("pointerup", onPointerUp);
      canvas.removeEventListener("pointercancel", onPointerUp);
    };
  }, [strands, reduced]);

  if (!strands) return null;
  return <canvas className="walking-loop-canvas" ref={canvasRef} aria-label="Every rendition of the home loop, stacked chronologically" />;
}

/* ── Tab ───────────────────────────────────────────────────────────── */
export function WalkingTab() {
  const [ov, setOv] = useState<WalkingOverview | null>(null);
  const [loop, setLoop] = useState<WalkingLoop | null>(null);
  const [phase, setPhase] = useState<"loading" | "ready" | "error">("loading");
  const [retryKey, setRetryKey] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setPhase("loading");
    Promise.all([api.walkingOverview(), api.walkingLoop()])
      .then(([o, l]) => {
        if (cancelled) return;
        setOv(o);
        setLoop(l);
        setPhase("ready");
      })
      .catch(() => {
        if (!cancelled) setPhase("error");
      });
    return () => {
      cancelled = true;
    };
  }, [retryKey]);

  const heroSteps = useCountUp(ov?.total.steps ?? 0);
  const weekSteps = useCountUp(ov?.week.steps ?? 0, 1600);
  const monthSteps = useCountUp(ov?.month.steps ?? 0, 1900);
  const loopCount = useCountUp(loop?.count ?? 0, 2000);

  if (phase !== "ready" || !ov || !loop) {
    return (
      <div className="walking-tab walking-tab-center">
        <div className="walking-loading">
          <h1 className="walking-title">The Riverwalk</h1>
          {phase === "loading" ? (
            <>
              <span className="walking-loading-line" />
              <span className="walking-loading-text">Following the current…</span>
            </>
          ) : (
            <>
              <span className="walking-loading-text">Couldn't load your walking history.</span>
              <button className="walking-btn" onClick={() => setRetryKey((k) => k + 1)}>
                Try again
              </button>
            </>
          )}
        </div>
      </div>
    );
  }

  const firstYear = ov.years[0]?.year;
  const lastYear = ov.years[ov.years.length - 1]?.year;
  const avgDay = ov.daily.length > 0 ? ov.total.steps / ov.daily.length : 0;
  const earthPct = Math.round((ov.total.distance_m / EARTH_CIRCUMFERENCE_M) * 100);
  const maxLoopYear = Math.max(1, ...loop.per_year.map((y) => y.count));
  const r = ov.records;

  return (
    <div className="walking-tab">
      {/* ── Hero: the current ── */}
      <section className="walking-hero">
        <CurrentCanvas daily={ov.daily} />
        <div className="walking-hero-copy">
          <span className="walking-eyebrow">
            Every step · {firstYear}–{lastYear}
          </span>
          <span className="walking-hero-number">{fmtInt(heroSteps)}</span>
          <span className="walking-hero-sub">
            steps — <em>{fmtKm(ov.total.distance_m)} km on foot</em>, {earthPct}% of the way
            around the Earth
          </span>
        </div>
        <span className="walking-hero-footnote">
          each drifting light is one recorded day · brighter means further
        </span>
      </section>

      {/* ── Now ── */}
      <section className="walking-section">
        <header className="walking-section-head">
          <span className="walking-eyebrow">Now</span>
        </header>
        <div className="walking-now-grid">
          <div className="walking-now-card">
            <span className="walking-now-label">This week</span>
            <span className="walking-now-value">{fmtInt(weekSteps)}</span>
            <span className="walking-now-meta">
              {fmtKm(ov.week.distance_m)} km · {ov.week.walks} {ov.week.walks === 1 ? "walk" : "walks"}
            </span>
          </div>
          <div className="walking-now-card">
            <span className="walking-now-label">This month</span>
            <span className="walking-now-value">{fmtInt(monthSteps)}</span>
            <span className="walking-now-meta">
              {fmtKm(ov.month.distance_m)} km · {ov.month.walks} {ov.month.walks === 1 ? "walk" : "walks"}
            </span>
          </div>
          <div className="walking-now-card">
            <span className="walking-now-label">Every day, on average</span>
            <span className="walking-now-value">{fmtInt(avgDay)}</span>
            <span className="walking-now-meta">steps · {ov.daily.length.toLocaleString()} days recorded</span>
          </div>
        </div>
      </section>

      {/* ── The years ── */}
      <section className="walking-section">
        <header className="walking-section-head">
          <span className="walking-eyebrow">The years</span>
          <h2 className="walking-section-title">A river of {fmtInt(ov.total.steps)} steps</h2>
        </header>
        <YearRiver years={ov.years} />
      </section>

      {/* ── The loop ── */}
      <section className="walking-section walking-loop-section">
        <div className="walking-loop-stage">
          <LoopColumn loop={loop} />
          <span className="walking-loop-hint">drag to turn · every strand is one walk</span>
        </div>
        <aside className="walking-loop-panel">
          <span className="walking-eyebrow">The ritual</span>
          <h2 className="walking-loop-title">The Aare Loop</h2>
          <p className="walking-loop-prose">
            Out the front door, along the river, over the bridge, and home. The same walk,{" "}
            <strong>{fmtInt(loopCount)} times</strong>
            {loop.first_date ? ` since ${fmtDate(loop.first_date)}` : ""}.
          </p>
          <div className="walking-loop-stats">
            <div>
              <span className="walking-loop-stat">{fmtKm(loop.distance_m)} km</span>
              <span className="walking-loop-stat-label">along the same water</span>
            </div>
            <div>
              <span className="walking-loop-stat">{fmtInt(loop.steps)}</span>
              <span className="walking-loop-stat-label">steps on the loop</span>
            </div>
          </div>
          <div className="walking-loop-years">
            {loop.per_year.map((y) => (
              <div key={y.year} className="walking-loop-year">
                <span className="walking-loop-year-label">{y.year}</span>
                <span className="walking-loop-year-bar">
                  <i style={{ width: `${(y.count / maxLoopYear) * 100}%` }} />
                </span>
                <span className="walking-loop-year-count">{y.count}</span>
              </div>
            ))}
          </div>
        </aside>
      </section>

      {/* ── Records ── */}
      <section className="walking-section">
        <header className="walking-section-head">
          <span className="walking-eyebrow">High water marks</span>
        </header>
        <div className="walking-records-grid">
          <div className="walking-record">
            <span className="walking-record-value">{fmtInt(r.best_day_steps)}</span>
            <span className="walking-record-label">steps in a single day</span>
            <span className="walking-record-date">{fmtDate(r.best_day_date)}</span>
          </div>
          <div className="walking-record">
            <span className="walking-record-value">{r.longest_streak_days} days</span>
            <span className="walking-record-label">longest streak over 10,000</span>
            <span className="walking-record-date">
              {r.longest_streak_end ? `ended ${fmtDate(r.longest_streak_end)}` : "—"}
            </span>
          </div>
          <div className="walking-record">
            <span className="walking-record-value">{(r.longest_walk_m / 1000).toFixed(1)} km</span>
            <span className="walking-record-label">longest single walk</span>
            <span className="walking-record-date">{fmtDate(r.longest_walk_date)}</span>
          </div>
          <div className="walking-record">
            <span className="walking-record-value">{fmtInt(ov.total.walks)}</span>
            <span className="walking-record-label">recorded walks</span>
            <span className="walking-record-date">
              {firstYear}–{lastYear}
            </span>
          </div>
        </div>
      </section>
    </div>
  );
}
