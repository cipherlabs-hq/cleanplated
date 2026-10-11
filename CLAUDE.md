# CleanPlated — Project Instructions

## Overview

CleanPlated is a restaurant health inspection data platform for Southern California. It aggregates public health inspection records from county/city agencies and presents them in a mobile-first web interface.

- **Backend**: Rust/Axum API + PostgreSQL + Redis
- **Frontend**: Vue 3 + Carbon Design System (v10) + Vite
- **Deployment**: GCP Cloud Run via Cloud Build
- **Repo**: `cipherlabs-hq/cleanplated`

## Frontend Development — Carbon Design System

**All frontend work MUST follow IBM's Carbon Design System v10 conventions.** Do not introduce custom component styles that conflict with Carbon's design language.

### Stack

- **Framework**: Vue 3 (Composition API, `<script setup>`)
- **Components**: `@carbon/vue` v3 (registered globally via `app.use(CarbonComponentsVue)`)
- **Icons**: `@carbon/icons-vue` (v10, size-suffixed: `Search16`, `Filter16`, etc.)
- **Styles**: `carbon-components/css/carbon-components.css` imported in `main.ts`
- **Custom CSS**: `src/style.css` — only for layout, spacing, and brand overrides
- **Font**: IBM Plex Sans (loaded via Google Fonts in `index.html`)

### Carbon Components in Use

Use these Carbon Vue components — do NOT replace them with custom HTML/CSS equivalents:

| Purpose | Component | Notes |
|---------|-----------|-------|
| Search | `cv-search` | Size `lg`, `form-item=false` for inline use |
| Buttons | `cv-button` | `kind="primary"` (green), `kind="tertiary"`, `kind="secondary"` |
| Dropdowns | `cv-select` + `cv-select-option` | With `hide-label` for compact layout |
| Slider | `cv-slider` | For radius control |
| Checkbox | `cv-checkbox` | For filter toggles |
| Tags | `cv-tag` | `kind="green"` for filter count badges |

### Carbon Design Rules

1. **No conflicting border-radius.** Carbon v10 uses `0px` border-radius on buttons, inputs, and tiles. Do not add `border-radius: 12px` or similar to containers, cards, or interactive elements. The only exception is the mobile shell frame on desktop (`border-radius: 16px`) and the logo (`border-radius: 50%`).

2. **Use border-based separation, not card-based.** Result items use `border-bottom: 1px solid var(--cp-border)` between items, not individual rounded cards. This follows Carbon's structured list / tile pattern.

3. **Green brand theme.** Carbon's default blue interactive color (`#0f62fe`) is overridden to CleanPlated green (`#24a148`) via CSS overrides on `.bx--btn--primary`, `.bx--slider__thumb`, `.bx--checkbox:checked`, `.bx--search-input:focus`, etc. All green overrides are in the "Carbon overrides" section at the bottom of `style.css`.

4. **Carbon spacing scale.** Use the 8px grid via CSS custom properties:
   - `--sp-1: 4px`, `--sp-2: 8px`, `--sp-3: 12px`, `--sp-4: 16px`, `--sp-5: 24px`, `--sp-6: 32px`, `--sp-7: 48px`

5. **Carbon type scale.** Use IBM Plex Sans. Body text at 14px/1.43. Headings use 600 weight. Labels use 12px with `letter-spacing: 0.32px`.

6. **Let Carbon handle interactive element styling.** Do not write custom CSS for buttons, search inputs, selects, checkboxes, or sliders. Only override Carbon styles for brand color changes.

7. **Score badges** are the one exception — they use colored backgrounds (`--cp-green-100`, `--cp-yellow-100`, `--cp-red-100`) because they are data visualization elements, not interactive components.

### Layout Conventions

- **Mobile shell**: Max width `430px`, centered on desktop with `border-radius: 16px` phone frame
- **Panels**: `border-top` + `border-bottom` (Carbon tile pattern), no side margins
- **Results**: Full-width items separated by `border-bottom`
- **Stats**: 3-column grid with column dividers
- **Footer**: Dark (`--cp-gray-100`) background with inverted brand colors

### File Structure

```
frontend/
  index.html          # Entry point, IBM Plex Sans font link
  src/
    main.ts           # Vue app setup, Carbon plugin registration
    App.vue           # Single-file component (all app logic)
    style.css         # Layout + brand overrides (loaded after Carbon CSS)
    lib/analytics.ts  # GA event tracking
  public/
    cleanplated-logo.svg  # Logo (viewBox cropped to content)
    omega-purple.svg      # Cipher Labs logo
  server.mjs          # Express SSR server for production
  Dockerfile          # Multi-stage: build with Vite, serve with Express on port 8080
```

### API

The backend API base URL is configured via `VITE_API_BASE_URL` (build-time env var).

Key endpoints:
- `GET /api/v1/facilities` — search/browse with pagination, filters, geo
- `GET /api/v1/facilities/top-picks` — community favorites
- `POST /api/v1/facilities/:id/vote` — like/dislike
- `GET /api/v1/system/ingestion` — data source stats

### Docker Development

```bash
docker compose up --build -d          # Start all services
docker compose up --build -d frontend # Rebuild just frontend
```

- Frontend: `http://localhost:15173` (maps to container port 8080)
- Backend API: `http://localhost:18080`
- Frontend's `VITE_API_BASE_URL` is set to `http://localhost:18080` in docker-compose.yml

### Common Pitfalls

- **Port mapping**: Frontend Dockerfile `EXPOSE 8080` and server.mjs listens on 8080. Docker-compose maps `15173:8080`.
- **VITE_ env vars**: Baked into the JS bundle at build time. Changes require a full rebuild (`docker compose up --build`).
- **Carbon CSS import order**: `carbon-components.css` must be imported before `style.css` in `main.ts`.
- **Slider overflow**: The Carbon slider can overflow its container. Always add `overflow: hidden` to slider panel wrappers.
- **SVG logo**: The viewBox is cropped to `115 115 410 410` to eliminate the square bounding box. The background rectangle path (`M0,0h640v640H0V0Z`) was removed for transparency.

## Backend Development

- **Language**: Rust (edition 2021)
- **Framework**: Axum
- **Database**: PostgreSQL 16 with custom SQL migrations
- **Commit style**: Conventional commits (feat/fix/refactor/chore)

## CI/CD

- **Cloud Build triggers**: `cleanplated-backend-main` and `cleanplated-frontend-main` on push to `main`
- **Artifact Registry**: `us-central1-docker.pkg.dev/cleanplated/cleanplated`
- **Cloud Run services**: `cleanplated-api` (backend), `cleanplated-web` (frontend)
- **Google Maps API key**: Passed via Cloud Build substitution `_GOOGLE_MAPS_API_KEY`
