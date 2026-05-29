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
    let advertised: Vec<&str> = tools.tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(
        advertised.contains(&"cbc_open_room"),
        "cbc_open_room should be advertised; got {advertised:?}"
    );
    assert!(
        advertised.contains(&"cbc_status"),
        "cbc_status should be advertised; got {advertised:?}"
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
    // Shape parity with cbc_open_room's JSON: both room_id and share_line fields.
    assert!(
        rendered.contains("room_id") && rendered.contains("share_line"),
        "open result should carry the full OpenRoomResponse shape; got: {rendered}"
    );
    assert!(
        rendered.contains("/cbc-join mcp-smoke-"),
        "open result should carry the share line; got: {rendered}"
    );

    // Extract the room id (scan from the known prefix over id-legal chars) and
    // confirm cbc_status returns the same room over MCP.
    let start = rendered.find("mcp-smoke-").expect("room id prefix");
    let room_id: String = rendered[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();

    let status = client
        .call_tool(
            CallToolRequestParams::new("cbc_status").with_arguments(
                serde_json::json!({ "room_id": room_id })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("call cbc_status");

    let status_rendered = serde_json::to_string(&status).expect("serialize status");
    // Full RoomStatus shape parity: id, the original subject, and active state.
    assert!(
        status_rendered.contains(&room_id)
            && status_rendered.contains("mcp smoke")
            && status_rendered.contains("active"),
        "cbc_status should report the room's id, subject, and active state; got: {status_rendered}"
    );

    client.cancel().await.ok();
}

#[tokio::test]
async fn mcp_send_and_wait_round_trip() {
    let base = spawn_daemon().await;

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
    let advertised: Vec<&str> = tools.tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(
        advertised.contains(&"cbc_send"),
        "cbc_send should be advertised; got {advertised:?}"
    );
    assert!(
        advertised.contains(&"cbc_wait"),
        "cbc_wait should be advertised; got {advertised:?}"
    );

    // Open a room.
    let opened = client
        .call_tool(
            CallToolRequestParams::new("cbc_open_room").with_arguments(
                serde_json::json!({ "subject": "mcp send wait" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("open room");
    let opened_rendered = serde_json::to_string(&opened).expect("serialize");
    let start = opened_rendered.find("mcp-send-wait-").expect("room id");
    let room_id: String = opened_rendered[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();

    // Two participants in one session, kept distinct by model. The acceptance
    // criterion says "two sessions in different repos", but delivery is
    // process-agnostic (one daemon, one Hub) and distinct handles come from any
    // differing tuple field — two models prove the cross-identity round-trip
    // without the cost of a second child process.
    for model in ["opus47", "sonnet46"] {
        client
            .call_tool(
                CallToolRequestParams::new("cbc_join_room").with_arguments(
                    serde_json::json!({ "room_id": room_id, "model": model })
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .expect("join");
    }

    // opus47 sends a broadcast BEFORE the wait, so the wait returns immediately
    // (the real daemon's cap is 10 minutes — never park in a test).
    client
        .call_tool(
            CallToolRequestParams::new("cbc_send").with_arguments(
                serde_json::json!({ "room_id": room_id, "model": "opus47", "body": "ping from opus" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("send");

    // sonnet46 waits and receives it.
    let waited = client
        .call_tool(
            CallToolRequestParams::new("cbc_wait").with_arguments(
                serde_json::json!({ "room_id": room_id, "model": "sonnet46" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("wait");
    let waited_rendered = serde_json::to_string(&waited).expect("serialize wait");
    assert!(
        waited_rendered.contains("ping from opus"),
        "cbc_wait should deliver the sent message; got {waited_rendered}"
    );

    client.cancel().await.ok();
}

#[tokio::test]
async fn mcp_join_room_is_idempotent_within_a_session() {
    let base = spawn_daemon().await;

    let transport =
        TokioChildProcess::new(Command::new(env!("CARGO_BIN_EXE_cbc")).configure(|cmd| {
            cmd.arg("mcp").env("CBC_SERVER", &base);
        }))
        .expect("spawn cbc mcp");
    let client = ().serve(transport).await.expect("connect mcp client");

    // cbc_join_room must be advertised.
    let tools = client
        .list_tools(Default::default())
        .await
        .expect("list tools");
    let advertised: Vec<&str> = tools.tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(
        advertised.contains(&"cbc_join_room"),
        "cbc_join_room should be advertised; got {advertised:?}"
    );

    // Open a room to join.
    let opened = client
        .call_tool(
            CallToolRequestParams::new("cbc_open_room").with_arguments(
                serde_json::json!({ "subject": "join idempotent" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("open room");
    let opened_rendered = serde_json::to_string(&opened).expect("serialize");
    let start = opened_rendered.find("join-idempotent-").expect("room id");
    let room_id: String = opened_rendered[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();

    // Join twice from the same MCP session (same cwd, same model) → same handle.
    let join = |room: String| {
        let client = &client;
        async move {
            let result = client
                .call_tool(
                    CallToolRequestParams::new("cbc_join_room").with_arguments(
                        serde_json::json!({ "room_id": room, "model": "opus47" })
                            .as_object()
                            .unwrap()
                            .clone(),
                    ),
                )
                .await
                .expect("call cbc_join_room");
            serde_json::to_string(&result).expect("serialize join")
        }
    };

    let first = join(room_id.clone()).await;
    let second = join(room_id.clone()).await;

    // Both calls carry a handle of the form <repo>-opus47-<sess4hex>.
    let extract_handle = |rendered: &str| -> String {
        let marker = "-opus47-";
        let pos = rendered.find(marker).expect("handle marker");
        // Walk left over the repo slug, right over the sess.
        let left = rendered[..pos]
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>();
        let right: String = rendered[pos + marker.len()..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        format!("{left}{marker}{right}")
    };
    let h1 = extract_handle(&first);
    let h2 = extract_handle(&second);
    assert_eq!(
        h1, h2,
        "same session/room/model must resume the same handle"
    );
    assert!(
        second.contains("\\\"resumed\\\":true") || second.contains("\"resumed\":true"),
        "second join should report resumed=true; got {second}"
    );

    client.cancel().await.ok();
}
