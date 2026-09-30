//! J928-QtCodex: behavioral coverage for tool-activity's `ToolActivity` presentation levels.
//!
//! Covers the single three-level `codex_config::types::ToolActivity` setting
//! (`Off`/`Concise`/`Quiet`, default `Quiet`) and its presentation matrix. This replaces the
//! original independent `hide_tool_activity`/`hide_tool_errors` booleans:
//!   1. Runtime state (Task 2): default/set, `should_render_exec_activity`.
//!   2. Exec suppression (Task 3): hidden Agent success/Failed/Declined, UserShell, shown mode,
//!      unified-exec wait/interaction suppression, and completed-command replay.
//!   3. Non-command tool families (Task 4): patch, view-image, image-generation, MCP, web
//!      search, collab-agent, sub-agent, plus a preservation test and persisted-transcript
//!      (`thread_transcript`) coverage.
//!   4. Slash command (Task 5): `/tool-activity off|concise|quiet|status` and invalid input.

use super::*;
use codex_app_server_protocol::McpToolCallError;
use codex_app_server_protocol::McpToolCallResult;
use codex_app_server_protocol::McpToolCallStatus;
use codex_app_server_protocol::SubAgentActivityKind;
use codex_app_server_protocol::WebSearchAction;
use codex_app_server_protocol::WebSearchItem;
use codex_config::types::ToolActivity;
use pretty_assertions::assert_eq;

// ---------------------------------------------------------------------------------------------
// Small local helpers mirroring the style of `tests/helpers.rs`, scoped to this module because
// only these tests need to construct MCP/web-search/collab/sub-agent thread items directly.
// ---------------------------------------------------------------------------------------------

fn mcp_item(
    id: &str,
    status: McpToolCallStatus,
    result: Option<Box<McpToolCallResult>>,
    error: Option<McpToolCallError>,
) -> AppServerThreadItem {
    AppServerThreadItem::McpToolCall {
        id: id.to_string(),
        server: "server".to_string(),
        tool: "tool".to_string(),
        status,
        arguments: json!({}),
        app_context: None,
        mcp_app_resource_uri: None,
        mcp_app_ui: None,
        plugin_id: None,
        read_only_hint: None,
        result,
        error,
        duration_ms: Some(5),
    }
}

fn handle_mcp_started(chat: &mut ChatWidget, id: &str) {
    chat.handle_server_notification(
        ServerNotification::ItemStarted(ItemStartedNotification {
            thread_id: String::new(),
            turn_id: "turn-1".to_string(),
            started_at_ms: 0,
            item: mcp_item(id, McpToolCallStatus::InProgress, None, None),
        }),
        /*replay_kind*/ None,
    );
}

fn handle_mcp_completed(
    chat: &mut ChatWidget,
    id: &str,
    status: McpToolCallStatus,
    result: Option<Box<McpToolCallResult>>,
    error: Option<McpToolCallError>,
) {
    chat.handle_server_notification(
        ServerNotification::ItemCompleted(ItemCompletedNotification {
            thread_id: String::new(),
            turn_id: "turn-1".to_string(),
            completed_at_ms: 0,
            item: mcp_item(id, status, result, error),
        }),
        /*replay_kind*/ None,
    );
}

fn mcp_success_result() -> Option<Box<McpToolCallResult>> {
    Some(Box::new(McpToolCallResult {
        content: Vec::new(),
        structured_content: None,
        meta: None,
    }))
}

fn handle_web_search_completed(chat: &mut ChatWidget, id: &str, query: &str) {
    chat.handle_server_notification(
        ServerNotification::ItemCompleted(ItemCompletedNotification {
            thread_id: String::new(),
            turn_id: "turn-1".to_string(),
            completed_at_ms: 0,
            item: AppServerThreadItem::WebSearch(WebSearchItem {
                id: id.to_string(),
                query: query.to_string(),
                action: Some(WebSearchAction::Search {
                    query: Some(query.to_string()),
                    queries: None,
                }),
                results: None,
            }),
        }),
        /*replay_kind*/ None,
    );
}

fn collab_wait_item(id: &str, status: AppServerCollabAgentToolCallStatus) -> AppServerThreadItem {
    AppServerThreadItem::CollabAgentToolCall {
        id: id.to_string(),
        tool: AppServerCollabAgentTool::Wait,
        status,
        sender_thread_id: "sender-thread".to_string(),
        receiver_thread_ids: Vec::new(),
        prompt: None,
        model: None,
        reasoning_effort: None,
        agents_states: HashMap::new(),
    }
}

fn handle_collab_completed(
    chat: &mut ChatWidget,
    id: &str,
    status: AppServerCollabAgentToolCallStatus,
) {
    chat.handle_server_notification(
        ServerNotification::ItemCompleted(ItemCompletedNotification {
            thread_id: String::new(),
            turn_id: "turn-1".to_string(),
            completed_at_ms: 0,
            item: collab_wait_item(id, status),
        }),
        /*replay_kind*/ None,
    );
}

fn sub_agent_item(id: &str, kind: SubAgentActivityKind) -> AppServerThreadItem {
    AppServerThreadItem::SubAgentActivity {
        id: id.to_string(),
        kind,
        agent_thread_id: "agent-thread".to_string(),
        agent_path: "agent.md".to_string(),
    }
}

fn handle_sub_agent_completed(chat: &mut ChatWidget, id: &str, kind: SubAgentActivityKind) {
    chat.handle_server_notification(
        ServerNotification::ItemCompleted(ItemCompletedNotification {
            thread_id: String::new(),
            turn_id: "turn-1".to_string(),
            completed_at_ms: 0,
            item: sub_agent_item(id, kind),
        }),
        /*replay_kind*/ None,
    );
}

// =================================================================================================
// Task 2: runtime visibility state
// =================================================================================================

