use std::{cmp::Reverse, collections::HashMap};

use async_trait::async_trait;
use chrono::{Duration, Utc};
use tokio::sync::RwLock;

use crate::domain::{
    entities::{
        AutocompleteSuggestion, Facility, FacilitySearchQuery, FacilityVoteSummary,
        ScoreSliceCounts, SystemIngestionStatus, VoteValue,
    },
    errors::RepositoryError,
    geocoding::GeocodeResult,
    repositories::FacilityRepository,
};

const ELITE_THRESHOLD: u8 = 90;
const SOLID_THRESHOLD: u8 = 80;

#[derive(Default)]
pub struct InMemoryFacilityRepository {
    facilities: RwLock<Vec<Facility>>,
    ingestion_status: RwLock<Option<SystemIngestionStatus>>,
    votes: RwLock<HashMap<(String, String), VoteValue>>,
    geocodes: RwLock<HashMap<String, GeocodeResult>>,
}

impl InMemoryFacilityRepository {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl FacilityRepository for InMemoryFacilityRepository {
    async fn replace_all(&self, facilities: Vec<Facility>) -> Result<(), RepositoryError> {
        let mut write_guard = self.facilities.write().await;
        *write_guard = facilities;
        Ok(())
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<Facility>, RepositoryError> {
        let item = self
            .facilities
            .read()
            .await
            .iter()
            .find(|facility| facility.id == id)
            .cloned();

        Ok(item)
    }

    async fn set_system_ingestion_status(
        &self,
        status: SystemIngestionStatus,
    ) -> Result<(), RepositoryError> {
        let mut write_guard = self.ingestion_status.write().await;
        *write_guard = Some(status);
        Ok(())
    }

    async fn get_system_ingestion_status(
        &self,
    ) -> Result<Option<SystemIngestionStatus>, RepositoryError> {
        Ok(self.ingestion_status.read().await.clone())
    }

    async fn upsert_facility_vote(
        &self,
        facility_id: &str,
        voter_key: &str,
        vote: VoteValue,
    ) -> Result<FacilityVoteSummary, RepositoryError> {
        {
            let mut write_guard = self.votes.write().await;
            write_guard.insert((facility_id.to_owned(), voter_key.to_owned()), vote);
        }

        let all_votes = self.votes.read().await;
        let mut likes = 0_u64;
        let mut dislikes = 0_u64;
        for ((current_facility_id, _), current_vote) in all_votes.iter() {
            if current_facility_id != facility_id {
                continue;
            }
            match current_vote {
                VoteValue::Like => likes += 1,
                VoteValue::Dislike => dislikes += 1,
            }
        }

        Ok(FacilityVoteSummary { likes, dislikes })
    }

    async fn get_facility_vote_summaries(
        &self,
        facility_ids: &[String],
    ) -> Result<HashMap<String, FacilityVoteSummary>, RepositoryError> {
        if facility_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let id_set = facility_ids
            .iter()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        let all_votes = self.votes.read().await;
        let mut summaries: HashMap<String, FacilityVoteSummary> = HashMap::new();

        for ((facility_id, _), vote) in all_votes.iter() {
            if !id_set.contains(facility_id) {
                continue;
            }

            let summary = summaries.entry(facility_id.clone()).or_default();
            match vote {
                VoteValue::Like => summary.likes += 1,
                VoteValue::Dislike => summary.dislikes += 1,
            }
        }

        Ok(summaries)
    }

    async fn search_facilities(
        &self,
        query: &FacilitySearchQuery,
    ) -> Result<(Vec<Facility>, usize, ScoreSliceCounts), RepositoryError> {
        let has_search_term = query
            .q
            .as_ref()
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false);
        let mut facilities = self.facilities.read().await.clone();

        // Text filtering
        if let Some(term) = query.q.as_ref().map(|v| v.trim().to_ascii_lowercase())
            && !term.is_empty()
        {
            facilities.retain(|f| {
                f.name.to_ascii_lowercase().contains(&term)
                    || f.address.to_ascii_lowercase().contains(&term)
                    || f.city.to_ascii_lowercase().contains(&term)
                    || f.postal_code.contains(&term)
            });
        }

        // Jurisdiction filter
        if let Some(jurisdiction) = query
            .jurisdiction
            .as_ref()
            .map(|v| v.trim().to_ascii_lowercase())
            && !jurisdiction.is_empty()
            && jurisdiction != "all"
        {
            facilities.retain(|f| {
                f.jurisdiction.code() == jurisdiction
                    || f.jurisdiction.label().eq_ignore_ascii_case(&jurisdiction)
            });
        }

        // Geo-radius filter (only in browse mode, not text search)
        if !has_search_term
            && let (Some(lat), Some(lon), Some(radius)) =
                (query.latitude, query.longitude, query.radius_miles)
        {
            facilities.retain(|f| haversine_miles(lat, lon, f.latitude, f.longitude) <= radius);
        }

        // Recent-only filter: use latest inspection date
        if query.recent_only.unwrap_or(false) {
            let cutoff = Utc::now() - Duration::days(90);
            facilities.retain(|f| {
                f.inspections
                    .iter()
                    .map(|i| i.inspected_at)
                    .max()
                    .map(|latest| latest >= cutoff)
                    .unwrap_or(false)
            });
        }

        // Compute slice counts BEFORE score_slice filter
        let slice_counts = ScoreSliceCounts {
            all: facilities.len(),
            elite: facilities
                .iter()
                .filter(|f| f.trust_score >= ELITE_THRESHOLD)
                .count(),
            solid: facilities
                .iter()
                .filter(|f| f.trust_score >= SOLID_THRESHOLD && f.trust_score < ELITE_THRESHOLD)
                .count(),
            watch: facilities
                .iter()
                .filter(|f| f.trust_score < SOLID_THRESHOLD)
                .count(),
        };

        // Score slice filter
        if let Some(slice) = query
            .score_slice
            .as_ref()
            .map(|v| v.trim().to_ascii_lowercase())
        {
            match slice.as_str() {
                "elite" => facilities.retain(|f| f.trust_score >= ELITE_THRESHOLD),
                "solid" => facilities.retain(|f| {
                    f.trust_score >= SOLID_THRESHOLD && f.trust_score < ELITE_THRESHOLD
                }),
                "watch" => facilities.retain(|f| f.trust_score < SOLID_THRESHOLD),
                _ => {}
            }
        }

        // Sorting
        let sort = query.sort.as_ref().map(|v| v.trim().to_ascii_lowercase());
        match sort.as_deref() {
            Some("recent_desc") => {
                facilities.sort_by(|a, b| {
                    let a_latest = a.inspections.iter().map(|i| i.inspected_at).max();
                    let b_latest = b.inspections.iter().map(|i| i.inspected_at).max();
                    b_latest
                        .cmp(&a_latest)
                        .then(b.trust_score.cmp(&a.trust_score))
                });
            }
            Some("name_asc") => {
                facilities.sort_by(|a, b| a.name.cmp(&b.name));
            }
            _ => {
                facilities.sort_by_key(|facility| Reverse(facility.trust_score));
            }
        }

        let total_count = facilities.len();
        // page_size and page are pre-resolved by DirectoryService::search().
        let page_size = query.page_size.unwrap_or(50);
        let page = query.page.unwrap_or(1);
        let offset = (page - 1).saturating_mul(page_size);
        let page_facilities = facilities
            .into_iter()
            .skip(offset)
            .take(page_size)
            .collect();

        Ok((page_facilities, total_count, slice_counts))
    }

