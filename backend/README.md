# CleanPlated Backend (Rust + Clean Architecture)

This backend is structured around clean architecture boundaries:

- `domain`: core entities and repository contracts
- `application`: directory/search and trust-score normalization use cases
- `infrastructure`: jurisdiction connectors, scheduler, and repository implementations
- `presentation`: HTTP transport (Axum)

## Current Database Type

The backend now supports:

- **PostgreSQL (recommended/prod)** when `DATABASE_URL` is set
- **In-memory fallback** when `DATABASE_URL` is missing

For production, use a managed PostgreSQL URL (for example, Neon) injected through
Secret Manager as `DATABASE_URL`.

## Runtime Modes

Set `CLEANPLATED_RUN_MODE` to control execution:

- `api` (default): starts HTTP API only
- `worker`: long-running ingestion loop (interval-based)
- `refresh_once`: one-shot ingestion run, then process exits (ideal for Cloud Run Jobs)

## Run locally

```bash
cd backend
cp .env.example .env
cargo run
```

## Live Data Connectors

### San Diego (Socrata)

`SanDiegoConnector` pulls from the live SODA dataset:

- Base URL: `https://internal-sandiegocounty.data.socrata.com`
- Dataset ID: `c5ez-ufrd` (`Food Facility Permits`)

Configure with environment variables:

- `CLEANPLATED_SD_SOCRATA_BASE_URL`
- `CLEANPLATED_SD_SOCRATA_DATASET_ID`
- `CLEANPLATED_SD_SOCRATA_LIMIT`
- `CLEANPLATED_SD_SOCRATA_PAGE_SIZE`
- `CLEANPLATED_SD_SOCRATA_MAX_RECORDS` (optional cap)
- `CLEANPLATED_SD_SOCRATA_ACTIVE_ONLY`
- `CLEANPLATED_SD_SOCRATA_TIMEOUT_SECS`
- `CLEANPLATED_SD_SOCRATA_APP_TOKEN` (optional but recommended)

The implementation references:

- `docs/research/socal-food-safety-data-strategy.md`
- `docs/research/strategic-framework-full.md`

### Los Angeles County (ArcGIS)

`LaCountyConnector` loads facility inventory and inspections from the county's published CSV
items, whose IDs stay the same across quarterly releases. If a CSV download fails it falls back
to the ArcGIS FeatureServer, finding the newest service by title in the county's ArcGIS org,
because the county republishes those under new date-stamped names every quarter.

Config:

- `CLEANPLATED_LA_INVENTORY_CSV_URL`, `CLEANPLATED_LA_INSPECTIONS_CSV_URL` (override the CSV items)
- `CLEANPLATED_LA_INVENTORY_URL`, `CLEANPLATED_LA_INSPECTIONS_URL` (pin a FeatureServer instead of discovering it)
- `CLEANPLATED_LA_LIMIT`
- `CLEANPLATED_LA_PAGE_SIZE`
- `CLEANPLATED_LA_MAX_RECORDS` (optional cap)
- `CLEANPLATED_LA_TIMEOUT_SECS`

### Long Beach (Live web page)

`LongBeachConnector` fetches the live Long Beach restaurant-closures page with
fallback endpoint logic and strict parse checks (fails ingestion status when no rows parse).

Config:

- `CLEANPLATED_LONG_BEACH_CLOSURES_URL`
- `CLEANPLATED_LONG_BEACH_LIMIT`
- `CLEANPLATED_LONG_BEACH_TIMEOUT_SECS`

### LIVES Batch (San Bernardino + Riverside optional)

`LivesBatchConnector` fetches live ArcGIS records:

- San Bernardino: default live FeatureServer endpoint
- Riverside: optional live FeatureServer URL via env (set when available)

Config:

- `CLEANPLATED_SBC_ARCGIS_URL`
- `CLEANPLATED_RIVERSIDE_ARCGIS_URL` (optional)
- `CLEANPLATED_LIVES_LIMIT`
- `CLEANPLATED_LIVES_PAGE_SIZE`
- `CLEANPLATED_LIVES_MAX_RECORDS` (optional cap)
- `CLEANPLATED_LIVES_TIMEOUT_SECS`

### CPRA Imports (Orange + Pasadena)

`CpraConnector` uses a tiered strategy:

- Orange County: live `myhealthdepartment` restaurant-closure API ingestion (default),
  or CPRA export URL when configured.
- Pasadena: live ArcGIS restaurant directory ingestion (default), or CPRA export URL
  when configured.

Config:

- `CLEANPLATED_OC_CPRA_EXPORT_URL`
- `CLEANPLATED_PASADENA_CPRA_EXPORT_URL`
- `CLEANPLATED_CPRA_TIMEOUT_SECS`
- `CLEANPLATED_OC_LIVE_ENABLED`
- `CLEANPLATED_OC_LIVE_ENDPOINT`
- `CLEANPLATED_OC_LIVE_PAGE_SIZE`
- `CLEANPLATED_OC_LIVE_MAX_RECORDS`
- `CLEANPLATED_OC_LIVE_DAYS_WINDOW`
- `CLEANPLATED_PASADENA_LIVE_ENABLED`
- `CLEANPLATED_PASADENA_DIRECTORY_URL`
- `CLEANPLATED_PASADENA_PAGE_SIZE`
- `CLEANPLATED_PASADENA_MAX_RECORDS`

If CPRA URLs are omitted, live fallbacks run automatically. Disable fallbacks only if
you intentionally want CPRA-only behavior.

## Geocoding

Some sources publish addresses without coordinates (San Diego permits, Long Beach closures, and LA
inspections for facilities missing from the current inventory). Connectors give those a fallback
point and mark them `coordinates_approximate`. After stitching, ingestion geocodes their addresses
with the U.S. Census Bureau batch geocoder and moves them to the result.

Results, including addresses with no match, are stored in the `geocode_cache` table, which
survives the `facilities` rebuild on every run. Only new addresses are looked up, so the first run
takes a few minutes and later runs take seconds. Unmatched addresses are retried after 30 days, and
matches outside Southern California are rejected. A geocoder failure is logged and leaves the
fallback in place; it never fails the refresh.

Config:

- `CLEANPLATED_GEOCODING_ENABLED` (default `true`)
- `CLEANPLATED_GEOCODING_MAX_PER_RUN` (default `25000` new lookups per refresh)
- `CLEANPLATED_GEOCODER_BATCH_SIZE` (default `1000`, max `10000`)
- `CLEANPLATED_GEOCODER_CONCURRENCY` (default `4`)
- `CLEANPLATED_GEOCODER_TIMEOUT_SECS` (default `300`)

## Run with Docker Compose

From repository root:

```bash
docker compose up --build
```

Compose starts:

- `backend` in `api` mode
- `worker` in `worker` mode
- `postgres` (local dev DB)

## Core endpoints

- `GET /health`
- `GET /api/v1/facilities?q=sushi&latitude=34.0522&longitude=-118.2437&radius_miles=2&limit=20`
- `GET /api/v1/facilities/{id}`
- `GET /api/v1/system/ingestion` (last ingestion timestamp, per-source fetched counts, and total unique facilities)
- `POST /api/v1/system/refresh` (queues an async ingestion refresh)

## Notes

- All connectors now use live network calls and emit no hardcoded sample facilities.
- Riverside and CPRA sources support environment-driven overrides when you have higher-fidelity exports.
- The San Diego feed currently exposes permit-status metadata; Trust Score signals are derived from those fields until full inspection-line datasets are integrated.