#[tokio::test]
async fn tool_activity_defaults_to_quiet() {
    // `make_chatwidget_manual`'s shared `test_config()` pins tool-activity to `Off` so the many
    // pre-existing rendering/snapshot tests across this crate keep seeing full detail without
    // each opting in individually (see `test_config()` in helpers.rs). The actual production
    // default lives in `ToolActivity`'s `#[default]` variant, which every unconfigured `Config`
    // resolves to via `.unwrap_or_default()` (see `codex-rs/core/src/config/mod.rs`'s
    // `tui_tool_activity` field population) — check that directly instead.
    assert_eq!(ToolActivity::default(), ToolActivity::Quiet);
}

#[tokio::test]
async fn set_tool_activity_level_updates_state() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.set_tool_activity_level(ToolActivity::Off);
    assert_eq!(chat.tool_activity_level(), ToolActivity::Off);

    chat.set_tool_activity_level(ToolActivity::Concise);
    assert_eq!(chat.tool_activity_level(), ToolActivity::Concise);

    chat.set_tool_activity_level(ToolActivity::Quiet);
    assert_eq!(chat.tool_activity_level(), ToolActivity::Quiet);
}

#[tokio::test]
async fn should_render_exec_activity_hides_agent_but_always_shows_user_shell() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Off);

    // Shown mode: every source renders.
    assert!(chat.should_render_exec_activity(ExecCommandSource::Agent));
    assert!(chat.should_render_exec_activity(ExecCommandSource::UserShell));

    chat.set_tool_activity_level(ToolActivity::Concise);
    assert!(
        !chat.should_render_exec_activity(ExecCommandSource::Agent),
        "hidden mode must suppress agent-sourced exec activity"
    );
    assert!(
        chat.should_render_exec_activity(ExecCommandSource::UserShell),
        "UserShell must always render even when hidden"
    );
}

#[tokio::test]
async fn set_tool_activity_level_and_notify_emits_exact_status_strings() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.set_tool_activity_level_and_notify(ToolActivity::Concise);
    assert_eq!(chat.tool_activity_level(), ToolActivity::Concise);
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1);
    assert!(
        lines_to_single_string(&cells[0]).contains("Tool activity: concise (routine summaries)")
    );

    chat.set_tool_activity_level_and_notify(ToolActivity::Off);
    assert_eq!(chat.tool_activity_level(), ToolActivity::Off);
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1);
    assert!(lines_to_single_string(&cells[0]).contains("Tool activity: shown"));
}

// =================================================================================================
// Task 3: exec suppression without changing execution bookkeeping
// =================================================================================================

#[tokio::test]
async fn hidden_agent_exec_success_emits_concise_summary_and_completes_bookkeeping() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Concise);
    chat.on_task_started();

    let begin = begin_exec(&mut chat, "call-hidden-success", "printf hidden");
    assert!(chat.running_commands.contains_key("call-hidden-success"));

    end_exec(&mut chat, begin, "hidden\n", "", /*exit_code*/ 0);

    let cells = drain_insert_history(&mut rx);
    assert_eq!(
        cells.len(),
        1,
        "concise mode emits one compact completion record"
    );
    let text = lines_to_single_string(&cells[0]);
    assert!(
        text.contains("Completed:") && text.contains("printf hidden"),
        "unexpected concise summary: {text}"
    );
    assert!(
        chat.transcript.active_cell.is_none(),
        "hidden successful agent exec must not render an active cell either"
    );
    assert!(
        !chat.running_commands.contains_key("call-hidden-success"),
        "completion bookkeeping (running_commands removal) must still happen while hidden"
    );
    assert!(
        chat.transcript.had_work_activity,
        "work-activity bookkeeping must still be marked at completion while hidden"
    );
}

#[tokio::test]
async fn hidden_agent_exec_failed_without_exit_code_remains_visible() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Concise);
    chat.on_task_started();

    let begin = begin_exec(&mut chat, "call-hidden-failed", "printf broken");
    let AppServerThreadItem::CommandExecution {
        id,
        command,
        cwd,
        process_id,
        plugin_id,
        script_path,
        source,
        command_actions,
        ..
    } = begin
    else {
        panic!("expected command execution item");
    };
    handle_exec_end(
        &mut chat,
        AppServerThreadItem::CommandExecution {
            model_context: None,
            id,
            command,
            cwd,
            process_id,
            plugin_id,
            script_path,
            source,
            status: AppServerCommandExecutionStatus::Failed,
            command_actions,
            aggregated_output: Some("boom\n".to_string()),
            exit_code: None,
            duration_ms: Some(5),
        },
    );

    let cells = drain_insert_history(&mut rx);
    assert_eq!(
        cells.len(),
        1,
        "Failed status must fall through to the normal renderer even while hidden, regardless \
         of exit_code"
    );
    assert!(lines_to_single_string(&cells[0]).contains("printf broken"));
}

#[tokio::test]
async fn hidden_agent_exec_declined_without_exit_code_remains_visible() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Concise);
    chat.on_task_started();

    let begin = begin_exec(&mut chat, "call-hidden-declined", "printf declined-cmd");
    let AppServerThreadItem::CommandExecution {
        id,
        command,
        cwd,
        process_id,
        plugin_id,
        script_path,
        source,
        command_actions,
        ..
    } = begin
    else {
        panic!("expected command execution item");
    };
    handle_exec_end(
        &mut chat,
        AppServerThreadItem::CommandExecution {
            model_context: None,
            id,
            command,
            cwd,
            process_id,
            plugin_id,
            script_path,
            source,
            status: AppServerCommandExecutionStatus::Declined,
            command_actions,
            aggregated_output: None,
            exit_code: None,
            duration_ms: None,
        },
    );

    let cells = drain_insert_history(&mut rx);
    assert_eq!(
        cells.len(),
        1,
        "Declined status must remain visible even while hidden, regardless of exit_code"
    );
    assert!(lines_to_single_string(&cells[0]).contains("printf declined-cmd"));
}

