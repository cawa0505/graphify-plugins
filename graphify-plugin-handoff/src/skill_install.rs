//! Agent-skill installation (`graphify handoff skill install`).
//!
//! Distributes the embedded [`SKILL.md`](../SKILL.md) — the dual-track relay
//! skill (MCP + CLI fallback) — to local agent config directories as managed
//! copies. The content is embedded at build time (`include_str!`), so the
//! binary is self-contained and needs no repo path at install time.
//!
//! **Fail-safe rule**: installed copies carry a managed marker; `uninstall`
//! only ever removes files that carry that marker. Pre-existing user files at
//! the same path are never overwritten or deleted.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Marker embedded in installed copies; `uninstall` removes only marked files.
pub const MARKER: &str = "managed by graphify handoff skill";

/// Embedded canonical skill content (single source of truth, repo-root
/// `SKILL.md`).
const SKILL_CONTENT: &str = include_str!("../SKILL.md");

/// Agent whose skill directory we can install into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Opencode,
    Claude,
    Cursor,
    Cline,
}

impl Agent {
    pub const ALL: [Agent; 4] = [Agent::Opencode, Agent::Claude, Agent::Cursor, Agent::Cline];

    /// Parse a CLI agent name (`opencode|claude|cursor|cline`, case-insensitive).
    pub fn parse(name: &str) -> Option<Agent> {
        match name.to_ascii_lowercase().as_str() {
            "opencode" => Some(Agent::Opencode),
            "claude" => Some(Agent::Claude),
            "cursor" => Some(Agent::Cursor),
            "cline" => Some(Agent::Cline),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Agent::Opencode => "opencode",
            Agent::Claude => "claude",
            Agent::Cursor => "cursor",
            Agent::Cline => "cline",
        }
    }
}

/// Installation scope: user-global (`$HOME`) vs project (`cwd`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    User,
    Project,
}

impl Scope {
    /// Parse a CLI scope name (`user|project`, case-insensitive).
    pub fn parse(name: &str) -> Option<Scope> {
        match name.to_ascii_lowercase().as_str() {
            "user" => Some(Scope::User),
            "project" => Some(Scope::Project),
            _ => None,
        }
    }
}

