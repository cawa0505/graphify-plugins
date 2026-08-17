# Code Relay — Relay Tool Protocol (v1)

The interface contract for the Code Relay logic embedded in Graphify via
`graphify-plugin-handoff`. It is the single source of truth for tool names,
argument schemas, return texts, and state semantics. GraphifyMCP exposes these
as MCP tools at startup by calling into the plugin's public API; the tool
semantics MUST map 1:1 against the legacy `@jimmyyen/opencode-code-relay-plugin`
so existing consumers keep working.

Scope: state model · root resolution · 7 tools · rendering · spec sync · git
helpers · write safety · transport (owned by GraphifyMCP).

---

## 1. State model (`relay.json`, schema `1.0.0`)

```json
{
  "schema_version": "1.0.0",
  "project_context": "",
  "active_baton": "",
  "repos": {
    "<name>": {
      "name": "<name>",
      "path": "<root-relative subdir, defaults to <name>>",
      "role": "",
      "active_phase": "",
      "volatile_state": "",
      "confidence_score": 3,
      "debt_tag": [],
      "next_session_starter": "",
      "handoffs": [{ "source": "", "captured_at": "", "raw": "" }],
      "last_updated": ""
    }
  },
  "state_snapshot": { "last_session": "", "open_threads": [], "blockers": [] },
  "spec_sync": { "last_sync": "", "drift": [], "specs": { "<name>": "<sha1-12>" } },
  "updated_at": ""
}
```

- `repos[name].path` is **root-relative**; defaults to the repo name (each repo
  is a subdirectory of the relay root). The registry is `repos` itself — given a
  name, the path is `root/<path>` with zero filesystem search.
- All timestamps: ISO-8601 UTC with milliseconds (`YYYY-MM-DDTHH:MM:SS.mmmZ`,
  matching JS `Date.prototype.toISOString()`).
- Spec hashes: `sha1(content)` hex truncated to 12 chars (compat: existing
  `relay.json` files store sha1-12; switching algorithms would flag every spec
  as modified).

## 2. Root resolution

Order of application, exactly:

1. **Named-repo ops** (`relaySave`/`relayClose`/`relayResume`/`relaySwitch`
   with `repo`) — resolve purely against the in-memory state loaded from the
   cached root. Zero walk-up.
2. **No-repo ops** (`relayStatus`, `relayResume` without repo) — use the root
   cached at startup.
3. **`relayInit`** — the only write-style walk: walk up from cwd for the first
   directory containing `relay.json`; if found, refuse; if not, create a new
   root at cwd and cache it.

**Startup**: cwd is captured once at server start. Walk up from cwd; if a root
is found, load its `relay.json` into memory (the in-memory state is the runtime
source of truth). If none is found, record "no root" and let `relayInit`
establish one.

**Fail-fast**: any non-init tool with no cached root and no `repo` argument
returns the error `No relay.json found. Run relayInit first.` — never an
unbounded search or guess.

## 3. Tools

All tools return plain text. Known failure cases return the **same text** as
the legacy plugin but MUST be reported with `isError: true` in the MCP result.

### 3.1 `relayInit`
- args: `project_context?: string`, `kind?: "backend"|"frontend"|"infra"` (kept
  in the schema for compat; currently unused by the implementation)
- If a root exists: `relay.json already exists at <root>. Edit it or run relaySave.`
- Else: create `specs/` + `.code-relay/` at cwd, write a fresh state file,
  seed `.gitignore` (idempotent: append `relay.json`, `RESUME.md`,
  `next_step.md` under a `# Code Relay (local state)` header — only when cwd is
  a git repo). Returns:
  ```
  Initialized relay at <cwd>/relay.json
  - specs/ and .code-relay/ created
  - .gitignore updated (relay.json, RESUME.md, next_step.md)
  - run relaySave to register the current repo
  ```
  (The `.gitignore updated` line appears only in a git repo.)

### 3.2 `relaySave`
- args: `repo?`, `role?`, `active_phase?`, `volatile_state?`,
  `confidence?` (number), `next_session_starter?`, `debt_tag?` (string,
  comma-separated), `kind?`
- `repo` defaults to `basename(cwd)`.
- `debt_tag` is split on `,`, trimmed, empty entries dropped.
- `confidence` is clamped: `min(5, max(1, round(v)))`.
- Upserts the repo state (missing repo defaults: role/phase/volatile empty,
  confidence 3, empty debt/next/handoffs). If `active_baton` is unset, it is
  set to this repo.
- Returns:
  ```
  Saved state for "<repo>".
  Active baton: <active_baton>

  <rendered resume>
  ```
  (rendering also writes `RESUME.md` at the root)

### 3.3 `relayClose`
- args: `repo?`, `next_session_starter?`
- Runs the consistency check (§6) and spec diff (§6), updates
  `next_session_starter` if given, renders `next_step.md` (§5), and — when the
  root is a git repo — commits `relay.json` + `specs/` (ignore-aware, §7).
- Returns:
  ```
  Closing ritual for "<repo>".
  Consistency: OK|ISSUES
    - <issue>            (per issue, only when ISSUES)
  Spec sync: <a:added,b:modified> | no changes

  <rendered next_step.md>

  committed: <hash> | nothing to commit (files ignored or missing) | not a git repo — skipped commit
  ```

### 3.4 `relaySwitch`
- args: `repo` (required), `kind?`
- `repo "<repo>" not registered. Run relaySave in that repo first.` if absent.
- Sets `active_baton = repo`. Returns:
  ```
  Baton passed to "<repo>".

  <rendered resume>
  ```

