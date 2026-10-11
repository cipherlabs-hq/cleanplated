use std::{collections::HashMap, sync::Arc};

use chrono::{Duration as ChronoDuration, Utc};
use tokio::sync::Mutex;
use tokio::time::{Duration, sleep};
use tracing::{info, warn};

use crate::{
    application::{
        dto::SourceFacilityInput,
        services::{ScoreSignals, TrustScoreService},
    },
    domain::{
        entities::{ConnectorIngestionStatus, Facility, Inspection, SystemIngestionStatus},
        geocoding::{GeocodeRequest, GeocodeResult, Geocoder, address_key},
        repositories::FacilityRepository,
    },
    infrastructure::connectors::HealthDataConnector,
};

#[derive(Clone)]
pub struct IngestionService {
    repository: Arc<dyn FacilityRepository>,
    trust_score_service: Arc<TrustScoreService>,
    connectors: Vec<Arc<dyn HealthDataConnector>>,
    geocoder: Option<Arc<dyn Geocoder>>,
    max_geocodes_per_run: usize,
    refresh_lock: Arc<Mutex<()>>,
}

const CONNECTOR_MAX_ATTEMPTS: usize = 3;
/// Addresses the geocoder could not match are retried after this long, in case the source
/// corrected the address or the geocoder's data improved.
const GEOCODE_NO_MATCH_RETRY_DAYS: i64 = 30;

#[derive(Clone, Debug, serde::Serialize, Default)]
pub struct IngestionStats {
    pub last_refresh_at: Option<chrono::DateTime<Utc>>,
    pub unique_facilities: usize,
    pub connector_stats: Vec<ConnectorIngestionStatus>,
}

