use std::{
    collections::HashSet,
    collections::hash_map::DefaultHasher,
    env,
    hash::{Hash, Hasher},
    time::Duration,
};

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use reqwest::Client;
use serde_json::{Map, Value, json};

use crate::{
    application::dto::SourceFacilityInput,
    domain::entities::{Jurisdiction, Violation},
    infrastructure::connectors::HealthDataConnector,
};

const DEFAULT_TIMEOUT_SECS: u64 = 20;
const DEFAULT_BROWSER_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";
const DEFAULT_OC_LIVE_ENDPOINT: &str = "https://inspections.myhealthdepartment.com/";
const DEFAULT_OC_LIVE_PATH: &str = "orange-county-back-li";
const DEFAULT_OC_LIVE_LEGACY_ENDPOINT: &str =
    "https://inspections.myhealthdepartment.com/genericEndpoint";
const DEFAULT_OC_LIVE_LEGACY_REFERER: &str =
    "https://inspections.myhealthdepartment.com/orange-county/restaurant-closures";
const DEFAULT_OC_LIVE_PAGE_SIZE: usize = 25;
const DEFAULT_OC_LIVE_MAX_RECORDS: usize = 30_000;
const DEFAULT_OC_LIVE_PER_TERM_MAX_RECORDS: usize = 20_000;
const DEFAULT_OC_LIVE_DAYS_WINDOW: u32 = 3_650;
const DEFAULT_PASADENA_DIRECTORY_URL: &str = "https://services2.arcgis.com/zNjnZafDYCAJAbN0/arcgis/rest/services/Pasadena_Restaurant_Directory/FeatureServer/0";
const DEFAULT_PASADENA_PAGE_SIZE: usize = 200;
const DEFAULT_PASADENA_MAX_RECORDS: usize = 5_000;

#[derive(Clone)]
pub struct CpraConnector {
    client: Client,
    orange_county_url: Option<String>,
    pasadena_url: Option<String>,
    oc_live_enabled: bool,
    oc_live_endpoint: String,
    oc_live_path: String,
    oc_live_search_terms: Vec<String>,
    oc_live_page_size: usize,
    oc_live_max_records: usize,
    oc_live_per_term_max_records: usize,
    oc_live_days_window: u32,
    pasadena_live_enabled: bool,
    pasadena_directory_url: String,
    pasadena_page_size: usize,
    pasadena_max_records: usize,
}

impl Default for CpraConnector {
    fn default() -> Self {
        Self::from_env()
    }
}

impl CpraConnector {
    pub fn from_env() -> Self {
        let orange_county_url = env::var("CLEANPLATED_OC_CPRA_EXPORT_URL")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        let pasadena_url = env::var("CLEANPLATED_PASADENA_CPRA_EXPORT_URL")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        let timeout_secs = env::var("CLEANPLATED_CPRA_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_TIMEOUT_SECS);

        let oc_live_enabled = parse_bool_env("CLEANPLATED_OC_LIVE_ENABLED", true);
        let oc_live_endpoint = env::var("CLEANPLATED_OC_LIVE_ENDPOINT")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_OC_LIVE_ENDPOINT.to_owned());
        let oc_live_path = env::var("CLEANPLATED_OC_LIVE_PATH")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_OC_LIVE_PATH.to_owned());
        let oc_live_search_terms = parse_search_terms(
            env::var("CLEANPLATED_OC_LIVE_SEARCH_TERMS").ok(),
            default_orange_county_search_terms(),
        );
        let oc_live_page_size = env::var("CLEANPLATED_OC_LIVE_PAGE_SIZE")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(DEFAULT_OC_LIVE_PAGE_SIZE);
        let oc_live_max_records = env::var("CLEANPLATED_OC_LIVE_MAX_RECORDS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(DEFAULT_OC_LIVE_MAX_RECORDS);
        let oc_live_per_term_max_records = env::var("CLEANPLATED_OC_LIVE_PER_TERM_MAX_RECORDS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(DEFAULT_OC_LIVE_PER_TERM_MAX_RECORDS);
        let oc_live_days_window = env::var("CLEANPLATED_OC_LIVE_DAYS_WINDOW")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(DEFAULT_OC_LIVE_DAYS_WINDOW);

        let pasadena_live_enabled = parse_bool_env("CLEANPLATED_PASADENA_LIVE_ENABLED", true);
        let pasadena_directory_url = env::var("CLEANPLATED_PASADENA_DIRECTORY_URL")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_PASADENA_DIRECTORY_URL.to_owned());
        let pasadena_page_size = env::var("CLEANPLATED_PASADENA_PAGE_SIZE")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(DEFAULT_PASADENA_PAGE_SIZE);
        let pasadena_max_records = env::var("CLEANPLATED_PASADENA_MAX_RECORDS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(DEFAULT_PASADENA_MAX_RECORDS);

        let client = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .cookie_store(true)
            .user_agent(DEFAULT_BROWSER_UA)
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            orange_county_url,
            pasadena_url,
            oc_live_enabled,
            oc_live_endpoint,
            oc_live_path,
            oc_live_search_terms,
            oc_live_page_size,
            oc_live_max_records,
            oc_live_per_term_max_records,
            oc_live_days_window,
            pasadena_live_enabled,
            pasadena_directory_url,
            pasadena_page_size,
            pasadena_max_records,
        }
    }

