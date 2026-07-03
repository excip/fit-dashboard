import { type StyleSpecification } from "maplibre-gl";
import type { MapStyle } from "../stores/settingsStore";

export type BaseMapInfo = { label: string; tileUrl: string; attribution: string };

export const BASEMAPS: Record<"light" | "dark" | "openstreet" | "topo" | "satellite", BaseMapInfo> = {
  light: {
    label: "Light",
    tileUrl: "https://a.basemaps.cartocdn.com/light_all/{z}/{x}/{y}.png",
    attribution: "© OpenStreetMap contributors © CARTO"
  },
  openstreet: {
    label: "OpenStreet",
    tileUrl: "https://tile.openstreetmap.org/{z}/{x}/{y}.png",
    attribution: "© OpenStreetMap contributors"
  },
  topo: {
    label: "Topo",
    tileUrl: "https://a.tile.opentopomap.org/{z}/{x}/{y}.png",
    attribution: "© OpenStreetMap contributors, SRTM | OpenTopoMap"
  },
  satellite: {
    label: "Satellite",
    tileUrl: "https://services.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}",
    attribution: "Tiles © Esri, Maxar, Earthstar Geographics"
  },
  dark: {
    label: "Dark",
    tileUrl: "https://a.basemaps.cartocdn.com/dark_all/{z}/{x}/{y}.png",
    attribution: "© OpenStreetMap contributors © CARTO"
  }
};

export function styleFromMap(ms: MapStyle, theme: "light" | "dark"): StyleSpecification {
  const actualStyle = ms === "default" ? theme : ms;
  const s = BASEMAPS[actualStyle as keyof typeof BASEMAPS];
  return {
    version: 8,
    sources: { basemap: { type: "raster", tiles: [s.tileUrl], tileSize: 256, attribution: s.attribution } },
    layers: [{ id: "basemap", type: "raster", source: "basemap" }]
  };
}
