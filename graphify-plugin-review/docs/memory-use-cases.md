# Review Plugin: Memory-Integrated Use Cases

## Status

This document describes the intended application architecture. It is a design
concept, not a claim that every capability is currently implemented.

## Core Positioning

`graphify-plugin-review` combines structural impact analysis with historical
review knowledge.

The plugin uses:

1. Git diff and changed-symbol extraction
2. Graphify core graph traversal
3. Restricted core memory queries
4. Plugin-owned review domain memory
5. Bounded `.toon` impact subgraphs
6. Optional LLM-assisted interpretation

The plugin must not write arbitrary review records into Graphify core memory.
Review history belongs to the review plugin's isolated domain memory.

## Intended Review Flow

```text
git diff
  → changed symbols
  → impacted graph neighborhood
  → core memory query
  → historical review memory query
  → evidence-based findings
  → bounded review report
The structural graph is the primary source for dependency and blast-radius
relationships. Semantic memory is supporting context, not a replacement for
graph traversal.
Use Case 1: Blast-Radius Protection
Situation
A developer changes a low-level type, function signature, configuration field,
or shared storage abstraction.
Intended Flow
1. Parse the diff and identify changed symbols.
2. Traverse callers, callees, dependents, and related nodes.
3. Produce a bounded .toon impact subgraph.
4. Query core memory for relevant code descriptions and historical context.
5. Query review domain memory for similar past changes.
6. Report likely affected areas and required verification points.
Expected Result
The review should make the propagation path visible:
changed symbol
  → direct callers
  → dependent modules
  → storage/cache/API boundary
  → likely breaking-change locations
The plugin should provide evidence such as:
- changed file and line
- affected symbol
- graph relationship
- historical review reference
- suggested test or verification area
It must not claim that a breaking change exists unless the evidence supports
that conclusion.
Use Case 2: Historical Review Memory
Situation
A previous review identified an architectural hazard, such as using a
blocking lock in a high-load path.
Intended Flow
1. Store the review record in review domain memory.
2. Associate the record with:
- workspace_key
- affected symbols
- source files
- change or review identifier
- finding category
- resolution
3. During a later review, search for structurally or semantically similar
changes.
4. Present the previous finding as historical evidence.
5. Clearly separate historical precedent from a current confirmed defect.
Example Result
Historical review:
  similar change: review-142
  affected area: memory/storage boundary
  previous finding: blocking lock under high load
  previous resolution: use non-blocking coordination

Current change:
  similarity: candidate
  status: requires verification
Historical review memory must never be presented as a guaranteed diagnosis.
Use Case 3: Local-Model Context Reduction
Situation
A local model has a limited context window and may hallucinate relationships
when given a large repository or full diff.
Intended Flow
1. Perform deterministic symbol and graph analysis in Rust.
2. Limit traversal to the configured impact radius.
3. Serialize only the relevant subgraph as .toon.
4. Query memory for a small set of relevant historical findings.
5. Give the local model bounded, evidence-backed context.
Expected Result
The model performs semantic interpretation over a compact context instead of
trying to reconstruct the repository topology itself.
The plugin must preserve the distinction between:
- graph facts
- retrieved memory
- model-generated interpretation
- unresolved hypotheses
Use Case 4: Review Feedback Loop
Situation
A reviewer resolves a finding by accepting, rejecting, or modifying the
recommendation.
Intended Flow
1. Store the review outcome in review domain memory.
2. Link it to the affected symbols and change identity.
3. Retain the original finding and final resolution.
4. Use future searches to retrieve the outcome as historical context.
The plugin must define an explicit retention and update policy before treating
review memory as authoritative.
Memory Ownership
Graphify Core Memory
Core memory provides:
- semantic search over indexed code context
- workspace-scoped results
- bounded result limits
- storage-independent results
The review plugin may query it but may not write to it.
Review Domain Memory
Review domain memory owns:
- review record identity
- change or commit identity
- affected symbols
- finding and severity
- evidence references
- reviewer decision
- resolution
- timestamps
- optional links to external issue or pull request systems
Records should use a versioned common envelope:
format_version
workspace_key
plugin_id
record_id
record_kind
created_at
source_refs
payload
Review memory should use an isolated plugin collection or storage namespace.
The collection name and credentials must be managed by the Graphify memory
service, not assembled by plugin input.
Failure Handling
If core semantic memory is unavailable:
- graph-based blast-radius analysis must still work
- the review must report semantic memory as unavailable
- the plugin may use deterministic graph facts and local diff data
- the plugin must not fabricate historical matches
If review domain memory is unavailable:
- current graph analysis may continue
- historical findings must be marked unavailable
- the plugin must not silently treat an empty result as proof that no history
exists
.toon Integration
Plugin-specific review information must be stored in the reserved plugin_data
container.
Example:
metadata:
  format_version: "1"
  workspace_key: "..."
  plugin_data:
    review:
      change_id: "..."
      affected_symbols: ["..."]
      finding_ids: ["..."]
      status: "requires_verification"
The review plugin must not add arbitrary top-level fields to the core .toon
schema.