    async fn fetch_export(
        &self,
        source_url: &str,
        jurisdiction: Jurisdiction,
        id_prefix: &str,
    ) -> Result<Vec<SourceFacilityInput>> {
        let response = self
            .client
            .get(source_url)
            .send()
            .await
            .with_context(|| format!("{} CPRA export request failed", jurisdiction.label()))?
            .error_for_status()
            .with_context(|| {
                format!(
                    "{} CPRA export returned non-success status",
                    jurisdiction.label()
                )
            })?;

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let body = response
            .text()
            .await
            .with_context(|| format!("{} CPRA export body read failed", jurisdiction.label()))?;

        if body.trim().is_empty() {
            return Ok(Vec::new());
        }

        let records = if content_type.contains("json") || body.trim_start().starts_with(['{', '['])
        {
            parse_json_records(&body)?
        } else {
            parse_csv_records(&body)?
        };

        Ok(records
            .into_iter()
            .enumerate()
            .filter_map(|(idx, record)| map_record(record, jurisdiction.clone(), id_prefix, idx))
            .collect::<Vec<_>>())
    }

    async fn fetch_orange_county_live(&self) -> Result<Vec<SourceFacilityInput>> {
        let page_size = self.oc_live_page_size.clamp(1, 250);
        let max_records = self.oc_live_max_records.max(1);
        let max_per_term = self.oc_live_per_term_max_records.max(page_size);
        let base_page_url = format!(
            "https://inspections.myhealthdepartment.com/{}",
            self.oc_live_path
        );

        let _ = self
            .client
            .get(&base_page_url)
            .send()
            .await
            .with_context(|| {
                format!("Orange County live bootstrap request failed: {base_page_url}")
            })?
            .error_for_status()
            .context("Orange County live bootstrap returned non-success status")?;

        let mut rows = Vec::new();
        let mut seen_inspections = HashSet::new();

        for term in &self.oc_live_search_terms {
            if rows.len() >= max_records {
                break;
            }

            let mut start = 0usize;
            let baseline_rows = rows.len();
            loop {
                if rows.len() >= max_records || start >= max_per_term {
                    break;
                }

                let payload = json!({
                    "data": {
                        "path": self.oc_live_path,
                        "searchStr": term,
                        "programName": "",
                        "filters": {},
                        "start": start,
                        "count": page_size,
                        "returnHtml": false,
                        "lat": "0",
                        "lng": "0",
                        "sort": {}
                    },
                    "task": "searchInspections"
                });

                let response = self
                    .client
                    .post(&self.oc_live_endpoint)
                    .header(reqwest::header::ACCEPT, "application/json, text/plain, */*")
                    .header(reqwest::header::ORIGIN, "https://inspections.myhealthdepartment.com")
                    .header(reqwest::header::REFERER, &base_page_url)
                    .header("X-Requested-With", "XMLHttpRequest")
                    .json(&payload)
                    .send()
                    .await
                    .with_context(|| {
                        format!(
                            "Orange County live request failed (term='{term}', start={start})"
                        )
                    })?
                    .error_for_status()
                    .with_context(|| {
                        format!(
                            "Orange County live returned non-success status (term='{term}', start={start})"
                        )
                    })?;

                let body = response
                    .text()
                    .await
                    .context("Orange County live response body read failed")?;
                if body.trim().is_empty() {
                    break;
                }

                let parsed = parse_json_relaxed(&body).with_context(|| {
                    format!("Orange County live JSON parse failed (term='{term}')")
                })?;
                let Some(items) = parsed.as_array() else {
                    break;
                };
                if items.is_empty() {
                    break;
                }

                let mut added_this_page = 0usize;
                for item in items {
                    let Some(record) = item.as_object().cloned() else {
                        continue;
                    };

                    let Some(source_id) = rec_string(
                        &record,
                        &[
                            "inspectionID",
                            "permitID",
                            "permitNumber",
                            "inspectionId",
                            "id",
                        ],
                    ) else {
                        continue;
                    };

                    if !seen_inspections.insert(source_id) {
                        continue;
                    }

                    rows.push(record);
                    added_this_page += 1;

                    if rows.len() >= max_records {
                        break;
                    }
                }

                if added_this_page == 0 {
                    break;
                }
                start += items.len();
            }

            tracing::info!(
                source = "cpra_import_orange_pasadena",
                jurisdiction = "orange_county",
                term = term,
                fetched = rows.len().saturating_sub(baseline_rows),
                total = rows.len(),
                "Orange County live crawl term completed"
            );
        }

        if let Ok(legacy_rows) = self.fetch_orange_county_live_closures_legacy().await {
            for record in legacy_rows {
                let source_id = rec_string(
                    &record,
                    &[
                        "inspectionID",
                        "permitID",
                        "permitNumber",
                        "inspectionId",
                        "id",
                    ],
                )
                .or_else(|| rec_string(&record, &["source_id"]))
                .unwrap_or_default();

                if !source_id.is_empty() && !seen_inspections.insert(source_id) {
                    continue;
                }
                rows.push(record);
                if rows.len() >= max_records {
                    break;
                }
            }
        }

        tracing::info!(
            source = "cpra_import_orange_pasadena",
            jurisdiction = "orange_county",
            total = rows.len(),
            "Orange County live crawl completed"
        );

        Ok(rows
            .into_iter()
            .enumerate()
            .filter_map(|(idx, record)| {
                map_record(record, Jurisdiction::OrangeCounty, "oc-live", idx)
            })
            .collect::<Vec<_>>())
    }