    async fn top_picks(
        &self,
        limit: usize,
    ) -> Result<Vec<(Facility, FacilityVoteSummary)>, RepositoryError> {
        let capped = limit.clamp(1, 50);
        let facilities = self.facilities.read().await;
        let all_votes = self.votes.read().await;

        let mut vote_map: HashMap<String, FacilityVoteSummary> = HashMap::new();
        for ((fid, _), vote) in all_votes.iter() {
            let summary = vote_map.entry(fid.clone()).or_default();
            match vote {
                VoteValue::Like => summary.likes += 1,
                VoteValue::Dislike => summary.dislikes += 1,
            }
        }

        let mut ranked: Vec<(Facility, FacilityVoteSummary)> = facilities
            .iter()
            .map(|f| {
                let votes = vote_map.get(&f.id).cloned().unwrap_or_default();
                (f.clone(), votes)
            })
            .collect();

        ranked.retain(|(_, v)| v.likes > 0);
        if ranked.is_empty() {
            return Ok(Vec::new());
        }

        ranked.sort_by(|(lf, lv), (rf, rv)| {
            rv.likes
                .cmp(&lv.likes)
                .then(rv.score().cmp(&lv.score()))
                .then(rf.trust_score.cmp(&lf.trust_score))
                .then(rf.updated_at.cmp(&lf.updated_at))
        });

        Ok(ranked.into_iter().take(capped).collect())
    }

