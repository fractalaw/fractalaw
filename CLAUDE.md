# Fractalaw

Local-first fractal architecture for ESH (environment, safety, health) regulatory data.

## Project Structure

Rust workspace monorepo with 6 crates:

- `fractalaw-core` — Arrow schemas, shared types (pure Rust, no optional deps)
- `fractalaw-store` — DuckDB, LanceDB, DataFusion integration (feature-gated)
- `fractalaw-ai` — ONNX Runtime embeddings/classification (feature-gated)
- `fractalaw-sync` — Arrow Flight sync, Lance delta sync, Loro CRDTs (flight feature-gated)
- `fractalaw-host` — Wasmtime WASI Component Model runtime
- `fractalaw-cli` — Binary entry point

WIT interfaces live in `/wit/` (fractal:data, fractal:ai, fractal:events, fractal:audit).

## Build

```bash
# Default build (pure Rust only — no C toolchain needed)
cargo check --workspace
cargo test --workspace

# With heavy native deps (requires gcc/g++ and system libs)
cargo check -p fractalaw-store --features full
cargo check -p fractalaw-ai --features onnx
cargo check -p fractalaw-sync --features flight
```

## Feature Gates

Heavy C/C++ dependencies are behind optional features to keep the default build pure Rust:

| Crate | Feature | Dependencies |
|-------|---------|-------------|
| fractalaw-store | `duckdb` | duckdb (bundled C++) |
| fractalaw-store | `lancedb` | lancedb |
| fractalaw-store | `datafusion` | datafusion |
| fractalaw-store | `full` | all of the above |
| fractalaw-ai | `onnx` | ort (ONNX Runtime) |
| fractalaw-sync | `flight` | arrow-flight, tonic, prost |

## Conventions

- Edition 2024, resolver v2
- License: AGPL-3.0-or-later
- Arrow is the universal in-memory format — all data exchange uses Arrow RecordBatch
- Error handling: `thiserror` for library errors, `anyhow` for application/CLI errors
- Async runtime: tokio
- Logging: tracing
- Tests live next to source (`#[cfg(test)] mod tests`)

## Environment

- OS: Fedora Bluefin DX (atomic/immutable Linux)
- Rust: installed via rustup (userspace)
- WASM: wasm32-wasip1 + wasm32-wasip2 targets, cargo-component, wasm-tools
- C/C++ tools: brew (gcc, cmake, protobuf) — only needed for feature-gated deps
- IDE: Zed (Flatpak)