    async fn fetch_pasadena_live_directory(&self) -> Result<Vec<SourceFacilityInput>> {
        let base_url = self.pasadena_directory_url.trim_end_matches('/').to_owned();
        let query_url = format!("{base_url}/query");
        let page_size = self.pasadena_page_size.clamp(1, 1_000);
        let max_records = self.pasadena_max_records.max(1);

        let count_response: Value = self
            .client
            .get(&query_url)
            .query(&[("where", "1=1"), ("returnCountOnly", "true"), ("f", "json")])
            .send()
            .await
            .context("Pasadena directory count request failed")?
            .error_for_status()
            .context("Pasadena directory count request returned non-success status")?
            .json()
            .await
            .context("Pasadena directory count JSON parse failed")?;

        let total_count = count_response
            .get("count")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(0);

        if total_count == 0 {
            return Ok(Vec::new());
        }

        let target = total_count.min(max_records);
        let mut rows = Vec::new();
        let mut offset = 0usize;

        while offset < target {
            let request_count = (target - offset).min(page_size);
            let response: Value = self
                .client
                .get(&query_url)
                .query(&[
                    ("where", "1=1"),
                    ("outFields", "*"),
                    ("returnGeometry", "true"),
                    ("f", "json"),
                    ("resultOffset", &offset.to_string()),
                    ("resultRecordCount", &request_count.to_string()),
                ])
                .send()
                .await
                .context("Pasadena directory page request failed")?
                .error_for_status()
                .context("Pasadena directory page request returned non-success status")?
                .json()
                .await
                .context("Pasadena directory page JSON parse failed")?;

            let Some(features) = response.get("features").and_then(Value::as_array) else {
                break;
            };

            if features.is_empty() {
                break;
            }

            for feature in features {
                if rows.len() >= target {
                    break;
                }

                let Some(mut attributes) = feature
                    .get("attributes")
                    .and_then(Value::as_object)
                    .cloned()
                else {
                    continue;
                };

                if let Some(geometry) = feature.get("geometry").and_then(Value::as_object) {
                    if let Some(latitude) = geometry.get("y").and_then(Value::as_f64) {
                        attributes.insert("latitude".to_owned(), Value::from(latitude));
                    }
                    if let Some(longitude) = geometry.get("x").and_then(Value::as_f64) {
                        attributes.insert("longitude".to_owned(), Value::from(longitude));
                    }
                }

                rows.push(attributes);
            }

            if features.len() < request_count {
                break;
            }
            offset += request_count;
        }

        tracing::info!(
            source = "cpra_import_orange_pasadena",
            jurisdiction = "pasadena",
            total = rows.len(),
            "Pasadena live directory crawl completed"
        );

        Ok(rows
            .into_iter()
            .enumerate()
            .filter_map(|(idx, record)| map_record(record, Jurisdiction::Pasadena, "pas-live", idx))
            .collect::<Vec<_>>())
    }

