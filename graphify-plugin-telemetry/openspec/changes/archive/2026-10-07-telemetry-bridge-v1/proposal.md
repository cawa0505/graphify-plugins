# Proposal — graphify-plugin-telemetry（telemetry-bridge-v1）

> 以下內容為 Architecture & Technical Specification v1.0.0 逐字轉錄（§1-2）。

🛡️ Architecture & Technical Specification: graphify-plugin-telemetry
Document Version: 1.0.0

Status: Approved Blueprint

System Layer: Layer 2 (Operation Engine) & Layer 4 (MCP Efficiency Layer Gateway)

Core Philosophy: Pure Bridge (Draco MCP First), Zero Heavy OTLP Engine, Symbol-Native Hotspot Binding.

## 1. 系統定位與架構全景 (System Architecture)
graphify-plugin-telemetry 是一個專為 Draco MCP（或標準 OpenTelemetry / pprof / Flamegraph 導出檔）設計的 「可觀測性熱點與效能語意橋接器 (Observability & Hotspot Symbol Bridge)」。

本外掛不重造 複雜的 OTLP Receiver、Prometheus 輪詢器或 Flamegraph 解析引擎。它採用 Pure Bridge 姿態，透過 Draco MCP（以 System User 運行於背景的可觀測性 Scraper/Collector）抓取實時效能指標，並利用 Graphify Core AST 將這些指標 0ms 確定性地釘（Pin）在 Canonical AST Node ID（格式：{file_path}:{kind}:{name}）上，託管於 graphify.db。

當 Coding Agent 讀取 .toon 拓撲或進行效能重構時，能一秒感知哪些函數是線上 p99 Hotspot 或 Memory Leak 盲點。

Plaintext
┌─────────────────────────────────────────────────────────────┐
│  Draco MCP Server (Python / Go, System User Domain)         │
│  - 專責：對接 Prometheus / OTEL / Jaeger / pprof Scrape      │
└─────────────────────────────▲───────────────────────────────┘
                              │
                              │ MCP Tool Call (Draco MCP Client)
                              ▼
┌─────────────────────────────────────────────────────────────┐
│  graphify-plugin-telemetry (Rust GraphifyPlugin)            │
│  ├── Line/Stack-to-Symbol Resolver                          │
│  ├── telemetry_bindings DAO (graphify.db 託管)              │
│  └── Hotspot Impact & Threshold Guard Engine                │
└──────────────┬──────────────────────────────▲───────────────┘
               │                              │
  1. Register Tools                           │ 2. Dispatch Events
               ▼                              │
┌─────────────────────────────────────────────────────────────┐
│  graphify-mcp (MCP Gateway)                                 │
│  - 自動註冊 telemetry_ingest / telemetry_get_context       │
│  - 轉發 notifications/telemetry/hotspot_alert 給 Agent    │
└──────────────────────────────┬──────────────────────────────┘
                               │
                               ▼
                        Coding Agent (.toon Consumer)

## 2. 鋼鐵邊界與職責劃分 (Scope & Responsibilities)

### 2.1 ✅ In-Scope（本外掛職責）

Draco MCP 介面對接：透過 MCP Protocol 調用背景 Draco MCP 抓取 Top Slowest Functions / Memory Hotspots。

檔案型預備 Ingest：支援直接讀取標準 JSON Profile 檔（如 pprof.json 或 otel-metrics.json），確保離線確定性。

Symbol Mapping：將堆疊/點位（file_path + function_name / line_number）升維對齊至 Graphify 實體 Node ID（{file_path}:{kind}:{name}）。

SQLite 數據管理：於 graphify.db 維護 telemetry_bindings 表，進行 workspace_key 隔離。

.toon 語意合成：於 sync_toon 時將 p99 Latency、Memory Allocation 與 Call Count 高濃縮合成進 .toon 拓撲。

雙向 MCP Push (Notifications)：當程式碼修改（on_graph_updated）觸及 p99 > Threshold 的 Critical Hotspot 時，主動發送廣播。

### 2.2 ❌ Out-of-Scope（硬性禁止事項）

⛔ 不自建 OTLP/Prometheus 服務端：不安裝、不監聽 4317/4318 埠，不維護高頻 Time-series 記憶體 DB（全權交給 Draco MCP）。

⛔ 不執行主動 Profiling 採樣：不修改 binary 注入 eBPF 或 pprof agent。

⛔ 不修改 Core AST 圖譜：不將 Telemetry 節點硬塞進 Core Petgraph，避免污染程式碼拓撲結構。
