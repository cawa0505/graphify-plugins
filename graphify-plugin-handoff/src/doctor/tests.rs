//! handoff-doctor spec 情境測試（openspec/changes/handoff-doctor 逐 scenario 綁定）。

use std::path::Path;

use tempfile::tempdir;

use super::checks::{check_relay_file, Verdict};
use super::{apply_fix, check_dir, deletion_targets, exit_code, render, scan};

fn write_relay(dir: &std::path::Path, body: &str) {
    std::fs::write(dir.join("relay.json"), body).unwrap();
}

fn ws_relay(key: &str, path: &str) -> String {
    format!(
        r#"{{"schema_version":"2.0.0","project_context":"","active_baton":"{key}","repos":{{"{key}":{{"name":"{key}","path":"{path}"}}}}}}"#
    )
}

/// spec「foreign bare-name 條目判定為 dirty」。
#[test]
fn foreign_bare_name_entry_is_dirty() {
    let root = tempdir().unwrap();
    write_relay(
        root.path(),
        r#"{"schema_version":"1.0.0","repos":{"llm-stock-analyzer-integration":{"name":"x","path":"llm-stock-analyzer-integration"}}}"#,
    );
    let f = check_relay_file(&root.path().join("relay.json"), None, false);
    assert_eq!(f.verdict, Verdict::Dirty);
    assert!(
        f.reasons
            .iter()
            .any(|r| r.contains("llm-stock-analyzer-integration")),
        "{:?}",
        f.reasons
    );
}

/// spec「monorepo 子目錄不誤殺」：root 下同名子目錄存在 → 不判 foreign。
#[test]
fn monorepo_subdir_entry_not_killed() {
    let root = tempdir().unwrap();
    let sub = root.path().join("graphify-sdk-php");
    std::fs::create_dir_all(&sub).unwrap();
    write_relay(
        root.path(),
        &ws_relay("graphify-sdk-php", "graphify-sdk-php"),
    );
    let f = check_relay_file(&root.path().join("relay.json"), None, false);
    assert_eq!(f.verdict, Verdict::Clean, "{:?}", f.reasons);
}

/// spec「legacy 自身條目（stale bare path）不誤殺」：key == root basename、
/// path 為同名裸名、root 下無同名子目錄（workspace 內容在 root 本身）→ 僅 INFO。
#[test]
fn legacy_self_entry_with_stale_bare_path_not_killed() {
    let root = tempdir().unwrap();
    let name = root.path().file_name().unwrap().to_string_lossy().to_string();
    let body = format!(
        r#"{{"schema_version":"1.0.0","project_context":"self","active_baton":"{name}","repos":{{"{name}":{{"name":"{name}","path":"{name}"}}}}}}"#
    );
    write_relay(root.path(), &body);
    let f = check_relay_file(&root.path().join("relay.json"), None, false);
    assert_eq!(f.verdict, Verdict::Info, "{:?}", f.reasons);
    assert!(
        !f.reasons.iter().any(|r| r.contains("foreign")),
        "不得含 foreign 原因: {:?}",
        f.reasons
    );
    // --fix 不得刪除（INFO 不入將刪清單）。
    let report = super::check_dir(root.path(), None, false).unwrap();
    assert!(super::deletion_targets(std::slice::from_ref(&report)).is_empty());
}

/// spec「絕對路徑指向 root 外判定為 dirty」。
#[test]
fn absolute_path_outside_root_is_dirty() {
    let root = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let sub = outside.path().join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let body = format!(
        r#"{{"schema_version":"2.0.0","repos":{{"a":{{"name":"a","path":"{}"}}}}}}"#,
        sub.display()
    );
    write_relay(root.path(), &body);
    let f = check_relay_file(&root.path().join("relay.json"), None, false);
    assert_eq!(f.verdict, Verdict::Dirty);
    assert!(
        f.reasons.iter().any(|r| r.contains("outside relay root")),
        "{:?}",
        f.reasons
    );
}

/// 無法解析的 JSON → DIRTY（檢查項 1）。
#[test]
fn unparseable_json_is_dirty() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("relay.json"), "{not json").unwrap();
    let f = check_relay_file(&dir.path().join("relay.json"), None, false);
    assert_eq!(f.verdict, Verdict::Dirty);
    assert!(f.reasons.iter().any(|r| r.contains("unparseable")));
}

