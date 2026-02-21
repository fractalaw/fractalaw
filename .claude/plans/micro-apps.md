# Micro-Apps Brainstorm

*2026-02-21 -- Informing Phase 3 priorities for `fractalaw-host` and WIT interface design*

## 1. The Micro-App Mental Model

A micro-app is a single focused AI capability, compiled to WebAssembly, sandboxed by the host runtime, given exactly the data slice and AI tools it needs, and nothing more. It does one thing well. It runs on the hub, or on the edge, or on both -- determined by where the data and the user are.

This is not monolithic app thinking. A monolithic EHS compliance platform would be a single binary that does ingestion, classification, search, reporting, gap analysis, audit trail, and regulatory monitoring all in one process. That design fails on three axes: it ships all data to one place, it requires network connectivity, and it bundles capabilities that different users need at different times in different locations.

The micro-app model inverts this. Each capability is an independently deployable WASM component. The host runtime (`fractalaw-host`) provides the execution environment -- Wasmtime with pooling allocator, fuel metering, and capability-based security. Micro-apps import only the WIT interfaces they need (`fractal:data/query`, `fractal:ai/embeddings`, `fractal:ai/classify`, `fractal:events/emit`, `fractal:audit/log`). They receive data as Arrow IPC bytes through the WIT boundary. They cannot see each other's memory. They cannot access the network or filesystem unless the host explicitly grants it.

**Is this the right way to think about micro-apps -- they do one thing with AI optimised to that one thing?** Yes. Each micro-app is a focused AI capability in a WASM sandbox with exactly the data and AI tools it needs. The two anchor examples make this concrete:

### Anchor Example 1: DRRP Polisher (hub-side)

The sister Elixir app scrapes legislation.gov.uk, parses the legislative text, and uses Regex to identify duties, responsibilities, rights, and powers (DRRP). The regex finds the general area where these exist -- it knows a section contains a duty imposed on "the employer" -- but it struggles to extract the precise core provision. A section may be 500 words long with qualifications, exceptions, cross-references, and nested sub-paragraphs. The actual duty might be a single sentence buried in the middle.

The DRRP Polisher micro-app receives the Elixir app's rough DRRP annotations (synced into fractalaw via the bridge layer), loads the corresponding legislation text from LanceDB, and uses `fractal:ai/inference` to extract the precise provision text. It writes the polished DRRP back to the legislation table's `duties`, `rights`, `responsibilities`, and `powers` columns (the `DRRPEntry` structs in the schema: holder, duty_type, clause, article).

This micro-app runs on the hub because it needs access to the full legislation corpus and a capable generative model (8B parameter). It processes in batch. It does one thing: polish DRRP extractions from rough regex matches to precise provisions.

### Anchor Example 2: Field Research Tool (edge-side)

An EHS auditor is on site at a chemical manufacturing facility. They are standing next to a process that emits volatile organic compounds to atmosphere. They want to know: what laws govern this emission? What are the duty holder's obligations? What permits apply?

The Field Research Tool micro-app runs on their tablet or laptop. It has a synced data slice containing all environmental/pollution/climate-change legislation relevant to the site's jurisdiction and industry sector. It uses `fractal:ai/embeddings` to embed the auditor's natural language query, searches the local LanceDB partition via `fractal:data/query`, and returns ranked legislation sections with their DRRP annotations.

No network needed. The ONNX embedding model runs locally. The data was synced from the hub during the last connectivity window. The micro-app does one thing: answer "what law applies to this?" from a local data slice.

### The Pattern

Every micro-app follows this structure:

| Aspect | Description |
|--------|-------------|
| **Scope** | One focused capability -- not a module, not a feature, one specific AI-assisted task |
| **Runtime** | WASM component, sandboxed by Wasmtime, fuel-metered, memory-limited |
| **Data** | Receives exactly the data partition it needs via `fractal:data/query` (Arrow IPC) |
| **AI** | Uses exactly the inference capabilities it needs (`embeddings`, `classify`, `inference`) |
| **Events** | Can emit domain events via `fractal:events/emit` for other micro-apps to react to |
| **Audit** | All actions logged via `fractal:audit/log` (hash-chained, immutable) |
| **Deployment** | OCI artifact, signed, version-pinned, hot-swappable |
| **Composition** | Can be statically composed with other micro-apps via `wasm-tools compose`, or dynamically chained by the host via events |

---

## 2. Hub-Side Micro-Apps (AI refinement, batch processing, centroid training)

Hub-side micro-apps run on the powerful hub machine (Threadripper/Ryzen, 64-512GB RAM, discrete GPU optional). They have access to the full dataset across all jurisdictions, all time periods, all families. They use heavier AI models (8B generative, full ONNX classifiers). They process data in bulk and their outputs feed edge nodes via sync.

Hub-side micro-apps are the "refinement engine" of the system. The sister Elixir app does the heavy lifting of scraping, parsing, and initial annotation. The hub micro-apps take those rough outputs and polish them with AI. The results flow to edge devices as pre-computed, high-quality data.

