use std::{collections::HashMap, env, time::Duration};

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use reqwest::Client;
use serde::{Deserialize, de::DeserializeOwned};
use tracing::warn;

use crate::{
    application::dto::SourceFacilityInput, domain::entities::Jurisdiction,
    infrastructure::connectors::HealthDataConnector,
};

const DEFAULT_INVENTORY_URL: &str = "https://services.arcgis.com/RmCCgQtiZLDCtblq/arcgis/rest/services/Environmental_Health_Restaurant_and_Market_Inventory_04012026/FeatureServer";
const DEFAULT_INSPECTIONS_URL: &str = "https://services.arcgis.com/RmCCgQtiZLDCtblq/arcgis/rest/services/Environmental_Health_Restaurant_and_Market_Inspections_04012023_to_033120026/FeatureServer";
const DEFAULT_INVENTORY_CSV_URL: &str =
    "https://www.arcgis.com/sharing/rest/content/items/4f31c9a99e444a40a3806e3bbe7b5fdd/data";
const DEFAULT_INSPECTIONS_CSV_URL: &str =
    "https://www.arcgis.com/sharing/rest/content/items/19b6607ac82c4512b10811870975dbdc/data";
const DEFAULT_PAGE_SIZE: usize = 2_000;

#[derive(Clone)]
pub struct LaCountyConnector {
    client: Client,
    inventory_url: String,
    inspections_url: String,
    inventory_csv_url: String,
    inspections_csv_url: String,
    page_size: usize,
    max_records: Option<usize>,
}

impl Default for LaCountyConnector {
    fn default() -> Self {
        Self::from_env()
    }
}

