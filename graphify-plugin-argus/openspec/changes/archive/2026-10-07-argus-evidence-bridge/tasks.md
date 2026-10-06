# Tasks — argus-evidence-bridge

## 1. 骨架
- [x] 1.1 建 `graphify-plugin-argus/Cargo.toml`（path deps: `graphify-core`、`graphify-registry`；serde/serde_json/thiserror；dev-dep tempfile）
- [x] 1.2 `GraphifyPlugins/Cargo.toml` members 加入 `graphify-plugin-argus`

## 2. toon 封包（`src/sync.rs`）
- [x] 2.1 `FORMAT_VERSION` / `emit_packet` / `emit_error_packet` / `parse_meta` / `major_mismatch`（比照 review plugin 慣例）
- [x] 2.2 `now_rfc3339`（stdlib civil-from-days，不引入 chrono）
- [x] 2.3 測試：封包 roundtrip、error 封包、MAJOR 不符

## 3. RetestSignal（`src/retest.rs`）
- [x] 3.1 `RetestSignal` struct（Serialize）+ `detect_retests(graph, seeds, workspace_key) -> Vec<RetestSignal>`
- [x] 3.2 判定：worker 節點 id 命中種子，或其 `source_file` 命中種子節點 → 受影響 phase；同 phase 去重
- [x] 3.3 測試：命中、未命中、同 phase 去重、空種子

## 4. Plugin（`src/lib.rs`）
- [x] 4.1 `ArgusPlugin` 結構（workspace_key / root_path / argus_bin / state_dir / graph_cache / notify_cb / prev_node_ids）+ `new` / `with_argus_bin` / `with_state_dir` / `bind_for_cli`
- [x] 4.2 `run_argus_graph()`：subprocess 執行 `argus graph -o <tmp>`，讀回 JSON → `GraphOutput`（失敗回 `Result`）
- [x] 4.3 `impl GraphifyPlugin`：`get_id` / `bind` / `get_workspace_key` / `sync_toon`（雙路徑）/ `on_graph_updated` / `set_notify_callback` / `on_health_check`
- [x] 4.4 `impact_seeds`：modified_nodes 優先，空則 prev/cur diff，首次同步只建 baseline
- [x] 4.5 `emit_notify`：有 callback 才送
- [x] 4.6 測試：bind roundtrip、sync_toon(Some) 解析、health check（binary 不存在 → false）、on_graph_updated 產 RetestSignal 且 callback 收到

## 5. 真資料對接驗證
- [x] 5.1 用 ArgusOrchestrator 真實 `argus graph` 產出餵 plugin，驗節點/邊與 toon 解析
- [x] 5.2 驗證 phase spec 變動 → RetestSignal（phase 正確）

## 6. 文件與索引
- [x] 6.1 `README.md` + `README.zh-TW.md`（功能、限制、設定、驗證結果）
- [x] 6.2 根 `README.md` 已實作 Plugins 表加入 argus 一列

## 7. 驗收
- [x] 7.1 `cargo test`（workspace）全綠
- [x] 7.2 `cargo clippy --all-targets` 零警告
- [x] 7.3 README 與實際能力一致（含未實作項明確標示）

## 8. 驗證記錄（2026-09-23 e2e 實證）

- **5.1**：真實 ArgusOrchestrator binary（`target/debug/argus`）+ 真實 phase store
  （fixture phase `demo`：a→b DAG，2 workers + 2 evidence）。`sync_toon(None)`
  子程序實跑 `argus graph -o <tmp>`，快取 5 nodes / 5 edges，toon 封包
  `plugin_data["argus"] = {nodes:5, edges:5}`。Argus 匯出的 `file_path` 經
  graphify-core `#[serde(alias)]` 正確映射進 `source_file`
  （`worker:demo/a` → `.argus/phases/demo.toml`）。
- **5.2**：`on_graph_updated`（seed = `.argus/phases/demo.toml`，Indexed）→
  恰好 1 個 RetestSignal，`phase == "demo"`、`workspace_key` 正確、
  `nodes` 含 `phase:demo`；無關 seed（`src/unrelated.rs`）不發訊號。
- **發現並修復**：Argus CLI 原本無 `--version`，plugin `on_health_check`
  spawn `argus --version` 恆敗 → probe 顯示 Unavailable。已在 Argus 端補上
  `--version` / `-V`（exit 0），probe 轉 Healthy。
- **7.1/7.2**：GraphifyPlugins workspace `cargo test` 全綠（含 2 個 e2e）、
  `cargo clippy --all-targets` 零警告、`cargo fmt --check` 通過。
  e2e 測試在無 `ARGUS_BIN`/`ARGUS_STATE_DIR` env 時優雅 skip，裸 `cargo test` 不炸。
- 測試檔：`graphify-plugin-argus/tests/e2e_real_argus.rs`（真 binary e2e，
  env-gated）。
