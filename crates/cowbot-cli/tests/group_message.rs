use std::process::Command;

use cowbot_client::{Client, LocalClient, NewConversation};

#[tokio::test]
async fn message_post_wait_answers_a_group_conversation() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("cowbot.db");
    let config_path = temp.path().join("cowbot.toml");

    let channel_id;
    {
        let client = LocalClient::open(&db).await.unwrap();
        let agents = client
            .instantiate_agents("builtin-software-team")
            .await
            .unwrap();
        let channel = client
            .create_conversation(&NewConversation {
                title: Some("CLI group".to_string()),
                agent_ids: vec![agents[0].id.clone(), agents[1].id.clone()],
                provider_id: None,
                model: "mock-model".to_string(),
                router_enabled: false,
                default_responder: Some(agents[0].id.clone()),
            })
            .await
            .unwrap();
        channel_id = channel.id.0;
    }

    std::fs::write(&config_path, format!("db = {:?}\n", db.to_string_lossy())).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_cowbot"))
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "message",
            "post",
            "--channel",
            &channel_id,
            "--content",
            "@team status",
            "--wait",
            "--dry-run",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "cowbot exited with an error\nstderr: {stderr}\nstdout: {stdout}"
    );
    assert!(
        stdout.contains("2 agent(s) answered in channel"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("route: mention"), "stdout: {stdout}");
    assert!(stdout.contains("2 replied"), "stdout: {stdout}");
}