**Characteristics:**
- Tier: `standard` or `heavy` (64-256MB memory, 1B-10B fuel)
- Trigger: scheduled (cron), event-driven (new data arrives), or manual (operator kicks off a batch)
- AI models: full ONNX classifiers, generative models for extraction/summarisation
- Data access: full corpus via `fractal:data/query`
- Outputs: enriched data written back to DuckDB/LanceDB, or events emitted for downstream apps

**Typical workflows:**
1. **Batch enrichment** -- new legislation arrives from Elixir, hub micro-apps classify it, extract DRRP, generate embeddings, compute centroids
2. **Model training** -- centroid refinement, confidence threshold tuning, conflict resolution
3. **Quality assurance** -- cross-checking AI outputs against ground truth, flagging anomalies
4. **Report generation** -- compliance reports, regulatory change digests, statistical analysis

---

## 3. Edge-Side Micro-Apps (field tools, offline search, real-time assistance)

Edge-side micro-apps run on mobile devices, laptops, tablets, or mini PCs at the point of use -- the factory floor, the construction site, the offshore platform, the inspector's car. They work offline with a synced data slice. They use lightweight ONNX models (all-MiniLM-L6-v2 at 23MB, quantised classifiers at single-digit MB). They answer specific operational questions in real time.

Edge micro-apps consume what the hub produces. The hub's batch enrichment pipeline creates high-quality classified, DRRP-annotated, embedded legislation. The sync engine delivers the relevant partition to the edge node. The edge micro-app's job is to make that data useful at the moment of need.

**Characteristics:**
- Tier: `lightweight` or `standard` (16-64MB memory, 100M-1B fuel)
- Trigger: user interaction (search query, scan, question), or scheduled (daily compliance check)
- AI models: quantised ONNX embeddings, lightweight classifiers -- no generative models on constrained devices
- Data access: local partition only (site-specific, jurisdiction-specific, family-specific)
- Outputs: answers, alerts, checklists rendered to the user; field data captured and queued for hub sync

**Typical workflows:**
1. **Search and research** -- "what law applies to this process?" answered from local data
2. **Checklist generation** -- given the applicable laws, what should the auditor check?
3. **Incident capture** -- classify and record an incident on-site, queue for hub sync
4. **Quick compliance check** -- is this site in compliance for the specific regulation being inspected?

---

## 4. Bridge Micro-Apps (sync, transform, align)

Bridge micro-apps handle the data flow between systems. They sit at the boundaries: between the sister Elixir app and fractalaw, between hub and edge, between fractalaw and external regulatory feeds.

The sister Elixir app produces data in PostgreSQL (LRT, LAT, amendment tables). That data needs to flow into fractalaw's DuckDB/LanceDB stores in Arrow format. Bridge micro-apps handle this transformation. They also handle the reverse flow: fractalaw's AI-enriched data flowing back to the Elixir app's Postgres for the web UI.

**Characteristics:**
- Tier: `standard` or `heavy` (depends on data volume)
- Trigger: event-driven (new data in source), scheduled (periodic sync), or manual
- AI models: usually none -- bridges are data transformation, not AI inference
- Data access: source system (via `fractal:data/query` or Flight) and target system (via `fractal:data/mutate`)
- Outputs: transformed data written to target store, sync metadata emitted as events

**Key bridge patterns:**
1. **Elixir-to-Fractalaw** -- Postgres export (Parquet/Arrow IPC) ingested into DuckDB + LanceDB
2. **Fractalaw-to-Elixir** -- AI classification results, polished DRRP, embeddings exported back to Postgres
3. **Hub-to-Edge** -- partition selection, delta sync, embedding skip (regenerate locally)
4. **External-to-Hub** -- regulatory feed ingestion (legislation.gov.uk Atom feeds, EUR-Lex SPARQL)

---

## 5. Brainstorm: Concrete Micro-App Ideas

### 5.1 DRRP Polisher

| Attribute | Detail |
|-----------|--------|
| **One-liner** | Extracts precise duty/right/responsibility/power provisions from legislation sections that regex has flagged as containing DRRP. |
| **Runs on** | Hub |
| **Tier** | Standard |
| **WIT imports** | `fractal:data/query`, `fractal:data/mutate`, `fractal:ai/inference`, `fractal:audit/log` |
| **AI model** | Generative (8B) -- extracts the core provision sentence from a longer section |
| **Data slice** | `legislation_text` rows where the parent law has `is_making = true` and DRRP metadata from Elixir sync |
| **User story** | The Elixir app has identified that Section 2 of the Health and Safety at Work etc. Act 1974 imposes a duty on "every employer." But the section is 200 words long with qualifications. The DRRP Polisher reads the full section text, identifies the core duty ("It shall be the duty of every employer to ensure, so far as is reasonably practicable, the health, safety and welfare at work of all his employees"), and writes a clean `DRRPEntry` with holder="every employer", duty_type="general duty", clause="s.2(1)". |

### 5.2 Field Research Tool

