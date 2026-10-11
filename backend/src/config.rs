use std::env;

#[derive(Clone, Debug)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub cors_origin: String,
    pub ingestion_interval_hours: u64,
    pub run_mode: RunMode,
    pub database_url: Option<String>,
    pub enable_background_ingestion: bool,
    /// Geocode facilities whose source had no coordinates (on by default).
    pub geocoding_enabled: bool,
    /// Cap on new geocoder lookups per refresh; cached addresses are always applied.
    pub max_geocodes_per_run: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunMode {
    Api,
    Worker,
    RefreshOnce,
}

impl Settings {
    pub fn from_env() -> Self {
        let run_mode = match env::var("CLEANPLATED_RUN_MODE")
            .unwrap_or_else(|_| "api".into())
            .to_ascii_lowercase()
            .as_str()
        {
            "worker" => RunMode::Worker,
            "refresh_once" => RunMode::RefreshOnce,
            _ => RunMode::Api,
        };

        Self {
            host: env::var("CLEANPLATED_HOST").unwrap_or_else(|_| "0.0.0.0".into()),
            port: env::var("CLEANPLATED_PORT")
                .ok()
                .and_then(|value| value.parse::<u16>().ok())
                .unwrap_or(8080),
            cors_origin: env::var("CLEANPLATED_CORS_ORIGIN")
                .unwrap_or_else(|_| "http://localhost:5173".into()),
            ingestion_interval_hours: env::var("CLEANPLATED_INGESTION_INTERVAL_HOURS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(24),
            run_mode,
            database_url: env::var("DATABASE_URL")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
            enable_background_ingestion: env::var("CLEANPLATED_ENABLE_BACKGROUND_INGESTION")
                .ok()
                .map(|value| {
                    matches!(
                        value.to_ascii_lowercase().as_str(),
                        "1" | "true" | "yes" | "on"
                    )
                })
                .unwrap_or(false),
            geocoding_enabled: env::var("CLEANPLATED_GEOCODING_ENABLED")
                .ok()
                .map(|value| {
                    !matches!(
                        value.to_ascii_lowercase().as_str(),
                        "0" | "false" | "no" | "off"
                    )
                })
                .unwrap_or(true),
            max_geocodes_per_run: env::var("CLEANPLATED_GEOCODING_MAX_PER_RUN")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(25_000),
        }
    }
}
