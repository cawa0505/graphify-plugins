---
name: graphify-opendoc
description: Doc↔code bidirectional traceability and drift detection via graphify (CLI) or graphify opendoc* MCP tools. Use when indexing markdown spec blocks, tracing which code symbols a doc covers, auditing document drift after code changes, or mapping a workspace to OpenDocuments.
---

# OpenDocuments — Skill (graphify-opendoc)

## 1. Summary — dual-track principle

Doc↔code traceability hands the workspace a bidirectional link registry via a single contract:

- **Links**: `# Symbol: <name>` hard-link annotations inside markdown spec blocks (`@spec:<path>` for back-references)
- **Two channels**, same operations:
  - **MCP tools** (`graphify_opendoc_index` / `graphify_opendoc_get_context` / `graphify_opendoc_audit_drift`) — the efficiency layer, when registered
  - **CLI** (`graphify opendoc ...`) — the resilience layer, always available
- **Direct SQLite reads** — zero-dependency introspection (read-only)

Layer 1 (hard links) has **zero OpenDocuments dependency**; Layer 2 (soft vector search) requires a workspace mapping plus an injected `SpecSearchBackend` (NoOp until OpenDocuments ships real search — see `design.md`). The CLI is the canonical fallback: if MCP is missing, erroring, or timing out, every opendoc operation still works through `graphify`. This skill maps **intent → command**. If this skill and the CLI disagree, **trust the CLI and `design.md`**.

## 2. Channel decision table

| Situation | Use |
|---|---|
| `opendoc*` MCP tools registered | MCP tools (fastest) |
| MCP error / timeout / not registered | CLI fallback: `graphify opendoc ...` |
| Neither available | Direct SQLite read (section 5) |

## 3. Quick start

```bash
which graphify          # CLI present? (installed via cargo install or GraphifyRust build)
graphify opendoc index                    # index all .md spec blocks under cwd
graphify opendoc trace-code crate::module::symbol  # which spec covers this code symbol?
graphify opendoc audit-drift              # re-read docs, compare block signatures
```

If `graphify` is missing, build GraphifyRust and add it to `PATH`. To install this skill into the local agent ecosystem:

```bash
graphify opendoc skill install            # detect + install for all found agents
graphify opendoc skill install --agent opencode --scope user
graphify opendoc skill uninstall
```

## 4. Operations

Each operation lists the MCP tool (where one exists), the CLI equivalent, the frozen return shape, and failure semantics.

### 4.1 Index

- **MCP**: `graphify_opendoc_index` — params: optional `doc_paths` (array of relative paths; if omitted, all `.md` under workspace root)
- **CLI**: `graphify opendoc index [--doc-paths a.md,b.md]`

**Return**:

```
[opendoc] indexed: <n> link rows
[opendoc] indexed all docs under workspace root: <n> link rows   (when no doc_paths given)
```

**Notes**:
- Reindexes: all rows for this `workspace_key` are replaced.
- Only `.md` files are scanned; `# Symbol: <name>` headings drive extraction.

### 4.2 Trace-doc (CLI-only)

- **CLI**: `graphify opendoc trace-doc <doc_path>`
- **MCP**: none — trace-doc has no MCP tool within the 3 registered tools.

**Return**: tab-separated rows `<spec_hash>\t<symbol>\t<doc_path>`, one per indexed hard link belonging to that doc.

**Failure**: empty output if the doc has no indexed symbols or the doc is not registered.

### 4.3 Trace-code

- **MCP**: `graphify_opendoc_get_context` — params: `symbol` (qualified string, e.g. `crate::auth::verify_token`)
- **CLI**: `graphify opendoc trace-code <symbol>`

**Return**: tab-separated rows `<spec_hash>\t<symbol>\t<doc_path>`.

**Notes**:
- Searches Layer 1 (hard links) first; if none, checks Layer 2 (workspace mapping set + injected backend → vector search).
- Layer 2 is NoOp by default (returns empty).

### 4.4 Audit-drift

- **MCP**: `graphify_opendoc_audit_drift` — no params.
- **CLI**: `graphify opendoc audit-drift`

**Return**: per-line `<spec_hash>\t<symbol>\t<doc_path>\t<UpToDate|DocChanged|DocMissing>`, preceded by header `[opendoc] <n> drift item(s):` when non-empty. If zero indexed links:

```
[opendoc] no indexed links to audit
```

**Notes**:
- `DocMissing` when the file was removed.
- `DocChanged` when the block signature (sha1 of block content) differs from the indexed one.
- `UpToDate` otherwise.

### 4.5 Audit-missing (doc→code, CLI-only)

- **CLI**: `graphify opendoc audit-missing --symbols crate::auth::login,crate::auth::verify_token`

