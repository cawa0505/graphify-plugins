//! SQLite telemetry registry — plugin 自有表，與 graphify-registry 共用同一 graphify.db 檔。
//!
//! ## 表
//! - `telemetry_bindings`: (workspace_key, metric_id → canonical node, p50/p99,
//!   alloc, call count, is_hotspot…) — Draco 效能點位升維綁定後的持久化。
//!
//! ## 為什麼不用 graphify-registry 的 RegistryDb？
//! 同 review plugin：`RegistryDb.conn` 為 private，本 plugin 以獨立
//! `rusqlite::Connection` 開啟同一 db 檔，建立自有表（`CREATE TABLE IF NOT
//! EXISTS`），不干涉 graphify-registry 的 schema 版本管理。
//!
//! ## 與 §3.2 spec 的偏差
//! spec 的 `id TEXT PRIMARY KEY`（單一 PK）在多 workspace 共用 graphify.db
//! 的情境下不成立：兩個 workspace 可能同時有 `tel-p99-001`。PK 改為
//! `(workspace_key, id)`，與 review_bindings / opendoc_links 同款 workspace
//! scoping（對齊 `#3115` plugin 間以 workspace_key 對齊的契約）。

use std::path::Path;

use rusqlite::{Connection, OptionalExtension};

/// 一筆效能指標綁定（telemetry_bindings 列）。
#[derive(Debug, Clone, PartialEq)]
pub struct TelemetryBinding {
    pub workspace_key: String,
    pub id: String,
    pub canonical_node_id: String,
    pub file_path: String,
    pub function_name: String,
    pub p50_ms: f64,
    pub p99_ms: f64,
    pub alloc_bytes: i64,
    pub call_count: i64,
    pub is_hotspot: bool,
    pub environment: String,
    pub created_at: String,
    pub updated_at: String,
}

/// plugin 自有 SQLite 連線。
pub struct TelemetryDb {
    conn: Connection,
}