    async fn fetch_orange_county_live_closures_legacy(&self) -> Result<Vec<Map<String, Value>>> {
        let page_size = self.oc_live_page_size.clamp(1, 200);
        let max_records = self.oc_live_max_records.max(1);
        let inspection_purposes = [
            "Inspection (Non-Routine)",
            "Notice of Violation Reinspection",
            "Reinspection",
            "Routine Inspection",
        ];

        let mut rows = Vec::new();
        let mut page = 1usize;
        let mut total_available = usize::MAX;

        while rows.len() < max_records && ((page - 1) * page_size) < total_available {
            let filter_by_val = json!([
                ["CLOSED", "CLOSED-OPERATOR INITIATED"],
                [self.oc_live_days_window.to_string(), "0"],
                ["Retail Food Facility Inspection"],
                inspection_purposes
            ])
            .to_string();

            let payload = json!({
                "jurisdictionPath": self.oc_live_path,
                "requestType": "inspclosures",
                "rows": page_size,
                "page": page,
                "searchTerm": "",
                "filterBySrc": "[\"This_Form\", \"This_Form\", \"Inspection_Type\",\"This_Form\"]",
                "filterByAct": "[\"EQUAL\", \"BETWEEN\", \"EQUAL\", \"EQUAL\"]",
                "filterByCol": "[\"result\", \"inspectionDate\", \"type\", \"InspectionTypeMRS\"]",
                "filterByVal": filter_by_val,
                "sort": "This_Form.inspectionDate|DESC",
            });

            let response = self
                .client
                .post(DEFAULT_OC_LIVE_LEGACY_ENDPOINT)
                .header(reqwest::header::ACCEPT, "application/json, text/plain, */*")
                .header(
                    reqwest::header::ORIGIN,
                    "https://inspections.myhealthdepartment.com",
                )
                .header(reqwest::header::REFERER, DEFAULT_OC_LIVE_LEGACY_REFERER)
                .header("X-Requested-With", "XMLHttpRequest")
                .json(&payload)
                .send()
                .await
                .with_context(|| {
                    format!("Orange County legacy closures request failed (page={page})")
                })?
                .error_for_status()
                .with_context(|| {
                    format!("Orange County legacy closures non-success status (page={page})")
                })?;

            let body = response
                .text()
                .await
                .context("Orange County legacy closures body read failed")?;
            let parsed = parse_json_relaxed(&body)
                .context("Orange County legacy closures JSON parse failed")?;

            if parsed
                .get("error")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                break;
            }

            total_available = parsed
                .pointer("/data/availableRows")
                .and_then(Value::as_u64)
                .map(|value| value as usize)
                .unwrap_or(total_available);

            let Some(data) = parsed.pointer("/data/DATA").and_then(Value::as_array) else {
                break;
            };
            if data.is_empty() {
                break;
            }

            for item in data {
                if rows.len() >= max_records {
                    break;
                }
                if let Some(record) = item.as_object().cloned() {
                    rows.push(record);
                }
            }

            if data.len() < page_size {
                break;
            }
            page += 1;
        }

        Ok(rows)
    }
}