| Attribute | Detail |
|-----------|--------|
| **One-liner** | Answers "what law applies to this process/hazard/emission?" from a local data slice using semantic search. |
| **Runs on** | Edge |
| **Tier** | Lightweight |
| **WIT imports** | `fractal:data/query`, `fractal:ai/embeddings`, `fractal:audit/log` |
| **AI model** | ONNX embeddings (all-MiniLM-L6-v2, 23MB) |
| **Data slice** | Legislation text partition for the site's jurisdiction + relevant families (e.g., Pollution, Environmental Protection, Climate Change for a chemical site) |
| **User story** | An EHS auditor at a paint manufacturing facility sees a solvent recovery unit venting to atmosphere. They type "volatile organic compound emission limits" into the Field Research Tool. It embeds the query, searches the local LanceDB partition, and returns ranked sections from the Environmental Permitting Regulations 2016, the Solvent Emissions Directive transposition, and the Clean Air Act 1993. Each result shows the section text, its DRRP annotation (who has the duty, what the duty is), and a confidence score. |

### 5.3 Regulatory Change Monitor

| Attribute | Detail |
|-----------|--------|
| **One-liner** | Detects new or amended legislation and identifies which sites, families, and obligations are affected. |
| **Runs on** | Hub |
| **Tier** | Standard |
| **WIT imports** | `fractal:data/query`, `fractal:ai/embeddings`, `fractal:ai/classify`, `fractal:events/emit`, `fractal:audit/log` |
| **AI model** | ONNX embeddings + centroid classifier |
| **Data slice** | Full legislation table, law_edges, legislation_text |
| **User story** | A new Statutory Instrument amending the Environmental Permitting Regulations is published. The Elixir app scrapes it and syncs the raw data into fractalaw. The Regulatory Change Monitor runs automatically. It embeds the new SI's text, classifies it by domain/family, traces its amendment edges to identify which existing regulations are affected, and emits a `regulatory-change` event listing the affected laws and families. Downstream micro-apps (compliance checkers, report generators) pick up this event and re-evaluate. The compliance officer receives a summary: "SI 2026/412 amends EPR 2016 reg.35 -- affects 12 sites with environmental permits." |

### 5.4 Compliance Gap Analyser

| Attribute | Detail |
|-----------|--------|
| **One-liner** | Compares a site's permits and compliance records against applicable legislation to identify gaps. |
| **Runs on** | Both (hub for batch across all sites, edge for single-site on-demand) |
| **Tier** | Standard |
| **WIT imports** | `fractal:data/query`, `fractal:ai/classify`, `fractal:audit/log` |
| **AI model** | ONNX classifier (domain/family matching) |
| **Data slice** | Hub: full legislation + all site compliance records. Edge: site-specific partition |
| **User story** | A compliance manager wants to know: for Site 42 (a waste processing facility), which regulatory obligations are we meeting and which have gaps? The Compliance Gap Analyser queries the site's permits, inspections, and monitoring data against the WASTE and ENVIRONMENTAL PROTECTION family legislation. It classifies each obligation as "met" (evidence exists), "gap" (no evidence), or "unclear" (partial evidence). Output: a ranked list of gaps with the specific legislation section, the obligation text, and a suggested action. |

### 5.5 Incident Classifier

| Attribute | Detail |
|-----------|--------|
| **One-liner** | Auto-categorises an incident report by regulatory domain, family, severity, and applicable legislation. |
| **Runs on** | Both (edge for immediate field classification, hub for batch reprocessing) |
| **Tier** | Lightweight |
| **WIT imports** | `fractal:ai/classify`, `fractal:ai/embeddings`, `fractal:data/query`, `fractal:audit/log` |
| **AI model** | ONNX classifier + embeddings |
| **Data slice** | Incident description text (input), legislation centroid data (reference) |
| **User story** | A site operator reports: "Chemical spill in warehouse -- approximately 200L of solvent leaked from a damaged drum onto an unbunded floor. No drainage nearby. Spill contained with absorbent." The Incident Classifier embeds this text, classifies it (domain: environment + health_safety, family: POLLUTION + OH&S), assigns severity based on volume/substance/containment, and links to applicable regulations (COSHH Regulations, Environmental Damage Regulations). The operator sees the classification immediately on their device. |

### 5.6 Centroid Trainer

| Attribute | Detail |
|-----------|--------|
| **One-liner** | Recomputes classification centroids from confirmed labels, resolves conflicts, and publishes updated centroids for all classifiers. |
| **Runs on** | Hub |
| **Tier** | Heavy |
| **WIT imports** | `fractal:data/query`, `fractal:data/mutate`, `fractal:events/emit`, `fractal:audit/log` |
| **AI model** | None (pure computation over embeddings) |
| **Data slice** | Full legislation table (labels + embeddings), legislation_text (section embeddings) |
| **User story** | An expert reviews the 103 conflict cases from the initial classification run. They confirm 60, correct 30, and flag 13 as genuine multi-family edge cases. The Centroid Trainer ingests these corrections, recomputes centroids with the updated confirmed labels, applies weighted averaging (confirmed labels weighted higher than predicted), and publishes new centroids. The `regulatory-change` event triggers downstream reclassification of all predicted-only laws. Classification agreement rate improves from 74.6% to ~85%. |