    async fn autocomplete(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<AutocompleteSuggestion>, RepositoryError> {
        let lower = prefix.to_ascii_lowercase();
        let capped = limit.clamp(1, 20);
        let facilities = self.facilities.read().await;

        let suggestions = facilities
            .iter()
            .filter(|f| {
                f.name.to_ascii_lowercase().contains(&lower)
                    || f.city.to_ascii_lowercase().contains(&lower)
                    || f.postal_code.starts_with(prefix)
            })
            .take(capped)
            .map(|f| AutocompleteSuggestion {
                id: f.id.clone(),
                name: f.name.clone(),
                city: f.city.clone(),
                postal_code: f.postal_code.clone(),
                trust_score: f.trust_score,
            })
            .collect();

        Ok(suggestions)
    }

    async fn search_markers(
        &self,
        query: &FacilitySearchQuery,
        limit: usize,
    ) -> Result<Vec<(String, String, f64, f64, u8)>, RepositoryError> {
        let capped = limit.clamp(1, 200);
        let mut facilities = self.facilities.read().await.clone();

        // Bounding box filter
        if let (Some(min_lat), Some(max_lat), Some(min_lon), Some(max_lon)) =
            (query.min_lat, query.max_lat, query.min_lon, query.max_lon)
        {
            facilities.retain(|f| {
                f.latitude >= min_lat
                    && f.latitude <= max_lat
                    && f.longitude >= min_lon
                    && f.longitude <= max_lon
            });
        }

        // Jurisdiction filter
        if let Some(jurisdiction) = query
            .jurisdiction
            .as_ref()
            .map(|v| v.trim().to_ascii_lowercase())
            && !jurisdiction.is_empty()
            && jurisdiction != "all"
        {
            facilities.retain(|f| {
                f.jurisdiction.code() == jurisdiction
                    || f.jurisdiction.label().eq_ignore_ascii_case(&jurisdiction)
            });
        }

        // Text filter
        if let Some(term) = query.q.as_ref().map(|v| v.trim().to_ascii_lowercase())
            && !term.is_empty()
        {
            facilities.retain(|f| {
                f.name.to_ascii_lowercase().contains(&term)
                    || f.address.to_ascii_lowercase().contains(&term)
                    || f.city.to_ascii_lowercase().contains(&term)
                    || f.postal_code.contains(&term)
            });
        }

        facilities.sort_by_key(|facility| Reverse(facility.trust_score));

        Ok(facilities
            .into_iter()
            .take(capped)
            .map(|f| (f.id, f.name, f.latitude, f.longitude, f.trust_score))
            .collect())
    }

    async fn get_geocodes(
        &self,
        keys: &[String],
    ) -> Result<HashMap<String, GeocodeResult>, RepositoryError> {
        let cache = self.geocodes.read().await;
        Ok(keys
            .iter()
            .filter_map(|key| cache.get(key).map(|result| (key.clone(), result.clone())))
            .collect())
    }

    async fn save_geocodes(&self, results: &[GeocodeResult]) -> Result<(), RepositoryError> {
        let mut cache = self.geocodes.write().await;
        for result in results {
            cache.insert(result.key.clone(), result.clone());
        }
        Ok(())
    }
}

fn haversine_miles(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r = 3958.8;
    let d_lat = (lat2 - lat1).to_radians();
    let d_lon = (lon2 - lon1).to_radians();
    let a = (d_lat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (d_lon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());
    r * c
}