#[async_trait]
impl HealthDataConnector for CpraConnector {
    fn source_name(&self) -> &'static str {
        "cpra_import_orange_pasadena"
    }

    async fn fetch_facilities(&self) -> Result<Vec<SourceFacilityInput>> {
        let mut facilities = Vec::new();
        let mut errors = Vec::new();
        let mut source_enabled = false;

        if let Some(url) = &self.orange_county_url {
            source_enabled = true;
            match self
                .fetch_export(url, Jurisdiction::OrangeCounty, "oc")
                .await
            {
                Ok(records) => facilities.extend(records),
                Err(error) => errors.push(format!("Orange County CPRA export failed: {error:#}")),
            }
        } else if self.oc_live_enabled {
            source_enabled = true;
            match self.fetch_orange_county_live().await {
                Ok(records) => facilities.extend(records),
                Err(error) => errors.push(format!("Orange County live portal failed: {error:#}")),
            }
        }

        if let Some(url) = &self.pasadena_url {
            source_enabled = true;
            match self.fetch_export(url, Jurisdiction::Pasadena, "pas").await {
                Ok(records) => facilities.extend(records),
                Err(error) => errors.push(format!("Pasadena CPRA export failed: {error:#}")),
            }
        } else if self.pasadena_live_enabled {
            source_enabled = true;
            match self.fetch_pasadena_live_directory().await {
                Ok(records) => facilities.extend(records),
                Err(error) => errors.push(format!("Pasadena live directory failed: {error:#}")),
            }
        }

        if facilities.is_empty() {
            if !source_enabled {
                anyhow::bail!(
                    "CPRA connector not configured and live fallbacks disabled (set CLEANPLATED_OC_CPRA_EXPORT_URL and/or CLEANPLATED_PASADENA_CPRA_EXPORT_URL)"
                );
            }
            if errors.is_empty() {
                anyhow::bail!("CPRA connector fetched zero records from all enabled sources");
            }

            anyhow::bail!(
                "CPRA connector failed across all enabled sources: {}",
                errors.join(" | ")
            );
        }

        Ok(facilities)
    }
}