#[tokio::test]
async fn hidden_user_shell_exec_always_visible() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Concise);

    let begin = begin_exec_with_source(
        &mut chat,
        "call-hidden-user-shell",
        "printf from-user",
        ExecCommandSource::UserShell,
    );
    end_exec(&mut chat, begin, "from-user\n", "", /*exit_code*/ 0);

    let cells = drain_insert_history(&mut rx);
    assert_eq!(
        cells.len(),
        1,
        "UserShell commands must always render, even while tool activity is hidden"
    );
    assert!(lines_to_single_string(&cells[0]).contains("printf from-user"));
}

#[tokio::test]
async fn shown_mode_reproduces_stock_exec_rendering() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Off);
    assert_eq!(chat.tool_activity_level(), ToolActivity::Off);
    chat.on_task_started();

    let begin = begin_exec(&mut chat, "call-shown", "printf shown");
    end_exec(&mut chat, begin, "shown\n", "", /*exit_code*/ 0);
    let cells = drain_insert_history(&mut rx);
    let history = cells
        .iter()
        .map(|cell| lines_to_single_string(cell))
        .collect::<String>();
    assert!(
        history.contains("printf shown"),
        "shown mode must retain routine command output"
    );
}

#[tokio::test]
async fn hidden_unified_exec_interaction_row_is_suppressed_for_hidden_call() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Concise);
    chat.on_task_started();

    begin_unified_exec_startup(&mut chat, "call-hidden-wait", "proc-hidden-wait", "sleep 5");
    assert!(drain_insert_history(&mut rx).is_empty());

    // Non-empty stdin interaction: must not leak a transcript row for a call latched hidden at
    // start, even though `on_terminal_interaction` runs outside normal `ExecCell` completion.
    terminal_interaction(&mut chat, "call-hidden-wait", "proc-hidden-wait", "ls\n");
    assert_eq!(
        drain_insert_history(&mut rx).len(),
        0,
        "hidden unified-exec interaction must not insert a transcript row"
    );
    assert!(
        chat.transcript.active_cell.is_none(),
        "hidden unified-exec interaction must not create an active cell either"
    );
}

#[tokio::test]
async fn hidden_unified_exec_wait_streak_flush_does_not_leak_row() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Concise);
    chat.on_task_started();

    begin_unified_exec_startup(
        &mut chat,
        "call-hidden-streak",
        "proc-hidden-streak",
        "just fix",
    );
    // Empty stdin polls build up a wait streak.
    terminal_interaction(&mut chat, "call-hidden-streak-a", "proc-hidden-streak", "");
    terminal_interaction(&mut chat, "call-hidden-streak-b", "proc-hidden-streak", "");

    handle_turn_completed(&mut chat, "turn-1", /*duration_ms*/ None);

    let cells = drain_insert_history(&mut rx);
    let combined = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<String>();
    assert!(
        !combined.contains("Ran") && !combined.to_lowercase().contains("just fix"),
        "flushing a wait streak for a hidden call must not leak an interaction row: {combined:?}"
    );
}

#[tokio::test]
async fn hidden_replay_of_completed_agent_command_emits_concise_record() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Concise);

    chat.replay_thread_item(
        AppServerThreadItem::CommandExecution {
            model_context: None,
            id: "call-replay-hidden".to_string(),
            command: "printf replayed".to_string(),
            cwd: chat.config.cwd.clone().into(),
            process_id: None,
            plugin_id: None,
            script_path: None,
            source: ExecCommandSource::Agent,
            status: AppServerCommandExecutionStatus::Completed,
            command_actions: Vec::new(),
            aggregated_output: Some("replayed\n".to_string()),
            exit_code: Some(0),
            duration_ms: Some(5),
        },
        "turn-1".to_string(),
        ReplayKind::ThreadSnapshot,
    );

    let cells = drain_insert_history(&mut rx);
    assert_eq!(
        cells.len(),
        1,
        "concise replay emits one compact completion record"
    );
    let text = lines_to_single_string(&cells[0]);
    assert!(
        text.contains("Completed:") && text.contains("printf replayed"),
        "unexpected concise replay summary: {text}"
    );
    assert!(
        chat.transcript.had_work_activity,
        "replay completion must preserve work-activity bookkeeping"
    );
}

#[tokio::test]
async fn hidden_replay_of_user_shell_command_remains_visible() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Concise);

    chat.replay_thread_item(
        AppServerThreadItem::CommandExecution {
            model_context: None,
            id: "call-replay-user-shell".to_string(),
            command: "printf replayed-user".to_string(),
            cwd: chat.config.cwd.clone().into(),
            process_id: None,
            plugin_id: None,
            script_path: None,
            source: ExecCommandSource::UserShell,
            status: AppServerCommandExecutionStatus::Completed,
            command_actions: Vec::new(),
            aggregated_output: Some("replayed-user\n".to_string()),
            exit_code: Some(0),
            duration_ms: Some(5),
        },
        "turn-1".to_string(),
        ReplayKind::ThreadSnapshot,
    );

    assert_eq!(drain_insert_history(&mut rx).len(), 1);
}

// =================================================================================================
// Task 4: non-command tool families + preservation + persisted-transcript filtering
// =================================================================================================

#[tokio::test]
async fn hidden_patch_apply_begin_and_success_are_suppressed_but_failure_is_visible() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Concise);

    let mut changes = HashMap::new();
    changes.insert(
        PathBuf::from("foo.txt"),
        FileChange::Add {
            content: "hi".to_string(),
        },
    );
    handle_patch_apply_begin(&mut chat, "call-patch-hidden", "turn-1", changes.clone());
    assert_eq!(
        drain_insert_history(&mut rx).len(),
        1,
        "concise mode must emit one compact patch summary"
    );

    handle_patch_apply_end(
        &mut chat,
        "call-patch-hidden",
        "turn-1",
        changes.clone(),
        AppServerPatchApplyStatus::Completed,
    );
    assert_eq!(
        drain_insert_history(&mut rx).len(),
        0,
        "hidden mode must skip the routine patch success completion"
    );

    handle_patch_apply_end(
        &mut chat,
        "call-patch-hidden-failure",
        "turn-1",
        changes,
        AppServerPatchApplyStatus::Failed,
    );
    let cells = drain_insert_history(&mut rx);
    assert_eq!(
        cells.len(),
        1,
        "a patch failure must remain visible even while hidden"
    );
}