### 5.7 Cross-Jurisdiction Comparator

| Attribute | Detail |
|-----------|--------|
| **One-liner** | Compares regulatory obligations across jurisdictions for a given topic, highlighting differences and gaps. |
| **Runs on** | Hub |
| **Tier** | Standard |
| **WIT imports** | `fractal:data/query`, `fractal:ai/embeddings`, `fractal:ai/inference`, `fractal:audit/log` |
| **AI model** | ONNX embeddings for semantic alignment + generative model for summarising differences |
| **Data slice** | Legislation from multiple jurisdictions (UK, Scotland, Wales, Northern Ireland, EU), filtered by family |
| **User story** | A multinational company operates sites in England and Scotland. Their EHS manager asks: "How do waste regulations differ between England and Scotland?" The Comparator queries WASTE-family legislation for both jurisdictions, uses embedding similarity to align equivalent provisions, and generates a structured comparison: "The Waste (England and Wales) Regulations 2011 require X; the Waste (Scotland) Regulations 2012 require Y. Key difference: Scottish regulations impose additional requirements on Z." This is essential for companies operating across devolved jurisdictions. |

### 5.8 Penalty Researcher

| Attribute | Detail |
|-----------|--------|
| **One-liner** | Finds and summarises penalty provisions (fines, imprisonment, enforcement notices) for a given offence or regulation. |
| **Runs on** | Both (hub for comprehensive research, edge for quick lookup) |
| **Tier** | Lightweight |
| **WIT imports** | `fractal:data/query`, `fractal:ai/embeddings`, `fractal:audit/log` |
| **AI model** | ONNX embeddings for semantic search |
| **Data slice** | Legislation text filtered to sections containing penalty language (can be pre-tagged by the Elixir app or by a hub-side tagger) |
| **User story** | A regulator is preparing an enforcement case for a breach of the Environmental Permitting Regulations 2016, reg.38(1). They query: "What are the penalties for operating without a permit under EPR 2016?" The Penalty Researcher finds reg.38(6): "on conviction on indictment, to a fine or imprisonment for a term not exceeding 12 months, or both." It also surfaces related sentencing guidelines and precedent penalty ranges from enforcement data. |

### 5.9 Point-in-Time Law Viewer

| Attribute | Detail |
|-----------|--------|
| **One-liner** | Shows the state of a law as it was in force at a specific date, resolving all amendments and commencements. |
| **Runs on** | Both |
| **Tier** | Standard |
| **WIT imports** | `fractal:data/query`, `fractal:audit/log` |
| **AI model** | None (pure data resolution using amendment annotations and commencement data) |
| **Data slice** | Target law's text + all amendment_annotations + commencement data + law_edges (amending/amended_by) |
| **User story** | An investigator is reviewing a workplace accident that occurred on 15 March 2019. They need to know what version of the Construction (Design and Management) Regulations 2015 was in force on that date. The Point-in-Time Viewer resolves all amendments up to that date, shows which sections had commenced, which had been amended by subsequent SIs, and presents the text as it would have read on 15 March 2019. This is critical for enforcement -- you prosecute based on the law as it was at the time of the offence. |

### 5.10 Obligation Tracker

| Attribute | Detail |
|-----------|--------|
| **One-liner** | Extracts time-bound obligations from legislation (deadlines, review periods, reporting dates) and tracks their status. |
| **Runs on** | Hub (extraction), Edge (tracking/alerts) |
| **Tier** | Standard |
| **WIT imports** | `fractal:data/query`, `fractal:ai/inference`, `fractal:data/mutate`, `fractal:events/emit`, `fractal:audit/log` |
| **AI model** | Generative (hub-side extraction), none (edge-side tracking) |
| **Data slice** | Legislation text for obligations with temporal language; site-specific obligation records on edge |
| **User story** | The Environmental Permitting Regulations require permit holders to submit annual monitoring reports by 31 January. The Obligation Tracker (hub-side) extracts this deadline from the legislation text. The edge-side tracker, synced to the site's device, shows: "Annual EPR monitoring report due 31 January 2027 -- 45 days remaining. Status: draft in progress." It emits reminder events at 60, 30, and 7 days. |

### 5.11 Audit Checklist Generator

| Attribute | Detail |
|-----------|--------|
| **One-liner** | Generates a structured audit checklist from applicable legislation for a specific site and scope. |
| **Runs on** | Both (hub for generation, edge for use in the field) |
| **Tier** | Standard |
| **WIT imports** | `fractal:data/query`, `fractal:ai/inference`, `fractal:audit/log` |
| **AI model** | Generative (hub-side to produce checklist items from legislation text) |
| **Data slice** | DRRP-annotated legislation for the site's applicable families + previous audit findings |
| **User story** | An inspector is preparing for an OH&S audit at a manufacturing site. They select the scope: "Occupational Health and Safety -- machinery guarding, manual handling, noise." The Checklist Generator queries the DRRP-annotated provisions for the relevant regulations (Provision and Use of Work Equipment Regulations, Manual Handling Operations Regulations, Control of Noise at Work Regulations), and produces a structured checklist: "1. PUWER reg.11 -- Are all dangerous parts of machinery adequately guarded? [duty holder: employer] 2. MHOR reg.4 -- Has a suitable and sufficient assessment been made? [duty holder: employer]..." The inspector takes this checklist to the field on their tablet. |

