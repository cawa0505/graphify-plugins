//! Slice 2：衝擊半徑偵測（Impact Radius Inspection Engine）。
//!
//! 依 design §8.1/§8.2：
//! - 以 `event.modified_nodes` 為 BFS 種子（缺省時退回 prev/cur node-id diff），
//!   `max_depth = 2`，在 plugin 快取的 `GraphOutput` 上以 `graphify_core` 的
//!   `build_graph` + `query_bfs` 展開（T2.1 裁決 3：公開 helper 直接複用，
//!   不重寫 mapping 層）。
//! - 對 BFS 涵蓋集合內每個 node 查 `review_bindings` 中
//!   `status = 'unresolved' AND severity IN ('critical', 'high')` 的綁定；
//!   命中即產出一個 [`ImpactAlert`]（T2.2）。
//! - `ImpactAlert` 為 plugin 領域事件，經 v1.1 `NotifyCallback` 跨邊界送出
//!   （design §8.3 方案 A）；此模組不碰任何 MCP/IO。

use std::collections::HashMap;

use graphify_core::types::{GraphOutput, NodeId};
use graphify_core::{build_graph, query_bfs};
use serde::Serialize;

use crate::registry::ReviewDb;
use crate::sync::now_rfc3339;

/// 單一衝擊事件：某變動節點（`modified_node`）的 BFS 衝擊半徑內，
/// 存在未解決的 critical/high review 綁定節點（`impacted_node`）。
///
/// 欄位依 design §8.2；`event_id` 為 uuid v4，`generated_at` 為 RFC 3339。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImpactAlert {
    pub workspace_key: String,
    pub modified_node: NodeId,
    pub impacted_node: NodeId,
    pub review_ids: Vec<String>,
    pub severities: Vec<String>,
    pub max_severity: String,
    pub alert_message: String,
    pub event_id: String,
    pub generated_at: String,
}

/// severity 等級排序（critical > high > medium > low；未知視為最低）。
#[must_use]
pub fn severity_rank(severity: &str) -> u8 {
    match severity {
        "critical" => 4,
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0,
    }
}

