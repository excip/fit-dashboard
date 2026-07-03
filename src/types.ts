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
  name: string | null;
  start_date: string;
  end_date: string;
  nights: number;
  activity_ids: number[];
  total_distance_m: number;
  total_gain: number;
  total_loss: number;
  total_steps: number;
};

export type TripDay = {
  garmin_activity_id: number;
  dashboard_activity_id: number | null;
  date: string;
  distance_m: number;
  elevation_gain: number;
  elevation_loss: number;
  steps: number;
  duration_s: number;
  location_name: string | null;
};

export type RecoveryDay = {
  date: string;
  sleep_score: number | null;
  sleep_seconds: number | null;
  resting_hr: number | null;
  hrv_last_night_avg: number | null;
  body_battery_high: number | null;
  body_battery_low: number | null;
  avg_stress: number | null;
  training_readiness: number | null;
};

export type Superlatives = {
  longest_day_m: number;
  biggest_climb_m: number;
  highest_point_m: number | null;
};

export type TripDetail = {
  trip: Trip;
  merged: boolean;
  has_previous: boolean;
  superlatives: Superlatives;
  days: TripDay[];
  recovery: RecoveryDay[];
};