#[tokio::test]
async fn hidden_patch_apply_failure_remains_visible_at_quiet_level() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Quiet);

    let mut changes = HashMap::new();
    changes.insert(
        PathBuf::from("foo.txt"),
        FileChange::Add {
            content: "hi".to_string(),
        },
    );
    handle_patch_apply_end(
        &mut chat,
        "call-patch-quiet-failure",
        "turn-1",
        changes,
        AppServerPatchApplyStatus::Failed,
    );
    assert_eq!(
        drain_insert_history(&mut rx).len(),
        1,
        "at Quiet, a patch failure must remain visible"
    );
}

#[tokio::test]
async fn hidden_view_image_tool_call_is_suppressed() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Concise);
    let image_path = chat.config.cwd.join("example.png");

    handle_view_image_tool_call(&mut chat, "call-image-hidden", image_path);

    assert_eq!(
        drain_insert_history(&mut rx).len(),
        1,
        "concise view-image calls must emit one compact summary"
    );
}

#[tokio::test]
async fn hidden_image_generation_success_hidden_but_non_success_visible() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Concise);

    handle_image_generation_end(
        &mut chat,
        "call-image-gen-success",
        "completed",
        Some("a square".into()),
        Some(test_path_buf("/tmp/ig-hidden.png").abs()),
    );
    assert_eq!(
        drain_insert_history(&mut rx).len(),
        1,
        "concise image-generation completion must emit one compact summary"
    );

    handle_image_generation_end(
        &mut chat,
        "call-image-gen-failed",
        "failed",
        Some("a square".into()),
        /*saved_path*/ None,
    );
    let cells = drain_insert_history(&mut rx);
    assert_eq!(
        cells.len(),
        1,
        "a non-success (\"failed\") image-generation completion must remain visible"
    );
}

#[tokio::test]
async fn hidden_image_generation_failure_remains_visible_at_quiet_level() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Quiet);

    handle_image_generation_end(
        &mut chat,
        "call-image-gen-quiet-failed",
        "failed",
        Some("a square".into()),
        /*saved_path*/ None,
    );
    assert_eq!(
        drain_insert_history(&mut rx).len(),
        1,
        "at Quiet, a non-success image-generation completion must remain visible"
    );
}

#[tokio::test]
async fn hidden_mcp_start_and_success_are_suppressed_but_error_is_visible() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Concise);

    handle_mcp_started(&mut chat, "call-mcp-hidden");
    assert_eq!(
        drain_insert_history(&mut rx).len(),
        1,
        "concise MCP start must emit one compact summary"
    );
    assert!(chat.transcript.active_cell.is_none());

    handle_mcp_completed(
        &mut chat,
        "call-mcp-hidden",
        McpToolCallStatus::Completed,
        mcp_success_result(),
        None,
    );
    assert_eq!(
        drain_insert_history(&mut rx).len(),
        0,
        "a successful MCP completion must not reappear via the fallback completion path"
    );

    handle_mcp_started(&mut chat, "call-mcp-error");
    handle_mcp_completed(
        &mut chat,
        "call-mcp-error",
        McpToolCallStatus::Failed,
        None,
        Some(McpToolCallError {
            message: "boom".to_string(),
        }),
    );
    let cells = drain_insert_history(&mut rx);
    assert_eq!(
        cells.len(),
        2,
        "concise MCP start summary plus failure must remain visible"
    );
}

#[tokio::test]
async fn hidden_mcp_completed_without_prior_started_still_renders() {
    // Unlike WebSearch (whose ItemCompleted handling always synthesizes a begin+end pair, see
    // `replay.rs`'s `ThreadItem::WebSearch` arm), MCP's ItemCompleted routes directly to
    // `on_mcp_tool_call_completed` with no synthesized start. So a completion that arrives
    // without a preceding started event (out-of-order delivery, reconnect) was never latched
    // into `hidden_mcp_calls`, and must fail safe by rendering visibly rather than being
    // silently swallowed.
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Concise);

    handle_mcp_completed(
        &mut chat,
        "call-mcp-no-start",
        McpToolCallStatus::Completed,
        mcp_success_result(),
        None,
    );
    let cells = drain_insert_history(&mut rx);
    assert_eq!(
        cells.len(),
        1,
        "an MCP completion with no prior started event must fail safe and render visibly, \
         not be silently dropped"
    );
}

#[tokio::test]
async fn hidden_web_search_begin_and_end_are_suppressed() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Concise);

    handle_web_search_completed(&mut chat, "call-web-hidden", "hidden query");

    assert_eq!(
        drain_insert_history(&mut rx).len(),
        1,
        "concise web search must emit one compact summary"
    );
    assert!(chat.transcript.active_cell.is_none());
}

#[tokio::test]
async fn shown_web_search_end_renders_a_cell() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Off);
    assert_eq!(chat.tool_activity_level(), ToolActivity::Off);

    handle_web_search_completed(&mut chat, "call-web-shown", "shown query");

    let cells = drain_insert_history(&mut rx);
    assert_eq!(
        cells.len(),
        1,
        "shown mode must reproduce stock web search rendering"
    );
}

#[tokio::test]
async fn hidden_collab_agent_completed_is_suppressed_but_failed_is_visible() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Concise);

    handle_collab_completed(
        &mut chat,
        "collab-hidden",
        AppServerCollabAgentToolCallStatus::Completed,
    );
    assert_eq!(
        drain_insert_history(&mut rx).len(),
        1,
        "concise routine collab-agent activity must emit one compact summary"
    );

    handle_collab_completed(
        &mut chat,
        "collab-failed",
        AppServerCollabAgentToolCallStatus::Failed,
    );
    let cells = drain_insert_history(&mut rx);
    assert_eq!(
        cells.len(),
        1,
        "a Failed collab-agent tool call must remain visible even while hidden"
    );
}