fn parse_json_records(body: &str) -> Result<Vec<Map<String, Value>>> {
    let value: Value = serde_json::from_str(body).context("JSON export parse failed")?;

    let map_list = if let Some(array) = value.as_array() {
        array
            .iter()
            .filter_map(|item| item.as_object().cloned())
            .collect::<Vec<_>>()
    } else if let Some(object) = value.as_object() {
        if let Some(features) = object.get("features").and_then(Value::as_array) {
            features
                .iter()
                .filter_map(|item| item.get("attributes"))
                .filter_map(Value::as_object)
                .cloned()
                .collect::<Vec<_>>()
        } else {
            ["data", "results", "records", "value"]
                .iter()
                .find_map(|key| object.get(*key))
                .and_then(Value::as_array)
                .map(|array| {
                    array
                        .iter()
                        .filter_map(Value::as_object)
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        }
    } else {
        Vec::new()
    };

    Ok(map_list)
}

fn parse_csv_records(body: &str) -> Result<Vec<Map<String, Value>>> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(body.as_bytes());

    let headers = reader
        .headers()
        .context("CSV headers parse failed")?
        .iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();

    let mut records = Vec::new();
    for row in reader.records() {
        let row = row.context("CSV row parse failed")?;
        let mut map = Map::new();

        for (idx, value) in row.iter().enumerate() {
            if let Some(header) = headers.get(idx) {
                map.insert(header.clone(), Value::String(value.trim().to_owned()));
            }
        }

        records.push(map);
    }

    Ok(records)
}

fn parse_json_relaxed(body: &str) -> Result<Value> {
    serde_json::from_str(body).or_else(|error| {
        let sanitized = sanitize_json_control_chars(body);
        serde_json::from_str(&sanitized).with_context(|| {
            format!("unable to parse raw JSON ({error}) and sanitized JSON fallback")
        })
    })
}

fn sanitize_json_control_chars(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escaped = false;

    for ch in input.chars() {
        if in_string {
            if escaped {
                escaped = false;
                output.push(ch);
                continue;
            }

            match ch {
                '\\' => {
                    escaped = true;
                    output.push(ch);
                }
                '"' => {
                    in_string = false;
                    output.push(ch);
                }
                '\n' => output.push_str("\\n"),
                '\r' => output.push_str("\\r"),
                '\t' => output.push_str("\\t"),
                c if c.is_control() => {
                    let mut encoded = String::with_capacity(6);
                    use std::fmt::Write;
                    let _ = write!(&mut encoded, "\\u{:04x}", c as u32);
                    output.push_str(&encoded);
                }
                _ => output.push(ch),
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
        }
        output.push(ch);
    }

    output
}

fn parse_search_terms(raw: Option<String>, default_terms: Vec<String>) -> Vec<String> {
    raw.map(|value| {
        value
            .split(',')
            .map(str::trim)
            .filter(|term| !term.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>()
    })
    .filter(|terms| !terms.is_empty())
    .unwrap_or(default_terms)
}

fn default_orange_county_search_terms() -> Vec<String> {
    let mut terms = Vec::with_capacity(1 + 26 + 10);
    terms.push(String::new());
    terms.extend(('a'..='z').map(|ch| ch.to_string()));
    terms.extend(('0'..='9').map(|ch| ch.to_string()));
    terms
}

fn map_record(
    record: Map<String, Value>,
    jurisdiction: Jurisdiction,
    id_prefix: &str,
    row_index: usize,
) -> Option<SourceFacilityInput> {
    let name = rec_string(
        &record,
        &[
            "facility_name",
            "Facility_Name",
            "name",
            "record_name",
            "business_name",
            "establishmentName",
            "permitName",
            "PR_Estabname",
            "CERS_Estab_LKPname",
            "Name_of_Restaurant_Cafe",
        ],
    )?;
    let address = rec_string(
        &record,
        &[
            "address",
            "Address",
            "facility_address",
            "FACILITY_ADDRESS",
            "street_address",
            "addressLine1",
            "FacilityAddress",
            "CERS_Est_AddaddressLine1",
            "PR_EstAddressaddressLine1",
            "Business_Address_in_Pasadena",
        ],
    )
    .unwrap_or_default();
    let city = rec_string(
        &record,
        &[
            "city",
            "City",
            "facility_city",
            "FACILITY_CITY",
            "facilityCity",
            "CERS_Est_Addcity",
            "PR_EstAddresscity",
        ],
    )
    .unwrap_or_else(|| jurisdiction.label().to_owned());
    let postal_code = rec_string(
        &record,
        &[
            "postal_code",
            "Zip",
            "zip",
            "FACILITY_ZIP",
            "zipcode",
            "facilityZip",
            "CERS_Est_Addzip",
            "PR_EstAddresszip",
        ],
    )
    .unwrap_or_default();
    let state = rec_string(
        &record,
        &[
            "state",
            "State",
            "FACILITY_STATE",
            "facilityState",
            "CERS_Est_Addstate",
            "PR_EstAddressstate",
        ],
    )
    .unwrap_or_else(|| "CA".to_owned());

    let source_id = rec_string(
        &record,
        &[
            "source_id",
            "facility_id",
            "Facility_ID",
            "record_id",
            "SERIAL_NUMBER",
            "id",
            "permitNumber",
            "inspectionID",
            "ObjectID",
        ],
    )
    .filter(|value| !value.is_empty())
    .unwrap_or_else(|| stable_id(id_prefix, &name, &address, &city, row_index));

    let raw_score = rec_f64(&record, &["raw_score", "score", "Score", "SCORE"]).map(|v| v as f32);
    let letter_grade = rec_string(&record, &["letter_grade", "grade", "Grade", "GRADE"]);
    let placard_status = rec_string(
        &record,
        &[
            "placard_status",
            "status",
            "Placard_Status",
            "inspection_status",
            "result",
            "FacilityRatingStatus",
        ],
    );

    let default_coordinates = match jurisdiction {
        Jurisdiction::OrangeCounty => (33.7175, -117.8311),
        Jurisdiction::Pasadena => (34.1478, -118.1445),
        _ => (34.0522, -118.2437),
    };
    let source_coordinates = rec_f64(&record, &["latitude", "Latitude", "FACILITY_LATITUDE"]).zip(
        rec_f64(&record, &["longitude", "Longitude", "FACILITY_LONGITUDE"]),
    );
    let (latitude, longitude) = source_coordinates.unwrap_or(default_coordinates);

    let inspected_at = rec_datetime(
        &record,
        &[
            "inspected_at",
            "inspection_date",
            "Inspection_Date",
            "ACTIVITY_DATE",
            "date",
            "Date",
            "inspectionDate",
            "LastInspection",
        ],
    )
    .unwrap_or_else(Utc::now);

    let violations = rec_string(
        &record,
        &[
            "violation_description",
            "VIOLATION_DESCRIPTION",
            "closure_reason",
            "reason",
            "ReasonforClosure",
            "generalComments",
        ],
    )
    .map(|description| {
        vec![Violation {
            code: rec_string(
                &record,
                &["violation_code", "VIOLATION_CODE", "code", "closure_code"],
            )
            .unwrap_or_else(|| "CPRA".to_owned()),
            points: rec_f64(&record, &["violation_points", "POINTS"])
                .map(|value| value as i16)
                .unwrap_or(0),
            critical: rec_bool(&record, &["critical", "is_critical"]).unwrap_or(false),
            description,
        }]
    })
    .unwrap_or_default();

    Some(SourceFacilityInput {
        source_id,
        name,
        address,
        city,
        state,
        postal_code,
        latitude,
        longitude,
        coordinates_approximate: source_coordinates.is_none(),
        jurisdiction,
        inspected_at,
        raw_score,
        letter_grade,
        placard_status,
        violations,
    })
}

fn stable_id(prefix: &str, name: &str, address: &str, city: &str, row_index: usize) -> String {
    let mut hasher = DefaultHasher::new();
    format!("{name}|{address}|{city}|{row_index}").hash(&mut hasher);
    format!("{prefix}-{:016x}", hasher.finish())
}

fn rec_string(record: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| record.get(*key))
        .and_then(|value| {
            if let Some(text) = value.as_str() {
                return Some(text.trim().to_owned());
            }
            if value.is_number() {
                return Some(value.to_string());
            }
            None
        })
        .filter(|text| !text.is_empty())
}

fn rec_f64(record: &Map<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        let value = record.get(*key)?;
        if let Some(number) = value.as_f64() {
            return Some(number);
        }
        value
            .as_str()
            .and_then(|text| text.trim().parse::<f64>().ok())
    })
}

