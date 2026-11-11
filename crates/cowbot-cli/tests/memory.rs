use std::process::Command;

use cowbot_client::{Client, LocalClient, MemoryFilter};
use cowbot_core::memory::MemoryStatus;

#[tokio::test]
async fn memory_add_list_and_archive_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("cowbot.db");
    let config_path = temp.path().join("cowbot.toml");
    std::fs::write(&config_path, format!("db = {:?}\n", db.to_string_lossy())).unwrap();

    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_cowbot"))
            .arg("--config")
            .arg(&config_path)
            .args(args)
            .output()
            .unwrap()
    };

    let output = run(&[
        "memory",
        "add",
        "--content",
        "The user prefers Rust for services.",
        "--kind",
        "preference",
        "--pinned",
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "memory add failed\nstderr: {stderr}\nstdout: {stdout}"
    );
    let entry_id = stdout
        .split("Added memory entry ")
        .nth(1)
        .and_then(|rest| rest.split('.').next())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .expect("the command prints the new entry id")
        .to_string();

    let output = run(&["memory", "list"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "memory list failed: {stdout}");
    assert!(stdout.contains(&entry_id), "stdout: {stdout}");
    assert!(stdout.contains("preference"), "stdout: {stdout}");
    assert!(stdout.contains("pinned"), "stdout: {stdout}");
    assert!(stdout.contains("[active]"), "stdout: {stdout}");

    let output = run(&["memory", "list", "--status", "pending"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "memory list failed: {stdout}");
    assert!(
        stdout.contains("No memory entries match."),
        "stdout: {stdout}"
    );

    let output = run(&["memory", "archive", &entry_id]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "memory archive failed: {stdout}");
    assert!(stdout.contains("Archived memory entry"), "stdout: {stdout}");

    let output = run(&["memory", "list", "--status", "archived"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "memory list failed: {stdout}");
    assert!(stdout.contains(&entry_id), "stdout: {stdout}");
    assert!(stdout.contains("[archived]"), "stdout: {stdout}");

    let client = LocalClient::open(&db).await.unwrap();
    let archived = client
        .list_memory_entries(&MemoryFilter {
            status: Some(MemoryStatus::Archived),
            ..MemoryFilter::default()
        })
        .await
        .unwrap();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].id, entry_id);
    assert!(archived[0].pinned);
}
