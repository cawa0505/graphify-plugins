//! E2E: real `argus graph` output → ArgusPlugin → RetestSignal.
//!
//! Drives the plugin against the actual ArgusOrchestrator binary and a real
//! phase store (fixture prepared by the caller via env), covering openspec
//! tasks 5.1 (ingest + toon summary) and 5.2 (spec change → correct phase).
//!
//! Run (from GraphifyPlugins/):
//!   ARGUS_BIN=<abs path to argus> ARGUS_STATE_DIR=<state dir> \
//!     cargo test -p graphify-plugin-argus --test e2e_real_argus -- --nocapture

use std::sync::Mutex;

use graphify_core::{GraphUpdateEvent, GraphUpdateKind, GraphifyPlugin, WorkspaceContext};
use graphify_plugin_argus::ArgusPlugin;

/// e2e needs the real `argus` binary + a prepared state dir; without them the
/// tests skip (silent pass) so bare `cargo test` stays green. Set both to run.
fn e2e_ready() -> Option<(String, String)> {
    match (std::env::var("ARGUS_BIN"), std::env::var("ARGUS_STATE_DIR")) {
        (Ok(bin), Ok(dir)) => Some((bin, dir)),
        _ => {
            eprintln!("skip: set ARGUS_BIN + ARGUS_STATE_DIR to run the real-binary e2e");
            None
        }
    }
}

/// 5.1: proactive sync (`sync_toon(None)`) must run the real `argus graph`
/// subprocess, cache the graph, and return a summary packet with node counts.
#[test]
fn e2e_ingest_real_argus_graph() {
    let Some((argus_bin, state_dir)) = e2e_ready() else {
        return;
    };

    let mut p = ArgusPlugin::new()
        .with_argus_bin(&argus_bin)
        .with_state_dir(&state_dir);
    p.bind(WorkspaceContext::new("w-e2e", "argus-e2e", "/tmp"));

    let packet = p.sync_toon(None);
    let raw = String::from_utf8(packet).expect("packet is utf8");
    println!("--- sync_toon(None) packet ---\n{raw}");

    let meta = graphify_plugin_argus::sync::parse_meta(&raw);
    assert_eq!(meta.format_version.as_deref(), Some("1.0.0"));
    assert_eq!(meta.workspace_key.as_deref(), Some("w-e2e"));
    assert!(meta.error.is_none(), "proactive sync failed: {raw}");

    // Real fixture graph: phase demo + 2 workers + 2 evidence = 5 nodes.
    let graph = p.graph().expect("graph cached after proactive sync");
    assert_eq!(graph.nodes.len(), 5, "fixture has 5 nodes");
    assert_eq!(graph.edges.len(), 5, "fixture has 5 edges");
    let kinds: Vec<&str> = graph.nodes.iter().map(|n| n.kind.as_str()).collect();
    assert!(kinds.contains(&"phase"));
    assert_eq!(kinds.iter().filter(|k| **k == "worker").count(), 2);
    assert_eq!(kinds.iter().filter(|k| **k == "evidence").count(), 2);

    // Argus exports `file_path`; core aliases it into source_file. The retest
    // matcher depends on this mapping — verify it survived ingestion.
    let worker = graph
        .nodes
        .iter()
        .find(|n| n.id.0 == "worker:demo/a")
        .expect("worker node present");
    assert_eq!(worker.source_file, ".argus/phases/demo.toml");
}

/// 5.2: a change touching the phase spec must produce exactly one
/// RetestSignal for the right phase, delivered through the notify callback.
#[test]
fn e2e_spec_change_emits_retest_signal_for_correct_phase() {
    let Some((argus_bin, state_dir)) = e2e_ready() else {
        return;
    };

    let mut p = ArgusPlugin::new()
        .with_argus_bin(&argus_bin)
        .with_state_dir(&state_dir);
    p.bind(WorkspaceContext::new("w-e2e", "argus-e2e", "/tmp"));

    let seen: std::sync::Arc<Mutex<Vec<serde_json::Value>>> =
        std::sync::Arc::new(Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&seen);
    p.set_notify_callback(Some(Box::new(move |v| {
        sink.lock().expect("sink lock").push(v);
    })));

    // Baseline: ingest the real graph first (no signals on first sync).
    let packet = p.sync_toon(None);
    let raw = String::from_utf8(packet).expect("packet is utf8");
    assert!(
        graphify_plugin_argus::sync::parse_meta(&raw)
            .error
            .is_none(),
        "baseline ingest failed: {raw}"
    );

    // Simulate `graphify index` touching the phase spec file node.
    let event = GraphUpdateEvent::new(
        "w-e2e",
        vec![graphify_core::NodeId(".argus/phases/demo.toml".to_string())],
        GraphUpdateKind::Indexed,
    );
    p.on_graph_updated(&event);

    {
        let signals = seen.lock().expect("sink lock");
        assert_eq!(signals.len(), 1, "expected exactly one RetestSignal");
        let s = &signals[0];
        assert_eq!(s["kind"], "RetestSignal");
        assert_eq!(s["phase"], "demo", "signal must name the demo phase");
        assert_eq!(s["workspace_key"], "w-e2e");
        let nodes = s["nodes"].as_array().expect("nodes array");
        assert!(nodes.contains(&serde_json::json!("phase:demo")));
    } // guard dropped — std Mutex is not reentrant, re-locking below would deadlock

    // A change touching an unrelated file must stay silent.
    let quiet = GraphUpdateEvent::new(
        "w-e2e",
        vec![graphify_core::NodeId("src/unrelated.rs".to_string())],
        GraphUpdateKind::Indexed,
    );
    p.on_graph_updated(&quiet);
    assert_eq!(seen.lock().expect("sink lock").len(), 1, "no new signal");
}
