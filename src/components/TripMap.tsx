import { useEffect, useRef } from "react";
import maplibregl from "maplibre-gl";
import type { RecordPoint } from "../types";
import { useSettingsStore } from "../stores/settingsStore";
import { styleFromMap } from "../lib/mapStyle";

type Props = { tracks: RecordPoint[][] };

/** Stitched multi-day route map: one line per day's activity. */
export function TripMap({ tracks }: Props) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const mapRef = useRef<maplibregl.Map | null>(null);
  const theme = useSettingsStore((s) => s.theme);
  const mapStyle = useSettingsStore((s) => s.mapStyle);

  useEffect(() => {
    if (!containerRef.current) return;
    const coordsPerTrack = tracks
      .map((t) => t.filter((p) => p.latitude != null && p.longitude != null)
        .map((p) => [p.longitude as number, p.latitude as number]))
      .filter((c) => c.length > 1);
    if (coordsPerTrack.length === 0) return;

    const map = new maplibregl.Map({
      container: containerRef.current,
      style: styleFromMap(mapStyle, theme === "dark" ? "dark" : "light"),
      attributionControl: { compact: true },
    });
    mapRef.current = map;
    map.addControl(new maplibregl.NavigationControl({ showCompass: false }));

    map.on("load", () => {
      map.addSource("trip", {
        type: "geojson",
        data: {
          type: "Feature",
          properties: {},
          geometry: { type: "MultiLineString", coordinates: coordsPerTrack },
        },
      });
      map.addLayer({
        id: "trip-line",
        type: "line",
        source: "trip",
        paint: { "line-color": "#2f7fd1", "line-width": 3 },
        layout: { "line-cap": "round", "line-join": "round" },
      });
      const bounds = new maplibregl.LngLatBounds();
      for (const track of coordsPerTrack) for (const c of track) bounds.extend(c as [number, number]);
      map.fitBounds(bounds, { padding: 40, animate: false });
    });

    return () => { map.remove(); mapRef.current = null; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tracks, mapStyle, theme]);

  if (tracks.every((t) => !t.some((p) => p.latitude != null))) return null;
  return <div className="trip-map" ref={containerRef} />;
}
