use std::{collections::HashMap, env, time::Duration};

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    application::dto::SourceFacilityInput, domain::entities::Jurisdiction,
    infrastructure::connectors::HealthDataConnector,
};

const DEFAULT_SAN_BERNARDINO_ARCGIS_URL: &str = "https://services.arcgis.com/OUDgwkiMsqiL8Tvp/arcgis/rest/services/San_Bernardio_Co_Food_Grades/FeatureServer";
const DEFAULT_PAGE_SIZE: usize = 1000;
const DEFAULT_TIMEOUT_SECS: u64 = 20;

#[derive(Clone)]
pub struct LivesBatchConnector {
    client: Client,
    san_bernardino_url: String,
    riverside_url: Option<String>,
    page_size: usize,
    max_records: Option<usize>,
}

impl Default for LivesBatchConnector {
    fn default() -> Self {
        Self::from_env()
    }
}

impl LivesBatchConnector {
    pub fn from_env() -> Self {
        let san_bernardino_url = env::var("CLEANPLATED_SBC_ARCGIS_URL")
            .unwrap_or_else(|_| DEFAULT_SAN_BERNARDINO_ARCGIS_URL.to_owned());
        let riverside_url = env::var("CLEANPLATED_RIVERSIDE_ARCGIS_URL")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        let page_size = env::var("CLEANPLATED_LIVES_PAGE_SIZE")
            .ok()
            .or_else(|| env::var("CLEANPLATED_LIVES_LIMIT").ok())
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .max(1);
        let max_records = env::var("CLEANPLATED_LIVES_MAX_RECORDS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0);
        let timeout_secs = env::var("CLEANPLATED_LIVES_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_TIMEOUT_SECS);

        let client = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            san_bernardino_url,
            riverside_url,
            page_size,
            max_records,
        }
    }

    async fn fetch_arcgis_facilities(
        &self,
        source_url: &str,
        jurisdiction: Jurisdiction,
        id_prefix: &str,
    ) -> Result<Vec<SourceFacilityInput>> {
        let endpoint = format!("{}/0/query", source_url.trim_end_matches('/'));

        let default_coordinates = match jurisdiction {
            Jurisdiction::SanBernardinoCounty => (34.1083, -117.2898),
            Jurisdiction::RiversideCounty => (33.9806, -117.3755),
            _ => (34.1083, -117.2898),
        };

        let mut facilities = Vec::new();
        let mut offset = 0usize;

        loop {
            let query = vec![
                ("where".to_owned(), "1=1".to_owned()),
                ("outFields".to_owned(), "*".to_owned()),
                ("resultOffset".to_owned(), offset.to_string()),
                ("resultRecordCount".to_owned(), self.page_size.to_string()),
                ("f".to_owned(), "json".to_owned()),
            ];

            let response: ArcGisResponse = self
                .client
                .get(&endpoint)
                .query(&query)
                .send()
                .await
                .with_context(|| format!("{} ArcGIS request failed", jurisdiction.label()))?
                .error_for_status()
                .with_context(|| {
                    format!(
                        "{} ArcGIS returned non-success status",
                        jurisdiction.label()
                    )
                })?
                .json::<ArcGisResponse>()
                .await
                .with_context(|| {
                    format!("{} ArcGIS response parse failed", jurisdiction.label())
                })?;

            let page_count = response.features.len();
            let page_start = facilities.len();

            facilities.extend(response.features.into_iter().enumerate().filter_map(
                |(idx, feature)| {
                    let attrs = feature.attributes;

                    let name = attr_string(&attrs, &["Facility_Name", "FACILITY_NAME", "name"])?;
                    let source_id = attr_string(
                        &attrs,
                        &[
                            "Facility_ID",
                            "FACILITY_ID",
                            "Permit_Number",
                            "permit_number",
                            "id",
                        ],
                    )
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| format!("{id_prefix}-{}", page_start + idx));

                    let city = attr_string(&attrs, &["City", "CITY", "city"])
                        .unwrap_or_else(|| jurisdiction.label().to_owned());
                    let state = attr_string(&attrs, &["State", "STATE", "state"])
                        .unwrap_or_else(|| "CA".to_owned());
                    let postal_code = attr_string(&attrs, &["Zip", "ZIP", "zip", "postal_code"])
                        .unwrap_or_default();

                    let latitude = attr_f64(&attrs, &["Latitude", "LATITUDE", "latitude"])
                        .unwrap_or(default_coordinates.0);
                    let longitude = attr_f64(&attrs, &["Longitude", "LONGITUDE", "longitude"])
                        .unwrap_or(default_coordinates.1);

                    let raw_score =
                        attr_f64(&attrs, &["Score", "SCORE", "score"]).map(|score| score as f32);
                    let letter_grade = raw_score.and_then(score_to_grade);
                    let inspected_at = attr_datetime(
                        &attrs,
                        &[
                            "Inspection_Date",
                            "INSPECTION_DATE",
                            "inspection_date",
                            "ACTIVITY_DATE",
                        ],
                    )
                    .unwrap_or_else(Utc::now);

                    Some(SourceFacilityInput {
                        source_id,
                        name,
                        address: attr_string(
                            &attrs,
                            &["Address", "FACILITY_ADDRESS", "address", "StreetAddress"],
                        )
                        .unwrap_or_default(),
                        city,
                        state,
                        postal_code,
                        latitude,
                        longitude,
                        jurisdiction: jurisdiction.clone(),
                        inspected_at,
                        raw_score,
                        letter_grade,
                        placard_status: None,
                        violations: Vec::new(),
                    })
                },
            ));

            if let Some(max_records) = self.max_records
                && facilities.len() >= max_records {
                    facilities.truncate(max_records);
                    break;
                }

            if page_count == 0 {
                break;
            }

            offset = offset.saturating_add(page_count);
            let exceeded = response.exceeded_transfer_limit.unwrap_or(false);
            if !exceeded && page_count < self.page_size {
                break;
            }
        }

        Ok(facilities)
    }
}