### 3.5 `relayResume`
- args: `repo?`, `kind?`
- Target = `repo` ?? `active_baton`.
- `No active baton set and no repo given. Run relaySwitch <repo> first.` if no
  target; `repo "<target>" not registered.` if unknown.
- Returns the rendered resume (§5), also written to `RESUME.md`.

### 3.6 `relayStatus`
- args: none
- Summary:
  ```
  Relay root: <root>
  Project: <project_context | "(unset)">
  Active baton: <active_baton | "(none)">
  Repos (<n>):
    - <name> [<active_phase | "?">] conf=<confidence_score>[ · <n> handoff(s)][ → <next_session_starter, first 60 chars>]
  Specs: <names, comma-joined | "(none)">
  Drift: <spec (added), ...> | "(none)"
  Updated: <updated_at>
  ```

### 3.7 `relayAdd`
- args: `file` (required), `repo?`
- Resolves `file` against cwd; `File not found: <file>` if missing.
- Appends a handoff entry `{source: basename(file), captured_at: now, raw: <content>}`
  to the repo, and appends each non-empty trimmed line of the content into
  `state_snapshot.open_threads` (deduped).
- Returns:
  ```
  Added handoff "<basename(file)>" to "<repo>".
  Parsed <n> line(s) into open_threads.
  Total open threads: <total>
  ```

## 4. Rendering

- Templates `backend.md` / `frontend.md` / `infra.md` ship with the server; an
  invalid `kind` falls back to `backend`.
- Substitution: every `{{var}}` token; unknown tokens become empty. Var set:
  `project_context`, `repo_name`, `role`, `active_phase`, `volatile_state`,
  `confidence_score`, `debt_tag` (`- item` lines or `(none)`),
  `next_session_starter` (`(none planned)`), `last_updated`, `git_commit`
  (`(not a git repo)`), `git_stat`, `spec_intent` (`(no spec yet)`),
  `schema_version`, `handoffs` (`### From <source> (<captured_at>)` blocks or
  `(none)`).
- `spec_intent` = first `#`/`##`/`###` heading (or first line) of
  `specs/<repo>.md`, truncated to 120 chars.
- Rendering always writes `RESUME.md` (resume tools) or `next_step.md`
  (close) at the root, in addition to returning the text.

## 5. Spec sync & consistency

- `diffSpecs`: hash every `specs/*.md` (sha1-12), compare with the
  `spec_sync.specs` snapshot, persist new hashes + `last_sync`, and set
  `drift` to `"<spec> (added)"` / `"<spec> (modified)"` entries.
- `consistencyCheck` rules per spec file: missing top-level `#` title; contains
  `CONFLICT:`; contains `BROKEN:`; a `## REMOVED Requirements` section followed
  by non-whitespace content.

## 6. Git helpers (best-effort, never throw)

- `isRepo`: `git rev-parse --is-inside-work-tree` == `true`
- `lastCommit`: `git log -1 --format=%H`
- `shortStat`: `git diff --shortstat` + count of `git status --porcelain` lines
- `commit`: `git add` (only files that exist AND are not gitignored via
  `git check-ignore`) then `git commit` — a private/ignored `relay.json` must
  never abort the commit of `specs/`.

## 7. Write safety

- Lazy-write: state is mutated in memory; disk write happens on the write paths
  (init/save/close/switch/add).
- Atomicity: write a temp file then OS `rename` over `relay.json`.
- Concurrency: an exclusive file lock (`fs2`-style) around read-modify-write so
  a concurrently running legacy plugin or second server cannot corrupt state.

## 8. Transport (owned by GraphifyMCP, not this crate)

This crate is an embedded library — it does **not** implement stdio, JSON-RPC,
or any MCP transport. GraphifyMCP (in the GraphifyRust project) registers the
seven tools above as MCP tools at startup and forwards `tools/call` requests
into this plugin's public API.

Transport requirements that apply to GraphifyMCP (for reference, verified
against the official MCP spec version 2025-06-18): one UTF-8 JSON message per
line on stdout; inbound on stdin. **Never** write non-MCP content to stdout
(stderr is for logs). Messages MUST NOT contain embedded newlines.

Required methods:

- `initialize` — request `{protocolVersion, capabilities, clientInfo}`;
  response `{protocolVersion, capabilities: {tools: {}}, serverInfo:
  {name, version}}`. Echo the client's requested `protocolVersion` when
  supported; otherwise respond with the server's own latest supported
  version. After the response, the client sends `notifications/initialized`.
- `notifications/initialized` — acknowledge; no response.
- `tools/list` — `{tools: [{name, description, inputSchema}], nextCursor?}`;
  `inputSchema` is a JSON Schema object (draft-07 style).
- `tools/call` — `{name, arguments}` → `{content: [{type: "text", text}],
  isError: bool}`. Protocol errors (unknown tool, invalid arguments) use
  JSON-RPC error `-32602`; tool-level failures (e.g. "No relay.json
  found") return `isError: true` with the same frozen text.
- `ping` — respond `{result: {}}`.

Target protocol version: `2025-06-18`.

## 9. Compatibility notes

- All return/error texts are frozen strings from the legacy plugin — do not
  rephrase them; consumers may match on them.
- `repo` defaults to `basename(cwd)` where cwd is the **workspace root** of
  the Graphify session that bound this plugin, not a per-call directory.