/// 對快取圖執行衝擊半徑偵測（Slice 2）。
///
/// - 依 T2.1：以 `graphify_core::build_graph` 建立 `DiGraph` + `node_map`，
///   對每個種子以 `query_bfs(.., max_depth = 2)` 展開衝擊集合。
///   `query_bfs` 為雙向走訪（upstream callers + downstream callees 都涵蓋）；
///   design 8.1 的「逆向邊」為方向說明，實務上雙向涵蓋更保守（不漏報）。
/// - 每個涵蓋節點查未解決綁定，過濾 `critical`/`high`；命中即產出
///   `ImpactAlert`（含命中 review ids + severities，`max_severity` 取最高）。
/// - best-effort：種子不存在於圖中（已刪除節點）或 db 錯誤時靜默跳過該種子
///   （v1 契約：plugin 永不 panic）。
#[must_use]
pub fn detect_impact(
    graph: &GraphOutput,
    seeds: &[NodeId],
    db: &ReviewDb,
    workspace_key: &str,
) -> Vec<ImpactAlert> {
    if seeds.is_empty() {
        return Vec::new();
    }
    let Ok((di_graph, node_map)) = build_graph(&graph.nodes, &graph.edges) else {
        return Vec::new();
    };
    let now = now_rfc3339();
    let mut alerts = Vec::new();
    let mut seen: HashMap<String, ()> = HashMap::new(); // impacted node id → 已產出

    for seed in seeds {
        let Ok(cover) = query_bfs(&di_graph, &node_map, seed, 2) else {
            continue; // 種子不在圖中（已刪除）→ 跳過
        };
        for covered in &cover.nodes {
            let Ok(rows) = db.query_unresolved_by_node(workspace_key, &covered.id.0) else {
                continue;
            };
            let mut ids = Vec::new();
            let mut severities = Vec::new();
            let mut max_rank = 0u8;
            let mut max_sev = String::new();
            for b in rows {
                if severity_rank(&b.severity) < 3 {
                    continue; // 僅 critical / high
                }
                ids.push(b.id.clone());
                severities.push(b.severity.clone());
                if severity_rank(&b.severity) > max_rank {
                    max_rank = severity_rank(&b.severity);
                    max_sev = b.severity.clone();
                }
            }
            if ids.is_empty() {
                continue;
            }
            if seen.contains_key(&covered.id.0) {
                continue; // 多個種子涵蓋同一節點 → 去重（避免重複 alert）
            }
            let alert = ImpactAlert {
                workspace_key: workspace_key.to_string(),
                modified_node: seed.clone(),
                impacted_node: covered.id.clone(),
                review_ids: ids.clone(),
                severities: severities.clone(),
                max_severity: max_sev,
                alert_message: format!(
                    "modified node {} impacts {}: {} unresolved review(s) in BFS radius",
                    seed.0,
                    covered.id.0,
                    ids.len()
                ),
                event_id: uuid::Uuid::new_v4().to_string(),
                generated_at: now.clone(),
            };
            seen.insert(covered.id.0.clone(), ());
            alerts.push(alert);
        }
    }
    alerts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ReviewBinding;
    use graphify_core::types::{Edge, Node};

    fn node(id: &str) -> Node {
        Node {
            id: NodeId(id.to_string()),
            label: id.to_string(),
            file_type: graphify_core::types::FileType::Code,
            kind: "function".to_string(),
            language: "rust".to_string(),
            source_file: format!("{}.rs", id.replace(':', "_")),
            start_line: 1,
            end_line: 10,
            doc_comment: None,
            description: None,
            metadata: None,
        }
    }

    fn edge(from: &str, to: &str) -> Edge {
        Edge {
            source: NodeId(from.to_string()),
            target: NodeId(to.to_string()),
            relation: "calls".to_string(),
            source_file: format!("{}.rs", from.replace(':', "_")),
            confidence: "1.0".to_string(),
            source_location: String::new(),
            description: None,
        }
    }

    fn graph3() -> GraphOutput {
        GraphOutput {
            nodes: vec![node("a"), node("b"), node("c")],
            edges: vec![edge("a", "b"), edge("b", "c")],
            metadata: Default::default(),
        }
    }

    fn binding(id: &str, node_id: &str, severity: &str) -> ReviewBinding {
        ReviewBinding {
            workspace_key: "w-1".to_string(),
            id: id.to_string(),
            canonical_node_id: node_id.to_string(),
            file_path: String::new(),
            line_number: 1,
            signature_hash: "v1_default".to_string(),
            severity: severity.to_string(),
            category: String::new(),
            comment: String::new(),
            status: "unresolved".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
            resolution_reason: String::new(),
            resolved_at: String::new(),
            resolved_by: String::new(),
        }
    }

    fn db_with(rows: Vec<ReviewBinding>) -> ReviewDb {
        let dir = tempfile::tempdir().unwrap();
        let db = ReviewDb::open(&dir.path().join("t.db")).unwrap();
        for r in rows {
            db.upsert(&r).unwrap();
        }
        db
    }

    #[test]
    fn critical_in_radius_produces_alert() {
        let db = db_with(vec![binding("r-001", "c", "critical")]);
        let alerts = detect_impact(&graph3(), &[NodeId("a".into())], &db, "w-1");
        assert_eq!(alerts.len(), 1);
        let a = &alerts[0];
        assert_eq!(a.modified_node.0, "a");
        assert_eq!(a.impacted_node.0, "c");
        assert_eq!(a.review_ids, vec!["r-001"]);
        assert_eq!(a.max_severity, "critical");
        assert!(!a.event_id.is_empty());
        assert_eq!(a.workspace_key, "w-1");
    }

    #[test]
    fn high_also_alerted_medium_not() {
        let db = db_with(vec![
            binding("r-001", "c", "high"),
            binding("r-002", "b", "medium"),
        ]);
        let alerts = detect_impact(&graph3(), &[NodeId("a".into())], &db, "w-1");
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].review_ids, vec!["r-001"]);
    }

    #[test]
    fn no_seed_no_alert() {
        let db = db_with(vec![binding("r-001", "c", "critical")]);
        let alerts = detect_impact(&graph3(), &[], &db, "w-1");
        assert!(alerts.is_empty());
    }

    #[test]
    fn seed_not_in_graph_skipped() {
        let db = db_with(vec![binding("r-001", "c", "critical")]);
        let alerts = detect_impact(&graph3(), &[NodeId("ghost".into())], &db, "w-1");
        assert!(alerts.is_empty());
    }

    #[test]
    fn duplicate_coverage_deduped() {
        let db = db_with(vec![binding("r-001", "c", "critical")]);
        let alerts = detect_impact(
            &graph3(),
            &[NodeId("a".into()), NodeId("b".into())],
            &db,
            "w-1",
        );
        assert_eq!(alerts.len(), 1); // b 的 BFS 也涵蓋 c → 去重
    }

    #[test]
    fn severity_rank_order() {
        assert!(severity_rank("critical") > severity_rank("high"));
        assert!(severity_rank("high") > severity_rank("medium"));
        assert!(severity_rank("medium") > severity_rank("low"));
        assert_eq!(severity_rank("unknown"), 0);
    }

    #[test]
    fn alert_serializes_to_json() {
        let db = db_with(vec![binding("r-001", "c", "critical")]);
        let alerts = detect_impact(&graph3(), &[NodeId("a".into())], &db, "w-1");
        let v = serde_json::to_value(&alerts[0]).unwrap();
        assert_eq!(v["max_severity"], "critical");
        assert_eq!(v["review_ids"][0], "r-001");
        assert!(v["event_id"].as_str().unwrap().contains('-'));
    }
}
