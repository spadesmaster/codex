//! Non-command tool lifecycle rendering for `ChatWidget`.
//!
//! This module handles patch, MCP, web search, image, and collaborator tool
//! events as transcript cells.

use super::*;
use crate::thread_transcript::tools::McpHistory;
use codex_utils_path_uri::LegacyAppPathString;

impl ChatWidget {
    pub(super) fn on_patch_apply_begin(&mut self, changes: HashMap<PathBuf, FileChange>) {
        // tool-activity: the routine "Edited ..." row is presentation-only skippable; a failure is
        // rendered separately and unconditionally by `handle_file_change_completed_now`.
        if self.hides_routine() {
            self.add_concise_tool_activity_summary(format!("Edited {} file(s)", changes.len()));
            return;
        }
        self.add_to_history(history_cell::new_patch_event(changes, &self.config.cwd));
    }

    pub(super) fn on_view_image_tool_call(&mut self, path: LegacyAppPathString) {
        self.flush_answer_stream_with_separator();
        if self.hides_routine() {
            self.add_concise_tool_activity_summary("Viewed image".to_string());
        } else {
            self.add_to_history(history_cell::new_view_image_tool_call(path));
        }
        self.request_redraw();
    }

    pub(super) fn on_image_generation_begin(&mut self) {
        self.flush_answer_stream_with_separator();
        if self.bottom_pane.is_task_running() {
            self.bottom_pane.ensure_status_indicator();
        }
    }

    pub(super) fn on_image_generation_end(
        &mut self,
        call_id: String,
        status: String,
        revised_prompt: Option<String>,
        saved_path: Option<AbsolutePathBuf>,
    ) {
        self.flush_answer_stream_with_separator();
        // tool-activity: `new_image_generation_call` treats status == "failed" as the sole failure
        // discriminant (see history_cell::patches::new_image_generation_call); any other status
        // value is the routine successful-completion row and is hideable. The failure row is
        // retained at every tool-activity level.
        let is_failed = status == "failed";
        if self.hides_routine() && !is_failed {
            self.add_concise_tool_activity_summary("Image generation completed".to_string());
            return;
        }
        self.add_to_history(history_cell::new_image_generation_call(
            call_id,
            &status,
            revised_prompt,
            saved_path,
        ));
        self.request_redraw();
    }

    pub(super) fn on_file_change_completed(&mut self, item: ThreadItem) {
        self.defer_or_handle(
            item,
            InterruptManager::push_item_completed,
            Self::handle_file_change_completed_now,
        );
    }

    pub(super) fn on_mcp_tool_call_started(&mut self, item: ThreadItem) {
        self.defer_or_handle(
            item,
            InterruptManager::push_item_started,
            Self::handle_mcp_tool_call_started_now,
        );
    }

    pub(super) fn on_mcp_tool_call_completed(&mut self, item: ThreadItem) {
        self.defer_or_handle(
            item,
            InterruptManager::push_item_completed,
            Self::handle_mcp_tool_call_completed_now,
        );
    }

    pub(super) fn on_web_search_begin(&mut self, call_id: String) {
        self.flush_answer_stream_with_separator();
        if self.hides_routine() {
            // Web search has no distinct failure surface in this handler pair; concise mode keeps
            // one compact start record while quiet mode remains silent.
            self.add_concise_tool_activity_summary("Web search started".to_string());
            self.hidden_web_search_calls.insert(call_id);
            return;
        }
        self.flush_active_cell();
        self.transcript.active_cell = Some(Box::new(history_cell::new_active_web_search_call(
            call_id,
            String::new(),
            self.local_settings.tui.animations && self.local_settings.tui.effects.progress,
        )));
        self.bump_active_cell_revision();
        self.request_redraw();
    }

    pub(super) fn on_web_search_end(
        &mut self,
        call_id: String,
        query: String,
        action: codex_app_server_protocol::WebSearchAction,
    ) {
        self.flush_answer_stream_with_separator();
        if self.hidden_web_search_calls.remove(&call_id) {
            self.transcript.had_work_activity = true;
            return;
        }
        let mut handled = false;
        if let Some(cell) = self
            .transcript
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<WebSearchCell>())
            && cell.call_id() == call_id
        {
            cell.update(action.clone(), query.clone());
            cell.complete();
            self.bump_active_cell_revision();
            self.flush_active_cell();
            handled = true;
        }

