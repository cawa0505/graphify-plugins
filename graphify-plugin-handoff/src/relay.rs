//! relay* 工具（relayInit / relaySave / relayClose / relaySwitch / relayResume /
//! relayStatus / relayAdd）— frozen legacy 語意（PROTOCOL.md §3-§9，與
//! `@jimmyyen/opencode-code-relay-plugin` 1:1 對照）。
//!
//! 寫入安全（PROTOCOL.md §7）：所有 read-modify-write 走 `RelayPlugin::locked` —
//! fs2 獨佔鎖（`.relay/relay.json.lock`，位於已 gitignore 的 `.relay/` 內）→
//! 重讀磁碟 state → 套用變更 → `state::persist`（relay.json 原子寫 + TOON 鏡像）。

use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;
use std::process::Command;

use fs2::FileExt;
use sha1::{Digest, Sha1};

use crate::handoff;
use crate::root::RELAY_JSON;
use crate::state::{now_iso, Handoff, RelayState, RepoState};
use crate::{Error, RelayPlugin};
use graphify_core::plugin_memory::MemoryQueryCriteria;

// ---------- templates（與 legacy dist/templates/*.md 逐字一致） ----------

const TEMPLATES: [(&str, &str); 3] = [
    ("backend", include_str!("../templates/backend.md")),
    ("frontend", include_str!("../templates/frontend.md")),
    ("infra", include_str!("../templates/infra.md")),
];

fn load_template(kind: Option<&str>, custom: Option<&Path>) -> String {
    if let Some(path) = custom {
        if let Ok(text) = std::fs::read_to_string(path) {
            return text;
        }
    }
    let key = kind.unwrap_or("backend");
    TEMPLATES
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, text)| text.to_string())
        .unwrap_or_else(|| TEMPLATES[0].1.to_string())
}

const KINDS: [&str; 3] = ["backend", "frontend", "infra"];

// ---------- git helpers（best-effort，非 git repo 即優雅降級） ----------

