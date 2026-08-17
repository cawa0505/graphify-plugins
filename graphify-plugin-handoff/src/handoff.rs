//! Handoff snapshot 生產鏈（RFC-0004 §4.2 雙層模型）。
//!
//! 純資料裝配 + SQLite registry 同步，全部基於 graphify-core 的 plugin_memory
//! 契約型別 — 不重定義、不寫死 Qdrant point ID。
//!
//! - Tier 1：`HandoffPayload` → `PluginMemoryEnvelope`（由 P4 的
//!   `PluginDomainMemory` 鉤子寫入 `graphify_plugin_handoff` collection；見下）。
//! - Tier 2：`HandoffSnapshot` → `graphify-registry` 的 `handoff_registry`
//!   資料表（寫入時自動 TTL 清理 + 每 workspace 20 筆 FIFO）。
//!
//! # Collection 寫入邊界（ponytail: P4 上游鉤子）
//! `PluginDomainMemory`（graphify-memory）是儲存邊界，但該 crate 拖著
//! ONNX/ort/fastembed 等重依賴，plugin 直接依賴會違反離線編譯。依
//! openspec/sqlite-global-registry design.md，實際 collection 鉤子屬 P4
//! （"P4 hooks the plugin domain store here"）。本模組只提供 envelope 裝配
//! （`envelope_for`），寫入呼叫點留給 P4 整合。

use graphify_core::plugin_memory::{
    HandoffPayload, HandoffSnapshot, MemoryQueryCriteria, PluginMemoryEnvelope,
};
use graphify_registry::{RegistryDb, RegistryError};

/// Plugin id — 對應 `plugin_collection_name("handoff")` → `graphify_plugin_handoff`。
pub const PLUGIN_ID: &str = "handoff";

/// Envelope record kind（graphify-memory 契約）。
pub const RECORD_KIND_SNAPSHOT: &str = "snapshot";

/// 組裝一個 Tier 2 `HandoffSnapshot`。
///
/// `expires_at` 傳 0 表示由 registry 依預設 TTL（`created_at` + 7 天）填寫；
/// `created_at` 為 unix 秒。
///
/// 8 個參數與 `HandoffSnapshot` 欄位 1:1 對應，包成 struct 只是多一層鏡像。
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn build_snapshot(
    snapshot_id: impl Into<String>,
    session_id: impl Into<String>,
    workspace_key: impl Into<String>,
    task_goal: impl Into<String>,
    pinned_node_ids: Vec<String>,
    focused_subgraph_toon: impl Into<String>,
    query_metadata: MemoryQueryCriteria,
    created_at: i64,
) -> HandoffSnapshot {
    HandoffSnapshot {
        snapshot_id: snapshot_id.into(),
        session_id: session_id.into(),
        workspace_key: workspace_key.into(),
        created_at,
        expires_at: 0,
        payload: HandoffPayload {
            schema_version: HandoffPayload::SCHEMA_VERSION,
            task_goal: task_goal.into(),
            pinned_node_ids,
            focused_subgraph_toon: focused_subgraph_toon.into(),
            reconstructable_query_metadata: query_metadata,
        },
    }
}

/// 將 snapshot 包裝為 Tier 1 `PluginMemoryEnvelope<HandoffPayload>`。
///
/// 不寫死任何 Qdrant point ID — `reconstructable_query_metadata` 以查詢條件
/// 重現記憶鄰域，快照可在 re-index / collection 遷移後被確定性重建。
#[must_use]
pub fn envelope_for(snapshot: &HandoffSnapshot) -> PluginMemoryEnvelope<HandoffPayload> {
    PluginMemoryEnvelope::new(
        snapshot.workspace_key.clone(),
        PLUGIN_ID,
        snapshot.snapshot_id.clone(),
        RECORD_KIND_SNAPSHOT,
        snapshot.created_at,
        Vec::new(),
        snapshot.payload.clone(),
    )
}

/// 同步 snapshot 至 `graphify.db` 的 `handoff_registry`（P2 輔助函式）。
///
/// workspace 不存在時自動 upsert（全新 workspace 成為 active）。
/// `put_snapshot` 在單一交易內完成：TTL 清理（`expires_at < now`）+
/// 每 workspace 20 筆 FIFO 淘汰（`created_at` 排序，保留最新）。
///
/// # Errors
///
/// 回傳 [`RegistryError`]（SQLite 失敗或 schema 不符）。
pub fn sync_to_registry(
    registry: &RegistryDb,
    workspace_key: &str,
    root_path: &str,
    snapshot: &HandoffSnapshot,
) -> Result<(), RegistryError> {
    registry.upsert_workspace(workspace_key, root_path)?;
    registry.put_snapshot(snapshot)
}

