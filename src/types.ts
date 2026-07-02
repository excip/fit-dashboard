export type Activity = {
  id: number;
  file_name: string;
  activity_name: string;
  sport: string;
  device: string;
  start_ts_utc: string;
  end_ts_utc: string;
  duration_s: number;
  distance_m: number;
  start_latitude?: number;
  start_longitude?: number;
  metadata_json?: string;
};

export type RecordPoint = {
  timestamp_ms: number;
  latitude?: number;
  longitude?: number;
  altitude_m?: number;
  distance_m?: number;
  speed_m_s?: number;
  heart_rate?: number;
  cadence?: number;
  power?: number;
  temperature_c?: number;
};

export type OverviewStats = {
  activity_count: number;
  total_distance_m: number;
  total_duration_s: number;
};

export type TripCategory = "day_hike" | "weekend" | "thru_hike";

export type HikingOverview = {
  year: number | null;
  total_distance_m: number;
  total_steps: number;
  total_gain: number;
  total_loss: number;
  hike_count: number;
  trip_count: number;
};

export type Trip = {
  id: number;
  category: TripCategory;
  start_date: string;
  end_date: string;
  nights: number;
  activity_ids: number[];
  total_distance_m: number;
  total_gain: number;
  total_loss: number;
  total_steps: number;
};