### 5.12 Risk Scorer

| Attribute | Detail |
|-----------|--------|
| **One-liner** | Computes a risk score for a site based on compliance gaps, incident history, regulatory change exposure, and enforcement history. |
| **Runs on** | Hub |
| **Tier** | Standard |
| **WIT imports** | `fractal:data/query`, `fractal:ai/classify`, `fractal:audit/log` |
| **AI model** | ONNX classifier (risk category classification) |
| **Data slice** | Full site compliance records, incident history, enforcement actions, applicable legislation |
| **User story** | A regional EHS director manages 30 sites. They need to prioritise audit resources. The Risk Scorer computes a weighted score for each site: Site 7 scores 82/100 (high risk) because it has 3 open compliance gaps in COMAH regulations, 2 enforcement notices in the last 12 months, and recent regulatory changes to the Control of Major Accident Hazards Regulations that it hasn't responded to. Site 15 scores 23/100 (low risk) because all gaps are closed and no regulatory changes affect it. The director schedules audits accordingly. |

### 5.13 Elixir-to-Fractalaw Bridge

| Attribute | Detail |
|-----------|--------|
| **One-liner** | Transforms and ingests data from the sister Elixir app's Postgres exports (LRT, LAT, amendments) into fractalaw's DuckDB and LanceDB stores. |
| **Runs on** | Hub |
| **Tier** | Heavy |
| **WIT imports** | `fractal:data/mutate`, `fractal:events/emit`, `fractal:audit/log` |
| **AI model** | None (data transformation only) |
| **Data slice** | Postgres exports (Parquet or Arrow IPC) from the Elixir app |
| **User story** | The Elixir app has completed a scraping run and exported 500 new or updated laws to Parquet. The bridge micro-app reads these exports, validates them against the Arrow schemas in `fractalaw-core`, transforms column names and types as needed, inserts into DuckDB (legislation, law_edges) and LanceDB (legislation_text), and emits a `data-ingested` event listing the affected law names. Downstream micro-apps (Centroid Trainer, Regulatory Change Monitor, DRRP Polisher) react to this event. |

### 5.14 Fractalaw-to-Elixir Bridge

| Attribute | Detail |
|-----------|--------|
| **One-liner** | Exports AI-enriched data (classifications, polished DRRP, embeddings metadata) back to the Elixir app's Postgres for the web UI. |
| **Runs on** | Hub |
| **Tier** | Standard |
| **WIT imports** | `fractal:data/query`, `fractal:events/emit`, `fractal:audit/log` |
| **AI model** | None |
| **Data slice** | Classified legislation, polished DRRP entries, classification confidence scores |
| **User story** | The web UI served by the Elixir app needs to display AI-generated classifications and polished DRRP alongside the manually curated data. The Fractalaw-to-Elixir Bridge queries fractalaw for all laws where `classification_status` changed since the last sync, exports the `classified_domain`, `classified_family`, `classified_subjects`, `classification_confidence`, and polished DRRP columns as Arrow IPC, and writes them to Postgres. The Elixir app's web UI now shows "AI-suggested: ENERGY (confidence: 0.91)" next to manual classifications. |

### 5.15 Regulatory Feed Ingester

| Attribute | Detail |
|-----------|--------|
| **One-liner** | Pulls new legislation from external regulatory feeds (legislation.gov.uk Atom, EUR-Lex SPARQL) and normalises it into the fractalaw schema. |
| **Runs on** | Hub |
| **Tier** | Standard |
| **WIT imports** | `fractal:data/mutate`, `fractal:events/emit`, `fractal:audit/log` |
| **AI model** | None (or lightweight classifier for initial family tagging) |
| **Data slice** | External feed data (inbound) |
| **User story** | The legislation.gov.uk Atom feed publishes a new SI. The Regulatory Feed Ingester fetches the Atom entry, downloads the XML legislation text, parses it into the fractalaw legislation and legislation_text schemas, and inserts it into the stores. This provides a direct-from-source pipeline complementing the Elixir app's more comprehensive scraping. Useful for near-real-time monitoring of new publications without waiting for the Elixir app's batch cycle. |

### 5.16 Amendment Impact Mapper

| Attribute | Detail |
|-----------|--------|
| **One-liner** | Traces the chain of amendments from a new SI back through all affected primary and secondary legislation, producing an impact map. |
| **Runs on** | Hub |
| **Tier** | Standard |
| **WIT imports** | `fractal:data/query`, `fractal:events/emit`, `fractal:audit/log` |
| **AI model** | None (graph traversal using law_edges) |
| **Data slice** | law_edges (1M+ rows), legislation table |
| **User story** | A new amending SI is published: "The Environmental Permitting (England and Wales) (Amendment) Regulations 2026." The Amendment Impact Mapper walks the law_edges graph: this SI amends EPR 2016 regs 12, 35, and Schedule 5. EPR 2016 is itself an amending instrument that consolidated earlier permitting regulations. The mapper produces a full impact tree showing every affected provision, which sites hold permits under those provisions, and what the amendment changes. The compliance team knows exactly what to review. |

