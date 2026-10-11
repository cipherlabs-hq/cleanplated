use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::domain::entities::{Jurisdiction, ScoreSliceCounts, Violation};

#[derive(Clone, Debug)]
pub struct SourceFacilityInput {
    pub source_id: String,
    pub name: String,
    pub address: String,
    pub city: String,
    pub state: String,
    pub postal_code: String,
    pub latitude: f64,
    pub longitude: f64,
    /// True when the source had no coordinates and the connector used a fixed fallback point
    /// (a city or county centre). Ingestion geocodes these by address.
    pub coordinates_approximate: bool,
    pub jurisdiction: Jurisdiction,
    pub inspected_at: DateTime<Utc>,
    pub raw_score: Option<f32>,
    pub letter_grade: Option<String>,
    pub placard_status: Option<String>,
    pub violations: Vec<Violation>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FacilitySearchResult {
    pub data: Vec<FacilitySummary>,
    pub count: usize,
    pub total_count: usize,
    pub page: usize,
    pub page_size: usize,
    pub slice_counts: ScoreSliceCounts,
}

#[derive(Clone, Debug, Serialize)]
pub struct FacilitySummary {
    pub id: String,
    pub name: String,
    pub address: String,
    pub city: String,
    pub state: String,
    pub postal_code: String,
    pub latitude: f64,
    pub longitude: f64,
    pub jurisdiction: String,
    pub trust_score: u8,
    pub latest_inspection_at: Option<DateTime<Utc>>,
    pub likes: u64,
    pub dislikes: u64,
    pub vote_score: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct FacilityDetail {
    pub id: String,
    pub source_id: String,
    pub name: String,
    pub address: String,
    pub city: String,
    pub state: String,
    pub postal_code: String,
    pub latitude: f64,
    pub longitude: f64,
    pub jurisdiction: String,
    pub trust_score: u8,
    pub inspections_count: usize,
    pub latest_inspection_at: Option<DateTime<Utc>>,
    pub likes: u64,
    pub dislikes: u64,
    pub vote_score: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct MarkerPoint {
    pub id: String,
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
    pub trust_score: u8,
}

#[derive(Clone, Debug, Serialize)]
pub struct MarkerSearchResult {
    pub data: Vec<MarkerPoint>,
    pub count: usize,
}