fn git_output(dir: &Path, args: &[&str]) -> String {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn git_run_status(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn git_is_repo(dir: &Path) -> bool {
    git_output(dir, &["rev-parse", "--is-inside-work-tree"]) == "true"
}

fn git_last_commit(dir: &Path) -> String {
    git_output(dir, &["log", "-1", "--format=%H"])
}

fn git_short_stat(dir: &Path) -> String {
    let stat = git_output(dir, &["diff", "--shortstat"]);
    let porcelain = git_output(dir, &["status", "--porcelain"]);
    let files = if porcelain.is_empty() {
        0
    } else {
        porcelain.lines().count()
    };
    if stat.is_empty() {
        format!("{files} files uncommitted")
    } else {
        format!("{stat} ({files} files uncommitted)")
    }
}

fn git_is_ignored(dir: &Path, file: &Path) -> bool {
    git_run_status(
        dir,
        &["check-ignore", "-q", file.to_str().unwrap_or_default()],
    )
}

/// 只 add 存在且未被 gitignore 的檔案/目錄（legacy 用 existsSync，目錄也算；
/// relay.json 被 init seed 的 .gitignore 排除 → close 實際 commit 的是 specs/）。
/// 回傳：commit hash；無可提交 → "nothing to commit (files ignored or missing)"。
fn git_commit(root: &Path, message: &str, files: &[&str]) -> String {
    let addable: Vec<String> = files
        .iter()
        .filter(|f| root.join(f).exists() && !git_is_ignored(root, Path::new(f)))
        .map(|f| f.to_string())
        .collect();
    if addable.is_empty() {
        return "nothing to commit (files ignored or missing)".to_string();
    }
    let mut add_args = vec!["add"];
    add_args.extend(addable.iter().map(|s| s.as_str()));
    if !git_run_status(root, &add_args) {
        return "commit skipped: git add failed".to_string();
    }
    if !git_run_status(root, &["commit", "-m", message]) {
        return "commit skipped: git commit failed".to_string();
    }
    git_last_commit(root)
}

// ---------- spec sync（PROTOCOL.md §5） ----------

fn sha1_12(content: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())[..12].to_string()
}

/// specs/*.md 的檔名（去 .md 尾綴），排序以確保輸出確定性
/// （legacy 用 readdir 原始順序，排序是刻意的小修正，見 ponytail 原則）。
fn list_specs(root: &Path) -> Vec<String> {
    let dir = root.join("specs");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().is_file() && e.path().extension().is_some_and(|x| x == "md"))
                .filter_map(|e| {
                    e.path()
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// 取出 spec 的第一個 #/##/### 標題（或第一行），去標記、截 120 字。
fn extract_intent(content: &str) -> String {
    let line = content
        .lines()
        .find(|l| l.starts_with('#'))
        .unwrap_or_else(|| content.lines().next().unwrap_or(""));
    let trimmed = line.trim_start_matches('#').trim();
    if trimmed.chars().count() > 120 {
        let cut: String = trimmed.chars().take(117).collect();
        format!("{cut}...")
    } else {
        trimmed.to_string()
    }
}

fn spec_intent(root: &Path, repo_name: &str) -> String {
    let path = root.join("specs").join(format!("{repo_name}.md"));
    match std::fs::read_to_string(path) {
        Ok(content) => extract_intent(&content),
        Err(_) => "(no spec yet)".to_string(),
    }
}

/// 比對 specs/*.md 與 `spec_sync.specs` 的 sha1-12，更新快照並回傳 drift 條目
/// `(spec 名, added|modified|unchanged)`。
fn diff_specs(root: &Path, state: &mut RelayState) -> Vec<(String, String)> {
    let specs = list_specs(root);
    let prev = &state.spec_sync.specs;
    let mut next = BTreeMap::new();
    let mut diffs: Vec<(String, String)> = Vec::new();
    for spec in &specs {
        let content = std::fs::read_to_string(root.join("specs").join(format!("{spec}.md")))
            .unwrap_or_default();
        let hash = sha1_12(&content);
        next.insert(spec.clone(), hash.clone());
        let status = match prev.get(spec) {
            None => "added",
            Some(old) if *old == hash => "unchanged",
            Some(_) => "modified",
        };
        diffs.push((spec.clone(), status.to_string()));
    }
    state.spec_sync.specs = next;
    state.spec_sync.last_sync = now_iso();
    let drift: Vec<String> = diffs
        .iter()
        .filter(|(_, s)| s != "unchanged")
        .map(|(name, status)| format!("{name} ({status})"))
        .collect();
    // ponytail: legacy 只在 drift 非空時覆寫，空 drift 時保留舊值（1:1 相容）
    if !drift.is_empty() {
        state.spec_sync.drift = drift;
    }
    diffs
}

/// 一致性檢查（PROTOCOL.md §5）：標題缺失 / CONFLICT / BROKEN / REMOVED 殘留。
fn consistency_check(root: &Path) -> (bool, Vec<String>) {
    let mut issues = Vec::new();
    for spec in list_specs(root) {
        let content = std::fs::read_to_string(root.join("specs").join(format!("{spec}.md")))
            .unwrap_or_default();
        let has_title = content
            .lines()
            .any(|l| l.starts_with('#') && l[1..].chars().next().is_some_and(|c| c.is_whitespace()));
        if !has_title {
            issues.push(format!("{spec}: missing top-level title"));
        }
        let lower = content.to_lowercase();
        if lower.contains("conflict:") {
            issues.push(format!("{spec}: contains CONFLICT marker"));
        }
        if lower.contains("broken:") {
            issues.push(format!("{spec}: contains BROKEN marker"));
        }
        if let Some(removed) = content.lines().find(|l| {
            l.to_lowercase().trim_start_matches("##")
                .trim_start()
                .to_lowercase()
                .starts_with("removed requirements")
        }) {
            if let Some(pos) = content.find(removed) {
                let after = &content[pos + removed.len()..];
                if !after.trim().is_empty() {
                    issues.push(format!("{spec}: has REMOVED requirements (reconcile drift)"));
                }
            }
        }
    }
    (issues.is_empty(), issues)
}

// ---------- rendering（PROTOCOL.md §4） ----------

fn fill(template: &str, vars: &BTreeMap<&str, String>) -> String {
    // legacy regex: /\{\{(\w+)\}\}/g → 未知 key 替換為空字串
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let key = &after[..end];
                out.push_str(vars.get(key).map(String::as_str).unwrap_or(""));
                rest = &after[end + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn vars_for(
    state: &RelayState,
    root: &Path,
    repo: &RepoState,
) -> BTreeMap<&'static str, String> {
    let repo_dir = root.join(&repo.path);
    let is_repo = git_is_repo(&repo_dir);
    let mut vars = BTreeMap::new();
    vars.insert("project_context", non_empty_or(&state.project_context, "(unset)"));
    vars.insert("repo_name", repo.name.clone());
    vars.insert("role", non_empty_or(&repo.role, "(unset)"));
    vars.insert("active_phase", non_empty_or(&repo.active_phase, "(unset)"));
    vars.insert("volatile_state", non_empty_or(&repo.volatile_state, "(unset)"));
    vars.insert("confidence_score", repo.confidence_score.to_string());
    vars.insert(
        "debt_tag",
        if repo.debt_tag.is_empty() {
            "(none)".to_string()
        } else {
            repo.debt_tag.iter().map(|d| format!("- {d}")).collect::<Vec<_>>().join("\n")
        },
    );
    vars.insert(
        "next_session_starter",
        non_empty_or(&repo.next_session_starter, "(none planned)"),
    );
    vars.insert("last_updated", repo.last_updated.clone());
    vars.insert(
        "git_commit",
        if is_repo { git_last_commit(&repo_dir) } else { "(not a git repo)".to_string() },
    );
    vars.insert(
        "git_stat",
        if is_repo { git_short_stat(&repo_dir) } else { "n/a".to_string() },
    );
    vars.insert("spec_intent", spec_intent(root, &repo.name));
    vars.insert("schema_version", state.schema_version.clone());
    vars.insert(
        "handoffs",
        if repo.handoffs.is_empty() {
            "(none)".to_string()
        } else {
            repo.handoffs
                .iter()
                .map(|h| format!("### From {} ({})\n{}", h.source, h.captured_at, h.raw.trim()))
                .collect::<Vec<_>>()
                .join("\n\n")
        },
    );
    vars
}

fn non_empty_or(v: &str, fallback: &str) -> String {
    if v.is_empty() { fallback.to_string() } else { v.to_string() }
}

/// 渲染 RESUME.md 並回傳內文（無尾綴換行，與 legacy 一致）。
fn render_resume(state: &RelayState, root: &Path, repo: &RepoState, kind: Option<&str>) -> Result<String, Error> {
    let key = if kind.is_some_and(|k| KINDS.contains(&k)) { kind } else { Some("backend") };
    let template = load_template(key, None);
    let vars = vars_for(state, root, repo);
    let out = fill(&template, &vars);
    std::fs::write(root.join("RESUME.md"), format!("{out}\n"))?;
    Ok(out)
}

/// 渲染 next_step.md（固定模板，與 legacy 一致）。
fn render_next_step(state: &RelayState, root: &Path, repo: &RepoState) -> Result<String, Error> {
    let template = "# Next session starter — {{repo_name}}\n\n> Run this first in the next session.\n\n{{next_session_starter}}\n\n## Open debts\n{{debt_tag}}\n\n## Confidence\n{{confidence_score}}/5\n\n---\n_Generated {{last_updated}}_\n";
    let vars = vars_for(state, root, repo);
    let out = fill(template, &vars);
    std::fs::write(root.join("next_step.md"), format!("{out}\n"))?;
    Ok(out)
}

// ---------- .gitignore seeding（relayInit） ----------

const GITIGNORE_HEADER: &str = "# Code Relay (local state)";
const GITIGNORE_ENTRIES: [&str; 3] = ["relay.json", "RESUME.md", "next_step.md"];

/// 在 relay root 的 .gitignore 補上 relay 狀態檔（僅 git repo；冪等）。
fn ensure_gitignore(root: &Path) -> Option<String> {
    if !git_is_repo(root) {
        return None;
    }
    let path = root.join(".gitignore");
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let missing: Vec<&str> = GITIGNORE_ENTRIES
        .iter()
        .copied()
        .filter(|e| !content.lines().any(|l| l == *e))
        .collect();
    if missing.is_empty() {
        return None;
    }
    let header = if content.trim().is_empty() {
        format!("{GITIGNORE_HEADER}\n")
    } else {
        format!("\n{GITIGNORE_HEADER}\n")
    };
    let new_content = format!(
        "{}{}{}\n",
        content.trim_end_matches('\n'),
        header,
        missing.join("\n")
    );
    std::fs::write(&path, new_content).ok()?;
    Some(format!(".gitignore updated ({})", missing.join(", ")))
}

// ---------- 工具實作（RelayPlugin methods） ----------

fn basename(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// relaySave 參數（對應 legacy MCP tool args，snake_case 為內部命名）。
#[derive(Debug, Default, Clone)]
pub struct SaveArgs<'a> {
    pub repo: Option<&'a str>,
    pub role: Option<&'a str>,
    pub active_phase: Option<&'a str>,
    pub volatile_state: Option<&'a str>,
    pub confidence: Option<f64>,
    pub next_session_starter: Option<&'a str>,
    pub debt_tag: Option<&'a str>,
    pub kind: Option<&'a str>,
}

impl RelayPlugin {
    /// 啟動基準目錄 = bind 時注入的 workspace root（PROTOCOL.md §9）。
    fn cwd(&self) -> Result<&Path, Error> {
        self.ctx
            .as_ref()
            .map(|c| Path::new(&c.root_path))
            .ok_or(Error::NoRoot)
    }

    /// read-modify-write（PROTOCOL.md §7）：fs2 鎖 → 重讀磁碟 → 變更 → persist。
    fn locked<F: FnOnce(&mut RelayState)>(&mut self, f: F) -> Result<(), Error> {
        let root = self.root.clone().ok_or(Error::NoRoot)?;
        crate::state::ensure_relay_dir(&root)?;
        let lock = File::create(root.join(".relay/relay.json.lock"))?;
        lock.lock_exclusive()?;
        let mut state = crate::state::load(&root.join(RELAY_JSON))?
            .unwrap_or_else(RelayState::fresh);
        f(&mut state);
        crate::state::persist(&root, &mut state)?;
        self.state = Some(state);
        drop(lock);
        Ok(())
    }

    /// relayInit：建立 relay root（walk-up 已存在則拒絕）。
    pub fn relay_init(&mut self, project_context: &str, _kind: Option<&str>) -> Result<String, Error> {
        let start = self.cwd()?.to_path_buf();
        if let Some(existing) = crate::root::resolve_root(&start) {
            return Err(Error::RootExists(existing.display().to_string()));
        }
        std::fs::create_dir_all(start.join("specs"))?;
        std::fs::create_dir_all(start.join(".code-relay"))?;
        let mut state = RelayState::fresh();
        state.project_context = project_context.to_string();
        crate::state::persist(&start, &mut state)?;
        let gitignore_note = ensure_gitignore(&start);
        self.root = Some(start.clone());
        self.state = Some(state);
        let mut lines = vec![
            format!("Initialized relay at {}/relay.json", start.display()),
            "- specs/ and .code-relay/ created".to_string(),
        ];
        if let Some(note) = gitignore_note {
            lines.push(format!("- {note}"));
            lines.push("- run relaySave to register the current repo".to_string());
        } else {
            lines.push("- run relaySave to register the current repo".to_string());
        }
        Ok(lines.join("\n"))
    }

    /// relaySave：寫入/更新目前 repo 狀態並渲染 RESUME.md。
    pub fn relay_save(&mut self, args: SaveArgs<'_>) -> Result<String, Error> {
        let root = self.root.clone().ok_or(Error::NoRoot)?;
        let cwd = self.cwd()?.to_path_buf();
        let repo_name = args
            .repo
            .map(str::to_string)
            .unwrap_or_else(|| basename(&cwd));
        self.locked(|state| {
            let repo = state
                .repos
                .entry(repo_name.clone())
                .or_insert_with(|| RepoState::for_repo(&repo_name));
            repo.name = repo_name.clone();
            if let Some(v) = args.role {
                repo.role = v.to_string();
            }
            if let Some(v) = args.active_phase {
                repo.active_phase = v.to_string();
            }
            if let Some(v) = args.volatile_state {
                repo.volatile_state = v.to_string();
            }
            if let Some(v) = args.next_session_starter {
                repo.next_session_starter = v.to_string();
            }
            if let Some(v) = args.debt_tag {
                repo.debt_tag = v
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            if let Some(v) = args.confidence {
                repo.confidence_score = (v.round().clamp(1.0, 5.0)) as u8;
            }
            repo.last_updated = now_iso();
            if state.active_baton.is_empty() {
                state.active_baton = repo_name.clone();
            }
        })?;
        let state = self.state.as_ref().ok_or(Error::NoRoot)?;
        let repo = state
            .repos
            .get(&repo_name)
            .ok_or_else(|| Error::RepoUnknown(repo_name.clone()))?;
        let rendered = render_resume(state, &root, repo, args.kind)?;
        Ok(format!(
            "Saved state for \"{repo_name}\".\nActive baton: {}\n\n{rendered}",
            state.active_baton
        ))
    }

    /// relayClose：consistency check + spec diff + next_step.md + 原子 commit。
    pub fn relay_close(&mut self, repo: Option<&str>, next: Option<&str>) -> Result<String, Error> {
        let root = self.root.clone().ok_or(Error::NoRoot)?;
        let cwd = self.cwd()?.to_path_buf();
        let repo_name = repo
            .map(str::to_string)
            .unwrap_or_else(|| basename(&cwd));
        let (ok, issues) = consistency_check(&root);
        let mut diffs: Vec<(String, String)> = Vec::new();
        self.locked(|state| {
            diffs = diff_specs(&root, state);
            if let Some(n) = next {
                if let Some(r) = state.repos.get_mut(&repo_name) {
                    r.next_session_starter = n.to_string();
                    r.last_updated = now_iso();
                }
            }
        })?;
        let state = self.state.as_ref().ok_or(Error::NoRoot)?;
        let repo = state
            .repos
            .get(&repo_name)
            .ok_or_else(|| Error::RepoUnknown(repo_name.clone()))?;
        let next_md = render_next_step(state, &root, repo)?;
        let mut lines = vec![
            format!("Closing ritual for \"{repo_name}\"."),
            format!("Consistency: {}", if ok { "OK" } else { "ISSUES" }),
        ];
        lines.extend(issues.iter().map(|i| format!("  - {i}")));
        if diffs.is_empty() {
            lines.push("Spec sync: no changes".to_string());
        } else {
            let joined = diffs
                .iter()
                .map(|(name, status)| format!("{name}:{status}"))
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("Spec sync: {joined}"));
        }
        lines.push(String::new());
        lines.push(next_md);
        let commit_info = if git_is_repo(&root) {
            let message = format!("relay: close {repo_name} [{}]", now_iso());
            format!("committed: {}", git_commit(&root, &message, &["relay.json", "specs"]))
        } else {
            "not a git repo — skipped commit".to_string()
        };
        lines.push(String::new());
        lines.push(commit_info);
        if let Some(note) = self.persist_close_snapshot(&root, state, repo) {
            lines.push(String::new());
            lines.push(note);
        }
        Ok(lines.join("\n"))
    }

    /// Best-effort：把關閉時的狀態落成 HandoffSnapshot 並同步至 graphify.db。
    ///
    /// 成功為純副作用（close 輸出零變動）；失敗不回傳 Err（不破壞 close 流程），
    /// 只附一行輕量 note。
    fn persist_close_snapshot(
        &self,
        root: &Path,
        state: &RelayState,
        repo: &RepoState,
    ) -> Option<String> {
        let ws_key = self.workspace_key()?;
        let goal = if !repo.next_session_starter.is_empty() {
            repo.next_session_starter.clone()
        } else if !repo.volatile_state.is_empty() {
            repo.volatile_state.clone()
        } else {
            "(none)".to_string()
        };
        let created_at = handoff::unix_now();
        let snapshot = handoff::build_snapshot(
            format!("snap-{created_at}"),
            state.state_snapshot.last_session.clone(),
            ws_key,
            goal,
            Vec::new(), // pinned_node_ids：P4 graph capture 接入後由 caller 注入
            String::new(), // focused_subgraph_toon：P4 graph capture 接入後由 caller 注入
            MemoryQueryCriteria {
                target_symbols: Vec::new(),
                domain_categories: Vec::new(),
                search_terms: Vec::new(),
            },
            created_at,
        );
        let db_path = self
            .registry_path
            .clone()
            .unwrap_or_else(graphify_registry::registry_db_path);
        match handoff::sync_to_registry_at(&db_path, ws_key, &root.display().to_string(), &snapshot)
        {
            Ok(()) => None,
            Err(e) => Some(format!("Snapshot: skipped — {e}")),
        }
    }

    /// relaySwitch：把 baton 交給另一個已註冊 repo。
    pub fn relay_switch(&mut self, repo: &str, kind: Option<&str>) -> Result<String, Error> {
        let root = self.root.clone().ok_or(Error::NoRoot)?;
        let repo_name = repo.to_string();
        let exists = self
            .state
            .as_ref()
            .is_some_and(|s| s.repos.contains_key(&repo_name));
        if !exists {
            return Err(Error::RepoNotRegistered(repo_name));
        }
        self.locked(|state| {
            state.active_baton = repo_name.clone();
        })?;
        let state = self.state.as_ref().ok_or(Error::NoRoot)?;
        let repo = state
            .repos
            .get(&repo_name)
            .ok_or_else(|| Error::RepoUnknown(repo_name.clone()))?;
        let resume = render_resume(state, &root, repo, kind)?;
        Ok(format!("Baton passed to \"{repo_name}\".\n\n{resume}"))
    }

    /// relayResume：渲染指定（或 baton）repo 的 RESUME。
    pub fn relay_resume(&mut self, repo: Option<&str>, kind: Option<&str>) -> Result<String, Error> {
        let root = self.root.clone().ok_or(Error::NoRoot)?;
        let state = self.state.as_ref().ok_or(Error::NoRoot)?;
        let target = repo
            .map(str::to_string)
            .unwrap_or_else(|| state.active_baton.clone());
        if target.is_empty() {
            return Err(Error::NoActiveBaton);
        }
        let repo_state = state
            .repos
            .get(&target)
            .ok_or_else(|| Error::RepoUnknown(target.clone()))?;
        render_resume(state, &root, repo_state, kind)
    }

    /// relayStatus：全 repo 摘要。
    pub fn relay_status(&mut self) -> Result<String, Error> {
        let root = self.root.clone().ok_or(Error::NoRoot)?;
        let state = self.state.as_ref().ok_or(Error::NoRoot)?;
        let mut lines = vec![
            format!("Relay root: {}", root.display()),
            format!(
                "Project: {}",
                non_empty_or(&state.project_context, "(unset)")
            ),
            format!(
                "Active baton: {}",
                non_empty_or(&state.active_baton, "(none)")
            ),
            format!("Repos ({}):", state.repos.len()),
        ];
        for repo in state.repos.values() {
            let mut line = format!(
                "  - {} [{}] conf={}",
                repo.name,
                non_empty_or(&repo.active_phase, "?"),
                repo.confidence_score
            );
            if !repo.handoffs.is_empty() {
                line.push_str(&format!(" · {} handoff(s)", repo.handoffs.len()));
            }
            if !repo.next_session_starter.is_empty() {
                line.push_str(&format!(
                    " → {}",
                    repo.next_session_starter.chars().take(60).collect::<String>()
                ));
            }
            lines.push(line);
        }
        let specs = list_specs(&root);
        lines.push(format!(
            "Specs: {}",
            if specs.is_empty() { "(none)".to_string() } else { specs.join(", ") }
        ));
        lines.push(format!(
            "Drift: {}",
            if state.spec_sync.drift.is_empty() {
                "(none)".to_string()
            } else {
                state.spec_sync.drift.join(", ")
            }
        ));
        lines.push(format!("Updated: {}", state.updated_at));
        Ok(lines.join("\n"))
    }

    /// relayAdd：把 TODO/handoff 文件收進 relay 狀態與 open_threads。
    pub fn relay_add(&mut self, file: &Path, repo: Option<&str>) -> Result<String, Error> {
        let cwd = self.cwd()?.to_path_buf();
        let repo_name = repo
            .map(str::to_string)
            .unwrap_or_else(|| basename(&cwd));
        let resolved = cwd.join(file);
        if !resolved.is_file() {
            return Err(Error::FileNotFound(file.display().to_string()));
        }
        let raw = std::fs::read_to_string(&resolved)?;
        let source = basename(&resolved);
        let mut total_threads = 0usize;
        self.locked(|state| {
            let repo_state = state
                .repos
                .entry(repo_name.clone())
                .or_insert_with(|| RepoState::for_repo(&repo_name));
            repo_state.name = repo_name.clone();
            repo_state.handoffs.push(Handoff {
                source: source.clone(),
                captured_at: now_iso(),
                raw: raw.clone(),
            });
            repo_state.last_updated = now_iso();
            for line in raw.lines().map(str::trim).filter(|l| !l.is_empty()) {
                let t = line.to_string();
                if !state.state_snapshot.open_threads.contains(&t) {
                    state.state_snapshot.open_threads.push(t);
                }
            }
            total_threads = state.state_snapshot.open_threads.len();
        })?;
        let line_count = raw
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .count();
        Ok(format!(
            "Added handoff \"{source}\" to \"{repo_name}\".\nParsed {line_count} line(s) into open_threads.\nTotal open threads: {total_threads}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::tempdir;

    fn bind(dir: &Path) -> RelayPlugin {
        // registry 指向 sandbox 內 temp db，測試不得碰真實 graphify.db
        let mut p = RelayPlugin::new().with_registry_path(dir.join("graphify.db"));
        p.bind_for_cli(dir);
        p
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn load_template_known_and_fallback() {
        assert!(load_template(Some("backend"), None).starts_with("# Resume"));
        assert!(load_template(Some("frontend"), None).contains("{{repo_name}}"));
        assert!(load_template(Some("infra"), None).contains("{{repo_name}}"));
        // 未知 kind → backend 模板（legacy 語意）
        assert_eq!(load_template(Some("nope"), None), load_template(Some("backend"), None));
        assert_eq!(load_template(None, None), load_template(Some("backend"), None));
    }

    #[test]
    fn fill_substitutes_known_vars() {
        let mut vars = BTreeMap::new();
        vars.insert("repo_name", "api".to_string());
        vars.insert("volatile_state", "在寫測試".to_string());
        let out = fill("R: {{repo_name}} V: {{volatile_state}} U: {{unknown}}", &vars);
        assert_eq!(out, "R: api V: 在寫測試 U: ");
    }

    #[test]
    fn extract_intent_prefers_heading_and_truncates() {
        let doc = "# Full Title Line That Is Very Long And Should Be Truncated Past 120 Characters Because Legacy Cuts The Spec Intent String At 120 With An Ellipsis Suffix Marker\nbody";
        let intent = extract_intent(doc);
        assert_eq!(intent.chars().count(), 120);
        assert!(intent.ends_with("..."));
        assert_eq!(extract_intent("plain first line"), "plain first line");
        assert_eq!(extract_intent("### Sub\nmore"), "Sub");
        assert_eq!(extract_intent(""), "");
    }

    #[test]
    fn sha1_12_matches_known_vector() {
        // sha1("hello") = aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d
        assert_eq!(sha1_12("hello"), "aaf4c61ddcc5");
        assert_eq!(sha1_12(""), "da39a3ee5e6b");
    }

    #[test]
    fn consistency_check_flags_all_rules() {
        let dir = tempdir().unwrap();
        let specs = dir.path().join("specs");
        std::fs::create_dir_all(&specs).unwrap();
        std::fs::write(specs.join("no-title.md"), "no heading here\n").unwrap();
        std::fs::write(specs.join("conflict.md"), "# T\nsee conflict: between a and b\n").unwrap();
        std::fs::write(specs.join("broken.md"), "# T\nbroken: something\n").unwrap();
        std::fs::write(specs.join("removed.md"), "# T\n## REMOVED Requirements\nold requirement\n").unwrap();
        std::fs::write(specs.join("removed-clean.md"), "# T\n## REMOVED Requirements\n").unwrap();
        std::fs::write(specs.join("clean.md"), "# Good\nfine\n").unwrap();

        let (ok, issues) = consistency_check(dir.path());
        assert!(!ok);
        assert_eq!(issues.len(), 4, "{issues:?}");
        assert!(issues.iter().any(|i| i == "no-title: missing top-level title"));
        assert!(issues.iter().any(|i| i == "conflict: contains CONFLICT marker"));
        assert!(issues.iter().any(|i| i == "broken: contains BROKEN marker"));
        assert!(issues.iter().any(|i| i == "removed: has REMOVED requirements (reconcile drift)"));
    }

    #[test]
    fn diff_specs_tracks_added_and_modified() {
        let dir = tempdir().unwrap();
        let specs = dir.path().join("specs");
        std::fs::create_dir_all(&specs).unwrap();
        std::fs::write(specs.join("a.md"), "# A\none\n").unwrap();
        std::fs::write(specs.join("b.md"), "# B\none\n").unwrap();
        let mut state = RelayState::fresh();
        let first = diff_specs(dir.path(), &mut state);
        let statuses: Vec<&str> = first.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(statuses, vec!["added", "added"]);
        assert_eq!(state.spec_sync.drift.len(), 2);

        // 改 b → modified；a 不變
        std::fs::write(specs.join("b.md"), "# B\ntwo\n").unwrap();
        let second = diff_specs(dir.path(), &mut state);
        let statuses: Vec<&str> = second.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(statuses, vec!["unchanged", "modified"]);
        assert_eq!(state.spec_sync.drift.len(), 1);
        assert!(state.spec_sync.drift[0].contains("modified"));
    }

    #[test]
    fn init_refuses_existing_root_and_files_created() {
        let dir = tempdir().unwrap();
        let mut p = bind(dir.path());
        let out = p.relay_init("測試專案", None).unwrap();
        assert!(out.starts_with("Initialized relay at"));
        assert!(out.contains("- specs/ and .code-relay/ created"));
        assert!(dir.path().join("relay.json").is_file());
        assert!(dir.path().join("specs").is_dir());
        assert!(dir.path().join(".code-relay").is_dir());
        assert!(dir.path().join(".relay/relay.toon").is_file());
        assert!(dir.path().join(".relay/.gitignore").is_file());

        // 二次 init → 拒絕（frozen 文字）
        let err = p.relay_init("x", None).unwrap_err().to_string();
        assert!(err.starts_with("relay.json already exists at"), "{err}");
    }

    #[test]
    fn save_clamps_confidence_and_splits_debt_tag() {
        let dir = tempdir().unwrap();
        let mut p = bind(dir.path());
        p.relay_init("p", None).unwrap();
        p.relay_save(SaveArgs {
            repo: Some("api"),
            confidence: Some(4.6),
            debt_tag: Some("a,b, ,c"),
            role: Some("backend"),
            active_phase: Some("dev"),
            volatile_state: Some("寫 relay.rs"),
            next_session_starter: Some("繼續 Slice B"),
            ..Default::default()
        })
        .unwrap();
        let state = p.state().unwrap();
        let repo = &state.repos["api"];
        assert_eq!(repo.confidence_score, 5); // round(4.6) clamp 1..=5
        assert_eq!(repo.debt_tag, vec!["a", "b", "c"]);
        assert_eq!(repo.role, "backend");
        assert_eq!(state.active_baton, "api");
        assert!(dir.path().join("RESUME.md").is_file());
    }

    #[test]
    fn full_flow_lifecycle() {
        let dir = tempdir().unwrap();
        let mut p = bind(dir.path());
        p.relay_init("demo", None).unwrap();

        // save
        let out = p
            .relay_save(SaveArgs {
                repo: Some("api"),
                confidence: Some(4.6),
                debt_tag: Some("x"),
                ..Default::default()
            })
            .unwrap();
        assert!(out.starts_with("Saved state for \"api\"."), "{out}");
        assert!(out.contains("Active baton: api"));
        // 第二個 repo
        p.relay_save(SaveArgs { repo: Some("web"), ..Default::default() })
            .unwrap();

        // status
        let out = p.relay_status().unwrap();
        assert!(out.contains("Relay root:"));
        assert!(out.contains("Active baton: api"));
        assert!(out.contains("Repos (2):"));
        assert!(out.contains("conf=5"));
        assert!(out.contains("Specs: (none)"));

        // resume → baton api
        let out = p.relay_resume(None, None).unwrap();
        assert!(out.contains("Resume — api"), "{out}");

        // switch 未註冊 repo → frozen 文字
        let err = p.relay_switch("nope", None).unwrap_err().to_string();
        assert_eq!(err, "repo \"nope\" not registered. Run relaySave in that repo first.");
        // switch 成功
        let out = p.relay_switch("web", None).unwrap();
        assert!(out.starts_with("Baton passed to \"web\"."));
        assert_eq!(p.state().unwrap().active_baton, "web");

        // resume 未指定 → baton（web）
        let out = p.relay_resume(None, None).unwrap();
        assert!(out.contains("Resume — web"), "{out}");

        // add TODO 文件（3 行非空：# tasks / - fix x / - fix y）
        let todo = dir.path().join("todo.md");
        std::fs::write(&todo, "# tasks\n\n- fix x\n- fix y\n").unwrap();
        let out = p.relay_add(&todo, Some("api")).unwrap();
        assert_eq!(
            out,
            "Added handoff \"todo.md\" to \"api\".\nParsed 3 line(s) into open_threads.\nTotal open threads: 3"
        );
        // 重複 add → 去重
        let out = p.relay_add(&todo, Some("api")).unwrap();
        assert!(out.contains("Parsed 3 line(s)"), "{out}");
        assert_eq!(p.state().unwrap().state_snapshot.open_threads.len(), 3);
        assert_eq!(p.state().unwrap().repos["api"].handoffs.len(), 2);

        // close
        let out = p.relay_close(Some("api"), Some("下一個 session 從 Slice C 開始")).unwrap();
        assert!(out.contains("Closing ritual for \"api\"."));
        assert!(out.contains("Consistency: OK"));
        assert!(out.contains("Spec sync: no changes"));
        assert!(out.contains("not a git repo — skipped commit"));
        assert!(dir.path().join("next_step.md").is_file());
        let next = std::fs::read_to_string(dir.path().join("next_step.md")).unwrap();
        assert!(next.contains("Next session starter — api"));
        assert!(next.contains("下一個 session 從 Slice C 開始"));
        assert!(next.contains("Confidence"), "{next}");
        assert!(next.contains("5/5"), "{next}");
    }

    #[test]
    fn add_reports_file_not_found() {
        let dir = tempdir().unwrap();
        let mut p = bind(dir.path());
        p.relay_init("p", None).unwrap();
        let err = p.relay_add(Path::new("missing.md"), None).unwrap_err().to_string();
        assert_eq!(err, "File not found: missing.md");
    }

    #[test]
    fn resume_unknown_repo_text() {
        let dir = tempdir().unwrap();
        let mut p = bind(dir.path());
        p.relay_init("p", None).unwrap();
        p.relay_save(SaveArgs { repo: Some("api"), ..Default::default() })
            .unwrap();
        let err = p.relay_resume(Some("ghost"), None).unwrap_err().to_string();
        assert_eq!(err, "repo \"ghost\" not registered.");
        let err = p.relay_resume(Some(""), None).unwrap_err().to_string();
        assert_eq!(err, "No active baton set and no repo given. Run relaySwitch <repo> first.");
    }

    #[test]
    fn no_root_frozen_error_for_all_tools() {
        let dir = tempdir().unwrap();
        let mut p = bind(dir.path()); // 無 relay.json
        assert_eq!(p.relay_status().unwrap_err().to_string(), "No relay.json found. Run relayInit first.");
        assert_eq!(p.relay_save(SaveArgs::default()).unwrap_err().to_string(), "No relay.json found. Run relayInit first.");
        assert_eq!(p.relay_close(None, None).unwrap_err().to_string(), "No relay.json found. Run relayInit first.");
    }

    #[test]
    fn close_commits_in_git_repo_and_seeds_gitignore() {
        let dir = tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["config", "user.email", "relay@test.local"]);
        git(dir.path(), &["config", "user.name", "relay test"]);
        git(dir.path(), &["config", "commit.gpgsign", "false"]);

        let mut p = bind(dir.path());
        let out = p.relay_init("p", None).unwrap();
        assert!(out.contains(".gitignore updated (relay.json, RESUME.md, next_step.md)"), "{out}");
        assert!(dir.path().join(".gitignore").is_file());

        p.relay_save(SaveArgs { repo: Some("api"), ..Default::default() })
            .unwrap();
        // close 實際 commit 的是 specs/（relay.json 被 .gitignore 排除）
        std::fs::create_dir_all(dir.path().join("specs")).unwrap();
        std::fs::write(dir.path().join("specs/api.md"), "# API\nspec body\n").unwrap();
        let out = p.relay_close(Some("api"), None).unwrap();
        assert!(out.contains("committed: "), "{out}");
        assert!(!out.contains("skipped"), "{out}");
        // commit 只含 specs/，不含被忽略的 relay.json / 渲染檔 / toon 鏡像
        let log = git_output(dir.path(), &["log", "--oneline", "--stat", "-1"]);
        assert!(log.contains("relay: close api"), "{log}");
        assert!(log.contains("api.md"), "{log}");
        assert!(!log.contains("relay.json"), "{log}");
        assert!(!log.contains("RESUME.md"), "{log}");
        assert!(!log.contains("relay.toon"), "{log}");
    }

    #[test]
    fn close_persists_snapshot_to_registry() {
        let dir = tempdir().unwrap();
        let mut p = bind(dir.path());
        p.relay_init("p", None).unwrap();
        p.relay_save(SaveArgs { repo: Some("api"), ..Default::default() })
            .unwrap();
        let out = p.relay_close(Some("api"), Some("接 P4 collection 寫入")).unwrap();
        assert!(out.contains("Closing ritual for \"api\"."));
        assert!(!out.contains("Snapshot: skipped"), "成功時不該有 note:\n{out}");
        // registry 內有 1 筆，task_goal = close 的 next
        let ws = p.workspace_key().unwrap();
        let db = graphify_registry::RegistryDb::open(&dir.path().join("graphify.db")).unwrap();
        let rows = db.list_snapshots(ws).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].payload.task_goal, "接 P4 collection 寫入");
    }

    #[test]
    fn close_with_broken_registry_appends_note_only() {
        let dir = tempdir().unwrap();
        // 父路徑是「檔案」→ create_dir_all 得 ENOTDIR → RegistryDb::open 失敗
        // → 只附 note，不破壞 close（root 也躲不掉 ENOTDIR）
        std::fs::write(dir.path().join("blocker"), "x").unwrap();
        let broken = dir.path().join("blocker/graphify.db");
        let mut p = RelayPlugin::new().with_registry_path(broken);
        p.bind_for_cli(dir.path());
        p.relay_init("p", None).unwrap();
        p.relay_save(SaveArgs { repo: Some("api"), ..Default::default() })
            .unwrap();
        let out = p.relay_close(Some("api"), None).unwrap();
        assert!(out.contains("Closing ritual for \"api\"."));
        assert!(out.contains("Snapshot: skipped"), "{out}");
    }
}