impl TelemetryDb {
    /// 開啟 `path`（共用的 graphify.db），並確保 plugin schema 已建。
    ///
    /// # Errors
    /// 回傳 `rusqlite::Error` 於開啟或 DDL 執行失敗時。
    pub fn open(path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).ok();
            }
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS telemetry_bindings (
                workspace_key     TEXT NOT NULL,
                id                TEXT NOT NULL,
                canonical_node_id TEXT NOT NULL,
                file_path         TEXT NOT NULL,
                function_name     TEXT NOT NULL,
                p50_ms            REAL NOT NULL,
                p99_ms            REAL NOT NULL,
                alloc_bytes       INTEGER NOT NULL DEFAULT 0,
                call_count        INTEGER NOT NULL DEFAULT 0,
                is_hotspot        INTEGER NOT NULL DEFAULT 0,
                environment       TEXT NOT NULL,
                created_at        TEXT NOT NULL,
                updated_at        TEXT NOT NULL,
                PRIMARY KEY (workspace_key, id)
            );

            CREATE INDEX IF NOT EXISTS idx_telemetry_node
                ON telemetry_bindings (workspace_key, canonical_node_id);

            CREATE INDEX IF NOT EXISTS idx_telemetry_hotspot
                ON telemetry_bindings (workspace_key, is_hotspot);",
        )?;
        Ok(Self { conn })
    }

    /// 插入或覆寫一筆綁定（以 (workspace_key, metric_id) 為鍵）。已存在則
    /// 更新除 `created_at` 外的所有欄位並刷新 `updated_at`。
    ///
    /// # Errors
    /// SQLite DML 失敗時回傳 `rusqlite::Error`。
    pub fn upsert(&self, b: &TelemetryBinding) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO telemetry_bindings
                (workspace_key, id, canonical_node_id, file_path, function_name,
                 p50_ms, p99_ms, alloc_bytes, call_count, is_hotspot,
                 environment, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(workspace_key, id) DO UPDATE SET
                canonical_node_id = excluded.canonical_node_id,
                file_path         = excluded.file_path,
                function_name     = excluded.function_name,
                p50_ms            = excluded.p50_ms,
                p99_ms            = excluded.p99_ms,
                alloc_bytes       = excluded.alloc_bytes,
                call_count        = excluded.call_count,
                is_hotspot        = excluded.is_hotspot,
                environment       = excluded.environment,
                updated_at        = excluded.updated_at",
            rusqlite::params![
                b.workspace_key,
                b.id,
                b.canonical_node_id,
                b.file_path,
                b.function_name,
                b.p50_ms,
                b.p99_ms,
                b.alloc_bytes,
                b.call_count,
                b.is_hotspot,
                b.environment,
                b.created_at,
                b.updated_at,
            ],
        )?;
        Ok(())
    }

    /// 查詢一個 workspace 中指定 canonical node 的所有綁定（
    /// telemetry_get_context 主路徑；含 hotspot 與否，按 p99 降冪）。
    ///
    /// # Errors
    /// SQLite 查詢失敗時回傳 `rusqlite::Error`。
    pub fn query_by_node(
        &self,
        workspace_key: &str,
        node_id: &str,
    ) -> Result<Vec<TelemetryBinding>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT workspace_key, id, canonical_node_id, file_path, function_name,
                    p50_ms, p99_ms, alloc_bytes, call_count, is_hotspot,
                    environment, created_at, updated_at
             FROM telemetry_bindings
             WHERE workspace_key = ?1 AND canonical_node_id = ?2
             ORDER BY p99_ms DESC",
        )?;
        let rows = stmt.query_map([workspace_key, node_id], row_from_sql)?;
        rows.collect()
    }

    /// 查詢一個 workspace 中所有 hotspot（is_hotspot = 1；sync_toon 合成用），
    /// 按 p99 降冪。
    ///
    /// # Errors
    /// SQLite 查詢失敗時回傳 `rusqlite::Error`。
    pub fn query_hotspots(
        &self,
        workspace_key: &str,
    ) -> Result<Vec<TelemetryBinding>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT workspace_key, id, canonical_node_id, file_path, function_name,
                    p50_ms, p99_ms, alloc_bytes, call_count, is_hotspot,
                    environment, created_at, updated_at
             FROM telemetry_bindings
             WHERE workspace_key = ?1 AND is_hotspot = 1
             ORDER BY p99_ms DESC",
        )?;
        let rows = stmt.query_map([workspace_key], row_from_sql)?;
        rows.collect()
    }

    /// 統計一個 workspace 的綁定數。
    ///
    /// # Errors
    /// SQLite 查詢失敗時回傳 `rusqlite::Error`。
    pub fn count(&self, workspace_key: &str) -> Result<usize, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM telemetry_bindings WHERE workspace_key = ?1",
                [workspace_key],
                |r| r.get(0),
            )
            .map(|n: i64| n as usize)
    }

    /// 統計一個 workspace 的 hotspot 數（sync_toon 摘要用）。
    ///
    /// # Errors
    /// SQLite 查詢失敗時回傳 `rusqlite::Error`。
    pub fn count_hotspots(&self, workspace_key: &str) -> Result<usize, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM telemetry_bindings
                 WHERE workspace_key = ?1 AND is_hotspot = 1",
                [workspace_key],
                |r| r.get(0),
            )
            .map(|n: i64| n as usize)
    }

    /// 依 metric_id 找一筆綁定（檢查用）。
    ///
    /// # Errors
    /// SQLite 查詢失敗時回傳 `rusqlite::Error`。
    pub fn get(
        &self,
        workspace_key: &str,
        metric_id: &str,
    ) -> Result<Option<TelemetryBinding>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT workspace_key, id, canonical_node_id, file_path, function_name,
                        p50_ms, p99_ms, alloc_bytes, call_count, is_hotspot,
                        environment, created_at, updated_at
                 FROM telemetry_bindings
                 WHERE workspace_key = ?1 AND id = ?2",
                rusqlite::params![workspace_key, metric_id],
                row_from_sql,
            )
            .optional()
    }
}