fn rec_bool(record: &Map<String, Value>, keys: &[&str]) -> Option<bool> {
    keys.iter().find_map(|key| {
        let value = record.get(*key)?;
        if let Some(boolean) = value.as_bool() {
            return Some(boolean);
        }
        value
            .as_str()
            .and_then(|text| match text.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => Some(true),
                "false" | "0" | "no" => Some(false),
                _ => None,
            })
    })
}

fn rec_datetime(record: &Map<String, Value>, keys: &[&str]) -> Option<DateTime<Utc>> {
    let value = keys.iter().find_map(|key| record.get(*key))?;

    if let Some(number) = value.as_i64() {
        return timestamp_to_datetime(number);
    }

    let text = value.as_str()?.trim();
    if text.is_empty() {
        return None;
    }

    if let Ok(number) = text.parse::<i64>() {
        return timestamp_to_datetime(number);
    }

    if let Ok(parsed) = DateTime::parse_from_rfc3339(text) {
        return Some(parsed.with_timezone(&Utc));
    }

    if let Ok(parsed) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return parsed
            .and_hms_opt(0, 0, 0)
            .map(|naive| DateTime::from_naive_utc_and_offset(naive, Utc));
    }

    if let Ok(parsed) = NaiveDate::parse_from_str(text, "%m/%d/%Y") {
        return parsed
            .and_hms_opt(0, 0, 0)
            .map(|naive| DateTime::from_naive_utc_and_offset(naive, Utc));
    }

    if let Ok(parsed) = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S") {
        return Some(DateTime::from_naive_utc_and_offset(parsed, Utc));
    }

    if let Ok(parsed) = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f") {
        return Some(DateTime::from_naive_utc_and_offset(parsed, Utc));
    }

    if let Ok(parsed) = NaiveDateTime::parse_from_str(text, "%m/%d/%Y %H:%M:%S") {
        return Some(DateTime::from_naive_utc_and_offset(parsed, Utc));
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

fn parse_bool_env(key: &str, default_value: bool) -> bool {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .and_then(|value| match value.as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
        .unwrap_or(default_value)
}
