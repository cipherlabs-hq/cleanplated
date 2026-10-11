use async_trait::async_trait;
use std::collections::HashMap;

use crate::domain::{
    entities::{
        AutocompleteSuggestion, Facility, FacilitySearchQuery, FacilityVoteSummary,
        ScoreSliceCounts, SystemIngestionStatus, VoteValue,
    },
    errors::RepositoryError,
};

#[async_trait]
pub trait FacilityRepository: Send + Sync {
    async fn replace_all(&self, facilities: Vec<Facility>) -> Result<(), RepositoryError>;
    async fn get_by_id(&self, id: &str) -> Result<Option<Facility>, RepositoryError>;
    async fn set_system_ingestion_status(
        &self,
        status: SystemIngestionStatus,
    ) -> Result<(), RepositoryError>;
    async fn get_system_ingestion_status(
        &self,
    ) -> Result<Option<SystemIngestionStatus>, RepositoryError>;
    async fn upsert_facility_vote(
        &self,
        facility_id: &str,
        voter_key: &str,
        vote: VoteValue,
    ) -> Result<FacilityVoteSummary, RepositoryError>;
    async fn get_facility_vote_summaries(
        &self,
        facility_ids: &[String],
    ) -> Result<HashMap<String, FacilityVoteSummary>, RepositoryError>;
    async fn search_facilities(
        &self,
        query: &FacilitySearchQuery,
    ) -> Result<(Vec<Facility>, usize, ScoreSliceCounts), RepositoryError>;
    async fn autocomplete(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<AutocompleteSuggestion>, RepositoryError>;
    async fn top_picks(
        &self,
        limit: usize,
    ) -> Result<Vec<(Facility, FacilityVoteSummary)>, RepositoryError>;
    async fn search_markers(
        &self,
        query: &FacilitySearchQuery,
        limit: usize,
    ) -> Result<Vec<(String, String, f64, f64, u8)>, RepositoryError>;
}
