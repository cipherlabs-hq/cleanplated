# CleanPlated New-Project Migration Runbook

This runbook migrates from legacy `trustarant-*` resources in `us-west1` to a new `cleanplated-*` project and resources in `us-central1`.

## Current backend outage (verify first)

Observed from browser + direct HTTP:
- Frontend `https://cleanplated.com` calls `https://trustarant-api-3y7woju2vq-uw.a.run.app`
- Backend URL is returning Cloud Run gateway `500/503`
- Browser reports CORS because error responses do not include CORS headers

Run this immediately after authenticating gcloud:

```bash
gcloud auth login
gcloud config set project trustarant-2b14

gcloud run services describe trustarant-api \
  --region us-west1 \
  --format="yaml(status.url,status.latestReadyRevisionName,status.conditions)"

gcloud run revisions list \
  --service trustarant-api \
  --region us-west1 \
  --format="table(name,service,active,createTime,status.conditions[0].type,status.conditions[0].state)"

gcloud logging read \
  'resource.type="cloud_run_revision" AND resource.labels.service_name="trustarant-api"' \
  --project trustarant-2b14 \
  --limit 100 \
  --freshness 2h \
  --format="value(timestamp,textPayload,jsonPayload.message)"
```

If needed, roll traffic back to last healthy revision:

```bash
gcloud run services update-traffic trustarant-api \
  --region us-west1 \
  --to-revisions <REVISION_NAME>=100
```

## Target architecture

- New project ID: `cleanplated-prod` (or your preferred `cleanplated-*` ID)
- Region: `us-central1`
- Artifact Registry repo: `cleanplated`
- Cloud Run services/jobs:
  - `cleanplated-api`
  - `cleanplated-web`
  - `cleanplated-ingestion`
  - `cleanplated-ingestion-worker`
- Secret: `cleanplated-database-url`

## Phase 1: Create new project and baseline infra

```bash
cd infra/terraform/bootstrap
cp terraform.tfvars.example terraform.tfvars
# Set billing account and confirm project_name/project_id_prefix
# Example:
# project_name = "cleanplated"
# project_id_prefix = "cleanplated"
# region = "us-central1"

terraform init
terraform apply
terraform output
```

Capture:
- `project_id` (new cleanplated project)
- `terraform_state_bucket`

## Phase 2: Provision runtime stack in new project

```bash
cd ../environments/prod
cp terraform.tfvars.example terraform.tfvars
# Set:
# project_id = "<new_project_id>"
# region = "us-central1"
# backend_image = "us-central1-docker.pkg.dev/<new_project_id>/cleanplated/backend:latest"
# frontend_image = "us-central1-docker.pkg.dev/<new_project_id>/cleanplated/frontend:latest"

terraform init \
  -backend-config="bucket=<terraform_state_bucket>" \
  -backend-config="prefix=terraform/prod"

terraform apply
```

## Phase 3: Build and deploy app images to new project

```bash
PROJECT_ID=<new_project_id>
REGION=us-central1

gcloud auth configure-docker ${REGION}-docker.pkg.dev

docker build -t ${REGION}-docker.pkg.dev/${PROJECT_ID}/cleanplated/backend:latest ./backend
docker push ${REGION}-docker.pkg.dev/${PROJECT_ID}/cleanplated/backend:latest

docker build -t ${REGION}-docker.pkg.dev/${PROJECT_ID}/cleanplated/frontend:latest ./frontend
docker push ${REGION}-docker.pkg.dev/${PROJECT_ID}/cleanplated/frontend:latest
```

Re-apply terraform (or deploy via Cloud Build) so Cloud Run services reference the pushed images.

## Phase 4: Configure database secret in new project

```bash
PROJECT_ID=<new_project_id>
SECRET_ID=cleanplated-database-url

printf '%s' 'postgresql://...neon-url...' | \
  gcloud secrets versions add "${SECRET_ID}" \
    --project "${PROJECT_ID}" \
    --data-file=-
```

## Phase 5: Domain cutover

Map frontend domain to new frontend service:

```bash
gcloud beta run domain-mappings create \
  --project <new_project_id> \
  --region us-central1 \
  --service cleanplated-web \
  --domain cleanplated.com

gcloud beta run domain-mappings create \
  --project <new_project_id> \
  --region us-central1 \
  --service cleanplated-web \
  --domain www.cleanplated.com
```

Optional API domain mapping:

```bash
gcloud beta run domain-mappings create \
  --project <new_project_id> \
  --region us-central1 \
  --service cleanplated-api \
  --domain api.cleanplated.com
```

Fetch DNS records:

```bash
gcloud beta run domain-mappings describe --domain cleanplated.com \
  --project <new_project_id> \
  --region us-central1 \
  --format="yaml(status.resourceRecords)"

gcloud beta run domain-mappings describe --domain www.cleanplated.com \
  --project <new_project_id> \
  --region us-central1 \
  --format="yaml(status.resourceRecords)"
```

Validate readiness:

```bash
gcloud beta run domain-mappings list \
  --project <new_project_id> \
  --region us-central1
```

## Phase 6: CI/CD trigger migration

Use `scripts/create_cloudbuild_triggers.sh` with:
- `PROJECT_ID=<new_project_id>`
- `REGION=us-central1`
- `CONNECTION=cleanplated-conn`
- `REPO_NAME=cleanplated-repo`
- `REMOTE_URI=<github_repo_url>`

## Phase 7: Decommission old project resources

After traffic and data validation:
1. Disable old Cloud Build triggers
2. Scale down/remove old Cloud Run services/jobs in `trustarant-2b14`
3. Remove old domain mappings
4. Archive or delete old project per retention policy
