use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
    http::StatusCode,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::error;

use crate::{
    domain::entities::{FacilitySearchQuery, VoteValue},
    presentation::http::AppState,
};

#[derive(Debug, Deserialize)]
pub struct FacilitySearchParams {
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
}

#[derive(Debug, Deserialize)]
pub struct AutocompleteParams {
    pub q: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct TopPicksParams {
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct HealthPayload {
    pub status: &'static str,
    pub timestamp: String,
}

#[derive(Debug, Deserialize)]
pub struct VoteRequest {
    pub vote: String,
}

#[derive(Debug, Deserialize)]
pub struct MarkerSearchParams {
    pub min_lat: f64,
    pub max_lat: f64,
    pub min_lon: f64,
    pub max_lon: f64,
    pub jurisdiction: Option<String>,
    pub q: Option<String>,
    pub limit: Option<usize>,
}

pub async fn health() -> Json<HealthPayload> {
    Json(HealthPayload {
        status: "ok",
        timestamp: Utc::now().to_rfc3339(),
    })
}

pub async fn list_facilities(
    State(state): State<AppState>,
    Query(params): Query<FacilitySearchParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let facilities = state
        .directory_service
        .search(FacilitySearchQuery {
            q: params.q,
            latitude: params.latitude,
            longitude: params.longitude,
            radius_miles: params.radius_miles,
            jurisdiction: params.jurisdiction,
            sort: params.sort,
            score_slice: params.score_slice,
            recent_only: params.recent_only,
            page: params.page,
            page_size: params.page_size,
            limit: params.limit,
            min_lat: None,
            max_lat: None,
            min_lon: None,
            max_lon: None,
        })
        .await
        .map_err(internal_error)?;

    Ok(Json(serde_json::json!(facilities)))
}

pub async fn list_markers(
    State(state): State<AppState>,
    Query(params): Query<MarkerSearchParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let limit = params.limit.unwrap_or(200).clamp(1, 200);
    let result = state
        .directory_service
        .search_markers(
            FacilitySearchQuery {
                q: params.q,
                latitude: None,
                longitude: None,
                radius_miles: None,
                jurisdiction: params.jurisdiction,
                sort: None,
                score_slice: None,
                recent_only: None,
                page: None,
                page_size: None,
                limit: None,
                min_lat: Some(params.min_lat),
                max_lat: Some(params.max_lat),
                min_lon: Some(params.min_lon),
                max_lon: Some(params.max_lon),
            },
            limit,
        )
        .await
        .map_err(internal_error)?;

    Ok(Json(serde_json::json!(result)))
}

pub async fn get_facility(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let facility = state
        .directory_service
        .get(&id)
        .await
        .map_err(internal_error)?;

    match facility {
        Some(record) => Ok(Json(serde_json::json!({ "data": record }))),
        None => Err((StatusCode::NOT_FOUND, "Facility not found".to_owned())),
    }
}

pub async fn top_picks(
    State(state): State<AppState>,
    Query(params): Query<TopPicksParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let limit = params.limit.unwrap_or(10).clamp(1, 50);
    let data = state
        .directory_service
        .top_picks(limit)
        .await
        .map_err(internal_error)?;

    Ok(Json(serde_json::json!({
        "data": data,
        "count": data.len(),
    })))
}

pub async fn ingestion_status(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let stats = state.ingestion_service.stats().await;

    Ok(Json(serde_json::json!({ "data": stats })))
}

/// Starts an ingestion refresh. Requires `Authorization: Bearer <CLEANPLATED_ADMIN_TOKEN>`; the
/// scheduled refresh runs as a Cloud Run Job and does not use this endpoint.
pub async fn trigger_refresh(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, String)> {
    authorize_admin(state.admin_token.as_deref(), &headers)?;

    let ingestion_service = state.ingestion_service.clone();
    tokio::spawn(async move {
        if let Err(err) = ingestion_service.refresh().await {
            error!(error = %err, "Triggered ingestion refresh failed");
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "status": "queued" })),
    ))
}

pub async fn record_vote(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(payload): Json<VoteRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, String)> {
    let vote = match payload.vote.trim().to_ascii_lowercase().as_str() {
        "like" => VoteValue::Like,
        "dislike" => VoteValue::Dislike,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "vote must be one of: like, dislike".to_owned(),
            ));
        }
    };

    let voter_key = extract_voter_key(&headers);
    let limiter_key = format!("{voter_key}:{id}");
    if !state.vote_rate_limiter.allow(&limiter_key).await {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            "rate limit exceeded, please try again shortly".to_owned(),
        ));
    }

    let exists = state
        .directory_service
        .get(&id)
        .await
        .map_err(internal_error)?;
    if exists.is_none() {
        return Err((StatusCode::NOT_FOUND, "Facility not found".to_owned()));
    }

    let summary = state
        .vote_service
        .record_vote(&id, &voter_key, vote)
        .await
        .map_err(internal_error)?;

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "data": {
                "facility_id": id,
                "likes": summary.likes,
                "dislikes": summary.dislikes,
                "vote_score": summary.score(),
            }
        })),
    ))
}

pub async fn autocomplete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<AutocompleteParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let prefix = params.q.unwrap_or_default();
    let trimmed = prefix.trim();
    if trimmed.len() < 2 {
        return Ok(Json(serde_json::json!({ "data": [] })));
    }

    let client_key = extract_voter_key(&headers);
    if !state.autocomplete_rate_limiter.allow(&client_key).await {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            "rate limit exceeded, please try again shortly".to_owned(),
        ));
    }

    let limit = params.limit.unwrap_or(8).clamp(1, 20);
    let suggestions = state
        .directory_service
        .autocomplete(trimmed, limit)
        .await
        .map_err(internal_error)?;

    Ok(Json(serde_json::json!({ "data": suggestions })))
}

fn internal_error(error: impl std::fmt::Display) -> (StatusCode, String) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("internal error: {error}"),
    )
}

fn extract_voter_key(headers: &HeaderMap) -> String {
    let from_xff = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());

    if let Some(value) = from_xff {
        return value;
    }

    let from_real_ip = headers
        .get("x-real-ip")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());

    from_real_ip.unwrap_or_else(|| "unknown".to_owned())
}

/// Admin endpoints are hidden (404) unless a token is configured, and need a matching bearer
/// token (401) when one is.
fn authorize_admin(
    expected: Option<&str>,
    headers: &HeaderMap,
) -> Result<(), (StatusCode, String)> {
    let Some(expected) = expected else {
        return Err((StatusCode::NOT_FOUND, "Not found".to_owned()));
    };
    let provided = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    if constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
        Ok(())
    } else {
        Err((StatusCode::UNAUTHORIZED, "Unauthorized".to_owned()))
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

#[cfg(test)]
mod admin_auth_tests {
    use axum::http::{HeaderValue, header::AUTHORIZATION};

    use super::*;

    fn bearer(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        headers
    }

    #[test]
    fn admin_endpoints_are_hidden_without_a_configured_token() {
        let result = authorize_admin(None, &bearer("anything"));
        assert_eq!(result.unwrap_err().0, StatusCode::NOT_FOUND);
    }

    #[test]
    fn a_wrong_or_missing_token_is_rejected() {
        assert_eq!(
            authorize_admin(Some("secret-token"), &bearer("wrong"))
                .unwrap_err()
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            authorize_admin(Some("secret-token"), &HeaderMap::new())
                .unwrap_err()
                .0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn the_configured_token_is_accepted() {
        assert!(authorize_admin(Some("secret-token"), &bearer("secret-token")).is_ok());
    }
}
