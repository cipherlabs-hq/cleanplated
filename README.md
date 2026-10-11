<p align="center">
  <img src="assets/cleanplated-logo.svg" alt="CleanPlated logo" width="200">
</p>

# CleanPlated

CleanPlated is a mobile-first directory of Southern California restaurant health inspections,
live at [cleanplated.com](https://cleanplated.com). Each county and city publishes its inspection
data differently; CleanPlated pulls it into one schema and gives every facility a 0–100 trust
score you can search by name, address, or distance.

## How it works

A daily ingestion job pulls public records from each source, normalizes them, and replaces the
directory in one transaction:

| Source | Format |
|---|---|
| Los Angeles County | Published CSV items, with an ArcGIS FeatureServer fallback |
| San Diego County | Socrata (SODA) API |
| Riverside and San Bernardino counties | ArcGIS (LIVES batch) |
| Orange County | Public closures API |
| Pasadena | Public facility directory |
| Long Beach | Scraped closures page |

Along the way it:

- **Retries each source** and records per-source health, so one broken county doesn't fail the
  run. Status is at `GET /api/v1/system/ingestion`.
- **Refuses to publish a bad refresh.** If no source succeeds, or the new dataset is under half
  the size of the last one, the previous data stays live.
- **Geocodes facilities that arrive without coordinates** using the U.S. Census batch geocoder,
  with results cached in Postgres so each address is looked up once.
- **Scores every facility 0–100** from whatever the source provides: a raw score, a letter grade,
  or a placard color.

Search combines Postgres full-text search, trigram fuzzy matching, and radius queries.

## Stack

- **Backend** (`backend/`): Rust, axum, tokio, sqlx on PostgreSQL. Layered as domain,
  application, infrastructure, and presentation; repositories and data sources sit behind traits
  with Postgres and in-memory implementations. See [`backend/README.md`](backend/README.md) for
  the connectors and configuration.
- **Frontend** (`frontend/`): Vue 3, TypeScript, Vite, IBM Carbon, Google Maps.
- **Infrastructure** (`infra/terraform/`): Google Cloud Run (API, web, and the ingestion job),
  Cloud Scheduler, Secret Manager, and Artifact Registry, all in Terraform.
- **CI/CD** (`cloudbuild/`): Cloud Build deploys the backend and frontend on every push to
  `main` that touches them.

## Run it locally

```bash
cp .env.example .env            # add VITE_GOOGLE_MAPS_API_KEY for the map view
docker compose up --build
./scripts/seed-database.sh      # optional: load a subset of live data (2–5 minutes)
```

- Frontend: http://localhost:15173
- API: http://localhost:18080

Without `DATABASE_URL` the backend uses an in-memory repository. The compose file also starts
Redis, which nothing uses yet.

The Maps key needs the Maps JavaScript API enabled and should be restricted to HTTP referrers
(`localhost:15173` locally).

## Tests

```bash
cd backend
cargo test                      # unit tests, no network
cargo test -- --ignored         # also hit the live county and Census APIs
```

## Deployment

Pushes to `main` deploy through Cloud Build triggers. To recreate them in a new project, connect
the repository to Cloud Build and run `./scripts/create_cloudbuild_triggers.sh`.

`POST /api/v1/system/refresh` starts an ingestion run on demand. It needs
`Authorization: Bearer $CLEANPLATED_ADMIN_TOKEN` and returns 404 when no token is configured.