### 5.17 Due Diligence Scanner

| Attribute | Detail |
|-----------|--------|
| **One-liner** | Scans a target company's regulatory footprint for M&A due diligence -- identifies all applicable legislation, enforcement history, and compliance risk. |
| **Runs on** | Hub |
| **Tier** | Heavy |
| **WIT imports** | `fractal:data/query`, `fractal:ai/embeddings`, `fractal:ai/classify`, `fractal:ai/inference`, `fractal:audit/log` |
| **AI model** | Full suite -- embeddings, classifier, generative |
| **Data slice** | Full legislation corpus + enforcement/penalty data + industry sector mappings |
| **User story** | A company is acquiring a waste management firm. The Due Diligence Scanner takes the target's SIC code (waste management), geographic locations, and known permits as input. It queries all applicable legislation (WASTE family, ENVIRONMENTAL PROTECTION, OH&S), identifies the target's enforcement history (if available in the data), flags high-risk regulatory areas (e.g., COMAH upper-tier status, contaminated land liabilities), and generates a due diligence report: "Target is subject to 47 pieces of primary and secondary legislation. 3 enforcement notices in the last 5 years. High-risk area: landfill aftercare obligations under EPA 1990 s.61." |

### 5.18 Training Content Generator

| Attribute | Detail |
|-----------|--------|
| **One-liner** | Generates bite-sized training materials from legislation for workforce EHS onboarding and refresher training. |
| **Runs on** | Hub (generation), Edge (delivery) |
| **Tier** | Standard |
| **WIT imports** | `fractal:data/query`, `fractal:ai/inference`, `fractal:audit/log` |
| **AI model** | Generative (8B) for summarisation and plain-language rewriting |
| **Data slice** | DRRP-annotated legislation for specific roles (e.g., "duties of employees" across all applicable regulations) |
| **User story** | A new employee starts at a food manufacturing plant. The Training Content Generator takes their role ("production line operator") and the site's applicable legislation (Food Safety Act, HACCP Regulations, COSHH, MHOR) and generates role-specific training cards: "As a production line operator, you have a legal duty under COSHH reg.7 to use control measures provided by your employer. This means: [plain language explanation]. If you notice a control measure is not working, you must report it to your supervisor." Each card cites the specific legislation and DRRP provision. |

### 5.19 Enforcement Pattern Analyser

| Attribute | Detail |
|-----------|--------|
| **One-liner** | Analyses enforcement action data to identify patterns -- which regulations are most frequently breached, which industries, what penalty ranges. |
| **Runs on** | Hub |
| **Tier** | Standard |
| **WIT imports** | `fractal:data/query`, `fractal:ai/classify`, `fractal:audit/log` |
| **AI model** | ONNX classifier (enforcement action categorisation) |
| **Data slice** | Enforcement action records + legislation table + industry/site data |
| **User story** | A regulator wants to understand enforcement trends. The Enforcement Pattern Analyser queries 5 years of enforcement data, classifies actions by regulation family, and produces analysis: "COSHH breaches account for 23% of all enforcement notices in the chemical sector. The most common breach is reg.7 (failure to ensure adequate control of exposure). Penalty range: GBP 5,000-50,000. Trend: increasing 12% year-on-year." This informs both regulatory strategy and industry compliance prioritisation. |

### 5.20 Supply Chain Compliance Mapper

| Attribute | Detail |
|-----------|--------|
| **One-liner** | Maps regulatory obligations that flow through supply chains -- where a duty on one party creates requirements on suppliers or contractors. |
| **Runs on** | Hub |
| **Tier** | Standard |
| **WIT imports** | `fractal:data/query`, `fractal:ai/inference`, `fractal:ai/embeddings`, `fractal:audit/log` |
| **AI model** | Generative (for extracting supply chain obligation chains from legislation text) |
| **Data slice** | DRRP-annotated legislation, particularly duty_holder and role columns |
| **User story** | A construction company needs to understand its supply chain obligations. CDM 2015 places duties on clients, designers, and principal contractors. The Supply Chain Compliance Mapper traces these duty chains: "As principal contractor (CDM reg.13), you must ensure that every contractor is informed of the minimum amount of time for planning and preparation. This means your subcontractors must receive: [list]. Your designers must provide: [list]. Your client must have ensured: [list]." It maps the web of obligations flowing through the supply chain hierarchy. |

### 5.21 Permit Register

