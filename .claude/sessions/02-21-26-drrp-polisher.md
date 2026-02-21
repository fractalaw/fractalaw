# Session: 2026-02-21 — DRRP Polisher Micro-App

## Context

**Phase**: 3 (MicroApp Runtime)
**Goal**: Design and begin implementing the first "real" micro-app — the DRRP Polisher — which takes rough regex-flagged DRRP annotations from the Elixir app (sertantai) and uses generative AI to extract precise duty/right/responsibility/power provisions from legislation text.

**Previous session**: Phase 3 Session 1 complete — Wasmtime bootstrap working. `fractalaw run` executes guest components with audit logging, fuel metering, epoch interruption. 112 tests passing.

**Sister app**: `sertantai` (Elixir/Phoenix) — scrapes legislation.gov.uk, runs regex-based DRRP detection, needs to sync annotations to fractalaw for AI polishing, and receive polished results back.

## What the DRRP Polisher Does

```
sertantai (Elixir/Phoenix — always on)
├── Scrapes legislation.gov.uk
├── Regex flags sections containing DRRP
├── Stores annotations as JSONB
└── Exposes outbox endpoint: GET /api/outbox/annotations?since=<ts>
         │
         │  (hub wakes up, pulls new work)
         ▼
fractalaw hub (Rust — intermittently on, bedroom box)
├── CLI: fractalaw sync pull
│   └── GET sertantai/api/outbox/annotations?since=<last_sync>
│   └── Stores annotations in local DuckDB
│
├── CLI: fractalaw run drrp-polisher.wasm
│   ├── Query: load unpolished annotations (fractal:data/query)
│   ├── Query: load legislation text from LanceDB (fractal:data/query)
│   ├── For each: call Claude API to extract precise provision (fractal:ai/inference)
│   ├── Write polished DRRPEntry structs (fractal:data/mutate)
│   └── Audit: log processing stats (fractal:audit/log)
│
├── CLI: fractalaw sync push
│   └── POST sertantai/api/inbox/polished
│   └── Sends polished results back
│
└── (hub goes back to sleep)
         │
         ▼
sertantai picks up polished results on next request
```

### Concrete Example

**Input** (from sertantai regex): Section 2 of HSWA 1974 flagged as containing a "duty". Raw text is ~500 words with qualifications, cross-references, and multiple sub-provisions.

**Output** (after polishing):
```json
{
  "law_name": "UK_ukpga_1974_37",
  "provision": "s.2(1)",
  "drrp_type": "duty",
  "holder": "every employer",
  "text": "It shall be the duty of every employer to ensure, so far as is reasonably practicable, the health, safety and welfare at work of all his employees",
  "qualifier": "so far as is reasonably practicable",
  "clause_ref": "s.2(1)"
}
```

## Design Decisions

### 1. Sync Protocol: HTTP REST with JSON

**Decision**: HTTP REST with JSON payloads. Both sides expose simple endpoints.

**Rationale**:
- The hub is a box in the bedroom, intermittently on — not a server exposing endpoints
- Async outbox/inbox pattern: each side pushes what it owns, the other pulls when ready
- JSON for rapid iteration — Elixir/Phoenix has excellent JSON support natively
- Can migrate to Arrow IPC later if volume demands it, but the API shape stays the same
- No gRPC/Arrow Flight complexity — Elixir doesn't have a native client for that

**Endpoints on sertantai**:
- `GET /api/outbox/annotations?since=<timestamp>` — fractalaw pulls new annotations
- `POST /api/inbox/polished` — fractalaw pushes polished results

