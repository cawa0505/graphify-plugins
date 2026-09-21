# Tasks — argus-evidence-bridge

## 1. 骨架
- [ ] 1.1 建 `graphify-plugin-argus/Cargo.toml`（path deps: `graphify-core`、`graphify-registry`；serde/serde_json/thiserror；dev-dep tempfile）
- [ ] 1.2 `GraphifyPlugins/Cargo.toml` members 加入 `graphify-plugin-argus`

## 2. toon 封包（`src/sync.rs`）
- [ ] 2.1 `FORMAT_VERSION` / `emit_packet` / `emit_error_packet` / `parse_meta` / `major_mismatch`（比照 review plugin 慣例）
- [ ] 2.2 `now_rfc3339`（stdlib civil-from-days，不引入 chrono）
- [ ] 2.3 測試：封包 roundtrip、error 封包、MAJOR 不符

## 3. RetestSignal（`src/retest.rs`）
- [ ] 3.1 `RetestSignal` struct（Serialize）+ `detect_retests(graph, seeds, workspace_key) -> Vec<RetestSignal>`
- [ ] 3.2 判定：worker 節點 id 命中種子，或其 `source_file` 命中種子節點 → 受影響 phase；同 phase 去重
- [ ] 3.3 測試：命中、未命中、同 phase 去重、空種子

## 4. Plugin（`src/lib.rs`）
- [ ] 4.1 `ArgusPlugin` 結構（workspace_key / root_path / argus_bin / state_dir / graph_cache / notify_cb / prev_node_ids）+ `new` / `with_argus_bin` / `with_state_dir` / `bind_for_cli`
- [ ] 4.2 `run_argus_graph()`：subprocess 執行 `argus graph -o <tmp>`，讀回 JSON → `GraphOutput`（失敗回 `Result`）
- [ ] 4.3 `impl GraphifyPlugin`：`get_id` / `bind` / `get_workspace_key` / `sync_toon`（雙路徑）/ `on_graph_updated` / `set_notify_callback` / `on_health_check`
- [ ] 4.4 `impact_seeds`：modified_nodes 優先，空則 prev/cur diff，首次同步只建 baseline
- [ ] 4.5 `emit_notify`：有 callback 才送
- [ ] 4.6 測試：bind roundtrip、sync_toon(Some) 解析、health check（binary 不存在 → false）、on_graph_updated 產 RetestSignal 且 callback 收到

## 5. 真資料對接驗證
- [ ] 5.1 用 ArgusOrchestrator 真實 `argus graph` 產出餵 plugin，驗節點/邊與 toon 解析
- [ ] 5.2 驗證 phase spec 變動 → RetestSignal（phase 正確）

## 6. 文件與索引
- [ ] 6.1 `README.md` + `README.zh-TW.md`（功能、限制、設定、驗證結果）
- [ ] 6.2 根 `README.md` 已實作 Plugins 表加入 argus 一列

## 7. 驗收
- [ ] 7.1 `cargo test`（workspace）全綠
- [ ] 7.2 `cargo clippy --all-targets` 零警告
- [ ] 7.3 README 與實際能力一致（含未實作項明確標示）