| Attribute | Detail |
|-----------|--------|
| **One-liner** | Maintains a register of all permits/licences/consents applicable to a site, linked to the granting legislation and renewal dates. |
| **Runs on** | Edge |
| **Tier** | Lightweight |
| **WIT imports** | `fractal:data/query`, `fractal:data/mutate`, `fractal:events/emit`, `fractal:audit/log` |
| **AI model** | None |
| **Data slice** | Site-specific permit records + applicable legislation partition |
| **User story** | The site manager at a quarry opens the Permit Register on their tablet. It shows: "1. Planning Permission (TCPA 1990 s.57) -- expires 2028-06-30 -- status: active. 2. Environmental Permit (EPR 2016) -- annual review due 2026-03-15 -- status: review pending. 3. Explosives Certificate (ER 2014) -- expires 2026-09-01 -- status: active." The register links each permit to the specific legislation that grants and governs it. When a regulation changes, the Regulatory Change Monitor emits an event and the Permit Register flags affected permits. |

### 5.22 Section Summariser

| Attribute | Detail |
|-----------|--------|
| **One-liner** | Generates plain-language summaries of individual legislation sections, making legal text accessible to non-lawyers. |
| **Runs on** | Hub (batch generation), Edge (lookup of pre-generated summaries) |
| **Tier** | Standard (hub), Lightweight (edge) |
| **WIT imports** | `fractal:data/query`, `fractal:ai/inference` (hub), `fractal:data/mutate`, `fractal:audit/log` |
| **AI model** | Generative (hub-side, 8B) for summarisation |
| **Data slice** | legislation_text sections |
| **User story** | A site manager needs to understand reg.12 of the Control of Noise at Work Regulations 2005. The original text is dense legalese. The Section Summariser provides: "This regulation requires employers to keep health records for employees exposed to noise above the upper exposure action value (85 dB). Records must be kept for at least 40 years. Employees have a right to see their records." The summary is pre-generated on the hub and synced to the edge alongside the original text. |

---

## 6. Composition Patterns

Micro-apps compose in three ways: static composition (build-time wiring via `wasm-tools compose`), host-mediated sequencing (the router calls apps in order), and event-driven chaining (one app emits an event, others react). The most powerful workflows combine all three.

### Pipeline 1: New Legislation Ingestion and Enrichment

```
Elixir App (scrape/parse)
  |
  v
[Elixir-to-Fractalaw Bridge]      -- transforms Postgres export to Arrow, inserts into DuckDB/LanceDB
  |
  | emits: data-ingested { law_names: [...] }
  |
  +---> [Regulatory Change Monitor]  -- classifies new laws, identifies affected sites
  |       | emits: regulatory-change { affected_laws: [...], affected_sites: [...] }
  |       |
  |       +---> [Compliance Gap Analyser]  -- re-evaluates gaps for affected sites
  |       |       | emits: compliance-gap { site_id, gaps: [...] }
  |       |       |
  |       |       +---> [Risk Scorer]  -- updates risk scores for affected sites
  |       |
  |       +---> [Obligation Tracker]  -- extracts new deadlines from changed legislation
  |
  +---> [DRRP Polisher]             -- polishes DRRP for newly ingested "making" laws
  |       | writes: polished DRRP to legislation table
  |
  +---> [Centroid Trainer]           -- recomputes centroids if enough new confirmed labels exist
  |
  +---> [Fractalaw-to-Elixir Bridge] -- exports enriched data back to Postgres for web UI
```

This pipeline is entirely event-driven. The bridge emits `data-ingested` and four independent micro-apps react. Each downstream app may emit its own events, creating a cascade. The host's Scheduler ensures ordering where needed (e.g., DRRP Polisher must complete before Fractalaw-to-Elixir Bridge exports DRRP data).

### Pipeline 2: Field Audit Workflow

```
Auditor arrives at site (edge device)
  |
  v
[Audit Checklist Generator]        -- generates checklist from applicable legislation + DRRP
  |
  | produces: structured checklist (Arrow batch)
  |
  v
[Field Research Tool]              -- auditor researches specific questions during inspection
  |
  | (used ad-hoc throughout the audit)
  |
  v
[Incident Classifier]             -- if a non-compliance is found, classify it immediately
  |
  | emits: incident-recorded { classification, severity, legislation_ref }
  |
  v
[Permit Register]                 -- auditor cross-checks permit status for findings
  |
  v
(Audit complete -- device syncs to hub)
  |
  v
[Compliance Gap Analyser]         -- hub re-evaluates site's compliance based on audit findings
  |
  v
[Risk Scorer]                     -- hub updates site risk score
```

This pipeline starts on the edge and finishes on the hub. The auditor uses edge micro-apps in sequence during the inspection. When the device syncs, hub micro-apps process the field-captured data. The key property: the edge portion works entirely offline.

### Pipeline 3: Due Diligence Report Generation

```
Analyst inputs: target company profile (SIC codes, locations, known permits)
  |
  v
[Due Diligence Scanner]                -- identifies all applicable legislation
  |
  | produces: applicable_legislation (Arrow batch), risk_flags (Arrow batch)
  |
  v (static composition -- these three are wasm-tools composed into one component)
  +---> [Penalty Researcher]            -- finds penalty provisions for flagged regulations
  +---> [Enforcement Pattern Analyser]  -- identifies enforcement trends for target's sector
  +---> [Cross-Jurisdiction Comparator] -- if target operates in multiple jurisdictions
  |
  | combined output: due_diligence_data (Arrow batch)
  |
  v
[Report Generator]                     -- (not in the brainstorm above, but the standard
  |                                        report-generator from the fractal-plan.md)
  v
PDF/HTML due diligence report
```