/// Hard failure of an install/uninstall run (soft per-target issues land in
/// the report's `skipped` list instead).
#[derive(Debug, Error)]
pub enum SkillInstallError {
    #[error("cannot resolve $HOME")]
    NoHome,
    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

/// Per-target install results.
#[derive(Debug, Default)]
pub struct InstallReport {
    pub installed: Vec<PathBuf>,
    pub skipped: Vec<(PathBuf, String)>,
}

/// Per-target uninstall results.
#[derive(Debug, Default)]
pub struct UninstallReport {
    pub removed: Vec<PathBuf>,
    pub skipped: Vec<(PathBuf, String)>,
}

impl InstallReport {
    pub fn total(&self) -> usize {
        self.installed.len() + self.skipped.len()
    }
}

impl UninstallReport {
    pub fn total(&self) -> usize {
        self.removed.len() + self.skipped.len()
    }
}

/// Detect agents that have an existing config on this machine (default
/// install target set).
pub fn detect_agents(home: &Path, cwd: &Path) -> Vec<Agent> {
    let mut agents = Vec::new();
    if home.join(".config/opencode").exists() {
        agents.push(Agent::Opencode);
    }
    if home.join(".claude").exists() {
        agents.push(Agent::Claude);
    }
    if cwd.join(".cursor").exists() {
        agents.push(Agent::Cursor);
    }
    if cwd.join(".clinerules").exists() {
        agents.push(Agent::Cline);
    }
    agents
}

/// Install the skill into the given agents (default-scope aware: both user
/// and project targets for each agent).
pub fn install(targets: &[Agent], scope: Scope) -> Result<InstallReport, SkillInstallError> {
    let home = home_dir()?;
    let cwd = env::current_dir()?;
    install_into(&home, &cwd, targets, scope)
}

/// Uninstall the skill from the given agents.
pub fn uninstall(targets: &[Agent], scope: Scope) -> Result<UninstallReport, SkillInstallError> {
    let home = home_dir()?;
    let cwd = env::current_dir()?;
    uninstall_into(&home, &cwd, targets, scope)
}

/// Installed content: the canonical skill plus the managed marker inserted
/// right after the frontmatter block.
fn content_for_install() -> String {
    let marker = format!("<!-- {MARKER} -->\n");
    match SKILL_CONTENT.find("\n---\n") {
        // frontmatter closes at the second `---` line; the pattern is 5 bytes
        Some(pos) => {
            let (head, tail) = SKILL_CONTENT.split_at(pos + 5);
            format!("{head}{marker}{tail}")
        }
        None => format!("{marker}{SKILL_CONTENT}"),
    }
}

/// Target paths for one agent at one scope.
fn target_paths(home: &Path, cwd: &Path, agent: Agent, scope: Scope) -> Vec<PathBuf> {
    match (agent, scope) {
        (Agent::Opencode, Scope::User) => vec![home
            .join(".config/opencode/skills/graphify-relay/SKILL.md")],
        (Agent::Opencode, Scope::Project) => vec![cwd.join(".opencode/skills/graphify-relay/SKILL.md")],
        (Agent::Claude, Scope::User) => vec![home.join(".claude/skills/graphify-relay/SKILL.md")],
        // Claude in-repo uses the repo-root canonical SKILL.md; nothing to install.
        (Agent::Claude, Scope::Project) => vec![],
        (Agent::Cursor, Scope::Project) => vec![cwd.join(".cursor/rules/graphify-relay.mdc")],
        (Agent::Cursor, Scope::User) => vec![],
        (Agent::Cline, Scope::Project) => vec![cwd.join(".clinerules")],
        (Agent::Cline, Scope::User) => vec![],
    }
}

/// Install into explicit roots (testable without touching `$HOME`).
fn install_into(
    home: &Path,
    cwd: &Path,
    targets: &[Agent],
    scope: Scope,
) -> Result<InstallReport, SkillInstallError> {
    let mut report = InstallReport::default();
    let content = content_for_install();
    for agent in targets {
        for path in target_paths(home, cwd, *agent, scope) {
            match write_managed(&path, &content) {
                Ok(true) => report.installed.push(path),
                Ok(false) => report
                    .skipped
                    .push((path, "exists without managed marker — not touched".into())),
                Err(e) => report.skipped.push((path, e.to_string())),
            }
        }
    }
    Ok(report)
}

/// Uninstall into explicit roots.
fn uninstall_into(
    home: &Path,
    cwd: &Path,
    targets: &[Agent],
    scope: Scope,
) -> Result<UninstallReport, SkillInstallError> {
    let mut report = UninstallReport::default();
    for agent in targets {
        for path in target_paths(home, cwd, *agent, scope) {
            if !path.exists() {
                continue;
            }
            if is_managed(&path)? {
                fs::remove_file(&path)?;
                report.removed.push(path);
            } else {
                report
                    .skipped
                    .push((path, "exists without managed marker — left alone".into()));
            }
        }
    }
    Ok(report)
}

/// Write a managed copy: overwrite if absent or already managed; refuse
/// (return `Ok(false)`) when a user-created file occupies the target path.
fn write_managed(path: &Path, content: &str) -> Result<bool, SkillInstallError> {
    if path.exists() && !is_managed(path)? {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(true)
}

/// True when the file carries the managed marker.
fn is_managed(path: &Path) -> Result<bool, SkillInstallError> {
    Ok(fs::read_to_string(path)
        .map(|content| content.contains(MARKER))
        .unwrap_or(false))
}

fn home_dir() -> Result<PathBuf, SkillInstallError> {
    env::var_os("HOME").map(PathBuf::from).ok_or(SkillInstallError::NoHome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn install_writes_marked_copy() {
        let home = tempdir().unwrap();
        let cwd = tempdir().unwrap();
        let report = install_into(home.path(), cwd.path(), &[Agent::Opencode], Scope::User).unwrap();
        let path = home
            .path()
            .join(".config/opencode/skills/graphify-relay/SKILL.md");
        assert_eq!(report.installed, vec![path.clone()]);
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains(MARKER));
        assert!(content.contains("# Code Relay — Skill"));
        assert!(content.contains("graphify handoff status"));
    }

    #[test]
    fn reinstall_is_idempotent() {
        let home = tempdir().unwrap();
        let cwd = tempdir().unwrap();
        let first =
            install_into(home.path(), cwd.path(), &[Agent::Opencode], Scope::User).unwrap();
        let second =
            install_into(home.path(), cwd.path(), &[Agent::Opencode], Scope::User).unwrap();
        assert_eq!(first.installed, second.installed);
        let path = home
            .path()
            .join(".config/opencode/skills/graphify-relay/SKILL.md");
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content.matches(MARKER).count(), 1);
    }

    #[test]
    fn install_never_overwrites_user_file() {
        let home = tempdir().unwrap();
        let cwd = tempdir().unwrap();
        let path = home
            .path()
            .join(".config/opencode/skills/graphify-relay/SKILL.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "# my own skill\n").unwrap();
        let report = install_into(home.path(), cwd.path(), &[Agent::Opencode], Scope::User).unwrap();
        assert!(report.installed.is_empty());
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(fs::read_to_string(&path).unwrap(), "# my own skill\n");
    }

    #[test]
    fn uninstall_removes_only_managed_files() {
        let home = tempdir().unwrap();
        let cwd = tempdir().unwrap();
        let report = install_into(home.path(), cwd.path(), &[Agent::Opencode], Scope::User).unwrap();
        let path = &report.installed[0];
        // add an unmanaged file at a sibling agent target
        let claude_path = home.path().join(".claude/skills/graphify-relay/SKILL.md");
        fs::create_dir_all(claude_path.parent().unwrap()).unwrap();
        fs::write(&claude_path, "not managed\n").unwrap();

        let out = uninstall_into(home.path(), cwd.path(), &[Agent::Opencode], Scope::User).unwrap();
        assert_eq!(out.removed, vec![path.clone()]);
        assert!(!path.exists());
        assert!(claude_path.exists()); // untouched: different agent target
    }

    #[test]
    fn uninstall_skips_unmanaged_target() {
        let home = tempdir().unwrap();
        let cwd = tempdir().unwrap();
        let path = home.path().join(".claude/skills/graphify-relay/SKILL.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "# user content\n").unwrap();
        let out = uninstall_into(home.path(), cwd.path(), &[Agent::Claude], Scope::User).unwrap();
        assert!(out.removed.is_empty());
        assert_eq!(out.skipped.len(), 1);
        assert_eq!(fs::read_to_string(&path).unwrap(), "# user content\n");
    }

    #[test]
    fn cline_skips_unmanaged_rules_file() {
        let home = tempdir().unwrap();
        let cwd = tempdir().unwrap();
        let rules = cwd.path().join(".clinerules");
        fs::write(&rules, "always verify with cargo test\n").unwrap();
        let report = install_into(home.path(), cwd.path(), &[Agent::Cline], Scope::Project).unwrap();
        assert!(report.installed.is_empty());
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(
            fs::read_to_string(&rules).unwrap(),
            "always verify with cargo test\n"
        );
    }

    #[test]
    fn claude_project_scope_installs_nothing() {
        let home = tempdir().unwrap();
        let cwd = tempdir().unwrap();
        let report = install_into(home.path(), cwd.path(), &[Agent::Claude], Scope::Project).unwrap();
        assert!(report.installed.is_empty());
    }

    #[test]
    fn detect_agents_looks_at_home_and_cwd() {
        let home = tempdir().unwrap();
        let cwd = tempdir().unwrap();
        fs::create_dir_all(home.path().join(".config/opencode")).unwrap();
        fs::create_dir_all(home.path().join(".claude")).unwrap();
        fs::create_dir_all(cwd.path().join(".cursor")).unwrap();
        let agents = detect_agents(home.path(), cwd.path());
        assert_eq!(
            agents,
            vec![Agent::Opencode, Agent::Claude, Agent::Cursor]
        );
    }

    #[test]
    fn content_marker_lands_after_frontmatter() {
        let content = content_for_install();
        // marker comment sits directly after the frontmatter close
        assert!(content.contains("---\n<!-- managed by graphify handoff skill -->"));
        // the canonical repo file itself carries no marker
        assert!(!SKILL_CONTENT.contains(MARKER));
    }
}