**Endpoints on fractalaw** (not needed initially — hub pulls/pushes, doesn't serve):
- Future: if sertantai needs to push in real-time, fractalaw would need a listener
- For now: CLI-driven `fractalaw sync pull` / `fractalaw sync push`

### 2. Annotation Schema

**Decision**: Start with the minimum viable schema below. Evolve as the micro-app's needs become clearer.

```
annotation {
  law_name: string,          -- e.g. "UK_ukpga_1974_37"
  provision: string,         -- e.g. "s.2" or "s.2(1)"
  drrp_type: string,         -- "duty" | "right" | "responsibility" | "power"
  source_text: string,       -- the raw section text (rough, from regex match)
  confidence: float,         -- regex confidence (0.0–1.0)
  scraped_at: timestamp,     -- when sertantai scraped it
}
```

Sertantai stores these as JSONB internally. The outbox endpoint serialises them as a JSON array for the pull.

### 3. Polished Output Schema

**Decision**: Sertantai stores polished results as JSONB (similar structure to DRRPEntry). The polisher output maps to this:

```
polished_drrp {
  law_name: string,
  provision: string,
  drrp_type: string,
  holder: string,
  text: string,
  qualifier: option<string>, -- "so far as is reasonably practicable"
  clause_ref: string,
  confidence: float,         -- AI confidence
  polished_at: timestamp,
  model: string,             -- which model did the polishing
}
```

On the fractalaw side this is stored in DuckDB. On the sertantai side it arrives as JSON and goes into JSONB columns.

### 4. Generative AI Backend

**Decision**: Start with Claude API for quality. Transition to local ONNX model as the micro-app matures.

**Rationale**:
- Claude produces high-quality structured extraction from legal text
- The whole point of micro-apps is laser-focused AI inference — once the prompt is proven with Claude, distil into a fine-tuned small model (Phi-3-mini, Llama-3.2-3B) that runs locally via ONNX
- The `fractal:ai/inference` WIT interface is model-agnostic — the host function decides which backend to use
- Local model = no network dependency, fits local-first philosophy, runs on the bedroom box

**Migration path**: Claude (prove the prompt) → distil/fine-tune → ONNX (deploy locally)

### 5. Sync Pattern: Async Outbox with CLI-Driven Pull/Push

**Decision**: Outbox pattern. Each side maintains an outbox. The other side pulls when it's ready.

**Rationale**:
- The hub is intermittently on — can't receive pushes when it's off
- Sertantai (Phoenix) is always on — perfect as the "server" that the hub pulls from
- Fully async and decoupled — no coordination needed
- Hub workflow is a simple sequence: `sync pull` → `run polisher` → `sync push`
- Could be automated with a cron/systemd timer when the box is on

**Flow**:
```
1. Sertantai scrapes → stores annotations (outbox grows)
2. Hub wakes → fractalaw sync pull (pulls since last sync timestamp)
3. Hub runs → fractalaw run drrp-polisher.wasm (processes batch)
4. Hub pushes → fractalaw sync push (sends polished results to sertantai inbox)
5. Hub sleeps
6. Sertantai serves polished results to users
```

**Timestamp tracking**: fractalaw stores `last_sync_at` locally (in DuckDB or a config file). Each pull uses this as the `since` parameter. Each successful pull updates it.

## What Needs to Exist

### On the fractalaw side

| Component | Status | Needed For |
|-----------|--------|------------|
| `fractal:data/query` host function | Not implemented | Loading annotations + legislation text |
| `fractal:data/mutate` host function | Not implemented | Writing polished DRRP entries |
| `fractal:ai/inference` host function | Not implemented | Generative AI extraction (Claude → ONNX) |
| `fractal:audit/log` host function | **Done** (Session 1) | Audit trail |
| DRRP annotation table schema | Not implemented | Storing pulled annotations in DuckDB |
| Polished DRRP output table/columns | Not implemented | Storing polisher results |
| `fractalaw sync pull` CLI command | Not implemented | Pull annotations from sertantai outbox |
| `fractalaw sync push` CLI command | Not implemented | Push polished results to sertantai inbox |
| DRRP Polisher guest component | Not implemented | The micro-app itself |

### On the sertantai side

| Component | Status | Needed For |
|-----------|--------|------------|
| DRRP regex engine | Exists | Producing rough annotations |
| `GET /api/outbox/annotations?since=` | Needs building | Fractalaw pulls new annotations |
| `POST /api/inbox/polished` | Needs building | Fractalaw pushes polished results |
| JSONB storage for polished results | Needs building | Storing refined DRRP entries |

## Implementation Plan

### Phase A: Schema & Storage (fractalaw side)

1. Define `drrp_annotations` table schema in `fractalaw-core`
2. Define `polished_drrp` output table schema in `fractalaw-core`
3. Create/import tables in DuckDB via CLI

### Phase B: Data Host Functions (Phase 3 Session 2)

4. Implement `fractal:data/query` — guest queries DuckDB/DataFusion, gets Arrow IPC bytes
5. Implement `fractal:data/mutate` — guest writes Arrow IPC bytes to DuckDB
6. Uncomment `data-query` and `data-mutate` imports in `wit/world.wit`
7. Test with a simple guest that does query + write

### Phase C: AI Inference Host Function (Phase 3 Session 4)

8. Implement `fractal:ai/inference` — backed by Claude API initially
9. Design the prompt template for DRRP extraction
10. Test with a single known example (HSWA 1974 s.2)

### Phase D: Guest Component

11. Build `guests/drrp-polisher/` guest component
12. Wire together: query annotations → query text → infer → write results → audit
13. End-to-end test with real data

### Phase E: Sync CLI Commands (parallel with sertantai work)

14. `fractalaw sync pull --source <url>` — HTTP GET from sertantai outbox, store in DuckDB
15. `fractalaw sync push --target <url>` — HTTP POST polished results to sertantai inbox
16. Timestamp tracking for incremental sync

### Phase F: Sertantai Endpoints (Elixir side)

17. `GET /api/outbox/annotations?since=<ts>` — return unsynced annotations as JSON
18. `POST /api/inbox/polished` — receive and store polished DRRP entries as JSONB
19. Mark annotations as synced after successful pull

## Dependencies on Other Sessions

| Dependency | Session | Status |
|------------|---------|--------|
| Wasmtime bootstrap | Phase 3 Session 1 | **Done** |
| Data host functions (query/mutate) | Phase 3 Session 2 | Not started |
| AI inference host function | Phase 3 Session 4 | Not started |
| Sync CLI commands | New work | Not started |

## Files to Create/Modify

| File | Action | Description |
|------|--------|-------------|
| `crates/fractalaw-core/src/schema.rs` | Modify | Add DRRP annotation + polished output schemas |
| `crates/fractalaw-host/src/lib.rs` | Modify | Add data/query and data/mutate host functions |
| `crates/fractalaw-host/src/inference.rs` | Create | AI inference host function (Claude backend) |
| `wit/world.wit` | Modify | Uncomment data + AI imports as implemented |
| `guests/drrp-polisher/` | Create | The polisher micro-app guest |
| `crates/fractalaw-cli/src/main.rs` | Modify | Add `sync pull` / `sync push` commands |

## Progress

| Task | Status | Notes |
|------|--------|-------|
| Design sync protocol | [x] | HTTP REST + JSON, async outbox pattern, CLI-driven pull/push |
| Define annotation schema | [x] | Minimum viable: law_name, provision, drrp_type, source_text, confidence, scraped_at |
| Define polished output schema | [x] | Extended DRRPEntry: + qualifier, confidence, polished_at, model |
| Choose AI backend | [x] | Claude API initially → distil to local ONNX |
| Choose sync pattern | [x] | Async outbox: sertantai outbox → hub pulls → polisher runs → hub pushes → sertantai inbox |
| Define drrp_annotations schema | [x] | 8 columns: law_name, provision, drrp_type, source_text, confidence, scraped_at, polished, synced_at |
| Define polished_drrp schema | [x] | 11 columns: law_name, provision, drrp_type, holder, text, qualifier (nullable), clause_ref, confidence, polished_at, model, pushed |
| DuckDB table creation | [x] | `create_drrp_tables()` with CREATE TABLE IF NOT EXISTS, idempotent |
| Implement data/query host function | [x] | Host impl + Arrow IPC encoding done; wired into run_component() with DuckStore |
| Implement data/mutate host function | [x] | Host impl (insert + execute) done; CLI passes DuckStore to guests |
| Implement ai/inference host function | [x] | Claude Messages API backend; feature-gated `inference`; reads ANTHROPIC_API_KEY from env |
| Implement ai/embeddings host function | [x] | Stub returning "not configured" — ONNX embedder wiring deferred |
| Build DRRP Polisher guest | [x] | `guests/drrp-polisher/` — processes annotations via Claude, minimal IPC parser, 15 tests passing |
| Implement sync pull CLI | [x] | `fractalaw sync pull --url <sertantai>` — pulls annotations, stores in DuckDB |
| Implement sync push CLI | [x] | `fractalaw sync push --url <sertantai>` — pushes unpushed polished entries |
| Sertantai outbox endpoint | [ ] | Elixir side |
| Sertantai inbox endpoint | [ ] | Elixir side |
| End-to-end test | [ ] | |
