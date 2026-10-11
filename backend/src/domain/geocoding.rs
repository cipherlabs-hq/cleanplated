use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// One address to place on the map. `key` identifies it in the cache and in the result.
#[derive(Clone, Debug, PartialEq)]
pub struct GeocodeRequest {
    pub key: String,
    pub street: String,
    pub city: String,
    pub state: String,
    pub postal_code: String,
}

/// The outcome for one address. `coordinates` is `None` when the geocoder found no match; that is
/// cached too, so an unmatchable address is not retried on every run.
#[derive(Clone, Debug, PartialEq)]
pub struct GeocodeResult {
    pub key: String,
    pub coordinates: Option<(f64, f64)>,
    pub geocoded_at: DateTime<Utc>,
}

#[async_trait]
pub trait Geocoder: Send + Sync {
    /// Returns a result for every request the geocoder answered. A request missing from the
    /// output was not attempted (for example, its batch failed) and should be retried later.
    async fn geocode(&self, requests: &[GeocodeRequest]) -> anyhow::Result<Vec<GeocodeResult>>;
}

/// Builds the cache key for an address: trimmed, lowercased, and pipe-separated.
pub fn address_key(street: &str, city: &str, state: &str, postal_code: &str) -> String {
    [street, city, state, postal_code]
        .iter()
        .map(|part| {
            part.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase()
        })
        .collect::<Vec<_>>()
        .join("|")
}