#[tokio::test]
async fn hidden_collab_agent_failed_remains_visible_at_quiet_level() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Quiet);

    handle_collab_completed(
        &mut chat,
        "collab-quiet-failed",
        AppServerCollabAgentToolCallStatus::Failed,
    );
    assert_eq!(
        drain_insert_history(&mut rx).len(),
        1,
        "at Quiet, a Failed collab-agent tool call must remain visible"
    );
}

#[tokio::test]
async fn hidden_sub_agent_routine_activity_is_suppressed_but_interrupted_is_visible() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Concise);

    for kind in [
        SubAgentActivityKind::Started,
        SubAgentActivityKind::Interacted,
        SubAgentActivityKind::Completed,
    ] {
        handle_sub_agent_completed(&mut chat, "sub-agent-routine", kind);
        assert_eq!(
            drain_insert_history(&mut rx).len(),
            1,
            "concise routine sub-agent activity kind {kind:?} must emit one compact summary"
        );
    }

    handle_sub_agent_completed(
        &mut chat,
        "sub-agent-interrupted",
        SubAgentActivityKind::Interrupted,
    );
    let cells = drain_insert_history(&mut rx);
    assert_eq!(
        cells.len(),
        1,
        "an Interrupted sub-agent activity must remain visible even while hidden"
    );
}

#[tokio::test]
async fn hidden_sub_agent_interrupted_remains_visible_at_quiet_level() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Quiet);

    handle_sub_agent_completed(
        &mut chat,
        "sub-agent-quiet-interrupted",
        SubAgentActivityKind::Interrupted,
    );
    assert_eq!(
        drain_insert_history(&mut rx).len(),
        1,
        "at Quiet, an Interrupted sub-agent activity must remain visible"
    );
}

#[tokio::test]
async fn hidden_mode_preserves_assistant_prose_reasoning_approval_and_error() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Concise);

    // Reasoning summary stays visible. Real reasoning deltas carry a bold header followed
    // by a blank line before the body (`**Header**\n\nBody`); `split_reasoning_summary_parts`
    // treats that leading `**Header**` as the live status header rather than folding it into
    // a transcript-only cell. This widget surfaces the live reasoning header through
    // `chat.reasoning_header` (see `reasoning_status_tests.rs` and
    // `status_and_layout.rs::reasoning_header_*` for the same pattern) while the turn streams;
    // that field, not a transcript cell, is what tool-activity hiding must leave untouched.
    handle_agent_reasoning_delta(&mut chat, "**Because of X**\n\nI will do Y.");
    handle_agent_reasoning_final(&mut chat);
    chat.on_commit_tick();
    assert!(
        chat.reasoning_header.as_deref().unwrap_or_default().contains("I will do Y"),
        "reasoning summary must remain visible while hidden: {:?}",
        chat.reasoning_header
    );

    // Assistant prose stays visible.
    complete_assistant_message(
        &mut chat,
        "msg-preserved",
        "Here is my answer.",
        Some(MessagePhase::FinalAnswer),
    );
    let cells = drain_insert_history(&mut rx);
    assert!(
        cells
            .iter()
            .any(|lines| lines_to_single_string(lines).contains("Here is my answer")),
        "assistant prose must remain visible while hidden"
    );

    // Approval requests stay visible (rendered via the modal, not history).
    handle_exec_approval_request(
        &mut chat,
        "sub-preserved",
        ExecApprovalRequestEvent {
            kind: codex_app_server_protocol::CommandExecutionApprovalKind::Command,
            call_id: "call-preserved".into(),
            approval_id: Some("call-preserved".into()),
            turn_id: "turn-preserved".into(),
            environment_id: None,
            command: vec!["bash".into(), "-lc".into(), "echo preserved".into()],
            cwd: AbsolutePathBuf::current_dir().expect("current dir"),
            reason: Some("preservation test reason".into()),
            network_approval_context: None,
            proposed_execpolicy_amendment: None,
            proposed_network_policy_amendments: None,
            additional_permissions: None,
            available_decisions: None,
        },
    );
    assert!(
        !chat.no_modal_or_popup_active(),
        "an approval request must still surface a modal while hidden"
    );
    // Dismiss the modal so it doesn't interfere with later assertions in this test.
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    drain_insert_history(&mut rx);

    // Errors stay visible.
    handle_error(
        &mut chat,
        "something went wrong",
        /*codex_error_info*/ None,
    );
    let error_cells = drain_insert_history(&mut rx);
    assert!(
        error_cells
            .iter()
            .any(|lines| lines_to_single_string(lines).contains("something went wrong")),
        "errors must remain visible while hidden"
    );
}

// --- Persisted-transcript (`thread_transcript`) filtering ---------------------------------------

fn transcript_cell_text(
    items: Vec<AppServerThreadItem>,
    visibility: crate::thread_transcript::ToolActivityVisibility,
) -> String {
    let cwd = test_path_buf("/tmp/project").abs();
    let cells = crate::thread_transcript::thread_items_to_transcript_cells(
        /*thread_id*/ None,
        &cwd,
        items,
        crate::thread_transcript::RawReasoningVisibility::Hidden,
        visibility,
        /*config*/ None,
    );
    cells
        .iter()
        .map(|cell| lines_to_single_string(&cell.display_lines(/*width*/ 80)))
        .collect::<String>()
}

fn persisted_command(
    id: &str,
    source: ExecCommandSource,
    status: AppServerCommandExecutionStatus,
    exit_code: Option<i32>,
) -> AppServerThreadItem {
    AppServerThreadItem::CommandExecution {
        model_context: None,
        id: id.to_string(),
        command: format!("printf {id}"),
        cwd: test_path_buf("/tmp/project").abs().into(),
        process_id: None,
        plugin_id: None,
        script_path: None,
        source,
        status,
        command_actions: Vec::new(),
        aggregated_output: Some(format!("{id}\n")),
        exit_code,
        duration_ms: Some(5),
    }
}

