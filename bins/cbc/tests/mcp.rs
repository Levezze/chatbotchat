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
        rendered.contains("Join CBC room cbc-mcp-smoke-"),
        "open result should carry the slash-free share line; got: {rendered}"
    );
    assert!(
        !rendered.contains("/cbc-join"),
        "share line must not emit the /cbc-join slash trap; got: {rendered}"
    );

    // Extract the room id (scan from the known prefix over id-legal chars) and
    // confirm cbc_status returns the same room over MCP.
    let start = rendered.find("cbc-mcp-smoke-").expect("room id prefix");
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
    let start = opened_rendered.find("cbc-mcp-send-wait-").expect("room id");
    let room_id: String = opened_rendered[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();

    // Two participants in one MCP session, kept distinct by an explicit `as`
    // label (identity is now the instance token, so two agents sharing this one
    // child process's cwd/session must differ on `as`). Delivery is
    // process-agnostic (one daemon, one Hub), so this proves the cross-identity
    // round-trip without a second child process.
    for model in ["opus47", "sonnet46"] {
        client
            .call_tool(
                CallToolRequestParams::new("cbc_join_room").with_arguments(
                    serde_json::json!({ "room_id": room_id, "model": model, "as": model })
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
                serde_json::json!({ "room_id": room_id, "model": "opus47", "as": "opus47", "body": "ping from opus" })
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
                serde_json::json!({ "room_id": room_id, "model": "sonnet46", "as": "sonnet46" })
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
async fn mcp_signal_and_wait_round_trip() {
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
        advertised.contains(&"cbc_signal"),
        "cbc_signal should be advertised; got {advertised:?}"
    );

    let opened = client
        .call_tool(
            CallToolRequestParams::new("cbc_open_room").with_arguments(
                serde_json::json!({ "subject": "mcp signal wait" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("open room");
    let opened_rendered = serde_json::to_string(&opened).expect("serialize");
    let start = opened_rendered
        .find("cbc-mcp-signal-wait-")
        .expect("room id");
    let room_id: String = opened_rendered[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();

    for model in ["opus47", "sonnet46"] {
        client
            .call_tool(
                CallToolRequestParams::new("cbc_join_room").with_arguments(
                    serde_json::json!({ "room_id": room_id, "model": model, "as": model })
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .expect("join");
    }

    // opus47 signals it is consulting its user, carrying the question.
    client
        .call_tool(
            CallToolRequestParams::new("cbc_signal").with_arguments(
                serde_json::json!({
                    "room_id": room_id,
                    "model": "opus47",
                    "as": "opus47",
                    "type": "waiting_user",
                    "severity": "high",
                    "question_text": "should I merge to production?"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .expect("signal");

    // sonnet46 waits and receives the sentinel + its question.
    let waited = client
        .call_tool(
            CallToolRequestParams::new("cbc_wait").with_arguments(
                serde_json::json!({ "room_id": room_id, "model": "sonnet46", "as": "sonnet46" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("wait");
    let waited_rendered = serde_json::to_string(&waited).expect("serialize wait");
    assert!(
        waited_rendered.contains("should I merge to production?"),
        "cbc_wait should deliver the sentinel's question; got {waited_rendered}"
    );
    assert!(
        waited_rendered.contains("waiting_user"),
        "cbc_wait should carry the sentinel type; got {waited_rendered}"
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
    let start = opened_rendered
        .find("cbc-join-idempotent-")
        .expect("room id");
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

/// The anchor mechanism at the MCP boundary: a `cbc_join_room(as: "<label>")`
/// keys the participant on `<label>`, and a re-join with the SAME `as:` label
/// RESUMES that one participant (collapses to a single row, no ghost) — while a
/// DIFFERENT `as:` label mints a second, distinct participant. This is the exact
/// claim the identity-anchor fix rests on ("declared==polled, one stable row per
/// agent"), proven where the guidance change lives — the MCP `as:` layer. The
/// sibling `mcp_join_room_is_idempotent_within_a_session` covers the no-label
/// (session-derived) path; `same_model_distinct_as_are_separate_participants...`
/// (cli.rs) covers the CLI `--as` path across a fresh process.
#[tokio::test]
async fn mcp_join_room_anchors_one_row_per_as_label() {
    let base = spawn_daemon().await;

    let transport =
        TokioChildProcess::new(Command::new(env!("CARGO_BIN_EXE_cbc")).configure(|cmd| {
            cmd.arg("mcp").env("CBC_SERVER", &base);
        }))
        .expect("spawn cbc mcp");
    let client = ().serve(transport).await.expect("connect mcp client");

    let opened = client
        .call_tool(
            CallToolRequestParams::new("cbc_open_room").with_arguments(
                serde_json::json!({ "subject": "anchor label" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("open room");
    let opened_rendered = serde_json::to_string(&opened).expect("serialize");
    let start = opened_rendered.find("cbc-anchor-label-").expect("room id");
    let room_id: String = opened_rendered[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();

    let join_as = |label: &'static str| {
        let client = &client;
        let room = room_id.clone();
        async move {
            let result = client
                .call_tool(
                    CallToolRequestParams::new("cbc_join_room").with_arguments(
                        serde_json::json!({ "room_id": room, "model": "opus47", "as": label })
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

    // The displayed handle is a server-minted `<repo>-opus47-<4hex>`, NOT the
    // label — so distinct labels are told apart by distinct handles.
    let extract_handle = |rendered: &str| -> String {
        let marker = "-opus47-";
        let pos = rendered.find(marker).expect("handle marker");
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
    let resumed = |rendered: &str| -> bool {
        rendered.contains("\\\"resumed\\\":true") || rendered.contains("\"resumed\":true")
    };

    // First join under the anchor label mints the participant (not a resume).
    let first = join_as("cbc-worker-anchor").await;
    let anchor = extract_handle(&first);
    assert!(
        !resumed(&first),
        "first join under a fresh label is a mint, not a resume; got {first}"
    );

    // Re-join under the SAME anchor label resumes that one participant — the
    // anti-ghost guarantee the fix depends on across restart/fork/clear.
    let again = join_as("cbc-worker-anchor").await;
    assert_eq!(
        anchor,
        extract_handle(&again),
        "re-join with the same `as:` label must resume one handle; got {again}"
    );
    assert!(
        resumed(&again),
        "re-join with the same `as:` label must report resumed=true; got {again}"
    );

    // A DIFFERENT anchor label is a second, distinct participant (a real second
    // agent must stay visible — the label is per-role, not per-cwd).
    let other = join_as("cbc-orchestrator").await;
    let other_handle = extract_handle(&other);
    assert_ne!(
        anchor, other_handle,
        "a distinct `as:` label must mint a distinct handle; got {anchor} / {other_handle}"
    );

    // status lists exactly the two anchored identities.
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
    assert!(
        status_rendered.contains(&anchor) && status_rendered.contains(&other_handle),
        "status must list both anchored identities; got {status_rendered}"
    );

    client.cancel().await.ok();
}

#[tokio::test]
async fn mcp_recap_is_advertised_and_returns_message_bodies() {
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
        advertised.contains(&"cbc_recap"),
        "cbc_recap should be advertised; got {advertised:?}"
    );

    // Open + two participants + two messages.
    let opened = client
        .call_tool(
            CallToolRequestParams::new("cbc_open_room").with_arguments(
                serde_json::json!({ "subject": "mcp recap" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("open room");
    let opened_rendered = serde_json::to_string(&opened).expect("serialize");
    let start = opened_rendered.find("cbc-mcp-recap-").expect("room id");
    let room_id: String = opened_rendered[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();

    for model in ["opus47", "sonnet46"] {
        client
            .call_tool(
                CallToolRequestParams::new("cbc_join_room").with_arguments(
                    serde_json::json!({ "room_id": room_id, "model": model, "as": model })
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .expect("join");
    }
    for (model, body) in [
        ("opus47", "first recap line"),
        ("sonnet46", "second recap line"),
    ] {
        client
            .call_tool(
                CallToolRequestParams::new("cbc_send").with_arguments(
                    serde_json::json!({ "room_id": room_id, "model": model, "as": model, "body": body })
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .expect("send");
    }

    let recap = client
        .call_tool(
            CallToolRequestParams::new("cbc_recap").with_arguments(
                serde_json::json!({ "room_id": room_id })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("call cbc_recap");
    let rendered = serde_json::to_string(&recap).expect("serialize recap");
    assert!(
        rendered.contains("transcript_markdown"),
        "recap should carry a transcript_markdown field; got: {rendered}"
    );
    assert!(
        rendered.contains("first recap line") && rendered.contains("second recap line"),
        "recap should include every message body; got: {rendered}"
    );
    assert!(
        rendered.contains("git/gh"),
        "recap's next must carry the re-ground/verify instruction; got: {rendered}"
    );

    client.cancel().await.ok();
}

#[tokio::test]
async fn mcp_recap_does_not_advance_the_read_cursor() {
    let base = spawn_daemon().await;

    let transport =
        TokioChildProcess::new(Command::new(env!("CARGO_BIN_EXE_cbc")).configure(|cmd| {
            cmd.arg("mcp").env("CBC_SERVER", &base);
        }))
        .expect("spawn cbc mcp");
    let client = ().serve(transport).await.expect("connect mcp client");

    let opened = client
        .call_tool(
            CallToolRequestParams::new("cbc_open_room").with_arguments(
                serde_json::json!({ "subject": "recap cursor" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("open room");
    let opened_rendered = serde_json::to_string(&opened).expect("serialize");
    let start = opened_rendered.find("cbc-recap-cursor-").expect("room id");
    let room_id: String = opened_rendered[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();

    for model in ["opus47", "sonnet46"] {
        client
            .call_tool(
                CallToolRequestParams::new("cbc_join_room").with_arguments(
                    serde_json::json!({ "room_id": room_id, "model": model, "as": model })
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .expect("join");
    }

    // opus47 posts; sonnet46 has NOT yet read it.
    client
        .call_tool(
            CallToolRequestParams::new("cbc_send").with_arguments(
                serde_json::json!({ "room_id": room_id, "model": "opus47", "as": "opus47", "body": "unread tail" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("send");

    // sonnet46 re-reads the whole room via recap — this must NOT consume its cursor.
    client
        .call_tool(
            CallToolRequestParams::new("cbc_recap").with_arguments(
                serde_json::json!({ "room_id": room_id })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("recap");

    // sonnet46's cbc_wait must STILL deliver the unread message.
    let waited = client
        .call_tool(
            CallToolRequestParams::new("cbc_wait").with_arguments(
                serde_json::json!({ "room_id": room_id, "model": "sonnet46", "as": "sonnet46" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("wait");
    let waited_rendered = serde_json::to_string(&waited).expect("serialize wait");
    assert!(
        waited_rendered.contains("unread tail"),
        "recap must be cursor-independent: the unread message must still be delivered by cbc_wait; got: {waited_rendered}"
    );

    client.cancel().await.ok();
}

#[tokio::test]
async fn mcp_send_nudges_substance_and_wait_steers_reground() {
    let base = spawn_daemon().await;

    let transport =
        TokioChildProcess::new(Command::new(env!("CARGO_BIN_EXE_cbc")).configure(|cmd| {
            cmd.arg("mcp").env("CBC_SERVER", &base);
        }))
        .expect("spawn cbc mcp");
    let client = ().serve(transport).await.expect("connect mcp client");

    // cbc_send's advertised description must carry the structural nudge.
    let tools = client
        .list_tools(Default::default())
        .await
        .expect("list tools");
    let send_desc = tools
        .tools
        .iter()
        .find(|t| t.name.as_ref() == "cbc_send")
        .and_then(|t| t.description.as_ref().map(|d| d.to_string()))
        .expect("cbc_send must be advertised with a description");
    assert!(
        send_desc.to_lowercase().contains("substantive"),
        "cbc_send description should nudge a substantive turn; got: {send_desc}"
    );

    // cbc_close's description must (a) tell the agent to send everything before
    // voting close and (b) forbid the `--force` CLI escape hatch — the exact gap
    // that let an agent unilaterally force-close a room and drop the counterpart's
    // unsent, better answer.
    let close_desc = tools
        .tools
        .iter()
        .find(|t| t.name.as_ref() == "cbc_close")
        .and_then(|t| t.description.as_ref().map(|d| d.to_string()))
        .expect("cbc_close must be advertised with a description");
    let close_lc = close_desc.to_lowercase();
    assert!(
        close_lc.contains("--force") && close_lc.contains("human-only"),
        "cbc_close description must mark --force as a human-only escape hatch; got: {close_desc}"
    );
    assert!(
        close_lc.contains("sent everything") || close_lc.contains("send everything"),
        "cbc_close description must tell the agent to send everything before voting close; got: {close_desc}"
    );

    // Open + two participants + a message, then prove the delivered `next`
    // steers the agent to re-ground (cbc_recap) before replying.
    let opened = client
        .call_tool(
            CallToolRequestParams::new("cbc_open_room").with_arguments(
                serde_json::json!({ "subject": "mcp reground next" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("open room");
    let opened_rendered = serde_json::to_string(&opened).expect("serialize");
    let start = opened_rendered
        .find("cbc-mcp-reground-next-")
        .expect("room id");
    let room_id: String = opened_rendered[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    for model in ["opus47", "sonnet46"] {
        client
            .call_tool(
                CallToolRequestParams::new("cbc_join_room").with_arguments(
                    serde_json::json!({ "room_id": room_id, "model": model, "as": model })
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .expect("join");
    }
    client
        .call_tool(
            CallToolRequestParams::new("cbc_send").with_arguments(
                serde_json::json!({ "room_id": room_id, "model": "opus47", "as": "opus47", "body": "decision pending" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("send");
    let waited = client
        .call_tool(
            CallToolRequestParams::new("cbc_wait").with_arguments(
                serde_json::json!({ "room_id": room_id, "model": "sonnet46", "as": "sonnet46" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("wait");
    let waited_rendered = serde_json::to_string(&waited).expect("serialize wait");
    assert!(
        waited_rendered.contains("cbc_recap"),
        "a delivered message's next must steer the agent to re-ground; got: {waited_rendered}"
    );

    client.cancel().await.ok();
}

#[tokio::test]
async fn mcp_join_room_next_steers_to_background_poll() {
    // Regression guard for the read-and-vanish fix: joining a room must commit
    // the agent to pacing it. cbc_join_room's `next` has to steer the joiner to
    // re-ground (cbc_recap), reply, AND keep a background poll running — the same
    // discipline the sender already had. A `next` that just says "call cbc_wait
    // or cbc_send" (the prior text) lets the joiner read the opener and walk off.
    let base = spawn_daemon().await;

    let transport =
        TokioChildProcess::new(Command::new(env!("CARGO_BIN_EXE_cbc")).configure(|cmd| {
            cmd.arg("mcp").env("CBC_SERVER", &base);
        }))
        .expect("spawn cbc mcp");
    let client = ().serve(transport).await.expect("connect mcp client");

    let opened = client
        .call_tool(
            CallToolRequestParams::new("cbc_open_room").with_arguments(
                serde_json::json!({ "subject": "mcp join next" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("open room");
    let opened_rendered = serde_json::to_string(&opened).expect("serialize");
    let start = opened_rendered.find("cbc-mcp-join-next-").expect("room id");
    let room_id: String = opened_rendered[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();

    let joined = client
        .call_tool(
            CallToolRequestParams::new("cbc_join_room").with_arguments(
                serde_json::json!({ "room_id": room_id, "model": "opus47", "as": "opus47" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("join");
    let joined_rendered = serde_json::to_string(&joined).expect("serialize join");
    let joined_lc = joined_rendered.to_lowercase();
    assert!(
        joined_rendered.contains("cbc_recap"),
        "join next must steer the joiner to re-ground before replying; got: {joined_rendered}"
    );
    assert!(
        joined_lc.contains("poll"),
        "join next must steer the joiner to keep a background poll running; got: {joined_rendered}"
    );
    assert!(
        joined_lc.contains("stay in the room"),
        "join next must tell the joiner to stay (no read-and-vanish); got: {joined_rendered}"
    );

    client.cancel().await.ok();
}
