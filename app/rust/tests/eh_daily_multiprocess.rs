#![cfg(windows)]

use rust_lib_app::eh_subscription::{
    claim_daily, recover_daily_run, try_acquire_collection_lock, update_daily_run, EhAutomationRun,
    EhRules,
};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn test_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "rch-eh-daily-{label}-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).expect("create isolated test root");
    root
}

fn config_path(root: &Path) -> PathBuf {
    let path = root.join("eh_subscription_rules.json");
    let mut rules = EhRules::default();
    rules.automation.daily_enabled = true;
    std::fs::write(&path, rules.to_json()).expect("write isolated enabled config");
    path
}

fn spawn_test(test_name: &str, mode: &str, path: &Path, ready: &Path, gate: &Path) -> Child {
    Command::new(std::env::current_exe().expect("current Rust test executable"))
        .args(["--exact", test_name, "--nocapture"])
        .env("RCH_EH_DAILY_CHILD_MODE", mode)
        .env("RCH_EH_DAILY_CONFIG", path)
        .env("RCH_EH_DAILY_READY", ready)
        .env("RCH_EH_DAILY_GATE", gate)
        .spawn()
        .expect("start isolated child process")
}

fn wait_for_paths(paths: &[&Path]) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !paths.iter().all(|path| path.exists()) {
        assert!(
            Instant::now() < deadline,
            "child process did not reach barrier"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_gate(gate: &Path) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !gate.exists() {
        assert!(Instant::now() < deadline, "parent did not release barrier");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn parse_rules(path: &Path) -> EhRules {
    EhRules::from_json(&std::fs::read_to_string(path).expect("read isolated config"))
        .expect("parse isolated config")
}

#[test]
fn two_independent_processes_can_claim_a_date_only_once() {
    if std::env::var("RCH_EH_DAILY_CHILD_MODE").as_deref() == Ok("race") {
        let path = PathBuf::from(std::env::var_os("RCH_EH_DAILY_CONFIG").unwrap());
        let ready = PathBuf::from(std::env::var_os("RCH_EH_DAILY_READY").unwrap());
        let gate = PathBuf::from(std::env::var_os("RCH_EH_DAILY_GATE").unwrap());
        std::fs::write(&ready, b"ready").expect("signal child ready");
        wait_for_gate(&gate);
        let (claimed, _) = claim_daily(&path, "2026-10-12").expect("claim in child process");
        std::fs::write(ready.with_extension("result"), claimed.to_string())
            .expect("write child claim result");
        return;
    }

    let root = test_root("race");
    let path = config_path(&root);
    let gate = root.join("start");
    let ready_a = root.join("ready-a");
    let ready_b = root.join("ready-b");
    let child_a = spawn_test(
        "two_independent_processes_can_claim_a_date_only_once",
        "race",
        &path,
        &ready_a,
        &gate,
    );
    let child_b = spawn_test(
        "two_independent_processes_can_claim_a_date_only_once",
        "race",
        &path,
        &ready_b,
        &gate,
    );
    wait_for_paths(&[&ready_a, &ready_b]);
    std::fs::write(&gate, b"go").expect("release both independent processes");

    let output_a = child_a.wait_with_output().expect("wait for first child");
    let output_b = child_b.wait_with_output().expect("wait for second child");
    assert!(
        output_a.status.success(),
        "first child failed: {}",
        String::from_utf8_lossy(&output_a.stderr)
    );
    assert!(
        output_b.status.success(),
        "second child failed: {}",
        String::from_utf8_lossy(&output_b.stderr)
    );
    let claims = [
        ready_a.with_extension("result"),
        ready_b.with_extension("result"),
    ]
    .iter()
    .filter(|path| std::fs::read_to_string(path).is_ok_and(|value| value == "true"))
    .count();
    assert_eq!(claims, 1, "only one process may win the durable claim");
    let rules = parse_rules(&path);
    assert_eq!(
        rules.automation.last_auto_attempt_date.as_deref(),
        Some("2026-10-12")
    );
    assert_eq!(
        rules
            .automation
            .last_run
            .as_ref()
            .map(|run| run.status.as_str()),
        Some("claimed")
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn child_claim_before_network_is_not_retried_after_process_exit() {
    if std::env::var("RCH_EH_DAILY_CHILD_MODE").as_deref() == Ok("claimed") {
        let path = PathBuf::from(std::env::var_os("RCH_EH_DAILY_CONFIG").unwrap());
        let ready = PathBuf::from(std::env::var_os("RCH_EH_DAILY_READY").unwrap());
        let gate = PathBuf::from(std::env::var_os("RCH_EH_DAILY_GATE").unwrap());
        let (claimed, _) = claim_daily(&path, "2026-10-12").expect("claim before network");
        assert!(claimed);
        std::fs::write(ready, b"claimed").expect("signal durable claim");
        wait_for_gate(&gate);
        return;
    }

    let root = test_root("claimed-crash");
    let path = config_path(&root);
    let ready = root.join("claimed");
    let gate = root.join("stop-child");
    let mut child = spawn_test(
        "child_claim_before_network_is_not_retried_after_process_exit",
        "claimed",
        &path,
        &ready,
        &gate,
    );
    wait_for_paths(&[&ready]);
    child
        .kill()
        .expect("terminate only the child started by this test");
    let _ = child.wait();

    let recovered = recover_daily_run(&path).expect("recover stale claim after child exit");
    let rules = EhRules::from_json(&recovered).expect("parse recovered rules");
    assert_eq!(
        rules
            .automation
            .last_run
            .as_ref()
            .map(|run| run.status.as_str()),
        Some("interrupted_unknown")
    );
    assert!(!claim_daily(&path, "2026-10-12").unwrap().0);
    assert!(claim_daily(&path, "2026-10-13").unwrap().0);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn live_collection_is_not_recovered_as_a_crash_and_kill_releases_its_lease() {
    if std::env::var("RCH_EH_DAILY_CHILD_MODE").as_deref() == Ok("running") {
        let path = PathBuf::from(std::env::var_os("RCH_EH_DAILY_CONFIG").unwrap());
        let ready = PathBuf::from(std::env::var_os("RCH_EH_DAILY_READY").unwrap());
        let gate = PathBuf::from(std::env::var_os("RCH_EH_DAILY_GATE").unwrap());
        assert!(claim_daily(&path, "2026-10-12").unwrap().0);
        update_daily_run(
            &path,
            EhAutomationRun {
                date: "2026-10-12".into(),
                source: "daily_auto".into(),
                status: "running".into(),
                ..Default::default()
            },
        )
        .expect("persist running state");
        let _lease = try_acquire_collection_lock(&path)
            .expect("acquire collection lease")
            .expect("no other collection should be active");
        std::fs::write(ready, b"running").expect("signal running state and active lease");
        wait_for_gate(&gate);
        return;
    }

    let root = test_root("running-crash");
    let path = config_path(&root);
    let ready = root.join("running");
    let gate = root.join("stop-child");
    let mut child = spawn_test(
        "live_collection_is_not_recovered_as_a_crash_and_kill_releases_its_lease",
        "running",
        &path,
        &ready,
        &gate,
    );
    wait_for_paths(&[&ready]);

    let still_running = recover_daily_run(&path).expect("check active child process");
    assert_eq!(
        EhRules::from_json(&still_running)
            .unwrap()
            .automation
            .last_run
            .as_ref()
            .map(|run| run.status.as_str()),
        Some("running")
    );

    child
        .kill()
        .expect("terminate only the child started by this test");
    let _ = child.wait();
    let recovered = recover_daily_run(&path).expect("recover after the child exits");
    assert_eq!(
        EhRules::from_json(&recovered)
            .unwrap()
            .automation
            .last_run
            .as_ref()
            .map(|run| run.status.as_str()),
        Some("interrupted_unknown")
    );
    assert!(!claim_daily(&path, "2026-10-12").unwrap().0);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn manual_collection_blocks_cross_process_daily_claim_without_replacing_manual_state() {
    if std::env::var("RCH_EH_DAILY_CHILD_MODE").as_deref() == Ok("manual-running") {
        let path = PathBuf::from(std::env::var_os("RCH_EH_DAILY_CONFIG").unwrap());
        let ready = PathBuf::from(std::env::var_os("RCH_EH_DAILY_READY").unwrap());
        let gate = PathBuf::from(std::env::var_os("RCH_EH_DAILY_GATE").unwrap());
        update_daily_run(
            &path,
            EhAutomationRun {
                date: "2026-10-12".into(),
                source: "manual".into(),
                status: "running".into(),
                ..Default::default()
            },
        )
        .expect("persist manual running state");
        let lease = try_acquire_collection_lock(&path)
            .expect("acquire manual collection lease")
            .expect("no other collection should be active");
        std::fs::write(&ready, b"manual-running").expect("signal manual scan active");
        wait_for_gate(&gate);
        update_daily_run(
            &path,
            EhAutomationRun {
                date: "2026-10-12".into(),
                source: "manual".into(),
                status: "completed".into(),
                ..Default::default()
            },
        )
        .expect("manual completion must remain writable");
        drop(lease);
        std::fs::write(ready.with_extension("done"), b"done")
            .expect("signal manual completion");
        return;
    }

    let root = test_root("manual-daily-contention");
    let path = config_path(&root);
    let ready = root.join("manual-running");
    let gate = root.join("finish-manual");
    let child = spawn_test(
        "manual_collection_blocks_cross_process_daily_claim_without_replacing_manual_state",
        "manual-running",
        &path,
        &ready,
        &gate,
    );
    wait_for_paths(&[&ready]);

    let (claimed_during_manual, snapshot_during_manual) =
        claim_daily(&path, "2026-10-12").expect("check competing Daily claim");
    std::fs::write(&gate, b"finish").expect("allow manual child to finish");
    let child_output = child.wait_with_output().expect("wait for manual child");
    let final_rules = parse_rules(&path);

    assert!(
        child_output.status.success(),
        "manual child failed: {}",
        String::from_utf8_lossy(&child_output.stderr)
    );
    assert!(
        !claimed_during_manual,
        "Daily must not claim while another process owns a manual scan"
    );
    let during_rules = EhRules::from_json(&snapshot_during_manual).expect("parse claim snapshot");
    assert_eq!(
        during_rules
            .automation
            .last_run
            .as_ref()
            .map(|run| (run.source.as_str(), run.status.as_str())),
        Some(("manual", "running")),
        "a competing claim must preserve the manual run state"
    );
    assert_eq!(
        final_rules
            .automation
            .last_run
            .as_ref()
            .map(|run| (run.source.as_str(), run.status.as_str())),
        Some(("manual", "completed")),
        "manual completion must not be overwritten by a Daily claim"
    );
    assert!(
        claim_daily(&path, "2026-10-12").unwrap().0,
        "Daily may claim after the manual scan has completed"
    );
    let _ = std::fs::remove_dir_all(root);
}