#[tokio::test]
async fn hidden_persisted_transcript_omits_routine_successful_agent_command() {
    let text = transcript_cell_text(
        vec![persisted_command(
            "call-persisted-agent",
            ExecCommandSource::Agent,
            AppServerCommandExecutionStatus::Completed,
            Some(0),
        )],
        crate::thread_transcript::ToolActivityVisibility::Hidden,
    );
    assert!(
        !text.contains("call-persisted-agent") && !text.contains("printf call-persisted-agent"),
        "hidden persisted transcript must omit a routine successful Agent command: {text:?}"
    );
}

#[tokio::test]
async fn hidden_persisted_transcript_keeps_user_shell_command_visible() {
    let text = transcript_cell_text(
        vec![persisted_command(
            "call-persisted-user",
            ExecCommandSource::UserShell,
            AppServerCommandExecutionStatus::Completed,
            Some(0),
        )],
        crate::thread_transcript::ToolActivityVisibility::Hidden,
    );
    assert!(text.contains("printf call-persisted-user"));
}

#[tokio::test]
async fn hidden_persisted_transcript_keeps_failed_and_declined_commands_visible() {
    for status in [
        AppServerCommandExecutionStatus::Failed,
        AppServerCommandExecutionStatus::Declined,
    ] {
        let text = transcript_cell_text(
            vec![persisted_command(
                "call-persisted-failure",
                ExecCommandSource::Agent,
                status.clone(),
                /*exit_code*/ None,
            )],
            crate::thread_transcript::ToolActivityVisibility::Hidden,
        );
        assert!(
            text.contains("printf call-persisted-failure"),
            "status {status:?} with exit_code=None must remain visible: {text:?}"
        );
    }
}

#[tokio::test]
async fn hidden_persisted_transcript_omits_routine_mcp_web_image_file_collab_subagent_dynamic() {
    let items = vec![
        mcp_item(
            "mcp-ok",
            McpToolCallStatus::Completed,
            mcp_success_result(),
            None,
        ),
        AppServerThreadItem::WebSearch(WebSearchItem {
            id: "web-ok".to_string(),
            query: "persisted query".to_string(),
            action: None,
            results: None,
        }),
        AppServerThreadItem::ImageView {
            id: "image-view-ok".to_string(),
            path: chat_image_path(),
        },
        AppServerThreadItem::FileChange {
            id: "file-ok".to_string(),
            changes: Vec::new(),
            status: AppServerPatchApplyStatus::Completed,
        },
        collab_wait_item("collab-ok", AppServerCollabAgentToolCallStatus::Completed),
        sub_agent_item("sub-agent-ok", SubAgentActivityKind::Completed),
        AppServerThreadItem::DynamicToolCall {
            id: "dyn-ok".to_string(),
            namespace: None,
            tool: "dyn-tool".to_string(),
            arguments: json!({}),
            status: codex_app_server_protocol::DynamicToolCallStatus::Completed,
            content_items: None,
            success: Some(true),
            duration_ms: Some(5),
        },
    ];
    let text = transcript_cell_text(
        items,
        crate::thread_transcript::ToolActivityVisibility::Hidden,
    );
    assert_eq!(
        text, "",
        "hidden persisted transcript must omit every routine successful tool fallback cell: \
         {text:?}"
    );
}

fn chat_image_path() -> LegacyAppPathString {
    serde_json::from_value(json!("example.png")).expect("valid legacy app path string")
}

#[tokio::test]
async fn hidden_persisted_transcript_keeps_mcp_failure_visible() {
    let text = transcript_cell_text(
        vec![mcp_item(
            "mcp-failed",
            McpToolCallStatus::Failed,
            None,
            Some(McpToolCallError {
                message: "boom".to_string(),
            }),
        )],
        crate::thread_transcript::ToolActivityVisibility::Hidden,
    );
    // McpToolCallCell renders a failed result as "Error: <message>" (history_cell/mcp.rs), not
    // the literal word "failed" or the item id, so assert on the rendered error text and on
    // non-emptiness rather than a literal "failed"/"mcp-failed" substring.
    assert!(
        text.to_lowercase().contains("error") || text.contains("boom"),
        "a Failed MCP result must remain visible in the persisted transcript: {text:?}"
    );
    assert!(!text.is_empty());
}

#[tokio::test]
async fn hidden_persisted_transcript_keeps_dynamic_tool_call_failure_visible_at_concise_and_quiet()
{
    let failed_item = || AppServerThreadItem::DynamicToolCall {
        id: "dyn-failed".to_string(),
        namespace: None,
        tool: "dyn-tool".to_string(),
        arguments: json!({}),
        status: codex_app_server_protocol::DynamicToolCallStatus::Failed,
        content_items: None,
        success: Some(false),
        duration_ms: Some(5),
    };

    let concise_text = transcript_cell_text(
        vec![failed_item()],
        crate::thread_transcript::ToolActivityVisibility::Hidden,
    );
    assert!(
        !concise_text.is_empty(),
        "at Concise, a Failed DynamicToolCall must remain visible in the persisted transcript"
    );

    let quiet_text = transcript_cell_text(
        vec![failed_item()],
        crate::thread_transcript::ToolActivityVisibility::Hidden,
    );
    assert!(
        !quiet_text.is_empty(),
        "at Quiet, a Failed DynamicToolCall must remain visible in the persisted transcript: {quiet_text:?}"
    );
}

#[tokio::test]
async fn shown_persisted_transcript_is_stock_equivalent() {
    let items = vec![persisted_command(
        "call-shown-persisted",
        ExecCommandSource::Agent,
        AppServerCommandExecutionStatus::Completed,
        Some(0),
    )];
    let hidden_text = transcript_cell_text(
        items.clone(),
        crate::thread_transcript::ToolActivityVisibility::Hidden,
    );
    let shown_text = transcript_cell_text(
        items,
        crate::thread_transcript::ToolActivityVisibility::Shown,
    );

    assert!(hidden_text.is_empty());
    assert!(shown_text.contains("printf call-shown-persisted"));
}

// =================================================================================================
// Task 5: `/tool-activity off|concise|quiet|status`
// =================================================================================================