/// spec「$HOME 正下方 stray 檔判定為 dirty」。
#[test]
fn stray_under_home_is_dirty() {
    let dir = tempdir().unwrap();
    write_relay(dir.path(), &ws_relay("x", "."));
    let f = check_relay_file(&dir.path().join("relay.json"), Some(dir.path()), false);
    assert_eq!(f.verdict, Verdict::Dirty);
    assert!(
        f.reasons.iter().any(|r| r.contains("stray location")),
        "{:?}",
        f.reasons
    );
}

/// scan 模式：git repo 內非 toplevel 的孤兒狀態檔 → DIRTY。
#[test]
fn scan_mode_flags_orphan_inside_git_repo() {
    let repo = tempdir().unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo.path())
        .status()
        .unwrap();
    let sub = repo.path().join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    write_relay(&sub, &ws_relay("x", "."));
    let reports = scan(repo.path(), None);
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].finding.verdict, Verdict::Dirty);
    assert!(
        reports[0]
            .finding
            .reasons
            .iter()
            .any(|r| r.contains("orphan state file")),
        "{:?}",
        reports[0].finding.reasons
    );
}

/// spec「legacy schema 僅提示不判死」：非空全域欄位 → INFO；--fix 不刪。
#[test]
fn legacy_global_fields_are_info_not_dirty() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("api")).unwrap();
    write_relay(
        dir.path(),
        r#"{"schema_version":"1.0.0","project_context":"Legacy","active_baton":"api","repos":{"api":{"name":"api","path":"api"}}}"#,
    );
    let f = check_relay_file(&dir.path().join("relay.json"), None, false);
    assert_eq!(f.verdict, Verdict::Info, "{:?}", f.reasons);
    assert!(f.reasons.iter().any(|r| r.contains("legacy global schema")));
    // fix 刪除清單不含 INFO 檔。
    let report = check_dir(dir.path(), None, false).unwrap();
    assert!(deletion_targets(&[report]).is_empty());
    // 新碼 fresh() 的空 project_context 不觸發 INFO（非空內容才是訊號）。
    // foreign (a) 條件：key 須等於 root basename 或 root 下有同名子目錄——
    // 因 root basename 是 tempdir 隨機名，fixture 补 own 子目錄使其合法。
    let dir2 = tempdir().unwrap();
    std::fs::create_dir_all(dir2.path().join("own")).unwrap();
    write_relay(dir2.path(), &ws_relay("own", "own"));
    let f2 = check_relay_file(&dir2.path().join("relay.json"), None, false);
    assert_eq!(f2.verdict, Verdict::Clean, "{:?}", f2.reasons);
}

/// 多髒全列不短路：unparseable 之外，foreign + stray 同檔兩個原因都在。
#[test]
fn multiple_dirty_reasons_all_reported() {
    let home = tempdir().unwrap();
    write_relay(
        home.path(),
        r#"{"schema_version":"1.0.0","repos":{"ghost":{"name":"g","path":"ghost"}}}"#,
    );
    let f = check_relay_file(&home.path().join("relay.json"), Some(home.path()), false);
    assert_eq!(f.verdict, Verdict::Dirty);
    assert!(f.reasons.len() >= 2, "{:?}", f.reasons);
}

