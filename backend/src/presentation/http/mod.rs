pub mod handlers;
pub mod rate_limit;
pub mod routes;

use std::sync::Arc;

use crate::application::services::{DirectoryService, IngestionService, VoteService};
use crate::presentation::http::rate_limit::RateLimiter;

#[derive(Clone)]
pub struct AppState {
    pub directory_service: Arc<DirectoryService>,
    pub ingestion_service: Arc<IngestionService>,
    pub vote_service: Arc<VoteService>,
    pub vote_rate_limiter: RateLimiter,
    pub autocomplete_rate_limiter: RateLimiter,
    /// Bearer token for admin endpoints (`CLEANPLATED_ADMIN_TOKEN`). When unset, admin endpoints
    /// return 404.
    pub admin_token: Option<String>,
}