impl IngestionService {
    pub fn new(
        repository: Arc<dyn FacilityRepository>,
        trust_score_service: Arc<TrustScoreService>,
        connectors: Vec<Arc<dyn HealthDataConnector>>,
    ) -> Self {
        Self {
            repository,
            trust_score_service,
            connectors,
            geocoder: None,
            max_geocodes_per_run: 0,
            refresh_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Places records whose source had no coordinates by geocoding their address, at most
    /// `max_per_run` new lookups per refresh (cached results are always applied).
    pub fn with_geocoder(mut self, geocoder: Arc<dyn Geocoder>, max_per_run: usize) -> Self {
        self.geocoder = Some(geocoder);
        self.max_geocodes_per_run = max_per_run;
        self
    }

    pub async fn stats(&self) -> IngestionStats {
        match self.repository.get_system_ingestion_status().await {
            Ok(Some(status)) => IngestionStats {
                last_refresh_at: Some(status.last_refresh_at),
                unique_facilities: status.unique_facilities,
                connector_stats: status.connector_stats,
            },
            Ok(None) | Err(_) => IngestionStats::default(),
        }
    }

    pub async fn refresh(&self) -> anyhow::Result<()> {
        let _guard = self.refresh_lock.lock().await;
        let mut stitched: HashMap<String, SourceFacilityInput> = HashMap::new();
        let mut connector_stats = Vec::new();
        let mut successful_connectors = 0usize;
        let previous_status = self
            .repository
            .get_system_ingestion_status()
            .await
            .ok()
            .flatten();

        for connector in &self.connectors {
            match self.fetch_with_retry(connector.as_ref()).await {
                Ok(records) => {
                    successful_connectors += 1;
                    info!(
                        source = connector.source_name(),
                        records = records.len(),
                        "Fetched inspection records"
                    );
                    connector_stats.push(ConnectorIngestionStatus {
                        source: connector.source_name().to_owned(),
                        fetched_records: records.len(),
                        error: None,
                    });

                    for record in records {
                        let key = dedupe_key(&record);
                        stitched
                            .entry(key)
                            .and_modify(|current| {
                                if current.inspected_at < record.inspected_at {
                                    *current = record.clone();
                                }
                            })
                            .or_insert(record);
                    }
                }
                Err(error) => {
                    let error_chain = format!("{error:#}");
                    connector_stats.push(ConnectorIngestionStatus {
                        source: connector.source_name().to_owned(),
                        fetched_records: 0,
                        error: Some(error_chain.clone()),
                    });
                    warn!(
                        source = connector.source_name(),
                        error = %error_chain,
                        "Connector fetch failed"
                    );
                }
            }
        }

        if successful_connectors == 0 {
            anyhow::bail!("no connectors succeeded; keeping previous dataset untouched");
        }

        if stitched.is_empty() {
            anyhow::bail!("ingestion produced zero facilities; keeping previous dataset untouched");
        }

        if let Some(previous) = previous_status {
            let minimum_safe_count = (previous.unique_facilities / 2).max(1);
            if stitched.len() < minimum_safe_count {
                anyhow::bail!(
                    "ingestion result too small ({} < {}), keeping previous dataset untouched",
                    stitched.len(),
                    minimum_safe_count
                );
            }
        }

        self.place_approximate_records(&mut stitched).await;

        let facilities = stitched
            .into_values()
            .map(|record| self.normalize(record))
            .collect::<Vec<_>>();
        let unique_facilities = facilities.len();

        self.repository
            .replace_all(facilities)
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;

        let snapshot = SystemIngestionStatus {
            last_refresh_at: Utc::now(),
            unique_facilities,
            connector_stats,
        };

        self.repository
            .set_system_ingestion_status(snapshot.clone())
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;

        info!(
            unique_facilities = snapshot.unique_facilities,
            "Ingestion + normalization finished"
        );

        Ok(())
    }

    /// Replaces fallback coordinates with geocoded ones: from the cache when possible, otherwise
    /// from the geocoder. Failures are logged and leave the fallback in place; they never fail
    /// the refresh.
    async fn place_approximate_records(&self, stitched: &mut HashMap<String, SourceFacilityInput>) {
        let Some(geocoder) = self.geocoder.as_ref() else {
            return;
        };

        let mut requests = HashMap::<String, GeocodeRequest>::new();
        for record in stitched.values() {
            if !record.coordinates_approximate || record.address.trim().is_empty() {
                continue;
            }
            let key = address_key(
                &record.address,
                &record.city,
                &record.state,
                &record.postal_code,
            );
            requests
                .entry(key.clone())
                .or_insert_with(|| GeocodeRequest {
                    key,
                    street: record.address.trim().to_owned(),
                    city: record.city.trim().to_owned(),
                    state: record.state.trim().to_owned(),
                    postal_code: record.postal_code.trim().to_owned(),
                });
        }
        if requests.is_empty() {
            return;
        }

        let keys = requests.keys().cloned().collect::<Vec<_>>();
        let mut known = match self.repository.get_geocodes(&keys).await {
            Ok(cached) => cached,
            Err(error) => {
                warn!(%error, "Geocode cache lookup failed; skipping geocoding this run");
                return;
            }
        };

        let retry_before = Utc::now() - ChronoDuration::days(GEOCODE_NO_MATCH_RETRY_DAYS);
        let mut pending = requests
            .into_values()
            .filter(|request| match known.get(&request.key) {
                None => true,
                Some(cached) => cached.coordinates.is_none() && cached.geocoded_at < retry_before,
            })
            .collect::<Vec<_>>();
        // Deterministic order, so a capped run makes steady progress across runs.
        pending.sort_by(|left, right| left.key.cmp(&right.key));
        let deferred = pending.len().saturating_sub(self.max_geocodes_per_run);
        pending.truncate(self.max_geocodes_per_run);

        let mut geocoded = 0usize;
        if !pending.is_empty() {
            match geocoder.geocode(&pending).await {
                Ok(results) => {
                    geocoded = results.len();
                    if let Err(error) = self.repository.save_geocodes(&results).await {
                        warn!(%error, "Could not save geocode results; they will be looked up again next run");
                    }
                    known.extend(
                        results
                            .into_iter()
                            .map(|result| (result.key.clone(), result)),
                    );
                }
                Err(error) => {
                    warn!(error = %format!("{error:#}"), "Geocoding failed; keeping fallback coordinates this run");
                }
            }
        }

        let placed = apply_geocodes(stitched, &known);
        let approximate = stitched
            .values()
            .filter(|record| record.coordinates_approximate)
            .count();
        info!(
            placed,
            newly_geocoded = geocoded,
            deferred,
            still_approximate = approximate,
            "Placed facilities by address"
        );
    }

    async fn fetch_with_retry(
        &self,
        connector: &dyn HealthDataConnector,
    ) -> anyhow::Result<Vec<SourceFacilityInput>> {
        let mut attempt = 0usize;
        loop {
            attempt += 1;

            match connector.fetch_facilities().await {
                Ok(records) => return Ok(records),
                Err(error) if attempt < CONNECTOR_MAX_ATTEMPTS => {
                    let backoff_seconds = (attempt as u64) * 2;
                    let error_chain = format!("{error:#}");
                    warn!(
                        source = connector.source_name(),
                        attempt,
                        error = %error_chain,
                        "Connector fetch failed, retrying"
                    );
                    sleep(Duration::from_secs(backoff_seconds)).await;
                }
                Err(error) => {
                    return Err(error);
                }
            }
        }
    }

    fn normalize(&self, record: SourceFacilityInput) -> Facility {
        let trust_score = self.trust_score_service.score(&ScoreSignals {
            raw_score: record.raw_score,
            letter_grade: record.letter_grade.clone(),
            placard_status: record.placard_status.clone(),
        });

        let inspection = Inspection {
            inspection_id: format!("{}-{}", record.jurisdiction.code(), record.source_id),
            inspected_at: record.inspected_at,
            raw_score: record.raw_score,
            letter_grade: record.letter_grade,
            placard_status: record.placard_status,
            violations: record.violations,
        };

        Facility {
            id: format!("{}::{}", record.jurisdiction.code(), record.source_id),
            source_id: record.source_id,
            name: record.name,
            address: record.address,
            city: record.city,
            state: record.state,
            postal_code: record.postal_code,
            latitude: record.latitude,
            longitude: record.longitude,
            jurisdiction: record.jurisdiction,
            trust_score,
            inspections: vec![inspection],
            updated_at: Utc::now(),
        }
    }
}

fn dedupe_key(record: &SourceFacilityInput) -> String {
    format!(
        "{}|{}|{}|{}",
        record.name.trim().to_ascii_lowercase(),
        record.address.trim().to_ascii_lowercase(),
        record.city.trim().to_ascii_lowercase(),
        record.postal_code.trim().to_ascii_lowercase()
    )
}

/// Moves every approximate record whose address has geocoded coordinates onto them, and returns
/// how many were moved.
fn apply_geocodes(
    stitched: &mut HashMap<String, SourceFacilityInput>,
    known: &HashMap<String, GeocodeResult>,
) -> usize {
    let mut placed = 0usize;
    for record in stitched.values_mut() {
        if !record.coordinates_approximate {
            continue;
        }
        let key = address_key(
            &record.address,
            &record.city,
            &record.state,
            &record.postal_code,
        );
        if let Some((latitude, longitude)) = known.get(&key).and_then(|result| result.coordinates) {
            record.latitude = latitude;
            record.longitude = longitude;
            record.coordinates_approximate = false;
            placed += 1;
        }
    }
    placed
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    use super::*;
    use crate::{
        domain::entities::Jurisdiction, infrastructure::repositories::InMemoryFacilityRepository,
    };

    const FALLBACK: (f64, f64) = (34.0522, -118.2437);
    const GEOCODED: (f64, f64) = (34.1, -118.3);

    fn record(source_id: &str, address: &str, approximate: bool) -> SourceFacilityInput {
        SourceFacilityInput {
            source_id: source_id.to_owned(),
            name: format!("Facility {source_id}"),
            address: address.to_owned(),
            city: "Los Angeles".to_owned(),
            state: "CA".to_owned(),
            postal_code: "90001".to_owned(),
            latitude: if approximate { FALLBACK.0 } else { 33.9 },
            longitude: if approximate { FALLBACK.1 } else { -118.1 },
            coordinates_approximate: approximate,
            jurisdiction: Jurisdiction::LosAngelesCounty,
            inspected_at: Utc::now(),
            raw_score: Some(95.0),
            letter_grade: Some("A".to_owned()),
            placard_status: None,
            violations: Vec::new(),
        }
    }

    struct FixedConnector(Vec<SourceFacilityInput>);

    #[async_trait]
    impl HealthDataConnector for FixedConnector {
        fn source_name(&self) -> &'static str {
            "fixed"
        }

        async fn fetch_facilities(&self) -> anyhow::Result<Vec<SourceFacilityInput>> {
            Ok(self.0.clone())
        }
    }

    /// Matches addresses containing "Main", misses the rest, and counts lookups.
    #[derive(Default)]
    struct FakeGeocoder {
        lookups: AtomicUsize,
        fail: bool,
    }

    #[async_trait]
    impl Geocoder for FakeGeocoder {
        async fn geocode(&self, requests: &[GeocodeRequest]) -> anyhow::Result<Vec<GeocodeResult>> {
            self.lookups.fetch_add(requests.len(), Ordering::SeqCst);
            if self.fail {
                anyhow::bail!("geocoder unavailable");
            }
            Ok(requests
                .iter()
                .map(|request| GeocodeResult {
                    key: request.key.clone(),
                    coordinates: request.street.contains("Main").then_some(GEOCODED),
                    geocoded_at: Utc::now(),
                })
                .collect())
        }
    }

    fn service(
        repository: Arc<InMemoryFacilityRepository>,
        records: Vec<SourceFacilityInput>,
        geocoder: Arc<FakeGeocoder>,
        max_per_run: usize,
    ) -> IngestionService {
        IngestionService::new(
            repository,
            Arc::new(TrustScoreService),
            vec![Arc::new(FixedConnector(records))],
        )
        .with_geocoder(geocoder, max_per_run)
    }

    async fn coordinates(repository: &InMemoryFacilityRepository, source_id: &str) -> (f64, f64) {
        let facility = repository
            .get_by_id(&format!("lac::{source_id}"))
            .await
            .unwrap()
            .unwrap();
        (facility.latitude, facility.longitude)
    }

    #[tokio::test]
    async fn approximate_records_are_placed_and_exact_ones_are_left_alone() {
        let repository = Arc::new(InMemoryFacilityRepository::new());
        let geocoder = Arc::new(FakeGeocoder::default());
        let records = vec![
            record("approx", "100 Main St", true),
            record("unmatched", "1 Nowhere Rd", true),
            record("exact", "200 Main St", false),
        ];

        service(repository.clone(), records, geocoder.clone(), 100)
            .refresh()
            .await
            .unwrap();

        assert_eq!(coordinates(&repository, "approx").await, GEOCODED);
        assert_eq!(coordinates(&repository, "unmatched").await, FALLBACK);
        assert_eq!(coordinates(&repository, "exact").await, (33.9, -118.1));
        assert_eq!(
            geocoder.lookups.load(Ordering::SeqCst),
            2,
            "exact record must not be geocoded"
        );
    }

    #[tokio::test]
    async fn cached_matches_and_recent_misses_are_not_looked_up_again() {
        let repository = Arc::new(InMemoryFacilityRepository::new());
        let records = vec![
            record("approx", "100 Main St", true),
            record("unmatched", "1 Nowhere Rd", true),
        ];
        service(
            repository.clone(),
            records.clone(),
            Arc::new(FakeGeocoder::default()),
            100,
        )
        .refresh()
        .await
        .unwrap();

        let second = Arc::new(FakeGeocoder::default());
        service(repository.clone(), records, second.clone(), 100)
            .refresh()
            .await
            .unwrap();

        assert_eq!(second.lookups.load(Ordering::SeqCst), 0);
        assert_eq!(coordinates(&repository, "approx").await, GEOCODED);
    }

    #[tokio::test]
    async fn a_geocoder_failure_keeps_fallbacks_and_does_not_fail_the_refresh() {
        let repository = Arc::new(InMemoryFacilityRepository::new());
        let geocoder = Arc::new(FakeGeocoder {
            fail: true,
            ..Default::default()
        });

        service(
            repository.clone(),
            vec![record("approx", "100 Main St", true)],
            geocoder,
            100,
        )
        .refresh()
        .await
        .unwrap();

        assert_eq!(coordinates(&repository, "approx").await, FALLBACK);
    }

    #[tokio::test]
    async fn the_per_run_cap_limits_new_lookups() {
        let repository = Arc::new(InMemoryFacilityRepository::new());
        let geocoder = Arc::new(FakeGeocoder::default());
        let records = (0..5)
            .map(|index| record(&format!("r{index}"), &format!("{index} Main St"), true))
            .collect();

        service(repository, records, geocoder.clone(), 2)
            .refresh()
            .await
            .unwrap();

        assert_eq!(geocoder.lookups.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn address_keys_ignore_case_and_extra_whitespace() {
        assert_eq!(
            address_key(" 100  Main St ", "LOS ANGELES", "ca", "90001"),
            address_key("100 MAIN ST", "Los Angeles", "CA", " 90001")
        );
    }
}