#[async_trait]
impl HealthDataConnector for LivesBatchConnector {
    fn source_name(&self) -> &'static str {
        "lives_batch_riv_sbc"
    }

    async fn fetch_facilities(&self) -> Result<Vec<SourceFacilityInput>> {
        let mut facilities = self
            .fetch_arcgis_facilities(
                &self.san_bernardino_url,
                Jurisdiction::SanBernardinoCounty,
                "sbc",
            )
            .await?;

        if let Some(riverside_url) = &self.riverside_url {
            facilities.extend(
                self.fetch_arcgis_facilities(riverside_url, Jurisdiction::RiversideCounty, "riv")
                    .await?,
            );
        }

        Ok(facilities)
    }
}

#[derive(Debug, Deserialize)]
struct ArcGisResponse {
    #[serde(rename = "exceededTransferLimit")]
    exceeded_transfer_limit: Option<bool>,
    #[serde(default)]
    features: Vec<ArcGisFeature>,
}

#[derive(Debug, Deserialize)]
struct ArcGisFeature {
    #[serde(default)]
    attributes: HashMap<String, Value>,
}

fn attr_string(attrs: &HashMap<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| attrs.get(*key))
        .and_then(|value| {
            if let Some(text) = value.as_str() {
                return Some(text.trim().to_owned());
            }
            if value.is_number() {
                return Some(value.to_string());
            }
            None
        })
        .filter(|value| !value.is_empty())
}

fn attr_f64(attrs: &HashMap<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        let value = attrs.get(*key)?;
        if let Some(number) = value.as_f64() {
            return Some(number);
        }

        value
            .as_str()
            .and_then(|text| text.trim().parse::<f64>().ok())
    })
}

fn attr_datetime(attrs: &HashMap<String, Value>, keys: &[&str]) -> Option<DateTime<Utc>> {
    let value = keys.iter().find_map(|key| attrs.get(*key))?;

    if let Some(raw_number) = value.as_i64() {
        return timestamp_to_datetime(raw_number);
    }

    if let Some(text) = value.as_str() {
        if let Ok(raw_number) = text.trim().parse::<i64>() {
            return timestamp_to_datetime(raw_number);
        }

        if let Ok(parsed) = DateTime::parse_from_rfc3339(text.trim()) {
            return Some(parsed.with_timezone(&Utc));
        }

        if let Ok(parsed) = chrono::NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d") {
            return parsed
                .and_hms_opt(0, 0, 0)
                .map(|naive| DateTime::from_naive_utc_and_offset(naive, Utc));
        }

        if let Ok(parsed) = chrono::NaiveDate::parse_from_str(text.trim(), "%m/%d/%Y") {
            return parsed
                .and_hms_opt(0, 0, 0)
                .map(|naive| DateTime::from_naive_utc_and_offset(naive, Utc));
        }
    }

    None
}

fn timestamp_to_datetime(raw: i64) -> Option<DateTime<Utc>> {
    if raw > 10_000_000_000 {
        DateTime::from_timestamp_millis(raw)
    } else {
        DateTime::from_timestamp(raw, 0)
    }
}

fn score_to_grade(score: f32) -> Option<String> {
    if !(0.0..=100.0).contains(&score) {
        return None;
    }

    if score >= 90.0 {
        Some("A".to_owned())
    } else if score >= 80.0 {
        Some("B".to_owned())
    } else {
        Some("C".to_owned())
    }
}
