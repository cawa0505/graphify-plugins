//! SQLite link registry — plugin 自有表，與 graphify-registry 共用同一 graphify.db 檔。
//!
//! ## 表
//! - `opendoc_links`: (workspace_key, doc_path, spec_id, symbol, signature) 硬鏈結
//! - `opendoc_workspace_mapping`: (workspace_key → od_workspace_id) Layer 2 對映
//!
//! ## 為什麼不用 graphify-registry 的 RegistryDb？
//! `RegistryDb.conn` 為 private，不暴露 raw execute。本 plugin 以獨立
//! `rusqlite::Connection` 開啟同一 db 檔，建立自有表（`CREATE TABLE IF NOT
//! EXISTS`），不干涉 graphify-registry 的 schema 版本管理。

use std::path::Path;

use rusqlite::{Connection, OptionalExtension};

use crate::links::LinkRow;

/// plugin 自有 SQLite 連線。
pub struct LinkDb {
    conn: Connection,
}

impl LinkDb {
    /// 開啟 `path`（建立的 graphify.db 或同目錄其他 .db），並確保 plugin schema 已建。
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
            "CREATE TABLE IF NOT EXISTS opendoc_links (
                workspace_key TEXT NOT NULL,
                doc_path      TEXT NOT NULL,
                spec_id       TEXT NOT NULL,
                symbol        TEXT NOT NULL,
                signature     TEXT NOT NULL,
                PRIMARY KEY (workspace_key, doc_path, spec_id, symbol)
            );

            CREATE INDEX IF NOT EXISTS idx_ol_doc
                ON opendoc_links (workspace_key, symbol);

            CREATE TABLE IF NOT EXISTS opendoc_workspace_mapping (
                workspace_key   TEXT PRIMARY KEY,
                od_workspace_id TEXT NOT NULL
            );",
        )?;
        Ok(Self { conn })
    }

    /// 全量替換一個 workspace 的硬鏈結（先刪舊、再批量插入）。
    ///
    /// # Errors
    /// SQLite DML 失敗時回傳 `rusqlite::Error`。
    pub fn replace_links(&self, workspace_key: &str, rows: &[LinkRow]) -> Result<(), rusqlite::Error> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM opendoc_links WHERE workspace_key = ?1",
            [workspace_key],
        )?;
        for row in rows {
            tx.execute(
                "INSERT OR REPLACE INTO opendoc_links
                    (workspace_key, doc_path, spec_id, symbol, signature)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    row.workspace_key,
                    row.doc_path,
                    row.spec_id,
                    row.symbol,
                    row.signature,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 依 doc_path 反查屬於該 doc 的所有 spec blocks（doc→code 方向：拿 spec_id 取 spec_block）。
    ///
    /// 一般而言用於工作流程：「我修改了 docs/auth.md → 哪些 symbols 受影響？」
    ///
    /// # Errors
    /// SQLite 失敗回傳 `rusqlite::Error`。
    pub fn query_by_doc(&self, workspace_key: &str, doc_path: &str) -> Result<Vec<LinkRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT workspace_key, doc_path, spec_id, symbol, signature
             FROM opendoc_links
             WHERE workspace_key = ?1 AND doc_path = ?2
             ORDER BY spec_id, symbol",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![workspace_key, doc_path],
            |row| {
                Ok(LinkRow {
                    workspace_key: row.get(0)?,
                    doc_path: row.get(1)?,
                    spec_id: row.get(2)?,
                    symbol: row.get(3)?,
                    signature: row.get(4)?,
                })
            },
        )?;
        rows.collect::<Result<_, _>>()
    }

    /// 依 symbol 取 spec_id（code→doc 方向：「我修改了 verify_token → 哪份 spec？」）。
    ///
    /// # Errors
    /// SQLite 失敗回傳 `rusqlite::Error`。
    pub fn query_by_symbol(
        &self,
        workspace_key: &str,
        symbol: &str,
    ) -> Result<Vec<LinkRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT workspace_key, doc_path, spec_id, symbol, signature
             FROM opendoc_links
             WHERE workspace_key = ?1 AND symbol = ?2
             ORDER BY doc_path",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![workspace_key, symbol],
            |row| {
                Ok(LinkRow {
                    workspace_key: row.get(0)?,
                    doc_path: row.get(1)?,
                    spec_id: row.get(2)?,
                    symbol: row.get(3)?,
                    signature: row.get(4)?,
                })
            },
        )?;
        rows.collect::<Result<_, _>>()
    }

    /// 取一個 workspace 下的所有 spec→symbol 連結（drift audit 會用）。
    ///
    /// # Errors
    /// SQLite 失敗。
    pub fn all_links(&self, workspace_key: &str) -> Result<Vec<LinkRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT workspace_key, doc_path, spec_id, symbol, signature
             FROM opendoc_links
             WHERE workspace_key = ?1
             ORDER BY doc_path, spec_id, symbol",
        )?;
        let rows = stmt.query_map([workspace_key], |row| {
            Ok(LinkRow {
                workspace_key: row.get(0)?,
                doc_path: row.get(1)?,
                spec_id: row.get(2)?,
                symbol: row.get(3)?,
                signature: row.get(4)?,
            })
        })?;
        rows.collect::<Result<_, _>>()
    }

    /// 寫入或覆寫 workspace → od_workspace_id 對映（Layer 2 用）。
    ///
    /// # Errors
    /// SQLite 失敗。
    pub fn set_workspace_mapping(
        &self,
        workspace_key: &str,
        od_workspace_id: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT OR REPLACE INTO opendoc_workspace_mapping
                (workspace_key, od_workspace_id)
             VALUES (?1, ?2)",
            rusqlite::params![workspace_key, od_workspace_id],
        )?;
        Ok(())
    }

    /// 取一個 workspace 對應的 od_workspace_id（Layer 2 查詢用）。
    ///
    /// # Errors
    /// SQLite 失敗（不含「未設定」— 未設定回傳 `Ok(None)`）。
    pub fn get_workspace_mapping(
        &self,
        workspace_key: &str,
    ) -> Result<Option<String>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT od_workspace_id FROM opendoc_workspace_mapping
                 WHERE workspace_key = ?1",
                [workspace_key],
                |row| row.get::<_, String>(0),
            )
            .optional()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn db() -> (tempfile::TempDir, LinkDb) {
        let dir = tempdir().unwrap();
        let db = LinkDb::open(&dir.path().join("links.db")).unwrap();
        (dir, db)
    }

    fn sample_links() -> Vec<LinkRow> {
        vec![
            LinkRow {
                workspace_key: "ws".into(),
                doc_path: "auth.md".into(),
                spec_id: "abc123def456".into(),
                symbol: "crate::auth::verify_token".into(),
                signature: "deadbeef".into(),
            },
            LinkRow {
                workspace_key: "ws".into(),
                doc_path: "auth.md".into(),
                spec_id: "xyz789def456".into(),
                symbol: "crate::auth::login".into(),
                signature: "cafebabe".into(),
            },
            LinkRow {
                workspace_key: "ws".into(),
                doc_path: "db.md".into(),
                spec_id: "111111aaaaaa".into(),
                symbol: "crate::db::get_user".into(),
                signature: "baadf00d".into(),
            },
        ]
    }

    #[test]
    fn replace_links_persists_all() {
        let (_d, db) = db();
        db.replace_links("ws", &sample_links()).unwrap();
        assert_eq!(db.all_links("ws").unwrap().len(), 3);
    }

    #[test]
    fn replace_links_replaces_per_workspace() {
        let (_d, db) = db();
        db.replace_links("ws", &sample_links()).unwrap();
        db.replace_links("ws", &[]).unwrap();
        let all = db.all_links("ws").unwrap();
        let other = db.all_links("other").unwrap();
        assert!(all.is_empty(), "ws must be emptied");
        assert!(other.is_empty(), "other was never populated");
    }

    #[test]
    fn replace_links_does_not_touch_other_workspaces() {
        let (_d, db) = db();
        db.replace_links("ws", &sample_links()).unwrap();
        let other_rows = vec![LinkRow {
            workspace_key: "other".into(),
            doc_path: "other.md".into(),
            spec_id: "zzz".into(),
            symbol: "x".into(),
            signature: "y".into(),
        }];
        db.replace_links("other", &other_rows).unwrap();
        let ws = db.all_links("ws").unwrap();
        assert_eq!(ws.len(), 3, "ws should retain 3 links after other commit");
        let other = db.all_links("other").unwrap();
        assert_eq!(other.len(), 1);
    }

    #[test]
    fn query_by_doc_returns_correct_rows() {
        let (_d, db) = db();
        db.replace_links("ws", &sample_links()).unwrap();
        let rows = db.query_by_doc("ws", "auth.md").unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.doc_path == "auth.md"));
    }

    #[test]
    fn query_by_symbol_returns_correct_rows() {
        let (_d, db) = db();
        db.replace_links("ws", &sample_links()).unwrap();
        let rows = db.query_by_symbol("ws", "crate::db::get_user").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].doc_path, "db.md");
    }

    #[test]
    fn query_missing_symbol_returns_empty() {
        let (_d, db) = db();
        db.replace_links("ws", &sample_links()).unwrap();
        let rows = db.query_by_symbol("ws", "nonexistent").unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn workspace_mapping_roundtrip() {
        let (_d, db) = db();
        assert!(db.get_workspace_mapping("ws").unwrap().is_none());
        db.set_workspace_mapping("ws", "od-uuid-12345").unwrap();
        assert_eq!(
            db.get_workspace_mapping("ws").unwrap(),
            Some("od-uuid-12345".to_string())
        );
    }

    #[test]
    fn workspace_mapping_upsert() {
        let (_d, db) = db();
        db.set_workspace_mapping("ws", "old-id").unwrap();
        db.set_workspace_mapping("ws", "new-id").unwrap();
        assert_eq!(
            db.get_workspace_mapping("ws").unwrap(),
            Some("new-id".to_string())
        );
    }
}