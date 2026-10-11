mod application;
mod config;
mod domain;
mod infrastructure;
mod presentation;

use std::{net::SocketAddr, sync::Arc, time::Duration};

use application::services::{DirectoryService, IngestionService, TrustScoreService, VoteService};
use axum::Router;
use config::{RunMode, Settings};
use domain::repositories::FacilityRepository;
use infrastructure::{
    connectors::default_connectors,
    repositories::{InMemoryFacilityRepository, PostgresFacilityRepository},
    scheduler,
};
use presentation::http::{AppState, rate_limit::RateLimiter, routes::build_router};
use tokio::net::TcpListener;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{error, info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let settings = Settings::from_env();
    let repository = build_repository(&settings).await?;

    let trust_score_service = Arc::new(TrustScoreService);
    let ingestion_service = Arc::new(IngestionService::new(
        repository.clone(),
        trust_score_service,
        default_connectors(),
    ));

    if settings.run_mode == RunMode::RefreshOnce {
        info!("Running one-shot ingestion refresh");
        ingestion_service.refresh().await?;
        info!("One-shot ingestion refresh completed");
        return Ok(());
    }

    if settings.run_mode == RunMode::Worker {
        info!("Running ingestion worker mode");
        if let Err(error) = ingestion_service.refresh().await {
            error!(%error, "Initial worker refresh failed");
        } else {
            info!("Initial worker refresh completed");
        }
        scheduler::run(ingestion_service, settings.ingestion_interval_hours).await;
        return Ok(());
    }

    if settings.enable_background_ingestion {
        let initial_ingestion_service = ingestion_service.clone();
        tokio::spawn(async move {
            if let Err(err) = initial_ingestion_service.refresh().await {
                error!(error = %err, "Initial ingestion failed; API will still start");
            } else {
                info!("Initial ingestion completed");
            }
        });

        tokio::spawn(scheduler::run(
            ingestion_service.clone(),
            settings.ingestion_interval_hours,
        ));
    }

    let app_state = AppState {
        directory_service: Arc::new(DirectoryService::new(repository.clone())),
        ingestion_service: ingestion_service.clone(),
        vote_service: Arc::new(VoteService::new(repository)),
        vote_rate_limiter: RateLimiter::new(20, Duration::from_secs(60)),
        autocomplete_rate_limiter: RateLimiter::new(60, Duration::from_secs(60)),
    };

    let app = app_router(app_state, &settings);
    let addr: SocketAddr = format!("{}:{}", settings.host, settings.port).parse()?;
    let listener = TcpListener::bind(addr).await?;

    info!(address = %addr, "Cleanplated backend listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

fn app_router(state: AppState, settings: &Settings) -> Router {
    let cors = if settings.cors_origin.trim() == "*" {
        CorsLayer::new()
            .allow_origin(tower_http::cors::Any)
            .allow_methods(tower_http::cors::Any)
            .allow_headers(tower_http::cors::Any)
    } else {
        let allow_origin = settings
            .cors_origin
            .parse::<axum::http::HeaderValue>()
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("http://localhost:5173"));

        CorsLayer::new()
            .allow_origin(allow_origin)
            .allow_methods(tower_http::cors::Any)
            .allow_headers(tower_http::cors::Any)
    };

    build_router(state)
        .layer(TraceLayer::new_for_http())
        .layer(cors)
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cleanplated_backend=info,tower_http=info".into()),
        )
        .init();
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        error!(%error, "Unable to listen for shutdown signal");
    }

    info!("Shutdown signal received");
}

async fn build_repository(settings: &Settings) -> anyhow::Result<Arc<dyn FacilityRepository>> {
    if let Some(database_url) = settings.database_url.as_deref() {
        let repository = PostgresFacilityRepository::connect(database_url)
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        info!("Using PostgreSQL facility repository");
        return Ok(Arc::new(repository));
    }

    warn!("DATABASE_URL not set, falling back to in-memory repository");
    Ok(Arc::new(InMemoryFacilityRepository::new()))
}
