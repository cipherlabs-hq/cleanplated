use std::{env, time::Duration};

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::Deserialize;

use crate::{
    application::dto::SourceFacilityInput,
    domain::entities::{Jurisdiction, Violation},
    infrastructure::connectors::HealthDataConnector,
};

const DEFAULT_BASE_URL: &str = "https://internal-sandiegocounty.data.socrata.com";
const DEFAULT_DATASET_ID: &str = "c5ez-ufrd";
const DEFAULT_PAGE_SIZE: usize = 5000;
const DEFAULT_TIMEOUT_SECS: u64 = 20;

#[derive(Clone)]
pub struct SanDiegoConnector {
    client: reqwest::Client,
    base_url: String,
    dataset_id: String,
    page_size: usize,
    max_records: Option<usize>,
    active_only: bool,
}

impl Default for SanDiegoConnector {
    fn default() -> Self {
        Self::from_env()
    }
}

impl SanDiegoConnector {
    pub fn from_env() -> Self {
        let base_url = env::var("CLEANPLATED_SD_SOCRATA_BASE_URL")
            .unwrap_or_else(|_| DEFAULT_BASE_URL.to_owned())
            .trim_end_matches('/')
            .to_owned();
        let dataset_id = env::var("CLEANPLATED_SD_SOCRATA_DATASET_ID")
            .unwrap_or_else(|_| DEFAULT_DATASET_ID.to_owned());
        let page_size = env::var("CLEANPLATED_SD_SOCRATA_PAGE_SIZE")
            .ok()
            .or_else(|| env::var("CLEANPLATED_SD_SOCRATA_LIMIT").ok())
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .max(1);
        let max_records = env::var("CLEANPLATED_SD_SOCRATA_MAX_RECORDS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0);
        let active_only = env::var("CLEANPLATED_SD_SOCRATA_ACTIVE_ONLY")
            .ok()
            .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
            .unwrap_or(true);
        let timeout_secs = env::var("CLEANPLATED_SD_SOCRATA_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_TIMEOUT_SECS);

        let mut headers = HeaderMap::new();
        if let Ok(app_token) = env::var("CLEANPLATED_SD_SOCRATA_APP_TOKEN") {
            let trimmed = app_token.trim();
            if !trimmed.is_empty()
                && let Ok(value) = HeaderValue::from_str(trimmed) {
                    headers.insert(HeaderName::from_static("x-app-token"), value);
                }
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .default_headers(headers)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            client,
            base_url,
            dataset_id,
            page_size,
            max_records,
            active_only,
        }
    }
}

#[derive(Debug, Deserialize)]
struct SanDiegoPermitRow {
    record_id: Option<String>,
    record_name: Option<String>,
    address: Option<String>,
    city: Option<String>,
    state: Option<String>,
    zip: Option<String>,
    latitude: Option<String>,
    longitude: Option<String>,
    last_updated: Option<String>,
    permit_status: Option<String>,
    // The county changed this column from a boolean to text ("A") in 2026, so accept either.
    active_permit: Option<serde_json::Value>,
}

#[async_trait]
impl HealthDataConnector for SanDiegoConnector {
    fn source_name(&self) -> &'static str {
        "san_diego_socrata"
    }

    async fn fetch_facilities(&self) -> Result<Vec<SourceFacilityInput>> {
        // Source reference:
        // docs/research/socal-food-safety-data-strategy.md
        // The strategic framework documents San Diego as Socrata/SODA-first.
        let endpoint = format!("{}/resource/{}.json", self.base_url, self.dataset_id);
        // Filter on active_permit in Rust rather than SoQL: comparing it in the query breaks
        // whenever the county changes the column's type, which fails the whole source.
        let where_clause = "record_id IS NOT NULL AND record_name IS NOT NULL";

        let mut offset = 0usize;
        let mut rows = Vec::<SanDiegoPermitRow>::new();

        loop {
            let query = vec![
                (
                    "$select".to_owned(),
                    "record_id,record_name,address,city,state,zip,latitude,longitude,last_updated,permit_status,active_permit".to_owned(),
                ),
                ("$where".to_owned(), where_clause.to_owned()),
                ("$order".to_owned(), "last_updated DESC".to_owned()),
                ("$limit".to_owned(), self.page_size.to_string()),
                ("$offset".to_owned(), offset.to_string()),
            ];

            let page = self
                .client
                .get(&endpoint)
                .query(&query)
                .send()
                .await
                .context("San Diego Socrata request failed")?
                .error_for_status()
                .context("San Diego Socrata request returned non-success status")?
                .json::<Vec<SanDiegoPermitRow>>()
                .await
                .context("San Diego Socrata response could not be parsed")?;

            let page_count = page.len();
            rows.extend(page);

            if let Some(max_records) = self.max_records
                && rows.len() >= max_records {
                    rows.truncate(max_records);
                    break;
                }

            if page_count == 0 || page_count < self.page_size {
                break;
            }

            offset = offset.saturating_add(page_count);
        }

        let active_only = self.active_only;
        let facilities = rows
            .into_iter()
            .filter(|row| {
                !active_only || is_active_permit(row.active_permit.as_ref()).unwrap_or(true)
            })
            .filter_map(map_row_to_source_input)
            .collect::<Vec<_>>();

        Ok(facilities)
    }
}

fn map_row_to_source_input(row: SanDiegoPermitRow) -> Option<SourceFacilityInput> {
    let source_id = row.record_id?;
    let name = row.record_name?;
    let city = row.city.unwrap_or_else(|| "San Diego".to_owned());

    // Most current records in c5ez-ufrd omit coordinates. Use city-level fallback
    // to keep the directory searchable until the full graded inspection feed is wired.
    let latitude = row
        .latitude
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or_else(|| city_fallback_coordinates(&city).0);
    let longitude = row
        .longitude
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or_else(|| city_fallback_coordinates(&city).1);

    let inspected_at = row
        .last_updated
        .as_deref()
        .and_then(parse_socrata_datetime)
        .unwrap_or_else(Utc::now);

    let (raw_score, letter_grade, placard_status) = derive_scoring_signals(
        row.permit_status.as_deref(),
        is_active_permit(row.active_permit.as_ref()).unwrap_or(true),
    );

    Some(SourceFacilityInput {
        source_id,
        name,
        address: row.address.unwrap_or_default(),
        city,
        state: row.state.unwrap_or_else(|| "CA".to_owned()),
        postal_code: row.zip.unwrap_or_default(),
        latitude,
        longitude,
        jurisdiction: Jurisdiction::SanDiegoCounty,
        inspected_at,
        raw_score,
        letter_grade,
        placard_status,
        violations: vec![Violation {
            code: "SD-PERMIT".to_owned(),
            description: "Derived from public permit status (Socrata Food Facility Permits feed)"
                .to_owned(),
            points: 0,
            critical: false,
        }],
    })
}

/// Reads `active_permit` as either a JSON boolean or a text flag. `None` means the value was
/// missing or unrecognised.
fn is_active_permit(value: Option<&serde_json::Value>) -> Option<bool> {
    match value? {
        serde_json::Value::Bool(active) => Some(*active),
        serde_json::Value::String(flag) => match flag.trim().to_ascii_lowercase().as_str() {
            "a" | "active" | "true" | "t" | "y" | "yes" | "1" => Some(true),
            "i" | "inactive" | "false" | "f" | "n" | "no" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn parse_socrata_datetime(value: &str) -> Option<DateTime<Utc>> {
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Some(parsed.with_timezone(&Utc));
    }

    if let Ok(parsed) = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.3f") {
        return Some(DateTime::from_naive_utc_and_offset(parsed, Utc));
    }

    if let Ok(parsed) = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S") {
        return Some(DateTime::from_naive_utc_and_offset(parsed, Utc));
    }

    None
}

fn derive_scoring_signals(
    permit_status: Option<&str>,
    active_permit: bool,
) -> (Option<f32>, Option<String>, Option<String>) {
    if !active_permit {
        return (Some(40.0), Some("C".to_owned()), Some("Red".to_owned()));
    }

    let status = permit_status.unwrap_or("").to_ascii_lowercase();

    if status.contains("renewed") {
        return (Some(92.0), Some("A".to_owned()), None);
    }

    if status.contains("issued") {
        return (Some(88.0), Some("A".to_owned()), None);
    }

    if status.contains("expired") || status.contains("suspend") || status.contains("revok") {
        return (Some(55.0), Some("C".to_owned()), Some("Yellow".to_owned()));
    }

    (None, None, None)
}

fn city_fallback_coordinates(city: &str) -> (f64, f64) {
    match city.trim().to_ascii_uppercase().as_str() {
        "SAN DIEGO" => (32.7157, -117.1611),
        "CHULA VISTA" => (32.6401, -117.0842),
        "ESCONDIDO" => (33.1192, -117.0864),
        "OCEANSIDE" => (33.1959, -117.3795),
        "CARLSBAD" => (33.1581, -117.3506),
        "EL CAJON" => (32.7948, -116.9625),
        "VISTA" => (33.2000, -117.2425),
        "SAN MARCOS" => (33.1434, -117.1661),
        "NATIONAL CITY" => (32.6781, -117.0992),
        "LA MESA" => (32.7678, -117.0231),
        "ENCINITAS" => (33.0369, -117.2919),
        "SANTEE" => (32.8384, -116.9739),
        "POWAY" => (32.9628, -117.0359),
        "IMPERIAL BEACH" => (32.5839, -117.1131),
        "LEMON GROVE" => (32.7426, -117.0317),
        _ => (32.7157, -117.1611),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn active_permit_accepts_the_current_text_flag_and_the_old_boolean() {
        assert_eq!(is_active_permit(Some(&json!("A"))), Some(true));
        assert_eq!(is_active_permit(Some(&json!("I"))), Some(false));
        assert_eq!(is_active_permit(Some(&json!(true))), Some(true));
        assert_eq!(is_active_permit(Some(&json!(false))), Some(false));
        assert_eq!(is_active_permit(Some(&json!("?"))), None);
        assert_eq!(is_active_permit(None), None);
    }

    #[test]
    fn a_current_row_maps_with_city_fallback_coordinates() {
        let row: SanDiegoPermitRow = serde_json::from_value(json!({
            "record_id": "DEH2026-FFPP-022426",
            "record_name": "IHOP",
            "permit_status": "Issued",
            "active_permit": "A",
            "address": "9708 MISSION GORGE RD",
            "city": "SANTEE",
            "state": "CA",
            "zip": "92071",
            "last_updated": "2026-08-10T00:00:00.000"
        }))
        .unwrap();

        let facility = map_row_to_source_input(row).unwrap();

        assert_eq!(facility.source_id, "DEH2026-FFPP-022426");
        assert_eq!(
            (facility.latitude, facility.longitude),
            (32.8384, -116.9739)
        );
        assert_eq!(facility.letter_grade.as_deref(), Some("A"));
    }

    /// Hits the live county API. Run with `cargo test -- --ignored san_diego_live`.
    #[tokio::test]
    #[ignore]
    async fn san_diego_live_fetch_returns_facilities() {
        let facilities = SanDiegoConnector::from_env()
            .fetch_facilities()
            .await
            .unwrap();
        assert!(
            facilities.len() > 1000,
            "only {} facilities",
            facilities.len()
        );
    }
}
