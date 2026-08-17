# Graphify Plugin 生態系：Pure Symbol Bridge 外掛宇宙

經過前面把 handoff、opendoc 和 review 三個外掛收斂成極致優雅的「Pure Symbol Bridge」哲學後，我們其實已經摸索出 Graphify Plugin 生態系最重要的金科玉律：

> Graphify Core 只做確定性的 AST 拓撲與衝擊半徑計算；外掛不重造各領域的執行引擎，而是做各領域數據與 canonical_ast_node_id 之間的 Bridge & Binding。

沿著這個 Pure Bridge Pattern 繼續延伸，以下是幾個質感極高、對 Agent 開發具備爆發性乘數效應 (Multiplicative Effect) 的 Plugin 點子：

## 1. 🧪 graphify-plugin-test-coverage（測試與覆蓋率語意橋接器）

**痛點：** Agent 在寫 Code 或修 Bug 時，完全不知道「自己改的這段 Code 有沒有測試保護？」以及「跑測試失敗時，到底是哪一個 AST Node 爆掉了？」。傳統 Coverage 工具（如 tarpaulin / lcov / pytest-cov）產出的是給人類看的 HTML/LCOV 檔案。

**Bridge 做法：**

- **Ingest：** 讀取標準 lcov.info 或 coverage.json。
- **Bind：** 將行號覆蓋率升維釘在 canonical_ast_node_id 上，並註記 test_coverage_status（e.g., Uncovered, Covered 85%, Flaky）。

**Agent Context 賦能 (.toon)：**

```text
[crate::auth::verify_token (AST Node)]
 ├── ⚠️ Review Warning: PR #42 Security Flaw
 └── 🧪 Coverage: 0% (Blindspot! 建議補測試)
```

**殺手級情境：** Agent 要重構一個函數前，看一眼 .toon 發現它是 0% Coverage，會主動先寫 Test 再重構，避免改爆代碼。

## 2. ⚡ graphify-plugin-telemetry 或 profile（線上效能與 Tracing 點位橋接器）

**痛點：** Agent 幫專案做效能優化 (Performance Profiling) 時，只能靠通篇通篇看程式碼盲猜，不知道「線上真實運行時，最慢的瓶頸/Hotspot 到底在哪個函數？」。

**Bridge 做法：**

- **Ingest：** 讀取 OpenTelemetry / Jaeger Trace / Flamegraph (火焰圖) 導出的 JSON，或者 pprof 數據。
- **Bind：** 將線上採樣到的 p99 Latency、Allocated Memory 或 Flamegraph Hotspots 釘在 AST Symbol 上。

**Agent Context 賦能 (.toon)：**

```text
[crate::db::query_users (AST Node)]
 ├── ⚡ Profiling Hotspot: p99 1,200ms (SQL Bottleneck)
 └── 🔗 Impact Radius: 影響上游 8 個 API Endpoint
```

**殺手級情境：** 你對 Agent 說：「幫我優化 API 響應時間」，Agent 透過 Graphify 直接定位到被標註為 p99 Hotspot 的 AST 節點，1 秒鎖定戰場，不必盲目全專案掃描。

## 3. 🚨 graphify-plugin-secguard 或 snyk（資安漏洞與 CVE 語意橋接器）

**痛點：** Snyk、Cargo Audit、Trivy 等 SCA (Software Composition Analysis) 工具會回傳一堆漏洞清單（例如 CVE-2026-12345 在某第三方 crate 或內部函數）。但 Agent 拿著一份文字 Report，很難視覺化它在專案 AST 裡的擴散影響。

**Bridge 做法：**

- **Ingest：** 讀取 cargo-audit --json 或 Snyk/Dependabot 的 CVE 導出檔。
- **Bind：** 將漏洞資訊與對應的 AST Cargo/Import 節點綁定。

**Agent Context 賦能 (.toon)：**

```text
[crate::crypto::legacy_md5 (AST Node)]
 ├── 🚨 Vulnerability: CVE-2026-8888 (High Severity: Broken Crypto)
 └── 🔗 Downstream Impact: 被 3 個內部模組調用
```

**殺手級情境：** 當第三方庫爆發 CVE 時，Plugin 不僅標出漏洞，還利用 Graphify 的 BFS 衝擊半徑引擎，告訴 Agent：「整個專案中，哪些業務邏輯函數會被這個 CVE 污染」，Agent 能精準替換或加上 Sanitizer。

## 4. 📜 graphify-plugin-git-blame（領域知識與歷史變動頻率橋接器）

**痛點：** 有些代碼雖然寫得很怪，但背後是有歷史包袱（Domain Tribal Knowledge）或頻繁改動（High Churn Rate）。Agent 經常把那些「為了特殊邊界條件寫的 Code」當成冗餘給重構掉。

**Bridge 做法：**

- **Ingest：** 輕量調用 git log / git blame 分析模組歷史。
- **Bind：** 將 churn_rate（近一個月修改次數）、primary_authors 與最後修改的 Commit Message 釘在 AST Symbol 上。

**Agent Context 賦能 (.toon)：**

```text
[crate::payment::stripe_webhook (AST Node)]
 ├── 📜 Git Churn: High (修改 28 次/月 - 脆弱區域)
 └── 💬 Last Note: "Fix Apple Pay edge case in JP region"
```

**殺手級情境：** Agent 一看到 Git Churn: High 加上特定的 Commit Note，在重構時會變得極度謹慎，主動保留那些看起來像 bug 的邊界條件處置。

## 🏛️ 外掛宇宙的終極藍圖 (The Grand Matrix)

如果把這些 Plugin 放在一起，Graphify 就真的變成了 Codebase 的「神經總線 (Neural Bus)」：

| Plugin | 外部數據源 (Input Bridge) | 釘在 AST 上的核心語意 |
| --- | --- | --- |
| handoff | Human/Agent Active Session | 「Agent 當前的注意力與任務目標」 |
| opendoc | RFC / Spec / OpenAPI | 「業務規範與設計合約」 |
| review | code-review-graph | 「歷史審查警示與 PR 評語」 |
| test-coverage | LCOV / pytest-cov | 「測試覆蓋率與盲區 Warning」 |
| telemetry | OpenTelemetry / Flamegraph | 「線上真實 Latency / Memory 瓶頸」 |
| secguard | Cargo Audit / Snyk CVE | 「資安漏洞與衝擊擴散半徑」 |

**核心價值：** 無論外部數據是來自測試、效能分析、資安掃描還是文件，Graphify 都不必重寫這些工具，只要 0ms 將點位 Pin 在 AST 拓撲上，Agent 讀一次 .toon 就能擁有「全知視角」！

## Plugin 文件與索引流程

Graphify Plugin 的研究、規格、實作與驗證流程統一由根目錄 `AGENTS.md` 約束：

1. 先研究真實資料源與架構邊界，不發明不存在的 server、工具名稱或能力。
2. 先完成 In/Out-of-Scope、Ingest、Binding、Symbol Mapping、MCP tools 與驗收規格，再開始實作。
3. 實作維持 Pure Symbol Bridge，不重造領域執行引擎或 Core AST 圖譜。
4. 完成測試、lint/typecheck、必要的 graphify-mcp 整合檢查，並同步 Plugin README。
5. 每次研究、規格、實作或重大架構里程碑完成後，在同一工作階段回頭更新本文件。

本文件只描述已確認的現況。規劃中的能力必須明確標示；只有通過驗證的能力才可列為已完成。