**Return**: per-line `<spec_hash>\t<symbol>\t<doc_path>\t<CodeMissing>` for each indexed hard link whose symbol is not in the supplied `--symbols` list, preceded by header `[opendoc] <n> missing item(s):`.

**Notes**:
- `--symbols` is a comma-separated list of known-implemented symbols (the caller derives this from graphify-core's graph).
- MCP has no equivalent within the 3 registered tools (Layer 1 needs the external known-symbols list).
- `CodeMissing` when an indexed hard link's symbol is absent from that list.

### 4.6 Set-mapping (Layer 2 setup, CLI-only)

- **CLI**: `graphify opendoc set-mapping <od_workspace_id>`

**Return**:

```
[opendoc] workspace <workspace_key> → od_workspace_id "<od_workspace_id>"
```

**Notes**:
- Stores the Graphify `workspace_key` → OpenDocuments `workspace_id` (TEXT, not UUID) in the `opendoc_workspace_mapping` SQLite table.
- Needed for Layer 2 soft search; Layer 1 ignores this.

### 4.7 Get-mapping (CLI-only)

- **CLI**: `graphify opendoc get-mapping`

**Return**: `[opendoc] <workspace_key> → <od_workspace_id>` or `[opendoc] no workspace mapping set`.

## 5. Zero-dependency introspection (read state without tools)

If neither MCP nor the CLI is available, read the registry directly — it is plain SQLite:

- DB: `~/.local/share/graphify/graphify.db` (XDG default)
- Table `opendoc_links` — columns `workspace_key`, `spec_id`, `doc_path`, `symbol`, `signature` (all TEXT; pk = `spec_id`).
- Table `opendoc_workspace_mapping` — `workspace_key` (PK), `od_workspace_id` (TEXT).

Query with `sqlite3` one-liners (read-only), e.g.:

```bash
sqlite3 ~/.local/share/graphify/graphify.db \
  "SELECT spec_id, symbol, doc_path FROM opendoc_links WHERE workspace_key = '<key>';"
```

**Read-only rule**: direct DB access is for introspection only. **Never mutate** the registry by hand — writes go through the CLI or MCP. Layer 1 hard links live entirely in this SQLite; there is no separate JSON file (unlike relay.json in the handoff skill).

## 6. Verification checklist

Run once against a scratch directory (CLI path):

```bash
mkdir -p /tmp/opendoc-smoke && cd /tmp/opendoc-smoke
cat > test.md <<'EOF'
# Token spec
# Symbol: crate::auth::verify_token
EOF
graphify opendoc index
graphify opendoc audit-drift
graphify opendoc trace-code crate::auth::verify_token
```

Must verify: index returns `1 link rows`; drift returns `UpToDate`; trace returns the tab-row. MCP path: the same cycle through `graphify_opendoc_index` → `graphify_opendoc_audit_drift` → `graphify_opendoc_get_context`.

## 7. Installing in other agents

`graphify opendoc skill install` detects the local agent ecosystem and installs this skill (self-contained copy of this file, tagged with a managed marker so `uninstall` never removes user-created files):

| Agent | Target |
|---|---|
| opencode | `~/.config/opencode/skills/graphify-opendoc/SKILL.md` (user) or cwd `.opencode/skills/graphify-opendoc/SKILL.md` (project) |
| Claude | `~/.claude/skills/graphify-opendoc/SKILL.md` (user; project reuses repo-root SKILL.md) |
| Cursor | `.cursor/rules/graphify-opendoc.mdc` (managed copy) |
| Cline | `.clinerules` (managed copy) |
| Project | `.opencode/skills/graphify-opendoc/SKILL.md` |

In-repo (this repository): Claude Code auto-discovers the repo-root `SKILL.md`; opencode loads it via the project or global install above.

## 8. Error handling & resilience

- Tool results are returned as-is; frozen error texts are preserved.
- The plugin never panics — errors are returned as text.
- MCP: a tool error result still becomes the agent response. If MCP times out or is unregistered, retry the same operation through the CLI.
- CLI errors mirror the MCP error texts.
- `graphify` missing → section 5 (sqlite3 one-liners) still gives you the link registry.

## 9. Privacy & deployment

- No internal network topologies, private hostnames, credentials, or absolute local paths appear in this skill or the plugin.
- Layer 2 OpenDocuments backend URL is injected at runtime by graphify-mcp, never hardcoded.
- The plugin reads only under the workspace root; there are no global scans.

## 10. References

- Layer 1 + Layer 2 architecture (authoritative spec): `openspec/changes/opendoc-native-plugin/design.md`
- Resolved questions: `openspec/changes/opendoc-native-plugin/proposal.md`
- `GraphifyPlugin` trait: `graphify-core/src/plugin/`
