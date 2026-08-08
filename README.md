# graphify-plugin-opendoc

[繁體中文 (Traditional Chinese)](README.zh-TW.md)

A Graphify **embedded plugin** that bridges the code knowledge graph with the OpenDocuments vector store: bidirectional retrieval between code and unstructured documents (doc → code / code → doc), implemented as a native Rust crate that implements the `GraphifyPlugin` trait and integrates directly with Graphify Core.

## Key Features

- **Embedded, not a separate server**: Ships as a single Rust crate that Graphify Core embeds and loads at startup. No stdio JSON-RPC process, no extra binary to deploy, no external IPC handlers.
- **Zero Mock**: All vector retrieval goes directly against the real OpenDocuments Rust SDK / Storage Layer; all AST endpoints go against the real in-memory semantic tree (Petgraph) in Graphify. No simulated or fabricated data.
- **Dual-Key Alignment**:
  - `workspace_key`: Graphify's routing key for the local AST graph (per the `graphify-core` v1 `WorkspaceContext`), constraining the BFS trace to the current project's AST nodes.
  - OpenDocuments workspace UUID: used as a hard filter (`doc_meta.workspace_uuid == <uuid>`) on every vector / storage query so retrieval only returns documents of the current workspace.
- **Hybrid Retrieval**: business-intent queries return vector evidence plus a compressed `.toon` subgraph of the impacted code; symbol lookups return the related unstructured document context.
- **Workspace-aligned with the plugin ecosystem**: Plugins (handoff, opendoc, review, …) are aligned by `workspace_key` injected by Graphify — no per-plugin walk-up, no divergent root discovery.

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

No standalone server configuration is required. Graphify Core depends on this crate, loads it as a plugin, and GraphifyMCP registers the retrieval tools at startup. Configuration is fully dynamic and relative — no environment-level secrets, no hardcoded paths.

## Architecture Design

See the original spec draft (`SPEC.md`) and detailed requirements, specifications, and architecture decisions in the `openspec/` directory. The Graphify plugin contract (`GraphifyPlugin` trait, `WorkspaceContext`) is defined in Graphify Core and coordinated with the GraphifyRust project.

## License

MIT