/// spec「fix 只刪 dirty」：INFO 與 CLEAN 檔位元組不變。
#[test]
fn fix_deletes_only_dirty() {
    let dirty = tempdir().unwrap();
    write_relay(dirty.path(), r#"{"repos":{"ghost":{"path":"ghost"}}}"#);
    std::fs::create_dir_all(dirty.path().join(".relay")).unwrap();
    std::fs::write(dirty.path().join(".relay/relay.toon"), "mirror").unwrap();
    let info = tempdir().unwrap();
    write_relay(
        info.path(),
        r#"{"project_context":"Legacy","repos":{"api":{"name":"api","path":"api"}}}"#,
    );
    std::fs::create_dir_all(info.path().join("api")).unwrap();
    let clean = tempdir().unwrap();
    std::fs::create_dir_all(clean.path().join("api")).unwrap();
    write_relay(clean.path(), &ws_relay("api", "api"));
    let info_before = std::fs::read(info.path().join("relay.json")).unwrap();
    let clean_before = std::fs::read(clean.path().join("relay.json")).unwrap();

    let reports = vec![
        check_dir(dirty.path(), None, false).unwrap(),
        check_dir(info.path(), None, false).unwrap(),
        check_dir(clean.path(), None, false).unwrap(),
    ];
    let targets = deletion_targets(&reports);
    assert_eq!(targets.len(), 2, "{targets:?}"); // relay.json + .relay/
    let failures = apply_fix(&targets);
    assert!(failures.is_empty(), "{failures:?}");
    assert!(!dirty.path().join("relay.json").exists());
    assert!(!dirty.path().join(".relay").exists());
    assert_eq!(
        std::fs::read(info.path().join("relay.json")).unwrap(),
        info_before
    );
    assert_eq!(
        std::fs::read(clean.path().join("relay.json")).unwrap(),
        clean_before
    );
}

/// spec「fix 刪除前列清單」：報告節先列將刪路徑。
#[test]
fn fix_reports_targets_before_deletion() {
    let dirty = tempdir().unwrap();
    write_relay(dirty.path(), r#"{"repos":{"ghost":{"path":"ghost"}}}"#);
    let report = check_dir(dirty.path(), None, false).unwrap();
    let targets = deletion_targets(std::slice::from_ref(&report));
    let listing = targets
        .iter()
        .map(|t| t.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    assert!(listing.contains("relay.json"), "{listing}");
}

/// spec「掃描多 repo」：3 repo 一髒二清、退出碼 1。
#[test]
fn scan_reports_all_repos_and_exit_code_1() {
    let repos = tempdir().unwrap();
    let (a, b, c) = (
        repos.path().join("A"),
        repos.path().join("B"),
        repos.path().join("C"),
    );
    for d in [&a, &b, &c] {
        std::fs::create_dir_all(d).unwrap();
        std::fs::create_dir_all(d.join("own")).unwrap();
        write_relay(d, &ws_relay("own", "own"));
    }
    // B: foreign 汙染（bare-name、無同名目錄）
    write_relay(&b, r#"{"repos":{"ghost":{"path":"ghost"}}}"#);
    let reports = scan(repos.path(), None);
    assert_eq!(reports.len(), 3);
    let dirty = reports
        .iter()
        .filter(|r| r.finding.verdict == Verdict::Dirty)
        .count();
    assert_eq!(dirty, 1);
    assert_eq!(exit_code(&reports, None), 1);
    let out = render(&reports);
    assert!(out.contains("[DIRTY]"));
    assert_eq!(
        out.matches("[CLEAN]").count() + out.matches("[INFO]").count(),
        2
    );
}

/// spec「未 init 的 workspace」：無 relay.json → "no relay state file"、退出碼 0、零寫入。
#[test]
fn no_state_file_reports_clean_exit_0() {
    let ws = tempdir().unwrap();
    let reports = scan(ws.path(), None);
    assert!(reports.is_empty());
    assert_eq!(render(&reports), "no relay state file");
    assert_eq!(exit_code(&reports, None), 0);
    assert!(
        std::fs::read_dir(ws.path()).unwrap().count() == 0,
        "不得建立任何檔案"
    );
}

/// spec「CI 可用退出碼」：fix 成功 → 0；刪除失敗 → 1。
#[test]
fn exit_code_contract_after_fix() {
    let reports: Vec<super::Report> = Vec::new();
    assert_eq!(exit_code(&reports, Some(&[])), 0);
    let phantom = tempdir().unwrap();
    let failures = vec![(phantom.path().join("relay.json"), "gone".to_string())];
    assert_eq!(exit_code(&reports, Some(&failures)), 1);
}

/// 掃描root 不存在 → 執行錯誤（退出碼 2 由 CLI 層映射；此處驗證檢查面不 panic）。
#[test]
fn scan_nonexistent_root_yields_empty() {
    let reports = scan(Path::new("/nonexistent/relay-doctor-scan"), None);
    assert!(reports.is_empty());
}