#[tokio::test]
async fn slash_tool_activity_concise_sets_state_and_reports_hidden() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Off);

    chat.dispatch_command_with_args(
        SlashCommand::ToolActivity,
        "concise".to_string(),
        Vec::new(),
    );

    assert_eq!(chat.tool_activity_level(), ToolActivity::Concise);
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1);
    assert!(
        lines_to_single_string(&cells[0]).contains("Tool activity: concise (routine summaries)")
    );
}

#[tokio::test]
async fn slash_tool_activity_off_sets_state_and_reports_shown() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Quiet);

    chat.dispatch_command_with_args(SlashCommand::ToolActivity, "off".to_string(), Vec::new());

    assert_eq!(chat.tool_activity_level(), ToolActivity::Off);
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1);
    assert!(lines_to_single_string(&cells[0]).contains("Tool activity: shown"));
}

#[tokio::test]
async fn slash_tool_activity_sequential_transitions_report_each_resulting_state() {
    // No more `toggle` subcommand under the three-level enum (there is no well-defined "flip"
    // across three states). This replaces the old toggle-flip test's intent — that each mutating
    // dispatch reports the correct resulting state — with an off -> quiet -> off round trip.
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Off);

    chat.dispatch_command_with_args(SlashCommand::ToolActivity, "quiet".to_string(), Vec::new());
    assert_eq!(chat.tool_activity_level(), ToolActivity::Quiet);
    let cells = drain_insert_history(&mut rx);
    assert!(lines_to_single_string(&cells[0]).contains(
        "Tool activity: quiet (default; routine hidden; use /tool-activity off for full output)"
    ));

    chat.dispatch_command_with_args(SlashCommand::ToolActivity, "off".to_string(), Vec::new());
    assert_eq!(chat.tool_activity_level(), ToolActivity::Off);
    let cells = drain_insert_history(&mut rx);
    assert!(lines_to_single_string(&cells[0]).contains("Tool activity: shown"));
}

#[tokio::test]
async fn slash_tool_activity_status_does_not_mutate_state() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Concise);

    chat.dispatch_command_with_args(SlashCommand::ToolActivity, "status".to_string(), Vec::new());

    assert_eq!(
        chat.tool_activity_level(),
        ToolActivity::Concise,
        "status must not mutate state"
    );
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1);
    assert!(
        lines_to_single_string(&cells[0]).contains("Tool activity: concise (routine summaries)")
    );
}

#[tokio::test]
async fn bare_slash_tool_activity_reports_status_without_mutation() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Off);

    chat.dispatch_command(SlashCommand::ToolActivity);

    assert_eq!(
        chat.tool_activity_level(),
        ToolActivity::Off,
        "bare command must not mutate state"
    );
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1);
    assert!(lines_to_single_string(&cells[0]).contains("Tool activity: shown"));
}

#[tokio::test]
async fn slash_tool_activity_invalid_input_is_a_usage_error_and_state_unchanged() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Off);

    chat.dispatch_command_with_args(SlashCommand::ToolActivity, "wat".to_string(), Vec::new());

    assert_eq!(
        chat.tool_activity_level(),
        ToolActivity::Off,
        "invalid input must not mutate state"
    );
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1);
    assert!(
        lines_to_single_string(&cells[0])
            .contains("Usage: /tool-activity off|concise|quiet|status")
    );
}

#[tokio::test]
async fn tool_activity_slash_command_is_available_during_active_task() {
    assert!(SlashCommand::ToolActivity.available_during_task());
    assert!(SlashCommand::ToolActivity.supports_inline_args());
}

// Quiet and Concise share the same safety-preserving presentation matrix. Both hide routine
// successful activity while retaining Failed/Declined/error surfaces and other control records.

fn failed_exec_cell_count(
    chat: &mut ChatWidget,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
) -> usize {
    let begin = begin_exec(chat, "call-level2-failed", "printf level2-failed");
    let AppServerThreadItem::CommandExecution {
        id,
        command,
        cwd,
        process_id,
        plugin_id,
        script_path,
        source,
        command_actions,
        ..
    } = begin
    else {
        panic!("expected command execution item");
    };
    handle_exec_end(
        chat,
        AppServerThreadItem::CommandExecution {
            model_context: None,
            id,
            command,
            cwd,
            process_id,
            plugin_id,
            script_path,
            source,
            status: AppServerCommandExecutionStatus::Failed,
            command_actions,
            aggregated_output: Some("boom\n".to_string()),
            exit_code: None,
            duration_ms: Some(5),
        },
    );
    drain_insert_history(rx).len()
}

#[tokio::test]
async fn concise_reproduces_existing_shipped_behavior_failed_exec_stays_visible() {
    // Regression guard: ToolActivity::Concise must reproduce the original tool-activity level-1
    // behavior exactly — a Failed exec always renders.
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Concise);
    chat.on_task_started();

    assert_eq!(
        failed_exec_cell_count(&mut chat, &mut rx),
        1,
        "ToolActivity::Concise must still show a Failed exec"
    );
}

#[tokio::test]
async fn quiet_shows_failed_exec() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Quiet);
    chat.on_task_started();

    assert_eq!(
        failed_exec_cell_count(&mut chat, &mut rx),
        1,
        "ToolActivity::Quiet must show a Failed exec"
    );
}

