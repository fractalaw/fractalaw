# Fractal Architecture: Research & Planning Document

## 1. Vision & Core Philosophy

**Goal:** Build a distributed, local-first application for environment, safety, health (ESH) law and regulatory enforcement data — bringing processing and AI to where users have their data, rather than shipping data to the cloud.

**Inspiration:** [Fractal Computing (fractalweb.app)](https://fractalweb.app) — which demonstrates that cloud and hyperscale data centres are not required for fast, scalable software. The Fractal approach reimplements software and AI agents to be significantly smaller, deploys copies across locations where data resides, and coordinates them to act as a single system. Claims of 1,000x to 1,000,000x performance gains over legacy relational implementations through Locality Optimization.

**Architectural Anchors:**
- **Locality of Reference** — data that needs to be accessed together is kept physically local, so the CPU reads from cache rather than main memory or disk
- **Locality of Logic** — application logic is co-located with the storage schema it operates on, requiring only local knowledge (analogous to stored procedures, but generalised)
- **Fractal self-similarity** — the same architectural pattern repeats at every scale: edge node, hub, cluster

---

## 2. Architectural Principles

### 2.1 Local-First
- Data sovereignty: ESH regulatory data stays where the organisation owns it
- No mandatory cloud dependency — the system is functional offline
- Sync and coordination between nodes, not centralised storage
- Aligns with emerging regulatory trends (EU AI Act Aug 2026, data sovereignty mandates globally)

### 2.2 Data-Oriented Design
- Structure data for how it will be accessed, not how it is conceptually modelled
- Columnar/vectorised storage for analytical workloads
- Batch processing in cache-friendly contiguous memory layouts
- Minimise pointer chasing, maximise SIMD utilisation

### 2.3 Fractal Repetition
- Each node (edge device, hub, peer) runs the same architectural unit
- A node contains: storage engine + query engine + AI inference + micro-app runtime
- Nodes compose hierarchically: edge nodes sync to hub, hubs can federate
- The API surface is identical at every level — a single node and a cluster of 100 nodes expose the same interface

---

## 3. Technology Stack Analysis

### 3.1 Systems Language: Rust vs Zig

| Dimension | Rust | Zig |
|---|---|---|
| **Memory safety** | Compile-time guarantees (borrow checker). ~1 bug per 140K LOC in studies | Runtime optional checks. Higher bug rate observed in projects |
| **Ecosystem maturity** | Large crate ecosystem, strong WASM/WASI support | Younger ecosystem, excellent C interop |
| **WASM target** | First-class `wasm32-wasi` target, Wasmtime written in Rust | Compiles to WASM, but tooling less mature |
| **Data-oriented patterns** | Supports via ECS crates, manual struct-of-arrays | More natural — no borrow checker friction for flat data layouts |
| **Raw throughput** | Near-C performance | Benchmarks show 1.5-1.8x faster in some unsafe-equivalent scenarios |
| **Concurrency** | `async`/`await` with Tokio, `rayon` for data parallelism | Simpler model, manual but explicit |
| **LanceDB/DuckDB/Arrow** | Native SDKs and bindings available | Would require C FFI bindings |
| **Build times** | Slower (LLVM-based) | Self-hosted compiler in progress (targeting faster builds) |

**Recommendation:** Rust is the stronger choice for this project given:
- Wasmtime, LanceDB, DuckDB, Arrow, and DataFusion are all Rust-native or have first-class Rust bindings
- The entire WASI component model ecosystem is Rust-first
- Memory safety guarantees matter for a regulatory/compliance domain
- Zig remains viable for performance-critical leaf components compiled to WASM modules

### 3.2 Storage: DuckDB + LanceDB + Apache Arrow

**Apache Arrow** serves as the universal in-memory columnar format across the stack:
- Language-independent columnar memory layout optimised for SIMD
- Zero-copy data sharing between components (no serialisation overhead)
- Enables vectorised execution on modern CPUs
- DataFusion (Rust-native query engine) operates directly on Arrow buffers

**DuckDB** for structured analytical queries:
- In-process OLAP engine (like SQLite for analytics)
- Vectorised execution engine with morsel-driven parallelism
- Zero-copy integration with Apache Arrow — DuckDB can query Arrow buffers directly
- Pushes filters and projections into Arrow scans
- Handles larger-than-memory datasets via streaming
- Ideal for: regulatory lookups, compliance reporting, cross-referencing ESH data

**LanceDB** for vector/multimodal storage:
- Embedded vector database written in Rust, built on Lance columnar format
- Runs in-process like SQLite — no server required
- Native vector similarity search, full-text search, and SQL
- Stores vectors, metadata, and multimodal data (text, documents, images)
- Zero-copy, automatic versioning
- Ideal for: AI embeddings, semantic search over regulatory documents, RAG retrieval

**Combined architecture:**
```
[ Raw ESH Data ] --> [ Arrow IPC / Parquet files ]
                          |
               +----------+----------+
               |                     |
         [ DuckDB ]           [ LanceDB ]
         Analytical           Vector/Semantic
         queries              search & RAG
               |                     |
               +----------+----------+
                          |
                 [ DataFusion / SQL ]
                 Unified query layer
```

### 3.3 DataFusion: Unified Query Layer

DataFusion is the connective tissue of the storage stack — an Arrow-native, embeddable query engine written in Rust that sits above DuckDB and LanceDB and provides a single SQL/DataFrame interface across all data.

**Why DataFusion (not just DuckDB's SQL):**
- **Arrow-native from the ground up** — operates directly on Arrow record batches with zero serialisation. DuckDB has Arrow integration but uses its own internal vector format; DataFusion's internal representation *is* Arrow.
- **Extensible architecture** — 10+ major extension APIs. Custom `TableProvider` implementations let us register DuckDB tables, LanceDB vector indexes, Parquet files, and live sensor streams as first-class SQL tables in a single catalog.
- **Rust-native** — compiles into the Fractal Core binary. No FFI boundary for the query layer (DuckDB requires C FFI from Rust).
- **Top-level Apache project** — fastest engine for Parquet queries in ClickBench benchmarks (ahead of DuckDB, chDB, ClickHouse on the same hardware). SIGMOD-accepted research paper.
- **Streaming & multi-threaded** — columnar vectorised execution with partition-aware parallelism, matching the data-oriented design principle.

**Extension points relevant to this architecture:**

| Extension API | Use in Fractal Architecture |
|---|---|
| `TableProvider` | Register DuckDB analytical tables and LanceDB vector tables as unified SQL sources |
| `CatalogProvider` | Implement a Fractal Catalog that maps ESH data schemes to their locality-optimised partitions |
| Custom functions (UDFs) | Regulatory-specific functions: `compliance_status()`, `regulation_applies()`, `risk_score()` |
| Custom `ExecutionPlan` | Inject LanceDB vector similarity search as a physical plan node within SQL queries |
| `OptimizerRule` | Push locality-aware predicates down — e.g., partition pruning by site_id before scanning |
| `FileFormat` | Register Lance columnar format alongside Parquet/CSV/JSON |

**How it unifies the stack:**

```
                        ┌─────────────────────────┐
                        │    Application Layer     │
                        │  (SQL / DataFrame API)   │
                        └────────────┬────────────┘
                                     │
                        ┌────────────┴────────────┐
                        │     DataFusion Engine    │
                        │  ┌───────────────────┐   │
                        │  │  Fractal Catalog   │   │
                        │  │  (CatalogProvider) │   │
                        │  └─────────┬─────────┘   │
                        │     ┌──────┼──────┐      │
                        │     │      │      │      │
                        │  ┌──┴──┐┌──┴──┐┌──┴──┐   │
                        │  │DuckDB││Lance││Parq.│   │
                        │  │Table ││Table││Files│   │
                        │  │Prov. ││Prov.││Prov.│   │
                        │  └──┬──┘└──┬──┘└──┬──┘   │
                        └─────┼──────┼──────┼──────┘
                              │      │      │
                        ┌─────┴──────┴──────┴──────┐
                        │   Apache Arrow Buffers    │
                        │   (zero-copy shared mem)  │
                        └──────────────────────────┘
```

**Concrete query example — cross-engine federated query:**

```sql
-- Single SQL query spanning DuckDB (analytical) and LanceDB (vector)
SELECT
    s.site_name,
    s.compliance_score,
    r.regulation_text,
    r.relevance_score
FROM duckdb.site_compliance s
JOIN lance.regulation_embeddings r
    ON r.vector_search(
        query_embedding(s.industry_sector),
        top_k := 5
    )
WHERE s.region = 'EU'
  AND s.last_audit_date < '2025-06-01'
ORDER BY r.relevance_score DESC;
```

This query is impossible in DuckDB or LanceDB alone — DataFusion's custom `TableProvider` and `ExecutionPlan` extensions make it a single optimised plan that pushes the `region` filter to DuckDB and the vector search to LanceDB, with Arrow buffers flowing between them without copies.

**LanceDB's native DataFusion integration:**
LanceDB already uses DataFusion internally to support SQL queries. The Lance format supports predicate and projection pushdown from DataFusion, reducing scanned data. This means the `TableProvider` for Lance is largely built — the work is registering it alongside DuckDB in a unified catalog.

**DataFusion vs DuckDB — complementary, not competing:**

| Concern | DataFusion | DuckDB |
|---|---|---|
| Role | Query federation & orchestration | Analytical storage engine |
| Internal format | Arrow (native) | Custom vectors (Arrow at boundary) |
| Language | Rust (no FFI) | C++ (FFI from Rust) |
| Extensibility | 10+ API extension points | Extension system (less granular) |
| Strength | Unifying heterogeneous sources | Complex OLAP on structured data |
| In this arch | Query planner, catalog, federation | Executes analytical scans on ESH tables |

Both engines remain in the stack. DuckDB handles heavy analytical workloads on structured regulatory data (its morsel-driven parallelism excels here). DataFusion orchestrates queries that span DuckDB, LanceDB, Parquet files, and custom sources — providing micro-apps with a single SQL endpoint regardless of where data physically lives.

### 3.4 AI Inference: Quantised Local Models

Two runtime options evaluated:

**MLC LLM:**
- Universal deployment engine with ML compilation
- OpenAI-compatible API (REST, Python, JS, iOS, Android)
- Compiles models for target hardware at deployment time
- Optimised for LLM workloads specifically
- Good for: conversational AI agents, document Q&A, regulatory interpretation

**ONNX Runtime:**
- Industry standard for edge AI inference
- Supports INT8/INT4 quantisation (75-90% memory reduction)
- Hardware acceleration including NPUs on supported devices
- Cross-platform (cloud, edge, web, mobile, IoT)
- Good for: classification, NER, structured extraction, embeddings generation

**Recommended approach — use both:**
- ONNX Runtime for embedding generation, classification, and structured extraction tasks (smaller, faster models)
- MLC LLM (or llama.cpp via bindings) for generative tasks requiring reasoning about regulatory text
- Target model size: 1B-8B parameters (the current edge sweet spot)
- Quantisation: 4-bit (Q4_K_M) for generative models, INT8 for ONNX classification models
- Speculative decoding for 2-3x inference speedup on generative tasks

### 3.4 MicroApp Runtime Architecture

The micro-app runtime is the application layer of the Fractal architecture — where ESH domain logic lives. Rather than a monolithic application, functionality is decomposed into independently deployable WebAssembly components that the host runtime orchestrates. This is Fractal self-similarity applied to the application layer: each micro-app is a self-contained unit with the same structural pattern (inputs → logic → outputs over Arrow buffers), composable at any scale.

#### 3.4.1 Why WebAssembly

| Property | Benefit for Fractal Architecture |
|---|---|
| **Sandboxed execution** | Each micro-app is memory-isolated — a bug or compromise in one cannot affect others or the host |
| **Language-agnostic** | Apps can be written in Rust, Zig, C, C++, Go, JS, C#, Python (via componentize-py) |
| **Near-native performance** | 88% of native C++ throughput (2026 Wasmtime JIT benchmarks). Cranelift compiles 10x faster than LLVM. |
| **Sub-millisecond instantiation** | Suitable for per-request isolation. Pooled instantiation reduces this to microseconds. |
| **Portable** | Same `.wasm` binary runs on x86, ARM, RISC-V — build once, deploy to any edge node |
| **Deterministic execution** | Fuel metering enables reproducible runs — critical for regulatory audit reproducibility |
| **Capability-based security** | Zero ambient authority; micro-apps can only access what the host explicitly grants (see 6.2.3) |

#### 3.4.2 WASI Component Model

The [WebAssembly Component Model](https://component-model.bytecodealliance.org/) is the foundation — it defines how modules declare imports/exports, compose together, and interact with the host via typed interfaces.

**Current status:**

| Layer | Status | Key Capabilities |
|---|---|---|
| WASI 0.2 (Preview 2) | **Stable** | Filesystem, networking, clocks, random, environment, CLI |
| Component Model MVP | **Stable in Wasmtime** | WIT interfaces, typed imports/exports, resource handles |
| WASI 0.3 | **Preview (Feb 2026)** | Native async in Component Model — async functions, streams, futures |
| OCI distribution | **Production** | Publish/fetch components as OCI artifacts to any container registry |

**WIT (WebAssembly Interface Types)** is the interface definition language. It defines the contract between host and guest — what functions, types, and resources are available. The `wit-bindgen` tool generates Rust bindings on both sides:

- **Guest side** (micro-app): `wit-bindgen generate!` produces a `Guest` trait the app implements
- **Host side** (Fractal Core): `wasmtime::component::bindgen!` produces typed Rust APIs the host calls

#### 3.4.3 Host Runtime Architecture

The Fractal Core host runtime is a Rust binary that embeds Wasmtime and manages the full lifecycle of micro-apps:

```
┌──────────────────────────────────────────────────────────────────┐
│                     Fractal Core (Rust)                          │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │                    App Supervisor                          │  │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐  │  │
│  │  │ Registry │  │ Scheduler│  │ Lifecycle│  │  Router  │  │  │
│  │  │ (OCI)    │  │          │  │ Manager  │  │          │  │  │
│  │  └──────────┘  └──────────┘  └──────────┘  └──────────┘  │  │
│  └────────────────────────┬───────────────────────────────────┘  │
│                           │                                      │
│  ┌────────────────────────┴───────────────────────────────────┐  │
│  │                  Wasmtime Engine                            │  │
│  │  ┌───────────────────────────────────────────────────────┐ │  │
│  │  │              Instance Pool (pre-allocated)            │ │  │
│  │  │  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌────────┐  │ │  │
│  │  │  │ Import  │  │ Comply  │  │ Report  │  │ Audit  │  │ │  │
│  │  │  │ .wasm   │  │ .wasm   │  │ .wasm   │  │ .wasm  │  │ │  │
│  │  │  │ (slot 0)│  │ (slot 1)│  │ (slot 2)│  │(slot 3)│  │ │  │
│  │  │  └────┬────┘  └────┬────┘  └────┬────┘  └───┬────┘  │ │  │
│  │  └───────┼────────────┼────────────┼────────────┼────────┘ │  │
│  └──────────┼────────────┼────────────┼────────────┼──────────┘  │
│             │            │            │            │              │
│  ┌──────────┴────────────┴────────────┴────────────┴──────────┐  │
│  │                  Host Interface Layer                       │  │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌───────────┐  │  │
│  │  │ fractal: │  │ fractal: │  │ fractal: │  │ fractal:  │  │  │
│  │  │ data/    │  │ ai/      │  │ events/  │  │ audit/    │  │  │
│  │  │ query    │  │ infer    │  │ emit     │  │ log       │  │  │
│  │  └──────────┘  └──────────┘  └──────────┘  └───────────┘  │  │
│  └────────────────────────────────────────────────────────────┘  │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │              Core Services (native Rust)                   │  │
│  │  DataFusion │ DuckDB │ LanceDB │ ONNX │ MLC LLM │ Flight │  │
│  └────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────┘
```

**Key components:**

- **App Supervisor** — top-level orchestrator managing the micro-app fleet
  - **Registry** — fetches and caches WASM components from OCI-compatible registries (local or remote)
  - **Scheduler** — decides which micro-apps run, when, and with what resource budget
  - **Lifecycle Manager** — handles load, start, stop, hot-swap, and unload of components
  - **Router** — maps incoming requests (HTTP, event triggers, scheduled tasks) to the correct micro-app

- **Wasmtime Engine** — single shared `Engine` instance compiled with Cranelift, configured once at startup
  - Engine configuration: AOT compilation, pooling allocator, fuel metering, epoch interruption

- **Instance Pool** — pre-allocated slots for concurrent micro-app instances (see 3.4.5)

- **Host Interface Layer** — WIT-defined host functions that micro-apps import (see 3.4.4)

- **Core Services** — the native Rust subsystems (DataFusion, DuckDB, LanceDB, AI runtimes, Flight) that the host interface layer bridges into

#### 3.4.4 WIT Interface Definitions

The host exposes a set of WIT packages that micro-apps import. These form the stable API contract — micro-apps written against these interfaces work on any Fractal node regardless of version, as long as the WIT version is supported.

**Core WIT packages:**

```wit
// ─── fractal:data — query and mutate ESH data ───
package fractal:data@0.1.0;

interface query {
    /// Execute a SQL query, returns Arrow IPC-serialised RecordBatch
    query: func(sql: string) -> result<list<u8>, query-error>;

    /// Execute a prepared statement with parameters
    query-params: func(sql: string, params: list<param-value>) -> result<list<u8>, query-error>;

    /// Stream large result sets in chunks
    query-stream: func(sql: string, batch-size: u32) -> result<stream-handle, query-error>;
    stream-next: func(handle: stream-handle) -> result<option<list<u8>>, query-error>;
    stream-close: func(handle: stream-handle);

    variant param-value {
        text(string),
        integer(s64),
        float(f64),
        boolean(bool),
        null,
    }

    record query-error {
        code: u32,
        message: string,
    }

    type stream-handle = u64;
}

interface mutate {
    /// Insert Arrow IPC-serialised RecordBatch into a table
    insert: func(table: string, data: list<u8>) -> result<u64, mutate-error>;

    /// Execute a DML statement (UPDATE/DELETE) — returns rows affected
    execute: func(sql: string) -> result<u64, mutate-error>;

    record mutate-error {
        code: u32,
        message: string,
    }
}
```

```wit
// ─── fractal:ai — local AI inference ───
package fractal:ai@0.1.0;

interface embeddings {
    /// Generate embedding vector for text
    embed: func(text: string) -> result<list<f32>, ai-error>;

    /// Batch embed multiple texts
    embed-batch: func(texts: list<string>) -> result<list<list<f32>>, ai-error>;

    record ai-error {
        code: u32,
        message: string,
    }
}

interface inference {
    /// Run generative inference with a prompt and system context
    generate: func(request: generate-request) -> result<generate-response, ai-error>;

    record generate-request {
        system-prompt: option<string>,
        user-prompt: string,
        max-tokens: u32,
        temperature: f32,
    }

    record generate-response {
        text: string,
        tokens-used: u32,
        confidence: f32,
    }
}

interface classify {
    /// Classify text into categories (ONNX-backed)
    classify: func(text: string, categories: list<string>) -> result<list<classification>, ai-error>;

    record classification {
        category: string,
        score: f32,
    }
}
```

```wit
// ─── fractal:events — event emission and scheduling ───
package fractal:events@0.1.0;

interface emit {
    /// Emit a domain event (processed by other micro-apps or the sync engine)
    emit: func(event: domain-event) -> result<event-id, event-error>;

    record domain-event {
        event-type: string,
        payload: list<u8>,     // Arrow IPC or JSON
        source-app: string,
    }

    type event-id = u64;
    record event-error { code: u32, message: string }
}

interface schedule {
    /// Schedule a one-shot or recurring task
    schedule: func(task: scheduled-task) -> result<task-id, event-error>;
    cancel: func(id: task-id) -> result<_, event-error>;

    record scheduled-task {
        name: string,
        cron: option<string>,       // cron expression for recurring
        run-at: option<u64>,        // unix timestamp for one-shot
        target-app: string,
        payload: list<u8>,
    }

    type task-id = u64;
}
```

```wit
// ─── fractal:audit — immutable audit logging ───
package fractal:audit@0.1.0;

interface log {
    /// Record an audit event (append-only, cannot be deleted by micro-apps)
    record-event: func(entry: audit-entry);

    record audit-entry {
        event-type: string,
        resource: string,
        detail: string,
    }
    // node_id, timestamp, actor, and hash-chain are added by the host automatically
}
```

**Data exchange format:** all `list<u8>` payloads are Arrow IPC-serialised RecordBatches. This avoids the overhead of WIT's canonical ABI for large data transfers — the guest serialises an Arrow batch into bytes, passes the byte buffer through the WIT boundary, and the host deserialises it zero-copy via Arrow IPC. This is the pragmatic approach given that the Component Model does not yet support shared memory or `borrow<list<T>>` for zero-copy (an [open proposal](https://github.com/WebAssembly/component-model/issues/398)).

#### 3.4.5 Instance Pooling and Resource Management

Micro-app performance depends on fast instantiation and predictable resource consumption. Wasmtime's [pooling allocator](https://docs.wasmtime.dev/api/wasmtime/struct.PoolingAllocationConfig.html) is the key mechanism.

**Pooling allocator configuration:**

```rust
use wasmtime::{Config, Engine, PoolingAllocationConfig};

let mut pool = PoolingAllocationConfig::default();
pool.total_component_instances(64);       // max concurrent instances across all apps
pool.total_memories(128);                 // pre-allocated memory slots
pool.max_memory_size(64 * 1024 * 1024);  // 64 MB per instance max
pool.total_tables(128);
pool.table_elements(10_000);

let mut config = Config::new();
config.allocation_strategy(wasmtime::InstanceAllocationStrategy::Pooling(pool));
config.cranelift_opt_level(wasmtime::OptLevel::Speed);
config.async_support(true);               // for WASI 0.3 async
config.consume_fuel(true);                // enable fuel metering

let engine = Engine::new(&config)?;
```

**What this achieves:**
- Instance creation becomes a single `madvise` (reset linear memory) + optional `mprotect` — microsecond-level instantiation
- No `mmap`/`munmap` per request — memory is pre-allocated at startup
- Bounded resource consumption — the pool enforces hard limits on total memory, tables, and instances
- Suitable for edge devices with limited RAM — configure smaller pools on constrained hardware

**Instance lifecycle:**

```
┌──────────┐     ┌────────────┐     ┌───────────┐     ┌──────────┐
│  Idle    │────>│ Instantiate│────>│ Running   │────>│ Complete │
│  (pool)  │     │ (µs)       │     │ (fuel     │     │ (return  │
│          │     │            │     │  metered) │     │  to pool)│
└──────────┘     └────────────┘     └─────┬─────┘     └──────────┘
                                          │
                                    ┌─────┴─────┐
                                    │ Trapped   │
                                    │ (fuel     │
                                    │  exhausted│
                                    │  / error) │
                                    └───────────┘
```

**Fuel metering and epoch interruption:**

Two complementary mechanisms prevent micro-apps from consuming unbounded resources:

| Mechanism | How It Works | Use Case |
|---|---|---|
| **Fuel** | Each WASM instruction consumes fuel units. When fuel runs out, execution traps. Deterministic — same input always traps at the same point. | Batch jobs, audit-reproducible runs. Default: 1B fuel units per invocation. |
| **Epoch** | Wall-clock timer. Host increments an epoch counter; instances check it periodically (~10% overhead). Non-deterministic but cheaper. | Long-running interactive requests. Default: 30s timeout. |

Both can be configured per micro-app in the app manifest.

**Memory limits per micro-app:**

| App Tier | Max Memory | Max Fuel | Timeout | Use Case |
|---|---|---|---|---|
| `lightweight` | 16 MB | 100M | 5s | Classifiers, validators, transformers |
| `standard` | 64 MB | 1B | 30s | Report generators, compliance checkers |
| `heavy` | 256 MB | 10B | 120s | Large document importers, batch processors |

The [ResourceLimiter](https://docs.rs/wasmtime/latest/wasmtime/trait.ResourceLimiter.html) trait enforces these limits — the host implements it to reject memory growth requests that would exceed the tier's cap.

#### 3.4.6 Ahead-of-Time Compilation and Caching

Micro-apps can be compiled once and reused across invocations without re-compiling:

**AOT compilation pipeline:**

```
┌───────────┐     ┌───────────┐     ┌───────────────┐     ┌──────────────┐
│ .wasm     │────>│ Cranelift  │────>│ .cwasm        │────>│ Instantiate  │
│ component │     │ AOT compile│     │ (precompiled) │     │ (µs, no JIT) │
│ (portable)│     │ (per-arch) │     │ (arch-specific│     │              │
└───────────┘     └───────────┘     └───────────────┘     └──────────────┘
```

- `Module::serialize()` / `Component::serialize()` produces a precompiled `.cwasm` file
- `Module::deserialize()` loads it without invoking Cranelift — instantiation drops to microseconds
- Precompiled modules are cached on disk per architecture (x86 hub gets x86 `.cwasm`, ARM edge gets ARM `.cwasm`)
- The AOT cache is keyed by `(component_hash, engine_config_hash, target_arch)` — engine configuration changes invalidate the cache
- On the hub: compile all registered micro-apps at startup; on edge: either compile locally or receive pre-compiled `.cwasm` from hub during sync

#### 3.4.7 Hot-Swap and Live Update

Micro-apps can be updated without restarting the Fractal Core host or interrupting other running apps:

**Hot-swap sequence:**

```
1. New .wasm version arrives (pushed from registry or synced from hub)
2. Lifecycle Manager verifies Ed25519 signature (see 6.2.3)
3. New component is AOT-compiled (or pre-compiled .cwasm loaded)
4. Lifecycle Manager waits for in-flight invocations on old version to complete
   (drain timeout: configurable, default 10s)
5. Router atomically switches to new version (new invocations → new component)
6. Old component's pool slots are reclaimed
7. Audit log records: old_hash, new_hash, swap_time, who_triggered
```

**Guarantees:**
- No dropped requests — in-flight invocations complete on the old version
- No dual-version execution — the atomic switch ensures one version is active at a time
- Rollback — if the new version fails its health check (a designated `health` export), the swap is reverted and an alert is raised
- Version pinning — edge nodes can pin a specific version to avoid unexpected updates in the field

#### 3.4.8 Micro-App Composition

The Component Model allows micro-apps to be composed — one app's export wired to another's import — without going through the host:

```
┌────────────────────────────────────────────┐
│          Composed Component                │
│                                            │
│  ┌─────────────┐     ┌─────────────────┐  │
│  │ regulation- │     │ compliance-     │  │
│  │ parser      │────>│ checker         │  │
│  │             │     │                 │  │
│  │ export:     │     │ import:         │  │
│  │  parsed-reg │     │  parsed-reg     │  │
│  └─────────────┘     │                 │  │
│                      │ export:         │  │
│                      │  compliance-rpt │  │
│                      └─────────────────┘  │
└────────────────────────────────────────────┘
```

- Composition is done at build/deploy time using `wasm-tools compose` — no runtime overhead
- The composed component is a single `.wasm` that Wasmtime loads as one unit
- Internal calls between composed sub-components are direct function calls (no host round-trip)
- Enables a "pipeline" pattern: parse → validate → check → report as a single composed component

**Composition strategies:**

| Strategy | When to Use | Example |
|---|---|---|
| **Static composition** | Stable pipelines that rarely change | regulation-parser + compliance-checker |
| **Host-mediated** | Dynamic routing, conditional logic | Router invokes apps sequentially based on event type |
| **Event-driven** | Loose coupling, fan-out | Importer emits `data-ingested` event; multiple apps react independently |

#### 3.4.9 Distribution: OCI Registry

Micro-apps are packaged and distributed as [OCI artifacts](https://opensource.microsoft.com/blog/2024/09/25/distributing-webassembly-components-using-oci-registries), the same standard used for container images:

```
┌────────────────┐     ┌──────────────────────┐     ┌────────────┐
│ Developer      │     │ OCI Registry         │     │ Fractal    │
│                │     │ (local or remote)    │     │ Node       │
│ cargo build    │     │                      │     │            │
│ --target       │     │ fractal-apps/        │     │ wkg fetch  │
│ wasm32-wasip2  │────>│   compliance:v1.2.0  │────>│ → verify   │
│                │     │   importer:v2.0.1    │     │ → AOT      │
│ wkg publish    │     │   reporter:v1.0.0    │     │ → load     │
└────────────────┘     └──────────────────────┘     └────────────┘
```

- The [`wasm-pkg-tools`](https://github.com/bytecodealliance/wasm-pkg-tools) (`wkg` CLI) handles publish and fetch
- Any OCI-compatible registry works — Docker Hub, GitHub Container Registry, or a local registry on the hub node
- Components are tagged with semantic versions and signed with Ed25519
- The hub can act as a local OCI registry for edge nodes — micro-apps sync alongside data during the normal sync cycle
- WIT packages are also distributable as OCI artifacts — ensuring interface definitions are versioned and discoverable

#### 3.4.10 ESH Domain Micro-Apps (Initial Set)

| Micro-App | Tier | Function | Key Imports |
|---|---|---|---|
| `regulation-importer` | `heavy` | Ingest legislation from structured sources (XML, HTML, PDF), extract obligations, generate embeddings | `fractal:data/mutate`, `fractal:ai/embeddings` |
| `regulation-parser` | `standard` | Parse raw regulation text into structured obligation records | `fractal:data/query` |
| `compliance-checker` | `standard` | Compare site records against applicable regulations, produce gap analysis | `fractal:data/query`, `fractal:ai/classify` |
| `incident-classifier` | `lightweight` | Auto-categorise incident reports by regulation and severity | `fractal:ai/classify` |
| `report-generator` | `standard` | Produce compliance reports (PDF/HTML) from analytical queries and AI summaries | `fractal:data/query`, `fractal:ai/inference` |
| `audit-trail-viewer` | `lightweight` | Query and format the immutable audit log for review | `fractal:data/query`, `fractal:audit/log` |
| `regulatory-monitor` | `standard` | Detect changes in legislation feeds, identify affected sites, emit alerts | `fractal:data/query`, `fractal:events/emit`, `fractal:ai/embeddings` |
| `risk-assessor` | `standard` | Score sites by risk level using weighted compliance gaps and incident history | `fractal:data/query`, `fractal:ai/inference` |

Each app is independently versioned, tested, and deployable. An edge node at a specific site might only run `compliance-checker`, `incident-classifier`, and `audit-trail-viewer` — the Lifecycle Manager loads only the micro-apps configured for that node's role.

---

## 4. Hub Hardware: Non-Apple Alternatives

The hub needs to be a performant device running Linux natively. Evaluation of alternatives to Apple Mac Studio:

### 4.1 AMD Threadripper Workstations

**System76 Thelio Mega / Massive:**
- Up to AMD Threadripper PRO 7995WX (96 cores / 192 threads)
- Native Linux (Pop!_OS), open-source GPU drivers in-kernel
- Custom thermal design for sustained workloads
- Linus Torvalds switched to Threadripper for kernel builds (3x faster)

**Trade-off vs Mac Studio:**
- Mac Studio: 6W idle, exceptional perf/watt (Apple Silicon unified memory)
- Threadripper: 39W idle, ~4x power consumption at full load
- Threadripper wins on: raw core count, memory capacity (up to 512GB+ ECC), PCIe lanes, GPU choice, Linux-native
- Mac Studio wins on: power efficiency, compact form factor, unified memory bandwidth

### 4.2 Compact High-Performance Options

| Option | Cores | RAM | GPU | Form Factor | Notes |
|---|---|---|---|---|---|
| System76 Thelio | Up to 96c | 512GB ECC | Discrete AMD/NVIDIA | Tower | Linux-native, open firmware |
| AMD Ryzen 9 9950X build | 16c/32t | 192GB DDR5 | Discrete | Mini-ITX possible | Good perf/watt balance |
| Framework Desktop (upcoming) | TBD | TBD | TBD | Compact | Modular, repairable, Linux-first |
| Intel NUC successors (ASUS) | 14-24c | 96GB | Integrated/eGPU | Ultra-compact | Low power edge nodes |

### 4.3 Recommendation

**Hub:** AMD Ryzen 9 9950X or Threadripper PRO in a System76 Thelio — balances core count, memory, Linux compatibility, and the open-source GPU driver stack. The Threadripper PRO specifically offers ECC memory and massive I/O bandwidth important for concurrent DuckDB/LanceDB workloads.

**Edge nodes:** Mini PCs (AMD Ryzen 7/9, 32-64GB RAM) or even Raspberry Pi 5 (8GB) for lightweight inference with ONNX Runtime quantised models. Intel NUC-class devices with NPUs for dedicated inference tasks.

---

## 5. ESH Domain: Data Architecture

### 5.1 Data Categories
- **Legislation & regulations** — statutes, statutory instruments, directives, standards (ISO 14001, ISO 45001)
- **Compliance records** — inspections, audits, certifications, permits
- **Incident data** — accidents, near-misses, environmental releases, enforcement actions
- **Monitoring data** — emissions, water quality, noise, air quality sensor readings
- **Risk assessments** — workplace hazards, environmental impact assessments
- **Enforcement actions** — notices, prosecutions, penalties, remediation orders

### 5.2 Locality Mapping

Applying Fractal Locality of Reference to ESH data:

```
Scheme: "Site Compliance"
├── site_id, site_metadata
├── active_permits[]
├── recent_inspections[] (last 12 months)
├── open_enforcement_actions[]
├── current_risk_assessments[]
└── monitoring_data_summary (aggregated)

All co-located in a single Arrow/Lance partition.
CPU cache-friendly: one read loads everything
needed for a compliance status check.
```

The key insight from Fractal Computing: at data preparation time ("compile time"), data is structured for locality at run-time. Rather than normalised relational tables requiring joins across ESH categories, data is denormalised and co-located by access pattern.

### 5.3 Regulatory AI Use Cases
- **Document ingestion** — parse legislation, extract obligations, embed for semantic search (ONNX + LanceDB)
- **Compliance gap analysis** — compare site records against regulatory requirements (DuckDB analytical queries)
- **Regulatory change monitoring** — detect changes in legislation, identify affected sites (MLC LLM reasoning)
- **Incident classification** — auto-categorise incidents by regulatory framework (ONNX classifier)
- **Report generation** — produce compliance reports from local data (WASM micro-app + LLM)

---

## 6. System Architecture Overview

```
                    ┌─────────────────────────┐
                    │       Hub Node          │
                    │  (Threadripper/Ryzen)   │
                    │                         │
                    │  ┌─────────────────┐    │
                    │  │  Fractal Core   │    │
                    │  │  (Rust binary)  │    │
                    │  ├─────────────────┤    │
                    │  │  Wasmtime       │    │
                    │  │  MicroApp Host  │    │
                    │  ├─────────────────┤    │
                    │  │  DuckDB Engine  │    │
                    │  │  LanceDB Engine │    │
                    │  │  Arrow Buffers  │    │
                    │  ├─────────────────┤    │
                    │  │  MLC LLM (8B)  │    │
                    │  │  ONNX Runtime   │    │
                    │  └─────────────────┘    │
                    └───────────┬─────────────┘
                                │
                    ┌───────────┼───────────┐
                    │           │           │
              ┌─────┴─────┐ ┌──┴──────┐ ┌──┴──────┐
              │ Edge Node │ │Edge Node│ │Edge Node│
              │ (Mini PC) │ │(Mini PC)│ │(RPi 5)  │
              │           │ │         │ │         │
              │ Same arch │ │ Same    │ │ Same    │
              │ smaller   │ │ arch    │ │ arch    │
              │ models    │ │         │ │ lighter │
              └───────────┘ └─────────┘ └─────────┘
```

Each node — whether hub or edge — runs the identical Fractal Core binary (compiled per-architecture). The difference is only in:
- Model size (8B on hub, 1-3B on edge, ONNX-only on Pi)
- Data volume (full dataset on hub, relevant partition on edge)
- MicroApp set (all apps on hub, domain-specific subset on edge)

### 6.1 Sync Protocol: Hub-Edge Data Synchronisation

The sync protocol is the most architecturally critical component — it determines how the system remains a "single system" despite running as distributed copies. The design must satisfy three constraints simultaneously: **regulatory auditability** (every change traceable), **offline-first operation** (edge nodes work without connectivity), and **Locality of Reference** (each node holds exactly the data it needs).

#### 6.1.1 Transport Layer: Arrow Flight RPC

[Arrow Flight](https://arrow.apache.org/docs/format/Flight.html) is the wire protocol for all node-to-node data transfer:

- Built on gRPC (HTTP/2) with Arrow columnar format as the native payload — no serialisation/deserialisation overhead
- Supports parallel streaming across multiple channels — benchmarks show 60-90% reduction in transfer overhead vs JSON/row-based protocols
- Key operations used:
  - **DoGet** — edge pulls changed partitions from hub
  - **DoPut** — edge pushes locally-generated data (inspections, incidents) to hub
  - **DoExchange** — bidirectional streaming for real-time sync sessions
  - **GetFlightInfo** — metadata queries ("what partitions have changed since version N?")
- DataFusion has native Flight SQL integration — a query on the hub can transparently pull data from edge nodes via the `datafusion-federation` crate, and vice versa

```
Edge Node                          Hub Node
    │                                  │
    │── GetFlightInfo(since=v42) ─────>│
    │<── FlightInfo(changed=[p3,p7]) ──│
    │                                  │
    │── DoGet(partition=p3) ──────────>│
    │<── Arrow RecordBatch stream ─────│
    │                                  │
    │── DoPut(local_inspections) ─────>│
    │<── Ack(new_version=v43) ────────-│
    │                                  │
```

#### 6.1.2 Delta Sync via Lance Versioning

Lance's append-only transaction log provides the foundation for efficient delta sync:

- **Every write creates a new version** — the manifest tracks version history via metadata, previous versions remain available for rollback
- **MVCC (Multi-Version Concurrency Control)** — writes don't modify existing data; partial writes are invisible to readers until committed
- **Delta identification** — comparing manifest versions between hub and edge yields the exact set of changed data fragments (no full-table diff required)
- **Zero-copy versioning** — creating a new version doesn't duplicate unchanged data, only new/modified fragments are written

**Sync flow:**

```
1. Edge node stores its last-synced hub version: v_edge = 42
2. Edge requests: "what changed since v42?"
3. Hub compares manifest v42 → v_current (v47)
   → Returns: [fragment_ids added/modified in v43..v47]
4. Hub streams only those fragments as Arrow RecordBatches via Flight DoGet
5. Edge applies fragments to local Lance store → local version advances
6. Edge records: v_edge = 47
```

For **edge-to-hub** sync (e.g., field inspector uploads new audit data):

```
1. Edge has local writes at versions L1, L2, L3 (not yet synced)
2. Edge streams new fragments to hub via Flight DoPut
3. Hub applies fragments to its Lance store as a new version
4. Hub responds with the new version number
5. Edge marks local writes as synced
```

#### 6.1.3 Conflict Resolution Strategy

ESH regulatory data has specific conflict characteristics that inform the resolution strategy:

**Data classification by mutability:**

| Data Type | Mutability | Conflict Strategy |
|---|---|---|
| Legislation/regulations | Append-only (new versions published, old versions never modified) | No conflicts — append only |
| Compliance records | Write-once (an inspection happened or it didn't) | No conflicts — immutable events |
| Monitoring data | Write-once time-series (sensor readings) | No conflicts — append by timestamp |
| Risk assessments | Mutable (updated periodically) | LWW with audit trail |
| Enforcement actions | Mutable (status changes over time) | State-machine CRDT |
| Site metadata | Mutable (address, contacts change) | LWW with audit trail |

**Key insight:** the majority of ESH data is naturally append-only or write-once — conflicts only arise on the mutable minority.

**For mutable data — Loro CRDTs:**

[Loro](https://loro.dev/) is a high-performance CRDT library written in Rust, designed for local-first applications:

- **LWW Register** — for simple scalar fields (site name, contact details). Last-Writer-Wins with full causal history preserved
- **LWW Map** — for key-value metadata on records. Each field independently mergeable
- **MovableList** — for ordered collections (action items on an enforcement notice)
- **MovableTree** — for hierarchical data (organisational structure, regulation taxonomy)
- Built-in **time travel** — every state is reconstructable from the operation log, critical for regulatory audit trails
- Rust-native with WASM bindings — runs on hub, edge, and in browser-based micro-apps
- Efficient incremental sync — only operations since last sync are exported/imported

**Conflict resolution architecture:**

```
┌──────────────────────────────────────────────────────┐
│                   Sync Engine                        │
│                                                      │
│  ┌────────────────────────────────────────────────┐  │
│  │  Layer 1: Append-Only Data (no conflicts)      │  │
│  │  Legislation, inspections, monitoring, events  │  │
│  │  Strategy: Lance delta sync (version compare)  │  │
│  └────────────────────────────────────────────────┘  │
│                                                      │
│  ┌────────────────────────────────────────────────┐  │
│  │  Layer 2: Mutable Metadata (rare conflicts)    │  │
│  │  Risk assessments, site info, action status    │  │
│  │  Strategy: Loro CRDTs (auto-merge)             │  │
│  └────────────────────────────────────────────────┘  │
│                                                      │
│  ┌────────────────────────────────────────────────┐  │
│  │  Layer 3: AI-Generated Data (never conflicts)  │  │
│  │  Embeddings, classifications, summaries        │  │
│  │  Strategy: Recompute locally (deterministic)   │  │
│  └────────────────────────────────────────────────┘  │
│                                                      │
│  ┌────────────────────────────────────────────────┐  │
│  │  Audit Log (append-only, immutable)            │  │
│  │  Every sync operation recorded with:           │  │
│  │  - node_id, timestamp, version_before/after    │  │
│  │  - operation type, affected records            │  │
│  │  - conflict resolution decisions (if any)      │  │
│  └────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────┘
```

#### 6.1.4 Partition Strategy: Locality-Aware Data Distribution

Not all data belongs on every node. The partition strategy determines what data each edge node holds — directly implementing Fractal Locality of Reference:

**Partition dimensions:**

```
Partition Key = (region, site_id, data_category, time_range)

Examples:
  EU/SITE-0042/compliance/2025-H2    → Edge node at Site 42
  EU/*/legislation/current            → All EU edge nodes
  */*/monitoring/2025-Q4              → Time-windowed rolloff
```

**Assignment rules:**
- Each edge node declares its **partition affinity** — which sites, regions, and categories it needs
- The hub maintains a **partition map** — which node holds which partitions
- At sync time, the hub only streams partitions matching the edge node's affinity
- Partitions can be **pinned** (always local), **cached** (local with eviction), or **remote** (query-on-demand via Flight)

**Locality guarantee:** after sync, an edge node at Site 42 has all data needed for a compliance status check in local Lance storage — no network round-trip required at query time. This is the Fractal Locality of Reference principle applied to distributed storage.

#### 6.1.5 Sync Modes

| Mode | Trigger | Direction | Use Case |
|---|---|---|---|
| **Scheduled pull** | Timer (e.g., every 15 min) | Hub → Edge | Legislation updates, regulatory changes |
| **Event push** | Local write | Edge → Hub | New inspection uploaded, incident reported |
| **On-demand pull** | User/app request | Hub → Edge | Inspector needs data for unfamiliar site |
| **Bulk initialisation** | First boot / recovery | Hub → Edge | New edge node deployment, disaster recovery |
| **Federated query** | Cross-node SQL | Bidirectional | Hub queries data that lives only on an edge node |

**Offline behaviour:**
- Edge nodes queue writes in a local **outbox** (Lance append log)
- When connectivity resumes, outbox drains via DoPut
- Loro CRDT merges handle any concurrent edits that occurred during the offline period
- The system never blocks on sync — local operations always proceed

#### 6.1.6 Bandwidth & Efficiency

Designed for constrained edge environments (field sites, remote locations):

- **Arrow columnar compression** — dictionary encoding, run-length encoding, and zstd compression on Flight streams reduce bandwidth by 80-95% for typical regulatory tabular data
- **Predicate pushdown** — edge requests only columns and rows it needs (via Flight SQL predicates), not full partitions
- **Embedding skip** — AI embeddings (often 50%+ of storage) are not synced; each node regenerates embeddings locally from source text using its local ONNX model. This trades compute for bandwidth.
- **Manifest-only probe** — `GetFlightInfo` checks version numbers before any data transfer. If nothing changed, zero bytes transferred.

### 6.2 Security Model

The security model must satisfy two distinct audiences: **operational security** (protecting distributed ESH data from breach, tampering, and unauthorised access) and **regulatory compliance** (demonstrating auditable controls for GDPR, EU AI Act, and sector-specific ESH regulations). The local-first architecture has an inherent security advantage — data never traverses the public internet by default — but the distributed nature introduces node authentication, micro-app isolation, and data sovereignty challenges.

#### 6.2.1 Threat Model

```
┌─────────────────────────────────────────────────────────────┐
│                      Threat Landscape                       │
│                                                             │
│  External                                                   │
│  ├── Network interception (hub ↔ edge transit)              │
│  ├── Stolen/lost edge device (physical access)              │
│  ├── Rogue node joining the cluster                         │
│  └── Supply-chain compromise of WASM micro-app              │
│                                                             │
│  Internal                                                   │
│  ├── Over-privileged micro-app accessing restricted data    │
│  ├── Insider exfiltrating data via a custom micro-app       │
│  ├── AI model leaking training data via inference outputs   │
│  └── Audit log tampering to conceal compliance violations   │
│                                                             │
│  Regulatory                                                 │
│  ├── GDPR: personal data in ESH records (inspector names,   │
│  │   worker health data, incident witness details)          │
│  ├── EU AI Act: high-risk AI system requirements (Aug 2026) │
│  └── ESH sector: tamper-proof records for enforcement       │
└─────────────────────────────────────────────────────────────┘
```

#### 6.2.2 Zero-Trust Node Authentication

Every node — hub or edge — is untrusted by default. Identity is established cryptographically, not by network location.

**Mutual TLS (mTLS) for all node-to-node communication:**

- Arrow Flight RPC runs over gRPC, which has native mTLS support
- Each node holds a unique X.509 certificate signed by an organisation-controlled Certificate Authority (CA)
- Both sides of every connection verify the peer's certificate — a hub won't accept data from an unauthenticated edge, and an edge won't accept data from an impersonated hub
- Certificate rotation via short-lived certs (24-72h) with automated renewal
- Revocation: compromised node certificates are revoked at the CA; all other nodes reject the revoked cert on next connection

**Node identity and enrolment:**

```
1. New edge node generates a keypair locally (never leaves device)
2. Node submits a Certificate Signing Request (CSR) to the hub
3. Administrator approves the CSR (out-of-band verification)
4. Hub's CA signs the certificate and returns it
5. Node is now authenticated — can participate in sync
6. Node's partition affinity is configured (what data it may access)
```

**Rust implementation:** [rustls](https://docs.rs/rustls/) for TLS (pure Rust, no OpenSSL dependency), backed by [ring](https://docs.rs/ring/) or [RustCrypto](https://github.com/rustcrypto) for the underlying cryptographic primitives. The `tonic` gRPC crate (used by Arrow Flight) has built-in rustls integration.

#### 6.2.3 MicroApp Sandboxing: WASM Capability-Based Security

WebAssembly's security model is the primary defence against malicious or compromised micro-apps.

**Wasmtime sandboxing guarantees:**

- **Memory isolation** — each WASM module gets its own linear memory; it cannot read or write the host's memory or another module's memory
- **No ambient authority** — a WASM module has zero capabilities by default. It cannot access the filesystem, network, clock, or any OS resource unless explicitly granted by the host
- **Fault isolation** — a crashing module cannot bring down the host or other modules
- **Control-flow integrity** — Wasmtime implements hardware-backed CFI to prevent sandbox escape via ROP/JOP attacks

**Capability grants via WIT interfaces:**

Each micro-app declares its required capabilities in its WIT (WebAssembly Interface Type) definition. The host grants only what is declared and approved:

```wit
// Example: compliance-checker micro-app
package fractal:compliance-checker;

world compliance-checker {
    // Data access — read-only, scoped to specific schemas
    import fractal:data/query {
        // Can query site_compliance and regulation tables
        // Cannot access: incident_personal_data, worker_health
        query: func(sql: string) -> result<arrow-batch, error>;
    }

    // AI inference — can call embeddings, not generative
    import fractal:ai/embeddings {
        embed: func(text: string) -> list<f32>;
    }

    // No filesystem access
    // No network access
    // No clock access (deterministic execution)

    export run: func(site-id: string) -> compliance-report;
}
```

**Capability enforcement matrix:**

| Capability | Granted To | Denied To | Mechanism |
|---|---|---|---|
| Read ESH analytical data | compliance-checker, report-gen | All others | WIT import scope |
| Read personal data fields | Authorised apps only (named) | Default deny | Column-level ACL + WIT |
| Write data (create records) | data-importer, audit-trail | compliance-checker, report-gen | WIT export direction |
| Network access (outbound) | regulatory-feed-sync | All others | WASI capability not granted |
| AI generative inference | report-gen, doc-qa | compliance-checker | WIT import availability |
| Filesystem access | None | All micro-apps | WASI filesystem not granted |

**Micro-app supply chain security:**

- All WASM modules are signed with Ed25519 (via the host's build pipeline)
- Signature verified before instantiation — unsigned or tampered modules are rejected
- Module hash recorded in the audit log on every load/hot-swap
- Optional: deterministic builds for reproducibility (Rust + `wasm32-wasi` target supports this)

#### 6.2.4 Access Control: Hybrid RBAC + ABAC

ESH data has both clearly-defined roles (inspector, site manager, regulator) and context-sensitive access needs (location, time, data classification). A hybrid model provides both structure and flexibility.

**RBAC layer — role definitions:**

| Role | Description | Typical Access |
|---|---|---|
| `site-operator` | Site staff managing day-to-day compliance | Own site's compliance records, monitoring data. Read-only legislation. |
| `inspector` | Field inspector conducting audits | Assigned sites' full records. Write inspection results. Read legislation. |
| `regulator` | Regulatory authority staff | Cross-site read access. Write enforcement actions. Full legislation. |
| `system-admin` | Node and cluster administration | Node configuration, user management, audit logs. No ESH data by default. |
| `ai-operator` | Manages AI models and inference | Model deployment, embeddings config. Read ESH data for testing only. |

**ABAC layer — attribute-based refinements:**

```
Policy: "Inspector can only access sites they are currently assigned to"

Attributes evaluated:
  - subject.role = "inspector"
  - subject.assigned_sites = ["SITE-0042", "SITE-0089"]
  - resource.site_id = "SITE-0042"
  - environment.time = within_business_hours
  - action = "read"

Decision: ALLOW (role matches, site is in assignment list, within hours)
```

ABAC policies are evaluated at the DataFusion query layer — a custom `OptimizerRule` injects row-level security predicates before execution:

```sql
-- User query:
SELECT * FROM site_compliance WHERE region = 'EU';

-- After ABAC injection:
SELECT * FROM site_compliance
WHERE region = 'EU'
  AND site_id IN ('SITE-0042', 'SITE-0089')  -- injected by ABAC
```

This means access control is enforced at the query engine level, not the application level — micro-apps cannot bypass it.

#### 6.2.5 Data Protection: Encryption at Rest and in Transit

**In transit:**
- All node-to-node: mTLS via Arrow Flight/gRPC (see 6.2.2)
- TLS 1.3 minimum, with `CHACHA20_POLY1305` or `AES_256_GCM` cipher suites
- Arrow record batches are encrypted within the TLS stream — no plaintext ever on the wire

**At rest — Parquet Modular Encryption:**

For data stored as Parquet files (DuckDB's storage and Arrow exports):
- [Parquet Modular Encryption](https://arrow.apache.org/docs/python/parquet.html) (supported since Arrow 4.0) provides column-level encryption
- **Envelope encryption**: each column/file encrypted with a random Data Encryption Key (DEK), DEKs encrypted with a Master Encryption Key (MEK)
- **Column-level granularity**: personal data columns (inspector names, witness details, worker health) encrypted with a restricted MEK; non-sensitive columns (regulation IDs, timestamps) encrypted with a general MEK or left in plaintext for query performance
- MEKs managed via a local Key Management Service (on-node, no cloud KMS dependency)

```
┌─────────────────────────────────────────┐
│           Parquet File on Disk           │
│                                         │
│  ┌───────────┐  ┌───────────────────┐   │
│  │ Column A  │  │ Column B          │   │
│  │ site_id   │  │ inspector_name    │   │
│  │ (plain)   │  │ (AES-256-GCM)    │   │
│  │           │  │ DEK_B → MEK_PII  │   │
│  └───────────┘  └───────────────────┘   │
│  ┌───────────┐  ┌───────────────────┐   │
│  │ Column C  │  │ Column D          │   │
│  │ reg_id    │  │ health_data       │   │
│  │ (plain)   │  │ (AES-256-GCM)    │   │
│  │           │  │ DEK_D → MEK_HLTH │   │
│  └───────────┘  └───────────────────┘   │
│                                         │
│  Footer: encrypted column metadata      │
└─────────────────────────────────────────┘
```

**At rest — Lance files:**

Lance does not yet have built-in column-level encryption. Mitigation strategy:
- Filesystem-level encryption (LUKS/dm-crypt on Linux) for full-disk protection on all nodes
- Sensitive Lance columns containing personal data stored in a separate Lance dataset encrypted at the filesystem level with a distinct key
- Roadmap: contribute column-level encryption to Lance upstream (aligns with their v2 format extensibility)

**At rest — edge device loss/theft:**

- Full-disk encryption mandatory on all edge nodes (LUKS2 with TPM-backed key on supported hardware)
- Remote wipe capability: hub can issue a revocation that causes the edge node to zero its Lance/DuckDB storage on next boot
- Encrypted local key store: node private keys and MEKs stored in a hardware security module (TPM 2.0) or software keyring (Linux kernel keyring) — never on the plaintext filesystem

#### 6.2.6 Audit Trail: Immutable Compliance Log

The audit trail is the backbone of regulatory compliance — every action on ESH data must be reconstructable.

**What is logged:**

| Event Category | Recorded Fields |
|---|---|
| Data access | who, when, what query, which rows/columns returned, from which node |
| Data modification | who, when, old value hash, new value hash, reason/context |
| Sync operations | source node, dest node, version range, partition IDs, bytes transferred |
| Conflict resolution | field, conflicting values, resolution strategy applied, final value |
| AI inference | model ID, model version, input hash, output, confidence score |
| Micro-app lifecycle | module hash, load/unload time, capabilities granted |
| Authentication | node ID, cert fingerprint, connect/disconnect, auth success/failure |
| Access denied | who, what resource, why (role, ABAC attribute mismatch) |

**Immutability guarantees:**

- Audit log is **append-only** in a dedicated Lance dataset (separate from operational data)
- Each log entry is **hash-chained** — entry N includes `hash(entry N-1)`, creating a tamper-evident chain
- Periodic **signed checkpoints** — the hub signs a hash of the log state at intervals, anchoring the chain to a trusted timestamp
- Log entries are **replicated** from edge to hub during sync — an attacker would need to compromise both the edge log and the hub's copy to alter history
- Retention: minimum 6 months (EU AI Act Article 26 requirement), configurable up to 7 years for ESH regulatory retention

**Audit log schema (Arrow):**

```
audit_log {
    entry_id:       uint64        (monotonic, per-node)
    timestamp:      timestamp[ns] (UTC)
    node_id:        utf8
    actor_id:       utf8          (user or micro-app ID)
    actor_role:     utf8
    event_type:     utf8          (enum: access, modify, sync, inference, ...)
    resource:       utf8          (table.column or partition ID)
    detail:         utf8          (JSON — query text, model ID, etc.)
    prev_hash:      fixed_binary[32]  (SHA-256 of previous entry)
    signature:      fixed_binary[64]  (Ed25519, present on checkpoint entries)
}
```

#### 6.2.7 EU AI Act Compliance (High-Risk Systems)

ESH regulatory AI likely qualifies as high-risk under the EU AI Act (AI used in safety-critical contexts, regulatory enforcement, and potentially affecting fundamental rights). Full compliance required by **August 2, 2026**.

**Required measures and how the architecture addresses them:**

| EU AI Act Requirement | Article | Architecture Response |
|---|---|---|
| Risk management system | Art. 9 | Risk classification of each AI use case; confidence thresholds gate automated decisions |
| Data governance | Art. 10 | DataFusion ABAC enforces data quality gates; training data lineage tracked in audit log |
| Technical documentation | Art. 11 | Model cards stored alongside models; versioned in Lance with the deployment manifest |
| Record-keeping (logging) | Art. 12 | Immutable hash-chained audit log (6.2.6) — every inference logged with model version, input hash, output |
| Transparency to users | Art. 13 | AI-generated outputs are labelled; confidence scores exposed; human review flagged |
| Human oversight | Art. 14 | No autonomous enforcement decisions — AI provides recommendations, humans decide |
| Accuracy, robustness, cybersecurity | Art. 15 | Quantised model validation suite; mTLS + WASM sandbox + encryption stack |
| Conformity assessment | Art. 43 | Architecture supports self-assessment (Annex VI) with exportable documentation |
| Registration in EU database | Art. 49 | System metadata exportable in required format |

**Human-in-the-loop guarantee:**

```
┌──────────┐     ┌───────────┐     ┌──────────────┐     ┌──────────┐
│ ESH Data │────>│ AI Agent  │────>│ Recommendation│────>│ Human    │
│ (local)  │     │ (local    │     │ + Confidence  │     │ Decision │
│          │     │  inference)│     │ + Evidence    │     │ (final)  │
└──────────┘     └───────────┘     └──────────────┘     └──────────┘
                                          │
                                    ┌─────┴─────┐
                                    │ Audit Log │
                                    │ (both AI  │
                                    │  output & │
                                    │  human    │
                                    │  decision)│
                                    └───────────┘
```

AI never acts autonomously on enforcement decisions. Every AI recommendation is paired with its evidence trail and confidence score. The human decision and the AI recommendation are both recorded in the audit log — providing the transparency and oversight the EU AI Act demands.

#### 6.2.8 Data Sovereignty & Network Isolation

**Default posture: air-gapped local network.**

- Edge nodes communicate only with their designated hub over the local/private network
- No data egresses to the public internet unless an explicit sync policy is configured
- DNS, NTP, and software updates can optionally route through the hub as a controlled gateway
- Regulatory data partitions can be tagged with sovereignty constraints: `sovereignty: "EU"` prevents that partition from syncing to nodes outside the EU region

**Network segmentation:**

```
┌─────────────────────────────────────────────┐
│              Organisation Network            │
│                                              │
│   ┌──────────┐    mTLS    ┌──────────┐      │
│   │ Edge Node├────────────┤ Hub Node │      │
│   └──────────┘            └────┬─────┘      │
│   ┌──────────┐    mTLS         │             │
│   │ Edge Node├─────────────────┘             │
│   └──────────┘                               │
│                                              │
│   No outbound internet by default            │
│   ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─         │
│   Optional: hub → regulatory feed (HTTPS)    │
│   Optional: hub → software update (HTTPS)    │
└─────────────────────────────────────────────┘
```

#### 6.2.9 Cryptographic Library Selection

| Purpose | Library | Rationale |
|---|---|---|
| TLS (node-to-node) | [rustls](https://docs.rs/rustls/) | Pure Rust, no OpenSSL. Audited. Integrates with tonic/gRPC. |
| Symmetric encryption (AES-256-GCM) | [ring](https://docs.rs/ring/) | Hybrid Rust/ASM, hard-to-misuse API. FIPS-validated primitives. |
| Hashing (SHA-256 for audit chain) | [RustCrypto/sha2](https://github.com/RustCrypto/hashes) | Pure Rust, no unsafe. Constant-time. |
| Signing (Ed25519 for audit checkpoints, module signing) | [ring](https://docs.rs/ring/) or [ed25519-dalek](https://docs.rs/ed25519-dalek/) | ed25519-dalek for pure Rust; ring if already a dependency. |
| Key derivation (HKDF for DEK generation) | [ring](https://docs.rs/ring/) | HKDF-SHA256 for Parquet DEK derivation from MEK. |
| Certificate management | [rcgen](https://docs.rs/rcgen/) + [x509-cert](https://docs.rs/x509-cert/) | Certificate generation and parsing for the internal CA. |
| Secure random | [ring::rand](https://docs.rs/ring/) | OS-backed CSPRNG. |

All cryptographic dependencies are **pure Rust or Rust+ASM** — no C OpenSSL linkage, reducing supply-chain attack surface and simplifying cross-compilation to edge targets (ARM, RISC-V).

---

## 7. Development Roadmap (Phases)

### Phase 1: Foundation
- Rust workspace with core library crate
- DuckDB + LanceDB integration with shared Arrow buffers
- Basic ESH data schema (Arrow schema definitions)
- Data ingestion pipeline (regulatory documents -> embeddings -> LanceDB)
- CLI interface for queries

### Phase 2: AI Integration
- ONNX Runtime integration for embeddings and classification
- MLC LLM / llama.cpp integration for generative inference
- RAG pipeline: query -> LanceDB vector search -> LLM context -> response
- Regulatory document Q&A capability

### Phase 3: MicroApp Runtime
- Wasmtime Engine with pooling allocator, fuel metering, and epoch interruption
- WIT package definitions: fractal:data, fractal:ai, fractal:events, fractal:audit
- Host Interface Layer bridging WIT imports to DataFusion/DuckDB/LanceDB/ONNX
- App Supervisor: Registry (local OCI), Scheduler, Lifecycle Manager, Router
- AOT compilation pipeline with per-architecture .cwasm caching
- Arrow IPC serialisation for host ↔ guest data exchange
- First micro-apps: regulation-importer, compliance-checker, report-generator, incident-classifier
- Hot-swap with drain timeout, signature verification, and rollback
- Component composition via wasm-tools compose for stable pipelines
- ResourceLimiter per-app tiering (lightweight / standard / heavy)
- OCI artifact publishing and distribution workflow

### Phase 4: Distribution
- Arrow Flight RPC transport layer (DoGet/DoPut/DoExchange)
- Lance delta sync engine (version-based manifest comparison)
- Loro CRDT integration for mutable metadata conflict resolution
- Partition affinity system and locality-aware data distribution
- Sync modes: scheduled pull, event push, on-demand, bulk init
- Offline outbox with automatic drain on reconnect
- Federated query via DataFusion + Flight SQL across nodes
- Node discovery and coordination
- Edge node deployment tooling

### Phase 5: Security & Production Hardening
- Internal CA setup, mTLS certificate issuance and node enrolment workflow
- WASM micro-app signing pipeline (Ed25519) and signature verification on load
- RBAC role definitions and ABAC policy engine in DataFusion OptimizerRule
- Parquet Modular Encryption: column-level encryption for personal/health data
- LUKS2 full-disk encryption on all edge nodes, TPM-backed key storage
- Immutable hash-chained audit log with signed checkpoints
- Remote wipe capability for lost/stolen edge devices
- EU AI Act conformity: model cards, inference logging, human-in-the-loop enforcement
- Data sovereignty tagging and partition-level geo-fencing
- Monitoring, health checks, and alerting across nodes
- Performance profiling and Locality Optimization tuning

---

## 8. Key Risks & Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| WASI 0.3 not stable by deployment | Async micro-apps blocked | Use WASI 0.2 (stable) initially; async via host-side Tokio |
| Quantised model accuracy insufficient for regulatory text | Incorrect compliance advice | Hybrid: local inference for triage, human review for decisions; confidence scoring |
| Edge device heterogeneity | Build/test matrix explosion | Target 2-3 reference architectures; WASM provides abstraction |
| Data sync conflicts in multi-node | Regulatory data integrity | Append-only audit log; CRDTs for metadata; human resolution for conflicts |
| Rust ecosystem churn | Dependency maintenance burden | Pin versions; minimise dependency count; prefer stable crates |
| EU AI Act compliance | Legal exposure for AI-assisted regulatory decisions | Classify system risk level early; implement required documentation and human oversight |

---

## 9. Research Sources

- [Fractal Computing — Architecture Overview](https://fractalweb.app)
- [Fractal Locality Optimization Technology](https://fractalweb.app/Locality)
- [LanceDB — Embedded Vector Database](https://lancedb.com/)
- [LanceDB Rust SDK](https://docs.rs/lancedb/latest/lancedb/)
- [DuckDB + Arrow Zero-Copy Integration](https://duckdb.org/2021/12/03/duck-arrow)
- [Apache Arrow Columnar Format](https://arrow.apache.org/docs/format/Columnar.html)
- [Apache DataFusion — SQL Query Engine](https://datafusion.apache.org/)
- [DataFusion Rust API Docs](https://docs.rs/datafusion/latest/datafusion/)
- [DataFusion TableProvider Trait](https://docs.rs/datafusion/latest/datafusion/catalog/trait.TableProvider.html)
- [DataFusion SIGMOD Paper](https://dl.acm.org/doi/10.1145/3626246.3653368)
- [Lance + DataFusion Integration](https://lancedb.github.io/lance/integrations/datafusion/)
- [datafusion-table-providers (DuckDB, SQLite, Postgres connectors)](https://crates.io/crates/datafusion-table-providers)
- [MLC LLM — Universal Deployment Engine](https://github.com/mlc-ai/mlc-llm)
- [ONNX Runtime — Edge AI Inference](https://onnxruntime.ai/)
- [On-Device LLMs: State of the Union 2026](https://v-chandra.github.io/on-device-llms/)
- [Small Language Models 2026 Guide](https://localaimaster.com/blog/small-language-models-guide-2026)
- [Wasmtime — WebAssembly Runtime](https://github.com/bytecodealliance/wasmtime)
- [WASI Component Model Status](https://eunomia.dev/blog/2025/02/16/wasi-and-the-webassembly-component-model-current-status/)
- [WASI 0.3 and Composable Concurrency](https://medium.com/wasm-radar/hypercharge-through-components-why-wasi-0-3-and-composable-concurrency-are-a-game-changer-0852e673830a)
- [Wasmtime Plugin Architecture](https://docs.wasmtime.dev/wasip2-plugins.html)
- [WebAssembly System Programming with WASI](https://dasroot.net/posts/2026/01/webassembly-system-programming-wasi-wasmtime-rust/)
- [System76 Threadripper Workstations](https://system76.com/threadripper/)
- [Rust vs Zig Data-Driven Analysis](https://medium.com/@psalms142/zig-vs-rust-a-data-driven-analysis-of-systems-programming-6e84bbb6da7f)
- [Zig vs Rust Performance Benchmark 2026](https://app.daily.dev/posts/zig-vs-rust-performance-benchmark-2026-ad9jvhqaa)
- [EU AI Act Compliance 2026](https://www.wiz.io/academy/ai-security/ai-compliance)
- [Edge LLM Deployment Guide](https://kodekx-solutions.medium.com/edge-llm-deployment-on-small-devices-the-2025-guide-2eafb7c59d07)
- [Apache Arrow Flight RPC](https://arrow.apache.org/docs/format/Flight.html)
- [Arrow Flight Benchmarks — Wire-Speed Data Transfer](https://arxiv.org/abs/2204.03032)
- [Loro — High-Performance CRDT Library (Rust)](https://loro.dev/)
- [Loro GitHub](https://github.com/loro-dev/loro)
- [Lance v2 Columnar Format — Versioning & Transactions](https://blog.lancedb.com/lance-v2/)
- [DataFusion Federation — Distributed Query](https://lib.rs/crates/datafusion-federation)
- [Local-First Apps 2025 — CRDTs, Replication, Edge Storage](https://debugg.ai/resources/local-first-apps-2025-crdts-replication-edge-storage-offline-sync)
- [FOSDEM 2026 — Local-First, Sync Engines, CRDTs](https://fosdem.org/2026/schedule/track/local-first/)
- [Wasmtime Security Model](https://docs.wasmtime.dev/security.html)
- [WASI Capability-Based Security](https://marcokuoni.ch/blog/15_capabilities_based_security/)
- [Provably-Safe Sandboxing with WebAssembly (CMU)](https://www.cs.cmu.edu/~csd-phd-blog/2023/provably-safe-sandboxing-wasm/)
- [NIST SP 800-207 Zero Trust Architecture](https://nvlpubs.nist.gov/nistpubs/specialpublications/NIST.SP.800-207.pdf)
- [mTLS over gRPC for Trusted Communication](https://medium.com/deno-the-complete-reference/strengthening-microservices-implementing-mtls-over-grpc-for-trusted-communication-946b39333880)
- [Parquet Modular Encryption — Column-Level Encryption](https://arrow.apache.org/docs/python/parquet.html)
- [ring — Rust Cryptography Library](https://docs.rs/ring/)
- [RustCrypto Ecosystem](https://github.com/rustcrypto)
- [Awesome Rust Cryptography](https://cryptography.rs/)
- [rustls — Modern TLS in Rust](https://docs.rs/rustls/)
- [EU AI Act — High-Risk System Requirements](https://artificialintelligenceact.eu/article/26/)
- [EU AI Act 2026 Compliance Guide](https://secureprivacy.ai/blog/eu-ai-act-2026-compliance)
- [EU AI Act High-Risk Requirements (Dataiku)](https://www.dataiku.com/stories/blog/eu-ai-act-high-risk-requirements)
- [Wasmtime Component Model API](https://docs.wasmtime.dev/api/wasmtime/component/index.html)
- [wasmtime::component::bindgen! Macro](https://docs.wasmtime.dev/api/wasmtime/component/macro.bindgen.html)
- [WIT Resources in Rust](https://component-model.bytecodealliance.org/language-support/using-wit-resources/rust.html)
- [Building Host Implementations for WASM Interfaces](https://radu-matei.com/blog/wasm-components-host-implementations/)
- [wit-bindgen — Language Binding Generator](https://github.com/bytecodealliance/wit-bindgen)
- [Wasmtime Pooling Allocator](https://docs.wasmtime.dev/api/wasmtime/struct.PoolingAllocationConfig.html)
- [Wasmtime ResourceLimiter Trait](https://docs.rs/wasmtime/latest/wasmtime/trait.ResourceLimiter.html)
- [Wasmtime Fast Instantiation](https://docs.wasmtime.dev/examples-fast-instantiation.html)
- [Wasmtime Fuel Metering & Deterministic Execution](https://docs.wasmtime.dev/examples-deterministic-wasm-execution.html)
- [Wasmtime Interrupting Execution (Epochs)](https://docs.wasmtime.dev/examples-interrupting-wasm.html)
- [Wasmtime AOT Pre-Compilation](https://docs.wasmtime.dev/examples-pre-compiling-wasm.html)
- [Cranelift Code Generator](https://cranelift.dev/)
- [Zero-Copy Apache Arrow with WebAssembly](https://kylebarron.dev/blog/zero-copy-apache-arrow-with-webassembly/)
- [Component Model Flat Data / Zero-Copy Proposal](https://github.com/WebAssembly/component-model/issues/398)
- [Distributing WASM Components via OCI Registries](https://opensource.microsoft.com/blog/2024/09/25/distributing-webassembly-components-using-oci-registries)
- [wasm-pkg-tools (wkg CLI)](https://github.com/bytecodealliance/wasm-pkg-tools)
- [Spin 3.0 — WebAssembly Microservices Framework](https://www.fermyon.com/spin)
- [WASM Hot-Reload PoC in Rust](https://github.com/shekohex/rust-wasm-hotreload)
- [Component Model Explainer](https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md)