impl LaCountyConnector {
    pub fn from_env() -> Self {
        let inventory_url = env::var("CLEANPLATED_LA_INVENTORY_URL")
            .unwrap_or_else(|_| DEFAULT_INVENTORY_URL.to_owned());
        let inspections_url = env::var("CLEANPLATED_LA_INSPECTIONS_URL")
            .unwrap_or_else(|_| DEFAULT_INSPECTIONS_URL.to_owned());
        let inventory_csv_url = env::var("CLEANPLATED_LA_INVENTORY_CSV_URL")
            .unwrap_or_else(|_| DEFAULT_INVENTORY_CSV_URL.to_owned());
        let inspections_csv_url = env::var("CLEANPLATED_LA_INSPECTIONS_CSV_URL")
            .unwrap_or_else(|_| DEFAULT_INSPECTIONS_CSV_URL.to_owned());
        let page_size = env::var("CLEANPLATED_LA_PAGE_SIZE")
            .ok()
            .or_else(|| env::var("CLEANPLATED_LA_LIMIT").ok())
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .max(1);
        let max_records = env::var("CLEANPLATED_LA_MAX_RECORDS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0);
        let timeout_secs = env::var("CLEANPLATED_LA_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(20);

        let client = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            inventory_url,
            inspections_url,
            inventory_csv_url,
            inspections_csv_url,
            page_size,
            max_records,
        }
    }

    async fn query_features<T>(
        &self,
        source_url: &str,
        where_clause: &str,
        out_fields: &str,
        order_by_fields: Option<&str>,
    ) -> Result<Vec<T>>
    where
        T: DeserializeOwned,
    {
        let mut offset = 0usize;
        let mut rows = Vec::<T>::new();

        loop {
            let mut query = vec![
                ("where".to_owned(), where_clause.to_owned()),
                ("outFields".to_owned(), out_fields.to_owned()),
                ("resultOffset".to_owned(), offset.to_string()),
                ("resultRecordCount".to_owned(), self.page_size.to_string()),
                ("f".to_owned(), "json".to_owned()),
            ];
            if let Some(order_by) = order_by_fields {
                query.push(("orderByFields".to_owned(), order_by.to_owned()));
            }

            let response = self
                .client
                .get(format!("{}/0/query", source_url.trim_end_matches('/')))
                .query(&query)
                .send()
                .await?
                .error_for_status()?
                .json::<serde_json::Value>()
                .await?;
            if let Some(error) = response.get("error") {
                anyhow::bail!("ArcGIS query returned error for {source_url}: {error}");
            }

            let response: ArcGisResponse<T> =
                serde_json::from_value(response).with_context(|| {
                    format!("ArcGIS response missing expected features for {source_url}")
                })?;

            let page_count = response.features.len();
            rows.extend(response.features.into_iter().map(|f| f.attributes));

            if let Some(max_records) = self.max_records {
                if rows.len() >= max_records {
                    rows.truncate(max_records);
                    break;
                }
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

        Ok(rows)
    }

    async fn fetch_inspections(&self) -> Result<Vec<LaInspectionAttrs>> {
        match self.query_features(
            &self.inspections_url,
            "FACILITY_ID IS NOT NULL",
            "ACTIVITY_DATE,FACILITY_ID,FACILITY_NAME,FACILITY_ADDRESS,FACILITY_CITY,FACILITY_STATE,FACILITY_ZIP,SCORE,GRADE",
            Some("ACTIVITY_DATE DESC"),
        )
        .await
        .context("LA inspections FeatureServer request failed")
        {
            Ok(records) => Ok(records),
            Err(error) => {
                warn!(error = %error, "LA inspections FeatureServer failed; falling back to CSV item");
                self.fetch_inspections_csv().await
            }
        }
    }

    async fn fetch_inventory(&self) -> Result<HashMap<String, LaInventoryAttrs>> {
        let rows: Vec<LaInventoryAttrs> = match self
            .query_features(
                &self.inventory_url,
                "FACILITY_ID IS NOT NULL",
                "FACILITY_ID,FACILITY_NAME,FACILITY_ADDRESS,FACILITY_CITY,FACILITY__STATE,FACILITY_ZIP,FACILITY_LATITUDE,FACILITY_LONGITUDE",
                None,
            )
            .await
            .context("LA inventory FeatureServer request failed")
        {
            Ok(records) => records,
            Err(error) => {
                warn!(error = %error, "LA inventory FeatureServer failed; falling back to CSV item");
                self.fetch_inventory_csv().await?
            }
        };

        let mut inventory_map = HashMap::with_capacity(rows.len());
        for attrs in rows {
            if let Some(id) = attrs.facility_id.clone() {
                inventory_map.insert(id.trim().to_owned(), attrs);
            }
        }

        Ok(inventory_map)
    }

    async fn fetch_inspections_csv(&self) -> Result<Vec<LaInspectionAttrs>> {
        let body = self
            .client
            .get(&self.inspections_csv_url)
            .send()
            .await
            .context("LA inspections CSV request failed")?
            .error_for_status()
            .context("LA inspections CSV returned non-success status")?
            .text()
            .await
            .context("LA inspections CSV body read failed")?;

        let mut rows = csv::ReaderBuilder::new()
            .flexible(true)
            .trim(csv::Trim::All)
            .from_reader(body.as_bytes())
            .deserialize::<LaInspectionCsvRow>()
            .filter_map(|row| match row {
                Ok(row) => Some(row.into_attrs()),
                Err(error) => {
                    warn!(%error, "Skipping malformed LA inspections CSV row");
                    None
                }
            })
            .collect::<Vec<_>>();

        rows.sort_by(|left, right| right.activity_date.cmp(&left.activity_date));
        if let Some(max_records) = self.max_records {
            rows.truncate(max_records);
        }

        Ok(rows)
    }

    async fn fetch_inventory_csv(&self) -> Result<Vec<LaInventoryAttrs>> {
        let body = self
            .client
            .get(&self.inventory_csv_url)
            .send()
            .await
            .context("LA inventory CSV request failed")?
            .error_for_status()
            .context("LA inventory CSV returned non-success status")?
            .text()
            .await
            .context("LA inventory CSV body read failed")?;

        Ok(csv::ReaderBuilder::new()
            .flexible(true)
            .trim(csv::Trim::All)
            .from_reader(body.as_bytes())
            .deserialize::<LaInventoryCsvRow>()
            .filter_map(|row| match row {
                Ok(row) => Some(row.into_attrs()),
                Err(error) => {
                    warn!(%error, "Skipping malformed LA inventory CSV row");
                    None
                }
            })
            .collect::<Vec<_>>())
    }
}

#[async_trait]
impl HealthDataConnector for LaCountyConnector {
    fn source_name(&self) -> &'static str {
        "la_county_open_data"
    }

    async fn fetch_facilities(&self) -> Result<Vec<SourceFacilityInput>> {
        let inspections = self.fetch_inspections().await?;

        let inventory = match self.fetch_inventory().await {
            Ok(records) => records,
            Err(error) => {
                warn!(error = %error, "LA inventory enrichment failed; proceeding without inventory join");
                HashMap::new()
            }
        };

        let facilities = inspections
            .into_iter()
            .filter_map(|inspection| {
                let facility_id = inspection.facility_id?.trim().to_owned();
                let inv = inventory.get(&facility_id);

                let (latitude, longitude) = inv
                    .map(|record| {
                        (
                            record.facility_latitude.unwrap_or(34.0522),
                            record.facility_longitude.unwrap_or(-118.2437),
                        )
                    })
                    .unwrap_or((34.0522, -118.2437));

                let inspected_at = inspection
                    .activity_date
                    .and_then(DateTime::from_timestamp_millis)
                    .unwrap_or_else(Utc::now);

                Some(SourceFacilityInput {
                    source_id: facility_id,
                    name: inspection
                        .facility_name
                        .or_else(|| inv.and_then(|record| record.facility_name.clone()))
                        .unwrap_or_else(|| "Unknown Facility".to_owned()),
                    address: inspection
                        .facility_address
                        .or_else(|| inv.and_then(|record| record.facility_address.clone()))
                        .unwrap_or_default(),
                    city: inspection
                        .facility_city
                        .or_else(|| inv.and_then(|record| record.facility_city.clone()))
                        .unwrap_or_else(|| "Los Angeles".to_owned()),
                    state: inspection
                        .facility_state
                        .or_else(|| inv.and_then(|record| record.facility_state.clone()))
                        .unwrap_or_else(|| "CA".to_owned()),
                    postal_code: inspection
                        .facility_zip
                        .or_else(|| inv.and_then(|record| record.facility_zip.clone()))
                        .unwrap_or_default(),
                    latitude,
                    longitude,
                    jurisdiction: Jurisdiction::LosAngelesCounty,
                    inspected_at,
                    raw_score: inspection.score.map(|score| score as f32),
                    letter_grade: inspection.grade,
                    placard_status: None,
                    violations: Vec::new(),
                })
            })
            .collect::<Vec<_>>();

        Ok(facilities)
    }
}

#[derive(Debug, Deserialize)]
struct ArcGisResponse<T> {
    #[serde(rename = "exceededTransferLimit")]
    exceeded_transfer_limit: Option<bool>,
    features: Vec<ArcGisFeature<T>>,
}

#[derive(Debug, Deserialize)]
struct ArcGisFeature<T> {
    attributes: T,
}

#[derive(Debug, Deserialize)]
struct LaInspectionAttrs {
    #[serde(rename = "ACTIVITY_DATE")]
    activity_date: Option<i64>,
    #[serde(rename = "FACILITY_ID")]
    facility_id: Option<String>,
    #[serde(rename = "FACILITY_NAME")]
    facility_name: Option<String>,
    #[serde(rename = "FACILITY_ADDRESS")]
    facility_address: Option<String>,
    #[serde(rename = "FACILITY_CITY")]
    facility_city: Option<String>,
    #[serde(rename = "FACILITY_STATE")]
    facility_state: Option<String>,
    #[serde(rename = "FACILITY_ZIP")]
    facility_zip: Option<String>,
    #[serde(rename = "SCORE")]
    score: Option<f64>,
    #[serde(rename = "GRADE")]
    grade: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LaInventoryAttrs {
    #[serde(rename = "FACILITY_ID")]
    facility_id: Option<String>,
    #[serde(rename = "FACILITY_NAME")]
    facility_name: Option<String>,
    #[serde(rename = "FACILITY_ADDRESS")]
    facility_address: Option<String>,
    #[serde(rename = "FACILITY_CITY")]
    facility_city: Option<String>,
    #[serde(rename = "FACILITY__STATE")]
    facility_state: Option<String>,
    #[serde(rename = "FACILITY_ZIP")]
    facility_zip: Option<String>,
    #[serde(rename = "FACILITY_LATITUDE")]
    facility_latitude: Option<f64>,
    #[serde(rename = "FACILITY_LONGITUDE")]
    facility_longitude: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct LaInspectionCsvRow {
    #[serde(rename = "ACTIVITY DATE")]
    activity_date: Option<String>,
    #[serde(rename = "FACILITY ID")]
    facility_id: Option<String>,
    #[serde(rename = "FACILITY NAME")]
    facility_name: Option<String>,
    #[serde(rename = "FACILITY ADDRESS")]
    facility_address: Option<String>,
    #[serde(rename = "FACILITY CITY")]
    facility_city: Option<String>,
    #[serde(rename = "FACILITY STATE")]
    facility_state: Option<String>,
    #[serde(rename = "FACILITY ZIP")]
    facility_zip: Option<String>,
    #[serde(rename = "SCORE")]
    score: Option<f64>,
    #[serde(rename = "GRADE")]
    grade: Option<String>,
}

impl LaInspectionCsvRow {
    fn into_attrs(self) -> LaInspectionAttrs {
        LaInspectionAttrs {
            activity_date: self
                .activity_date
                .as_deref()
                .and_then(parse_lacounty_csv_date_millis),
            facility_id: self.facility_id,
            facility_name: self.facility_name,
            facility_address: self.facility_address,
            facility_city: self.facility_city,
            facility_state: self.facility_state,
            facility_zip: self.facility_zip,
            score: self.score,
            grade: self.grade,
        }
    }
}

#[derive(Debug, Deserialize)]
struct LaInventoryCsvRow {
    #[serde(rename = "FACILITY ID")]
    facility_id: Option<String>,
    #[serde(rename = "FACILITY NAME")]
    facility_name: Option<String>,
    #[serde(rename = "FACILITY ADDRESS")]
    facility_address: Option<String>,
    #[serde(rename = "FACILITY CITY")]
    facility_city: Option<String>,
    #[serde(rename = "FACILITY  STATE")]
    facility_state: Option<String>,
    #[serde(rename = "FACILITY ZIP")]
    facility_zip: Option<String>,
    #[serde(rename = "FACILITY LATITUDE")]
    facility_latitude: Option<f64>,
    #[serde(rename = "FACILITY LONGITUDE")]
    facility_longitude: Option<f64>,
}

impl LaInventoryCsvRow {
    fn into_attrs(self) -> LaInventoryAttrs {
        LaInventoryAttrs {
            facility_id: self.facility_id,
            facility_name: self.facility_name,
            facility_address: self.facility_address,
            facility_city: self.facility_city,
            facility_state: self.facility_state,
            facility_zip: self.facility_zip,
            facility_latitude: self.facility_latitude,
            facility_longitude: self.facility_longitude,
        }
    }
}

fn parse_lacounty_csv_date_millis(value: &str) -> Option<i64> {
    let date = NaiveDate::parse_from_str(value.trim(), "%m/%d/%Y").ok()?;
    let naive = date.and_hms_opt(0, 0, 0)?;
    Some(Utc.from_utc_datetime(&naive).timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lacounty_csv_date_uses_utc_midnight_millis() {
        assert_eq!(
            parse_lacounty_csv_date_millis("04/01/2026"),
            Some(1_775_001_600_000)
        );
    }

    #[test]
    fn csv_inspection_row_maps_headers_with_spaces() {
        let csv = "ACTIVITY DATE,FACILITY ID,FACILITY NAME,FACILITY ADDRESS,FACILITY CITY,FACILITY STATE,FACILITY ZIP,SCORE,GRADE\n 04/01/2026,FA1,Name,Addr,City,CA,90001,97,A\n";
        let row = csv::ReaderBuilder::new()
            .trim(csv::Trim::All)
            .from_reader(csv.as_bytes())
            .deserialize::<LaInspectionCsvRow>()
            .next()
            .unwrap()
            .unwrap()
            .into_attrs();

        assert_eq!(row.facility_id.as_deref(), Some("FA1"));
        assert_eq!(row.activity_date, Some(1_775_001_600_000));
        assert_eq!(row.score, Some(97.0));
    }
}