The middle three micro-apps are statically composed into a single WASM component. Internal calls between them are direct function calls with no host round-trip. The composed component receives the applicable legislation list from the Due Diligence Scanner and produces a combined output for the Report Generator. This is the `wasm-tools compose` pattern from Section 3.4.8 of the architecture document -- stable pipelines that rarely change are composed at build time.

---

## 7. What This Means for Phase 3

### Priority Micro-Apps for Phase 3

Based on the brainstorm, these micro-apps should be built first because they exercise the most critical host runtime capabilities, have immediate value, and progressively validate the architecture:

| Priority | Micro-App | Why First |
|----------|-----------|-----------|
| 1 | **Elixir-to-Fractalaw Bridge** | Validates `fractal:data/mutate` and `fractal:events/emit`. Replaces the current manual `import` CLI command. Exercises the WASM ↔ Arrow IPC data path. This is the foundational data flow -- everything else depends on data getting in. |
| 2 | **Field Research Tool** | Validates `fractal:data/query` and `fractal:ai/embeddings` on the edge. The simplest AI-using micro-app: embed a query, search, return results. Proves the ONNX-in-WASM-sandbox model works. |
| 3 | **DRRP Polisher** | Validates `fractal:ai/inference` (generative). The user's explicit use case. Exercises the generative model host function. Produces immediately useful output that the Elixir web UI can consume. |
| 4 | **Incident Classifier** | Validates `fractal:ai/classify`. Lightweight tier -- tests the smallest micro-app profile. Useful on both hub and edge -- validates cross-deployment. |
| 5 | **Regulatory Change Monitor** | Validates `fractal:events/emit` and event-driven composition. First app that triggers other apps. Proves the event routing system works. |

### Host Runtime Capabilities Required

The Phase 3 `fractalaw-host` implementation needs these capabilities in order of the micro-app priorities above:

| Capability | Required By | Description |
|------------|-------------|-------------|
| **Wasmtime Engine + pooling allocator** | All | Basic WASM execution. Priority 1 blocker. |
| **`fractal:data/query` host function** | Field Research, DRRP Polisher, all query-using apps | Bridges WIT `query(sql) -> list<u8>` to DataFusion. Arrow IPC serialisation on the host side. |
| **`fractal:data/mutate` host function** | Bridge, DRRP Polisher | Bridges WIT `insert(table, data) -> u64` to DuckDB/LanceDB writes. |
| **`fractal:ai/embeddings` host function** | Field Research, Incident Classifier | Bridges WIT `embed(text) -> list<f32>` to the ONNX runtime in `fractalaw-ai`. |
| **`fractal:ai/classify` host function** | Incident Classifier, Regulatory Change Monitor | Bridges WIT `classify(text, categories) -> list<classification>` to the centroid classifier. |
| **`fractal:ai/inference` host function** | DRRP Polisher | Bridges WIT `generate(request) -> response` to a generative model (requires MLC LLM or llama.cpp integration). |
| **`fractal:events/emit` host function** | Regulatory Change Monitor, Bridge | Bridges WIT `emit(event) -> event-id` to the event bus. Enables micro-app composition. |
| **`fractal:audit/log` host function** | All | Bridges WIT `record-event(entry)` to the append-only audit log. |
| **Fuel metering + tier enforcement** | All | `ResourceLimiter` implementation for lightweight/standard/heavy tiers. |
| **AOT compilation + caching** | All (performance) | `.cwasm` pre-compilation for fast instantiation. |
| **App Supervisor basics** | All (lifecycle) | Load, start, stop, health check. Hot-swap can come later. |

### Suggested Phase 3 Session Sequence

1. **Session 1: Wasmtime bootstrap** -- Engine configuration, pooling allocator, fuel metering, basic component loading. Get a "hello world" WASM component running with `fractal:audit/log` as the first host function.

2. **Session 2: Data host functions** -- Implement `fractal:data/query` and `fractal:data/mutate` host functions bridging to DataFusion/DuckDB/LanceDB. Build the Elixir-to-Fractalaw Bridge as the first real micro-app.

3. **Session 3: AI host functions** -- Implement `fractal:ai/embeddings` and `fractal:ai/classify` host functions bridging to `fractalaw-ai`. Build the Field Research Tool and Incident Classifier.

4. **Session 4: Generative AI + events** -- Implement `fractal:ai/inference` (requires generative model integration) and `fractal:events/emit`. Build the DRRP Polisher and Regulatory Change Monitor. Prove event-driven composition works.

5. **Session 5: App Supervisor** -- Registry (local OCI or filesystem), Lifecycle Manager, basic Router. Hot-swap. The infrastructure to manage a fleet of micro-apps rather than loading them ad-hoc.

This sequence means each session delivers a working micro-app while progressively building out the host runtime. By the end of Phase 3, the system has 5 running micro-apps exercising all 4 WIT packages, proving the architecture end-to-end.
