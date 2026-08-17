# graphify-plugin-opendoc

[繁體中文 (Traditional Chinese)](README.zh-TW.md)

A Graphify **embedded plugin** that bridges the code knowledge graph with
unstructured documents: bidirectional traceability between spec blocks and code
symbols (doc → code / code → doc), plus bidirectional drift auditing. Implemented
as a native Rust crate that implements the `GraphifyPlugin` trait and integrates
directly with Graphify Core.

Layer 2 vector search connects to [**OpenDocuments**](https://github.com/cawa0505/OpenDocuments)
(via its REST API at `POST /api/v1/search`); see
[`OpenDocuments/docs/OpenDocuments-Requirements.md`](https://github.com/cawa0505/OpenDocuments/blob/main/docs/OpenDocuments-Requirements.md)
for the cross-repo contract.

## Why a Plugin (Not Just MCP)

Pure MCP lets an Agent query documents and code separately, but the results are
isolated — the Agent must burn Context Window to stitch them together. This
plugin provides three cross-domain capabilities MCP cannot:

1. **Cross-Domain Subgraph Binding**: Markdown headers/sections become Spec
   nodes on the AST graph, connected via `implements_spec` edges. The Agent
   gets a unified micro-graph containing both spec logic and code dependencies.
2. **Bidirectional Drift Detection**: doc-side (file signature vs indexed
   signature) + code-side (symbol exists in AST graph?). Catches both "code
   changed but doc is stale" and "doc added a spec but code has no
   implementation".
3. **Deterministic & Hybrid Search**: hard links (`# Symbol: <name>` /
   `@spec:<path>`) give 100% deterministic match (0ms, no vector). Soft vector
   search is fallback only when hard links are absent.

## Architecture (Two Layers)

**Layer 1 — plugin-owned domain (zero external dependency, implementable now)**

- pulldown-cmark Markdown AST parsing → spec blocks.
- Hard links (`# Symbol: <name>` / `@spec:<path>`): 100% deterministic text
  match — 0ms, zero false positives, no vector search needed.
- SQLite link registry (doc ↔ code symbol + workspace mapping, via
  `graphify-registry`).
- Query API: doc → code symbols, symbol → spec blocks.
- Bidirectional drift audit: doc-side (signature comparison) + code-side
  (graph query via `sync_toon` + `query_bfs`).
- `sync_toon`: cross-session link index exchange.

**Layer 2 — vector soft search (trait interface, NoOp fallback)**

- `SpecSearchBackend` trait (pure Rust interface).
- Current implementation: `NoOpBackend` (returns empty; hard links take
  priority).
- `RestBackend`: built into the plugin, calls the OD REST API directly via
  `ureq` (`POST /api/v1/search`, `X-Workspace` header). Enabled when an OD
  base URL is set; falls back to `NoOpBackend` otherwise. Not a path dep on
  `opendoc-storage` (due to `libsqlite3-sys` version conflict between
  `sqlx 0.7` and `rusqlite 0.32`).
- Workspace mapping: manually configured, stored in plugin SQLite.

## MCP Efficiency Layer

The plugin core engine handles parsing, hard-link computation, registry, and
drift audit. The MCP layer exposes 2–3 minimal APIs for Agent queries:

| MCP Tool | Plugin API | Description |
|----------|------------|-------------|
| `opendoc_get_context` | `fetch_code_to_doc_context` | Return the most relevant spec block for a code node |
| `opendoc_audit_drift` | `audit_drift` | Check project-wide spec ↔ code drift |
| `opendoc_index` | `index_docs` | Index documents (extract hard links into registry) |

MCP tools are auto-registered by graphify-mcp at startup (same pattern as the
handoff plugin).

## Embedded, Not a Separate Server

Ships as a single Rust crate that Graphify Core embeds and loads at startup. No
stdio JSON-RPC process, no extra binary to deploy, no external IPC handlers.

## Developer & Verification Commands

```bash
# Build the project
cargo build

# Run quality checks
cargo check
cargo clippy

# Run unit tests
cargo test
```

## Setup

No standalone server configuration is required. Graphify Core depends on this
crate, loads it as a plugin. Configuration is fully dynamic and relative — no
environment-level secrets, no hardcoded paths.

## Architecture Design

See `openspec/changes/opendoc-native-plugin/design.md` for the implementation
spec. `SPEC.md` is a historical draft (superseded). The Graphify plugin contract
(`GraphifyPlugin` trait, `WorkspaceContext`) is defined in Graphify Core and
coordinated with the GraphifyRust project.

## License

MIT