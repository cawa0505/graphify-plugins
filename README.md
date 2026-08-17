# Graphify Plugins

Graphify 的外掛生態系集合。每個 Plugin 遵循 **Pure Symbol Bridge** 模式：將外部領域資料（測試覆蓋率、效能追蹤、審查記錄等）與 Graphify 的 canonical AST node 綁定，讓 Agent 透過 .toon 取得全知視角。

Graphify 核心引擎在 [graphify-rust](https://github.com/cawa0505/graphify-rust)。

## 已實作 Plugins

| Plugin | 外部資料源 | 綁定語意 |
|--------|-----------|----------|
| [handoff](graphify-plugin-handoff/) | Human/Agent Active Session | Agent 當前注意力與任務目標 |
| [opendoc](graphify-plugin-opendoc/) | RFC / Spec / OpenAPI | 業務規範與設計合約 |
| [review](graphify-plugin-review/) | code-review-graph | 歷史審查警示與 PR 評語 |
| [test-coverage](graphify-plugin-test-coverage/) | LCOV / pytest-cov | 測試覆蓋率與盲區 |
| [telemetry](graphify-plugin-telemetry/) | OpenTelemetry / Flamegraph | 線上 Latency / Memory 瓶頸 |

## 規劃中

- **secguard** — Cargo Audit / Snyk CVE 漏洞綁定
- **git-blame** — Git Churn / 領域知識綁定

## 開發

```bash
# 所有 plugin 在 workspace 中統一管理
cargo build
cargo test
```

每個 Plugin 的獨立 README 含詳細規格、MCP tools 與驗收條件。

## 授權

MIT