#[tokio::test]
async fn quiet_concise_and_off_show_mcp_failure() {
    // (a) Concise: MCP failure still shows (regression guard).
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Concise);
    handle_mcp_started(&mut chat, "call-mcp-level2-a");
    handle_mcp_completed(
        &mut chat,
        "call-mcp-level2-a",
        McpToolCallStatus::Failed,
        None,
        Some(McpToolCallError {
            message: "boom".to_string(),
        }),
    );
    assert_eq!(
        drain_insert_history(&mut rx).len(),
        2,
        "ToolActivity::Concise must still show an MCP failure"
    );

    // (b) Quiet: MCP failure still shows.
    chat.set_tool_activity_level(ToolActivity::Quiet);
    handle_mcp_started(&mut chat, "call-mcp-level2-b");
    handle_mcp_completed(
        &mut chat,
        "call-mcp-level2-b",
        McpToolCallStatus::Failed,
        None,
        Some(McpToolCallError {
            message: "boom".to_string(),
        }),
    );
    assert_eq!(
        drain_insert_history(&mut rx).len(),
        1,
        "ToolActivity::Quiet must show an MCP failure"
    );

    // (c) Off: MCP failure still shows.
    chat.set_tool_activity_level(ToolActivity::Off);
    handle_mcp_started(&mut chat, "call-mcp-level2-c");
    handle_mcp_completed(
        &mut chat,
        "call-mcp-level2-c",
        McpToolCallStatus::Failed,
        None,
        Some(McpToolCallError {
            message: "boom".to_string(),
        }),
    );
    assert_eq!(
        drain_insert_history(&mut rx).len(),
        1,
        "ToolActivity::Off must show an MCP failure"
    );

    // (d) Quiet: a failed status must remain visible even when the protocol carries a result.
    chat.set_tool_activity_level(ToolActivity::Quiet);
    handle_mcp_started(&mut chat, "call-mcp-level2-d");
    handle_mcp_completed(
        &mut chat,
        "call-mcp-level2-d",
        McpToolCallStatus::Failed,
        mcp_success_result(),
        None,
    );
    assert_eq!(
        drain_insert_history(&mut rx).len(),
        1,
        "ToolActivity::Quiet must show a failed MCP result"
    );
}

#[tokio::test]
async fn quiet_persisted_transcript_retains_failed_command() {
    let items = || {
        vec![persisted_command(
            "call-persisted-level2",
            ExecCommandSource::Agent,
            AppServerCommandExecutionStatus::Failed,
            /*exit_code*/ None,
        )]
    };

    // Shown: visible.
    let text = transcript_cell_text(
        items(),
        crate::thread_transcript::ToolActivityVisibility::Shown,
    );
    assert!(text.contains("printf call-persisted-level2"));

    // Hidden (Concise-equivalent): still visible — regression guard for the shipped behavior.
    let text = transcript_cell_text(
        items(),
        crate::thread_transcript::ToolActivityVisibility::Hidden,
    );
    assert!(
        text.contains("printf call-persisted-level2"),
        "Concise-equivalent Hidden must still show a Failed command in persisted transcript: \
         {text:?}"
    );
}

#[tokio::test]
async fn tool_activity_visibility_from_matrix() {
    use crate::thread_transcript::ToolActivityVisibility;

    assert_eq!(
        ToolActivityVisibility::from(ToolActivity::Off),
        ToolActivityVisibility::Shown
    );
    assert_eq!(
        ToolActivityVisibility::from(ToolActivity::Concise),
        ToolActivityVisibility::Hidden
    );
    assert_eq!(
        ToolActivityVisibility::from(ToolActivity::Quiet),
        ToolActivityVisibility::Hidden
    );
}

#[tokio::test]
async fn slash_tool_activity_quiet_sets_state_and_notifies() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_tool_activity_level(ToolActivity::Off);

    chat.dispatch_command_with_args(SlashCommand::ToolActivity, "quiet".to_string(), Vec::new());

    assert_eq!(chat.tool_activity_level(), ToolActivity::Quiet);
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1);
    assert!(lines_to_single_string(&cells[0]).contains(
        "Tool activity: quiet (default; routine hidden; use /tool-activity off for full output)"
    ));
}

#[tokio::test]
async fn slash_tool_activity_concise_after_quiet_relaxes_to_concise() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command_with_args(SlashCommand::ToolActivity, "quiet".to_string(), Vec::new());
    assert_eq!(chat.tool_activity_level(), ToolActivity::Quiet);
    drain_insert_history(&mut rx);

    chat.dispatch_command_with_args(
        SlashCommand::ToolActivity,
        "concise".to_string(),
        Vec::new(),
    );

    assert_eq!(
        chat.tool_activity_level(),
        ToolActivity::Concise,
        "`concise` after `quiet` must relax to Concise, not leave stale Quiet state"
    );
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1);
    assert!(
        lines_to_single_string(&cells[0]).contains("Tool activity: concise (routine summaries)")
    );
}

#[tokio::test]
async fn slash_tool_activity_off_clears_state_after_quiet() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.dispatch_command_with_args(SlashCommand::ToolActivity, "quiet".to_string(), Vec::new());
    drain_insert_history(&mut rx);

    chat.dispatch_command_with_args(SlashCommand::ToolActivity, "off".to_string(), Vec::new());

    assert_eq!(chat.tool_activity_level(), ToolActivity::Off);
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1);
    assert!(lines_to_single_string(&cells[0]).contains("Tool activity: shown"));
}

#[tokio::test]
async fn slash_tool_activity_status_reports_quiet_state_without_mutating() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.dispatch_command_with_args(SlashCommand::ToolActivity, "quiet".to_string(), Vec::new());
    drain_insert_history(&mut rx);

    chat.dispatch_command_with_args(SlashCommand::ToolActivity, "status".to_string(), Vec::new());

    assert_eq!(
        chat.tool_activity_level(),
        ToolActivity::Quiet,
        "status must not mutate tool_activity_level"
    );
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1);
    let text = lines_to_single_string(&cells[0]);
    assert!(text.contains(
        "Tool activity: quiet (default; routine hidden; use /tool-activity off for full output)"
    ));
}

#[tokio::test]
async fn set_tool_activity_level_and_notify_covers_all_three_notice_strings() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.set_tool_activity_level_and_notify(ToolActivity::Quiet);
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1);
    assert!(lines_to_single_string(&cells[0]).contains(
        "Tool activity: quiet (default; routine hidden; use /tool-activity off for full output)"
    ));

    chat.set_tool_activity_level_and_notify(ToolActivity::Concise);
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1);
    assert!(
        lines_to_single_string(&cells[0]).contains("Tool activity: concise (routine summaries)")
    );

    chat.set_tool_activity_level_and_notify(ToolActivity::Off);
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1);
    assert!(lines_to_single_string(&cells[0]).contains("Tool activity: shown"));
}