fn row_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<TelemetryBinding> {
    Ok(TelemetryBinding {
        workspace_key: row.get(0)?,
        id: row.get(1)?,
        canonical_node_id: row.get(2)?,
        file_path: row.get(3)?,
        function_name: row.get(4)?,
        p50_ms: row.get(5)?,
        p99_ms: row.get(6)?,
        alloc_bytes: row.get(7)?,
        call_count: row.get(8)?,
        is_hotspot: row.get(9)?,
        environment: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(ws: &str, id: &str, node: &str) -> TelemetryBinding {
        TelemetryBinding {
            workspace_key: ws.to_string(),
            id: id.to_string(),
            canonical_node_id: node.to_string(),
            file_path: "src/db/query.rs".to_string(),
            function_name: "query_users".to_string(),
            p50_ms: 12.5,
            p99_ms: 1250.0,
            alloc_bytes: 10_485_760,
            call_count: 5000,
            is_hotspot: true,
            environment: "production".to_string(),
            created_at: "2026-08-10T06:00:00Z".to_string(),
            updated_at: "2026-08-10T06:00:00Z".to_string(),
        }
    }

    fn open_tmp() -> (tempfile::TempDir, TelemetryDb) {
        let dir = tempfile::tempdir().unwrap();
        let db = TelemetryDb::open(&dir.path().join("graphify.db")).unwrap();
        (dir, db)
    }

    #[test]
    fn upsert_and_query_by_node() {
        let (_d, db) = open_tmp();
        db.upsert(&binding("w-1", "tel-1", "src/db/query.rs:function:query_users"))
            .unwrap();
        let rows = db
            .query_by_node("w-1", "src/db/query.rs:function:query_users")
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "tel-1");
        assert!(rows[0].is_hotspot);
        assert_eq!(rows[0].p99_ms, 1250.0);
    }

    #[test]
    fn upsert_same_metric_id_replaces() {
        let (_d, db) = open_tmp();
        let mut b = binding("w-1", "tel-1", "src/db/query.rs:function:query_users");
        db.upsert(&b).unwrap();
        b.canonical_node_id = "src/db/query.rs:function:query_other".to_string();
        b.p99_ms = 2.0;
        b.is_hotspot = false;
        db.upsert(&b).unwrap();
        let rows = db
            .query_by_node("w-1", "src/db/query.rs:function:query_other")
            .unwrap();
        assert_eq!(rows.len(), 1, "replaced, not duplicated");
        assert_eq!(rows[0].p99_ms, 2.0);
        let old = db
            .query_by_node("w-1", "src/db/query.rs:function:query_users")
            .unwrap();
        assert!(old.is_empty());
    }

    #[test]
    fn workspace_isolation_and_hotspot_filter() {
        let (_d, db) = open_tmp();
        db.upsert(&binding("w-1", "tel-1", "n")).unwrap();
        db.upsert(&binding("w-2", "tel-1", "n")).unwrap();
        assert_eq!(db.count("w-1").unwrap(), 1);
        assert_eq!(db.count("w-2").unwrap(), 1);
        assert_eq!(db.query_hotspots("w-1").unwrap().len(), 1);
        let hot = db.query_hotspots("w-1").unwrap();
        assert_eq!(hot[0].id, "tel-1");
    }

    #[test]
    fn non_hotspot_excluded_from_hotspots() {
        let (_d, db) = open_tmp();
        let mut b = binding("w-1", "tel-2", "n");
        b.is_hotspot = false;
        db.upsert(&b).unwrap();
        assert_eq!(db.count_hotspots("w-1").unwrap(), 0);
        assert!(db.query_hotspots("w-1").unwrap().is_empty());
    }

    #[test]
    fn get_by_metric_id() {
        let (_d, db) = open_tmp();
        db.upsert(&binding("w-1", "tel-1", "n")).unwrap();
        let row = db.get("w-1", "tel-1").unwrap().unwrap();
        assert_eq!(row.function_name, "query_users");
        assert!(db.get("w-1", "nope").unwrap().is_none());
    }
}