/// `sync_to_registry` 的開路版本：自行開啟 `db_path` 的 registry 後委派。
pub fn sync_to_registry_at(
    db_path: &std::path::Path,
    workspace_key: &str,
    root_path: &str,
    snapshot: &HandoffSnapshot,
) -> Result<(), RegistryError> {
    let registry = RegistryDb::open(db_path)?;
    sync_to_registry(&registry, workspace_key, root_path, snapshot)
}

/// 目前 unix 秒（snapshot created_at/expires_at 用）。
pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphify_registry::{HANDOFF_MAX_PER_WORKSPACE, HANDOFF_TTL_DAYS};

    const WS_KEY: &str = "ws_test_0001";
    const ROOT: &str = "/tmp/ws";

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    fn snapshot_at(created_at: i64, id: &str) -> HandoffSnapshot {
        build_snapshot(
            id,
            "session-1",
            WS_KEY,
            "繼續 Slice C",
            vec!["node:foo".to_string(), "node:bar".to_string()],
            "toon:…",
            MemoryQueryCriteria {
                target_symbols: vec!["relay_init".to_string()],
                domain_categories: vec!["plugin".to_string()],
                search_terms: vec!["handoff".to_string()],
            },
            created_at,
        )
    }

    fn temp_registry() -> (tempfile::TempDir, RegistryDb) {
        let dir = tempfile::tempdir().unwrap();
        let db = RegistryDb::open(&dir.path().join("graphify.db")).unwrap();
        (dir, db)
    }

    #[test]
    fn build_snapshot_two_tier_payload() {
        let s = snapshot_at(now(), "snap-1");
        assert_eq!(s.payload.schema_version, HandoffPayload::SCHEMA_VERSION);
        assert_eq!(s.payload.task_goal, "繼續 Slice C");
        assert_eq!(s.payload.pinned_node_ids, vec!["node:foo", "node:bar"]);
        assert_eq!(s.payload.focused_subgraph_toon, "toon:…");
        assert_eq!(
            s.payload.reconstructable_query_metadata.target_symbols,
            vec!["relay_init"]
        );
        // expires_at 由 registry TTL 填寫，組裝時為 0
        assert_eq!(s.expires_at, 0);
    }

    #[test]
    fn envelope_roundtrip_json() {
        let s = snapshot_at(now(), "snap-1");
        let env = envelope_for(&s);
        assert_eq!(env.format_version, PluginMemoryEnvelope::<()>::FORMAT_VERSION);
        assert_eq!(env.plugin_id, PLUGIN_ID);
        assert_eq!(env.record_id, "snap-1");
        assert_eq!(env.record_kind, RECORD_KIND_SNAPSHOT);
        let encoded = serde_json::to_string(&env).unwrap();
        let decoded: PluginMemoryEnvelope<HandoffPayload> =
            serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, env);
    }

    #[test]
    fn no_qdrant_point_ids_in_snapshot_or_envelope() {
        let s = snapshot_at(now(), "snap-1");
        let snap_json = serde_json::to_string(&s).unwrap().to_lowercase();
        let env_json = serde_json::to_string(&envelope_for(&s)).unwrap().to_lowercase();
        assert!(!snap_json.contains("point_id"), "{snap_json}");
        assert!(!snap_json.contains("\"vector\""), "{snap_json}");
        assert!(!env_json.contains("point_id"), "{env_json}");
    }

    #[test]
    fn sync_to_registry_persists_with_default_ttl() {
        let (_dir, db) = temp_registry();
        let created = now();
        let s = snapshot_at(created, "snap-1");
        sync_to_registry(&db, WS_KEY, ROOT, &s).unwrap();
        let row = db.get_snapshot("snap-1").unwrap().expect("snapshot 應存在");
        assert_eq!(row.snapshot_id, "snap-1");
        assert_eq!(row.expires_at, created + HANDOFF_TTL_DAYS * 86_400);
        // payload 完整保留（Tier 2 已由 registry 解析回 HandoffPayload）
        assert_eq!(row.payload, s.payload);
    }

    #[test]
    fn registry_fifo_prunes_beyond_max_per_workspace() {
        let (_dir, db) = temp_registry();
        let base = now() - 100;
        for i in 0..(HANDOFF_MAX_PER_WORKSPACE as i64 + 5) {
            let s = snapshot_at(base + i, &format!("snap-{i:03}"));
            sync_to_registry(&db, WS_KEY, ROOT, &s).unwrap();
        }
        let rows = db.list_snapshots(WS_KEY).unwrap();
        assert_eq!(rows.len() as u64, HANDOFF_MAX_PER_WORKSPACE);
        // FIFO：保留最新 20 筆，最舊 5 筆被淘汰
        assert!(rows.iter().any(|r| r.snapshot_id == "snap-024"));
        assert!(!rows.iter().any(|r| r.snapshot_id == "snap-000"));
    }
}
