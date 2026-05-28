use chatbotchat_server::{app_state, serve};
use rmcp::model::CallToolRequestParams;
use rmcp::service::ServiceExt;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use tokio::net::TcpListener;
use tokio::process::Command;

/// Bring up the daemon in-process on a loopback port, returning the base URL.
/// The tempdir is leaked into the spawned task so the DB file outlives the test.
async fn spawn_daemon() -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_url = format!("sqlite://{}", dir.path().join("state.db").display());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let state = app_state(&db_url).await.expect("state");
    tokio::spawn(async move {
        let _keep = dir;
        serve(listener, state).await.expect("serve");
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn mcp_lists_and_calls_open_room() {
    let base = spawn_daemon().await;

    // Spawn `cbc mcp` as an MCP server child process, pointed at our daemon.
    let transport =
        TokioChildProcess::new(Command::new(env!("CARGO_BIN_EXE_cbc")).configure(|cmd| {
            cmd.arg("mcp").env("CBC_SERVER", &base);
        }))
        .expect("spawn cbc mcp");

    let client = ().serve(transport).await.expect("connect mcp client");

    let tools = client
        .list_tools(Default::default())
        .await
        .expect("list tools");
    assert!(
        tools
            .tools
            .iter()
            .any(|t| t.name.as_ref() == "cbc_open_room"),
        "cbc_open_room should be advertised; got {:?}",
        tools
            .tools
            .iter()
            .map(|t| t.name.as_ref())
            .collect::<Vec<_>>()
    );

    let result = client
        .call_tool(
            CallToolRequestParams::new("cbc_open_room").with_arguments(
                serde_json::json!({ "subject": "mcp smoke" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("call cbc_open_room");

    let rendered = serde_json::to_string(&result).expect("serialize result");
    assert!(
        rendered.contains("mcp-smoke-"),
        "tool result should carry the new room id; got: {rendered}"
    );

    client.cancel().await.ok();
}
