use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Jurisdiction {
    LosAngelesCounty,
    SanDiegoCounty,
    LongBeach,
    RiversideCounty,
    SanBernardinoCounty,
    OrangeCounty,
    Pasadena,
}

impl Jurisdiction {
    pub fn code(&self) -> &'static str {
        match self {
            Self::LosAngelesCounty => "lac",
            Self::SanDiegoCounty => "sdc",
            Self::LongBeach => "lb",
            Self::RiversideCounty => "riv",
            Self::SanBernardinoCounty => "sbc",
            Self::OrangeCounty => "oc",
            Self::Pasadena => "pas",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::LosAngelesCounty => "Los Angeles County",
            Self::SanDiegoCounty => "San Diego County",
            Self::LongBeach => "Long Beach",
            Self::RiversideCounty => "Riverside County",
            Self::SanBernardinoCounty => "San Bernardino County",
            Self::OrangeCounty => "Orange County",
            Self::Pasadena => "Pasadena",
        }
    }

    /// A central point used when a record has no usable coordinates (latitude, longitude).
    pub fn centre(&self) -> (f64, f64) {
        match self {
            Self::LosAngelesCounty => (34.0522, -118.2437),
            Self::SanDiegoCounty => (32.7157, -117.1611),
            Self::LongBeach => (33.7701, -118.1937),
            Self::RiversideCounty => (33.9806, -117.3755),
            Self::SanBernardinoCounty => (34.1083, -117.2898),
            Self::OrangeCounty => (33.7175, -117.8311),
            Self::Pasadena => (34.1478, -118.1445),
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::LosAngelesCounty,
            Self::SanDiegoCounty,
            Self::LongBeach,
            Self::RiversideCounty,
            Self::SanBernardinoCounty,
            Self::OrangeCounty,
            Self::Pasadena,
        ]
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::all().iter().find(|j| j.code() == code).cloned()
    }

    pub fn from_label(label: &str) -> Option<Self> {
        let lower = label.to_ascii_lowercase();
        Self::all()
            .iter()
            .find(|j| j.label().to_ascii_lowercase() == lower)
            .cloned()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Violation {
    pub code: String,
    pub description: String,
    pub points: i16,
    pub critical: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Inspection {
    pub inspection_id: String,
    pub inspected_at: DateTime<Utc>,
    pub raw_score: Option<f32>,
    pub letter_grade: Option<String>,
    pub placard_status: Option<String>,
    pub violations: Vec<Violation>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Facility {
    pub id: String,
    pub source_id: String,
    pub name: String,
    pub address: String,
    pub city: String,
    pub state: String,
    pub postal_code: String,
    pub latitude: f64,
    pub longitude: f64,
    pub jurisdiction: Jurisdiction,
    pub trust_score: u8,
    pub inspections: Vec<Inspection>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ConnectorIngestionStatus {
    pub source: String,
    pub fetched_records: usize,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SystemIngestionStatus {
    pub last_refresh_at: DateTime<Utc>,
    pub unique_facilities: usize,
    pub connector_stats: Vec<ConnectorIngestionStatus>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum VoteValue {
    Like,
    Dislike,
}

impl VoteValue {
    pub fn to_i16(&self) -> i16 {
        match self {
            Self::Like => 1,
            Self::Dislike => -1,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct FacilityVoteSummary {
    pub likes: u64,
    pub dislikes: u64,
}

impl FacilityVoteSummary {
    pub fn score(&self) -> i64 {
        self.likes as i64 - self.dislikes as i64
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AutocompleteSuggestion {
    pub id: String,
    pub name: String,
    pub city: String,
    pub postal_code: String,
    pub trust_score: u8,
}

#[derive(Clone, Debug)]
pub struct FacilitySearchQuery {
    pub q: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub radius_miles: Option<f64>,
    pub jurisdiction: Option<String>,
    pub sort: Option<String>,
    pub score_slice: Option<String>,
    pub recent_only: Option<bool>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
    pub limit: Option<usize>,
    pub min_lat: Option<f64>,
    pub max_lat: Option<f64>,
    pub min_lon: Option<f64>,
    pub max_lon: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ScoreSliceCounts {
    pub all: usize,
    pub elite: usize,
    pub solid: usize,
    pub watch: usize,
}