        if !handled {
            self.add_to_history(history_cell::new_web_search_call(call_id, query, action));
        }
    }

    pub(super) fn on_collab_event(&mut self, cell: PlainHistoryCell) {
        self.flush_answer_stream_with_separator();
        self.add_to_history(cell);
        self.request_redraw();
    }

    pub(super) fn on_collab_agent_tool_call(&mut self, item: ThreadItem) {
        let ThreadItem::CollabAgentToolCall {
            id, tool, status, ..
        } = &item
        else {
            return;
        };
        if matches!(tool, CollabAgentTool::SpawnAgent)
            && let Some(spawn_request) = multi_agents::spawn_request_summary(&item)
        {
            self.pending_collab_spawn_requests
                .insert(id.clone(), spawn_request);
        }

        let cached_spawn_request = if matches!(tool, CollabAgentTool::SpawnAgent)
            && !matches!(status, CollabAgentToolCallStatus::InProgress)
        {
            self.pending_collab_spawn_requests.remove(id)
        } else {
            None
        };

        // tool-activity: spawn/pending metadata above is updated unconditionally regardless of
        // visibility. Only a routine Completed status is hideable; Failed/Interrupted/InProgress
        // status rendering decisions are left to tool_call_history_cell as normal. Under level 2
        // Failed/Interrupted records remain visible at every tool-activity level.
        if self.hides_routine() && matches!(status, CollabAgentToolCallStatus::Completed) {
            self.add_concise_tool_activity_summary("Collaboration completed".to_string());
            return;
        }

        if let Some(cell) = multi_agents::tool_call_history_cell(
            &item,
            cached_spawn_request.as_ref(),
            |thread_id| self.collab_agent_metadata(thread_id),
        ) {
            self.on_collab_event(cell);
        }
    }

    pub(super) fn on_sub_agent_activity(&mut self, item: ThreadItem) {
        // Background agents can finish while the parent answer is still streaming.
        // Keep that stream intact until its authoritative message completion.
        // After the turn stops, leftover prompts must not hold up late activity.
        if !self.turn_lifecycle.agent_turn_running && self.stream_controller.is_none() {
            self.handle_sub_agent_activity_now(item);
        } else {
            self.defer_or_handle(
                item,
                InterruptManager::push_item_completed,
                Self::handle_sub_agent_activity_now,
            );
        }
    }

    fn handle_sub_agent_activity_now(&mut self, item: ThreadItem) {
        // tool-activity: SubAgentActivityKind has no distinct failure/error variant; Started,
        // Interacted, and Completed are routine and hideable, while Interrupted signals an
        // abnormal end and stays visible, matching the Failed/Declined precedent elsewhere.
        // Interrupted records remain visible at every tool-activity level.
        if self.hides_routine()
            && matches!(
                item,
                ThreadItem::SubAgentActivity {
                    kind: codex_app_server_protocol::SubAgentActivityKind::Started
                        | codex_app_server_protocol::SubAgentActivityKind::Interacted
                        | codex_app_server_protocol::SubAgentActivityKind::Completed,
                    ..
                }
            )
        {
            self.add_concise_tool_activity_summary("Sub-agent activity completed".to_string());
            return;
        }
        if let Some(cell) = multi_agents::sub_agent_activity_history_cell(&item) {
            self.on_collab_event(cell);
        }
    }

    pub(crate) fn handle_file_change_completed_now(&mut self, item: ThreadItem) {
        let ThreadItem::FileChange { status, .. } = item else {
            return;
        };
        // If the patch was successful, just let the "Edited" block stand.
        // Otherwise, add a failure block at every tool-activity level.
        if matches!(status, codex_app_server_protocol::PatchApplyStatus::Failed) {
            self.add_to_history(history_cell::new_patch_apply_failure(String::new()));
        }
    }

    pub(crate) fn handle_mcp_tool_call_started_now(&mut self, item: ThreadItem) {
        let ThreadItem::McpToolCall {
            id,
            server,
            tool,
            arguments,
            ..
        } = item
        else {
            return;
        };
        if self.hides_routine() {
            self.add_concise_tool_activity_summary("MCP call started".to_string());
            // Latch the call id hidden and create no active cell. On completion,
            // a successful result stays hidden; a failure falls through to the existing
            // "no matching active cell" fallback below, which builds and shows it fresh.
            self.hidden_mcp_calls.insert(id);
            return;
        }
        self.flush_answer_stream_with_separator();
        let invocation = McpInvocation {
            server,
            tool,
            arguments: Some(arguments),
        };
        if invocation.is_computer_activity() {
            let call = history_cell::new_active_mcp_tool_call(
                id,
                invocation,
                self.local_settings.tui.animations && self.local_settings.tui.effects.progress,
            );
            self.update_computer_activity(|cell| cell.start(call));
            self.bump_active_cell_revision();
            self.request_redraw();
            return;
        }
        self.flush_active_cell();
        self.transcript.active_cell = Some(Box::new(history_cell::new_active_mcp_tool_call(
            id,
            invocation,
            self.local_settings.tui.animations && self.local_settings.tui.effects.progress,
        )));
        self.bump_active_cell_revision();
        self.request_redraw();
    }

    pub(crate) fn handle_mcp_tool_call_completed_now(&mut self, item: ThreadItem) {
        self.flush_answer_stream_with_separator();

        // Captured before `McpHistory::from_item` consumes `item`; `McpHistory` collapses status
        // and error into `result: Result<CallToolResult, String>` and does not retain the raw
        // `McpToolCallStatus`, which the hidden/visible decision below still needs.
        let status = match &item {
            ThreadItem::McpToolCall { status, .. } => Some(status.clone()),
            _ => None,
        };

        let Some(McpHistory {
            id,
            invocation,
            duration,
            result,
        }) = McpHistory::from_item(item)
        else {
            return;
        };
        let was_hidden = self.hidden_mcp_calls.remove(&id);
        if was_hidden
            && result.is_ok()
            && status.as_ref() != Some(&codex_app_server_protocol::McpToolCallStatus::Failed)
        {
            self.transcript.had_work_activity = true;
            return;
        }

        if invocation.is_computer_activity() {
            let call = history_cell::new_active_mcp_tool_call(
                id,
                invocation,
                self.local_settings.tui.animations && self.local_settings.tui.effects.progress,
            );
            self.update_computer_activity(|cell| cell.complete(call, duration, result));
            self.bump_active_cell_revision();
            self.request_redraw();
            return;
        }

        match self
            .transcript
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<McpToolCallCell>())
        {
            Some(cell) if cell.call_id() == id => cell.complete(duration, result),
            _ => {
                self.flush_active_cell();
                let mut cell = history_cell::new_active_mcp_tool_call(
                    id,
                    invocation,
                    self.local_settings.tui.animations && self.local_settings.tui.effects.progress,
                );
                cell.complete(duration, result);
                self.transcript.active_cell = Some(Box::new(cell));
            }
        };

        self.flush_active_cell();
    }

    /// Preserve pending calls and timers, returning the revisions before and after hydration.
    pub(crate) fn prepend_active_computer_history(
        &mut self,
        older: &dyn HistoryCell,
        turns: &[Turn],
    ) -> Option<(u64, u64)> {
        let previous_revision = self.transcript.active_cell_revision;
        let active = self.transcript.active_cell.as_mut().and_then(|cell| {
            cell.as_any_mut()
                .downcast_mut::<history_cell::ComputerActivityCell>()
        })?;
        let older = crate::thread_transcript::older_computer_group(older, active, turns)?;
        active.prepend(older);
        self.bump_active_cell_revision();
        Some((previous_revision, self.transcript.active_cell_revision))
    }

    /// Reuse only adjacent computer calls; all other active cells form a transcript boundary.
    fn update_computer_activity(
        &mut self,
        update: impl FnOnce(&mut history_cell::ComputerActivityCell),
    ) {
        if let Some(cell) = self.transcript.active_cell.as_mut().and_then(|cell| {
            cell.as_any_mut()
                .downcast_mut::<history_cell::ComputerActivityCell>()
        }) {
            update(cell);
        } else {
            self.flush_active_cell();
            let mut cell = history_cell::ComputerActivityCell::default();
            update(&mut cell);
            self.transcript.active_cell = Some(Box::new(cell));
        }
    }

    pub(crate) fn handle_queued_item_started_now(&mut self, item: ThreadItem) {
        match item {
            item @ ThreadItem::CommandExecution { .. } => {
                self.handle_command_execution_started_now(item);
            }
            item @ ThreadItem::McpToolCall { .. } => {
                self.handle_mcp_tool_call_started_now(item);
            }
            item @ ThreadItem::DynamicToolCall { .. } => self.handle_dynamic_tool_item_now(item),
            _ => {}
        }
    }

    pub(crate) fn handle_queued_item_completed_now(&mut self, item: ThreadItem) {
        match item {
            item @ ThreadItem::CommandExecution { .. } => {
                self.handle_command_execution_completed_now(item, false);
            }
            item @ ThreadItem::FileChange { .. } => self.handle_file_change_completed_now(item),
            item @ ThreadItem::McpToolCall { .. } => self.handle_mcp_tool_call_completed_now(item),
            item @ ThreadItem::DynamicToolCall { .. } => self.handle_dynamic_tool_item_now(item),
            item @ ThreadItem::SubAgentActivity { .. } => self.handle_sub_agent_activity_now(item),
            _ => {}
        }
    }
}
