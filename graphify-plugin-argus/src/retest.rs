//! RetestSignal — code 變動影響某 phase 時產出的領域事件。
//!
//! 第一階段（無 worker→檔案邊）：判定來源是 Argus graph 中 worker 節點的
//! `source_file`（指向 `.argus/phases/<phase_id>.toml`，Argus Phase07 §7.5）。
//! 種子節點命中 worker 節點本身或其 `source_file` → 該 phase 受影響。
//!
//! 本模組不碰 IO／MCP；事件由 `lib.rs` 經 v1.1 `NotifyCallback` 送出。

use std::collections::BTreeSet;

use graphify_core::types::{GraphOutput, NodeId};
use serde::Serialize;

use crate::sync::now_rfc3339;

/// 單一重測訊號：`phase` 因變動節點 `triggered_by` 而應重測。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RetestSignal {
    pub kind: &'static str,
    pub workspace_key: String,
    pub phase: String,
    pub reason: String,
    pub nodes: Vec<String>,
    pub event_id: String,
    pub generated_at: String,
}

/// 對快取圖判定受影響 phase（同 phase 去重）。
///
/// - `seeds` 為變動節點；為空時直接回空（呼叫端負責補 diff 種子）。
/// - 掃描 `kind == "worker"` 的節點：節點 id 命中種子，或其 `source_file`
///   命中種子（以 node id 或 source_file 字串比對）→ 由節點 id 前綴
///   `worker:<phase_id>/<worker_id>` 取 phase。
/// - best-effort：無法解析 phase 的節點靜默跳過（契約：永不 panic）。
#[must_use]
pub fn detect_retests(
    graph: &GraphOutput,
    seeds: &[NodeId],
    workspace_key: &str,
) -> Vec<RetestSignal> {
    if seeds.is_empty() {
        return Vec::new();
    }
    let seed_ids: BTreeSet<&str> = seeds.iter().map(|s| s.0.as_str()).collect();
    // 種子節點自身的 source_file（若種子是 worker 節點本身已由 seed_ids 涵蓋）。
    let seed_files: BTreeSet<&str> = graph
        .nodes
        .iter()
        .filter(|n| seed_ids.contains(n.id.0.as_str()))
        .map(|n| n.source_file.as_str())
        .collect();

    let now = now_rfc3339();
    let mut out: Vec<RetestSignal> = Vec::new();
    let mut seen_phases: BTreeSet<String> = BTreeSet::new();
    let mut seen_workers: BTreeSet<String> = BTreeSet::new();

    for node in &graph.nodes {
        if node.kind != "worker" {
            continue;
        }
        let hit_by_id = seed_ids.contains(node.id.0.as_str());
        let hit_by_file = !node.source_file.is_empty()
            && (seed_ids.contains(node.source_file.as_str())
                || seed_files.contains(node.source_file.as_str()));
        if !hit_by_id && !hit_by_file {
            continue;
        }
        let Some(phase) = phase_of_worker(&node.id.0) else {
            continue;
        };
        if !seen_workers.insert(node.id.0.clone()) {
            continue;
        }
        if seen_phases.contains(&phase) {
            continue;
        }
        seen_phases.insert(phase.clone());
        out.push(RetestSignal {
            kind: "RetestSignal",
            workspace_key: workspace_key.to_string(),
            phase: phase.clone(),
            reason: format!(
                "change touches worker {} (phase spec {})",
                node.id.0, node.source_file
            ),
            nodes: vec![node.id.0.clone(), format!("phase:{phase}")],
            event_id: uuid::Uuid::new_v4().to_string(),
            generated_at: now.clone(),
        });
    }
    // 亦處理「种子本身就是 phase 節點」→ 該 phase 重測。
    for node in &graph.nodes {
        if node.kind != "phase" {
            continue;
        }
        if !seed_ids.contains(node.id.0.as_str()) {
            continue;
        }
        let Some(phase) = node.id.0.strip_prefix("phase:").map(str::to_string) else {
            continue;
        };
        if !seen_phases.insert(phase.clone()) {
            continue;
        }
        out.push(RetestSignal {
            kind: "RetestSignal",
            workspace_key: workspace_key.to_string(),
            phase: phase.clone(),
            reason: format!("phase node {} changed", node.id.0),
            nodes: vec![node.id.0.clone()],
            event_id: uuid::Uuid::new_v4().to_string(),
            generated_at: now.clone(),
        });
    }
    out
}

/// 由 worker node id（`worker:<phase_id>/<worker_id>`）取 phase_id。
fn phase_of_worker(node_id: &str) -> Option<String> {
    let rest = node_id.strip_prefix("worker:")?;
    let (phase, _worker) = rest.split_once('/')?;
    if phase.is_empty() {
        return None;
    }
    Some(phase.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphify_core::types::{FileType, Node};

    fn node(id: &str, kind: &str, file_type: FileType, source_file: &str) -> Node {
        Node {
            id: NodeId(id.to_string()),
            label: id.to_string(),
            file_type,
            kind: kind.to_string(),
            language: "argus-spec".to_string(),
            source_file: source_file.to_string(),
            start_line: 1,
            end_line: 1,
            doc_comment: None,
            description: None,
            metadata: None,
        }
    }

    fn graph_with_worker() -> GraphOutput {
        GraphOutput {
            nodes: vec![
                node(
                    "phase:probe",
                    "phase",
                    FileType::Concept,
                    ".argus/phases/probe.toml",
                ),
                node(
                    "worker:probe/a",
                    "worker",
                    FileType::Code,
                    ".argus/phases/probe.toml",
                ),
                node(
                    "evidence:probe/a/0",
                    "evidence",
                    FileType::Rationale,
                    "probe-a.log",
                ),
            ],
            edges: vec![],
            metadata: Default::default(),
        }
    }

    #[test]
    fn worker_id_seed_triggers_phase() {
        let signals = detect_retests(
            &graph_with_worker(),
            &[NodeId("worker:probe/a".into())],
            "w-1",
        );
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].phase, "probe");
        assert_eq!(signals[0].kind, "RetestSignal");
        assert_eq!(signals[0].workspace_key, "w-1");
        assert!(!signals[0].event_id.is_empty());
    }

    #[test]
    fn phase_node_seed_triggers_phase() {
        let signals = detect_retests(&graph_with_worker(), &[NodeId("phase:probe".into())], "w-1");
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].phase, "probe");
        // phase spec 同時是 phase 與 worker 節點的 source_file，故命中可能由
        // 任一路徑觸發；契約只保證「該 phase 被標記重測」。
        assert!(signals[0].nodes.iter().any(|n| n == "phase:probe"));
    }

    #[test]
    fn empty_seeds_no_signal() {
        assert!(detect_retests(&graph_with_worker(), &[], "w-1").is_empty());
    }

    #[test]
    fn unrelated_seed_no_signal() {
        let signals = detect_retests(&graph_with_worker(), &[NodeId("ghost".into())], "w-1");
        assert!(signals.is_empty());
    }

    #[test]
    fn same_phase_deduped_once() {
        let signals = detect_retests(
            &graph_with_worker(),
            &[
                NodeId("worker:probe/a".into()),
                NodeId("phase:probe".into()),
            ],
            "w-1",
        );
        assert_eq!(signals.len(), 1);
    }

    #[test]
    fn serializes_with_kind_discriminator() {
        let signals = detect_retests(
            &graph_with_worker(),
            &[NodeId("worker:probe/a".into())],
            "w-1",
        );
        let v = serde_json::to_value(&signals[0]).unwrap();
        assert_eq!(v["kind"], "RetestSignal");
        assert_eq!(v["phase"], "probe");
    }
}
