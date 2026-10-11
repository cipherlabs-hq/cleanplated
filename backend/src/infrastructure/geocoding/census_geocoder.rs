use std::{env, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::{
    Client,
    multipart::{Form, Part},
};
use tokio::{sync::Semaphore, task::JoinSet, time::sleep};
use tracing::{info, warn};

use crate::domain::geocoding::{GeocodeRequest, GeocodeResult, Geocoder, in_southern_california};

/// The U.S. Census Bureau's free batch geocoder. It takes up to 10,000 addresses per request;
/// smaller batches keep one slow or failed request from losing much work.
const DEFAULT_URL: &str = "https://geocoding.geo.census.gov/geocoder/locations/addressbatch";
const DEFAULT_BENCHMARK: &str = "Public_AR_Current";
const DEFAULT_BATCH_SIZE: usize = 1_000;
const DEFAULT_CONCURRENCY: usize = 4;
const DEFAULT_TIMEOUT_SECS: u64 = 300;
/// The Census service occasionally stalls on one batch; a second attempt usually succeeds.
const BATCH_ATTEMPTS: u32 = 2;

#[derive(Clone)]
pub struct CensusGeocoder {
    client: Client,
    url: String,
    benchmark: String,
    batch_size: usize,
    concurrency: usize,
}

impl CensusGeocoder {
    pub fn from_env() -> Self {
        let timeout_secs = env::var("CLEANPLATED_GEOCODER_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_TIMEOUT_SECS);

        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(timeout_secs))
                .build()
                .unwrap_or_else(|_| Client::new()),
            url: env::var("CLEANPLATED_GEOCODER_URL").unwrap_or_else(|_| DEFAULT_URL.to_owned()),
            benchmark: env::var("CLEANPLATED_GEOCODER_BENCHMARK")
                .unwrap_or_else(|_| DEFAULT_BENCHMARK.to_owned()),
            batch_size: env::var("CLEANPLATED_GEOCODER_BATCH_SIZE")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(DEFAULT_BATCH_SIZE)
                .clamp(1, 10_000),
            concurrency: env::var("CLEANPLATED_GEOCODER_CONCURRENCY")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(DEFAULT_CONCURRENCY)
                .max(1),
        }
    }

    async fn geocode_batch_with_retry(
        &self,
        requests: Vec<GeocodeRequest>,
    ) -> Result<Vec<GeocodeResult>> {
        let mut attempt = 1;
        loop {
            match self.geocode_batch(&requests).await {
                Ok(results) => return Ok(results),
                Err(error) if attempt < BATCH_ATTEMPTS => {
                    info!(attempt, error = %format!("{error:#}"), "Census geocoder batch failed; retrying");
                    attempt += 1;
                    sleep(Duration::from_secs(10)).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    async fn geocode_batch(&self, requests: &[GeocodeRequest]) -> Result<Vec<GeocodeResult>> {
        let mut writer = csv::WriterBuilder::new()
            .has_headers(false)
            .from_writer(Vec::new());
        for (index, request) in requests.iter().enumerate() {
            writer.write_record([
                index.to_string().as_str(),
                request.street.as_str(),
                request.city.as_str(),
                request.state.as_str(),
                request.postal_code.as_str(),
            ])?;
        }
        let body = writer
            .into_inner()
            .context("could not build geocoder CSV")?;

        let form = Form::new()
            .part(
                "addressFile",
                Part::bytes(body)
                    .file_name("addresses.csv")
                    .mime_str("text/csv")?,
            )
            .text("benchmark", self.benchmark.clone());

        let response = self
            .client
            .post(&self.url)
            .multipart(form)
            .send()
            .await
            .context("Census geocoder request failed")?
            .error_for_status()
            .context("Census geocoder returned non-success status")?
            .text()
            .await
            .context("Census geocoder response could not be read")?;

        let geocoded_at = Utc::now();
        Ok(parse_census_response(&response)
            .into_iter()
            .filter_map(|(index, coordinates)| {
                requests.get(index).map(|request| GeocodeResult {
                    key: request.key.clone(),
                    coordinates,
                    geocoded_at,
                })
            })
            .collect())
    }
}

#[async_trait]
impl Geocoder for CensusGeocoder {
    async fn geocode(&self, requests: &[GeocodeRequest]) -> Result<Vec<GeocodeResult>> {
        let semaphore = Arc::new(Semaphore::new(self.concurrency));
        let mut batches = JoinSet::new();

        for chunk in requests.chunks(self.batch_size) {
            let geocoder = self.clone();
            let chunk = chunk.to_vec();
            let semaphore = semaphore.clone();
            batches.spawn(async move {
                let _permit = semaphore.acquire_owned().await?;
                geocoder.geocode_batch_with_retry(chunk).await
            });
        }

        let mut results = Vec::with_capacity(requests.len());
        let mut failed_batches = 0usize;
        while let Some(joined) = batches.join_next().await {
            match joined {
                Ok(Ok(batch)) => results.extend(batch),
                Ok(Err(error)) => {
                    failed_batches += 1;
                    warn!(error = %format!("{error:#}"), attempts = BATCH_ATTEMPTS, "Census geocoder batch failed; its addresses will be retried next run");
                }
                Err(error) => {
                    failed_batches += 1;
                    warn!(%error, "Census geocoder batch task panicked");
                }
            }
        }

        if results.is_empty() && failed_batches > 0 {
            anyhow::bail!("all {failed_batches} Census geocoder batches failed");
        }
        Ok(results)
    }
}

/// Parses the batch response: one CSV row per address, as
/// `id, input address, Match|No_Match|Tie, Exact|Non_Exact, matched address, "lon,lat", ...`.
/// Returns the row id with coordinates for an in-region match, or `None` for anything else.
fn parse_census_response(body: &str) -> Vec<(usize, Option<(f64, f64)>)> {
    csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(body.as_bytes())
        .records()
        .filter_map(|record| record.ok())
        .filter_map(|record| {
            let index = record.get(0)?.trim().parse::<usize>().ok()?;
            let coordinates = (record.get(2).map(str::trim) == Some("Match"))
                .then(|| parse_lon_lat(record.get(5)?))
                .flatten()
                .filter(|&(latitude, longitude)| in_southern_california(latitude, longitude));
            Some((index, coordinates))
        })
        .collect()
}

/// The Census API returns "longitude,latitude"; this returns (latitude, longitude).
fn parse_lon_lat(value: &str) -> Option<(f64, f64)> {
    let (longitude, latitude) = value.split_once(',')?;
    Some((
        latitude.trim().parse().ok()?,
        longitude.trim().parse().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_matches_misses_and_out_of_region_results() {
        let body = concat!(
            "\"0\",\"1900 E 18TH ST, NATIONAL CITY, CA, 91950\",\"Match\",\"Exact\",\"1900 E 18TH ST, NATIONAL CITY, CA, 91950\",\"-117.084127005122,32.67256784293\",\"195682682\",\"R\"\n",
            "\"1\",\"1 NOWHERE RD, SANTEE, CA, 92071\",\"No_Match\"\n",
            "\"2\",\"10 MAIN ST, SPRINGFIELD, CA, 00000\",\"Match\",\"Non_Exact\",\"10 MAIN ST, SPRINGFIELD, IL\",\"-89.65,39.78\",\"1\",\"L\"\n",
            "\"3\",\"5 ELM ST, VISTA, CA, 92083\",\"Tie\"\n",
        );

        let parsed = parse_census_response(body);

        assert_eq!(
            parsed,
            vec![
                (0, Some((32.67256784293, -117.084127005122))),
                (1, None),
                (2, None),
                (3, None),
            ]
        );
    }

    /// Hits the live Census API. Run with `cargo test -- --ignored census_live`.
    #[tokio::test]
    #[ignore]
    async fn census_live_geocodes_a_known_address() {
        let results = CensusGeocoder::from_env()
            .geocode(&[GeocodeRequest {
                key: "k".to_owned(),
                street: "1900 E 18th St".to_owned(),
                city: "National City".to_owned(),
                state: "CA".to_owned(),
                postal_code: "91950".to_owned(),
            }])
            .await
            .unwrap();

        let (latitude, longitude) = results[0].coordinates.unwrap();
        assert!((latitude - 32.67).abs() < 0.01 && (longitude + 117.08).abs() < 0.01);
    }
}
