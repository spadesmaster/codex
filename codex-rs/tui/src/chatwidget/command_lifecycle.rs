//! Command execution lifecycle handlers for `ChatWidget`.
//!
//! This module owns command start/output/completion rendering, including active
//! exec-cell grouping and unified exec wait state.

use super::*;
use crate::exec_cell::CommandOutput;
use std::fs::OpenOptions;
use std::io::Write;

/// Unified V3 tool-activity log line severity glyph: 🔴 failure (A3/A4), 🟡
/// warning (A5/A4 sharing the same log-write site, per the Spec's format table).
#[derive(Clone, Copy)]
pub(super) enum ToolActivitySeverity {
    Failure,
    Warning,
}

impl ToolActivitySeverity {
    fn glyph(self) -> &'static str {
        match self {
            Self::Failure => "🔴",
            Self::Warning => "🟡",
        }
    }
}

pub(super) fn append_v3_tool_line(
    path: &std::path::Path,
    severity: ToolActivitySeverity,
    tool: &str,
    call_id: &str,
    exit_code: i32,
    message: &str,
) {
    let message: String = message
        .replace(['\r', '\n'], " ")
        .chars()
        .take(400)
        .collect();
    let line = format!(
        "{} {} 🧰{} 🔗{} 📝\"exit={} {}\"\n",
        chrono::Local::now().format("%d-%b %H:%M:%S"),
        severity.glyph(),
        tool,
        call_id,
        exit_code,
        message
    );
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}

/// M135 A5/A8 warning marker for exec-command results: a case-insensitive
/// `"warning"` token in the command's own aggregated output. See the longer
/// rationale on `is_warning_output` in `core/src/context_manager/history.rs`
/// (the A8 counterpart) for why this is new, additive mechanism rather than a
/// reuse of an existing classification.
///
/// **False-positive narrowing (M135 gap-closure pass):** a plain substring
/// check also matches a clean run's own "0 warnings"/"no warnings found"
/// summary line, hiding output that reported nothing wrong. A line matching
/// one of `ZERO_WARNING_PHRASES` does not itself count as the marker, but any
/// *other* line in the same output that still contains "warning" does, so a
/// real warning sitting next to an unrelated "0 warnings" summary is still
/// caught. Kept behaviorally in sync with, but not code-shared with, the
/// core crate's own copy (`is_warning_output` in
/// `core/src/context_manager/history.rs`), matching how those two
/// independent classifications are already duplicated across crates. This
/// does not attempt every possible false-positive phrasing (e.g. reversed
/// "warnings: 0") — narrowing further than the operator-cited examples would
/// be inventing an unspecified heuristic, not closing the disclosed gap.
const ZERO_WARNING_PHRASES: &[&str] = &[
    "0 warnings",
    "0 warning",
    "no warnings",
    "no warning",
    "zero warnings",
];

pub(super) fn contains_warning_marker(aggregated_output: &str) -> bool {
    aggregated_output.lines().any(|line| {
        let lower = line.to_ascii_lowercase();
        lower.contains("warning")
            && !ZERO_WARNING_PHRASES
                .iter()
                .any(|phrase| lower.contains(phrase))
    })
}

impl ChatWidget {
    fn is_unified_exec_process_hidden(&self, process_id: &str) -> bool {
        self.unified_exec_processes
            .iter()
            .find(|process| process.key == process_id)
            .is_some_and(|process| self.hidden_exec_calls.contains(&process.call_id))
    }

    fn add_concise_exec_summary(&mut self, command: &[String]) {
        let summary = command.join(" ");
        if !summary.is_empty() {
            self.add_info_message(format!("Completed: {summary}"), None);
        }
    }

    pub(super) fn flush_unified_exec_wait_streak(&mut self) {
        let Some(wait) = self.unified_exec_wait_streak.take() else {
            return;
        };
        if self.is_unified_exec_process_hidden(&wait.process_id) {
            self.restore_reasoning_status_header();
            return;
        }
        let cell = history_cell::new_unified_exec_interaction(wait.command_display, String::new());
        if let Err(cell) = self.absorb_activity_detail(Box::new(cell)) {
            self.app_event_tx.send(AppEvent::InsertHistoryCell(cell));
        }
        self.restore_reasoning_status_header();
    }

    pub(super) fn on_command_execution_started(&mut self, item: ThreadItem) {
        let ThreadItem::CommandExecution {
            id,
            command,
            process_id,
            source,
            command_actions,
            ..
        } = &item
        else {
            return;
        };
        let (_command, parsed_cmd) = command_execution_command_and_parsed(command, command_actions);
        self.flush_answer_stream_with_separator();
        if is_unified_exec_source(*source) {
            if *source == ExecCommandSource::UnifiedExecStartup {
                self.track_unified_exec_process_begin(id, process_id.as_deref(), command);
            }
            // Latch hidden calls before early returns so completion bookkeeping stays paired.
            // A non-standard (`ParsedCommand::Unknown`) unified-exec command — e.g. `sleep`,
            // `cargo test`, `just fix` — never reaches `handle_command_execution_started_now`
            // (see the `is_standard_tool_call` early return), so without this the wait/interaction
            // suppression in `flush_unified_exec_wait_streak`/`on_terminal_interaction` would never
            // find the call latched and would leak a transcript row while hidden.
            if !self.should_render_exec_activity(*source) {
                self.hidden_exec_calls.insert(id.clone());
            }
            if !self.bottom_pane.is_task_running() {
                return;
            }
            // Unified exec may be parsed as Unknown; keep the working indicator visible regardless.
            self.bottom_pane.ensure_status_indicator();
            if !is_standard_tool_call(&parsed_cmd) {
                return;
            }
        }
        self.defer_or_handle(
            item,
            InterruptManager::push_item_started,
            Self::handle_command_execution_started_now,
        );
    }

    pub(super) fn on_exec_command_output_delta(&mut self, call_id: &str, delta: &str) {
        self.track_unified_exec_output_chunk(call_id, delta.as_bytes());
        if !self.bottom_pane.is_task_running() {
            return;
        }

        let Some(cell) = self
            .transcript
            .active_cell
            .as_mut()
            .and_then(|c| c.as_any_mut().downcast_mut::<ExecCell>())
        else {
            return;
        };

        if cell.append_output(call_id, delta) {
            self.bump_active_cell_revision();
            self.request_redraw();
        }
    }

    pub(super) fn on_terminal_interaction(&mut self, process_id: String, stdin: String) {
        if !self.bottom_pane.is_task_running() {
            return;
        }
        let command_display = self
            .unified_exec_processes
            .iter()
            .find(|process| process.key == process_id)
            .map(|process| process.command_display.clone());
        if stdin.is_empty() && command_display.is_none() {
            return;
        }

        self.flush_answer_stream_with_separator();
        if stdin.is_empty() {
            // Empty stdin means we are polling for background output.
            // Surface this in the status indicator (single "waiting" surface) instead of
            // the transcript. Keep the header short so the interrupt hint remains visible.
            self.bottom_pane.ensure_status_indicator();
            self.bottom_pane
                .set_interrupt_hint_visible(/*visible*/ true);
            self.status_state.terminal_title_status_kind =
                TerminalTitleStatusKind::WaitingForBackgroundTerminal;
            self.set_status(
                "Waiting for background terminal".to_string(),
                command_display.clone(),
                StatusDetailsCapitalization::Preserve,
                /*details_max_lines*/ 1,
            );
            match &mut self.unified_exec_wait_streak {
                Some(wait) if wait.process_id == process_id => {
                    wait.update_command_display(command_display);
                }
                Some(_) => {
                    self.flush_unified_exec_wait_streak();
                    self.unified_exec_wait_streak =
                        Some(UnifiedExecWaitStreak::new(process_id, command_display));
                }
                None => {
                    self.unified_exec_wait_streak =
                        Some(UnifiedExecWaitStreak::new(process_id, command_display));
                }
            }
            self.request_redraw();
        } else {
            if self
                .unified_exec_wait_streak
                .as_ref()
                .is_some_and(|wait| wait.process_id == process_id)
            {
                self.flush_unified_exec_wait_streak();
            }
            if !self.is_unified_exec_process_hidden(&process_id) {
                self.add_to_history(history_cell::new_unified_exec_interaction(
                    command_display,
                    stdin,
                ));
            }
        }
    }

    pub(super) fn on_command_execution_completed(&mut self, item: ThreadItem) {
        let ThreadItem::CommandExecution {
            id,
            process_id,
            source,
            ..
        } = &item
        else {
            return;
        };
        if is_unified_exec_source(*source) {
            if let Some(process_id) = process_id.as_deref()
                && self
                    .unified_exec_wait_streak
                    .as_ref()
                    .is_some_and(|wait| wait.process_id == process_id)
            {
                self.flush_unified_exec_wait_streak();
            }
            self.track_unified_exec_process_end(id, process_id.as_deref());
            if !self.bottom_pane.is_task_running() {
                return;
            }
        }
        self.defer_or_handle(item, InterruptManager::push_item_completed, |this, item| {
            this.handle_command_execution_completed_now(item, false)
        });
    }

    pub(super) fn track_unified_exec_process_begin(
        &mut self,
        call_id: &str,
        process_id: Option<&str>,
        command: &str,
    ) {
        let key = process_id.unwrap_or(call_id).to_string();
        let command = split_command_string(command);
        let command_display = strip_bash_lc_and_escape(&command);
        if let Some(existing) = self
            .unified_exec_processes
            .iter_mut()
            .find(|process| process.key == key)
        {
            existing.call_id = call_id.to_string();
            existing.command_display = command_display;
            existing.recent_chunks.clear();
        } else {
            self.unified_exec_processes.push(UnifiedExecProcessSummary {
                key,
                call_id: call_id.to_string(),
                command_display,
                recent_chunks: Vec::new(),
            });
        }
        self.sync_unified_exec_footer();
    }

    pub(super) fn track_unified_exec_process_end(
        &mut self,
        call_id: &str,
        process_id: Option<&str>,
    ) {
        let key = process_id.unwrap_or(call_id);
        let before = self.unified_exec_processes.len();
        self.unified_exec_processes
            .retain(|process| process.key != key);
        if self.unified_exec_processes.len() != before {
            self.sync_unified_exec_footer();
        }
    }

    pub(super) fn sync_unified_exec_footer(&mut self) {
        let processes = self
            .unified_exec_processes
            .iter()
            .map(|process| process.command_display.clone())
            .collect();
        self.bottom_pane.set_unified_exec_processes(processes);
    }

    /// Record recent stdout/stderr lines for the unified exec footer.
    pub(super) fn track_unified_exec_output_chunk(&mut self, call_id: &str, chunk: &[u8]) {
        let Some(process) = self
            .unified_exec_processes
            .iter_mut()
            .find(|process| process.call_id == call_id)
        else {
            return;
        };

        let text = String::from_utf8_lossy(chunk);
        for line in text
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty())
        {
            process.recent_chunks.push(line.to_string());
        }

        const MAX_RECENT_CHUNKS: usize = 3;
        if process.recent_chunks.len() > MAX_RECENT_CHUNKS {
            let drop_count = process.recent_chunks.len() - MAX_RECENT_CHUNKS;
            process.recent_chunks.drain(0..drop_count);
        }
    }

    pub(crate) fn handle_command_execution_started_now(&mut self, item: ThreadItem) {
        let ThreadItem::CommandExecution {
            id,
            command,
            source,
            command_actions,
            ..
        } = item
        else {
            return;
        };
        let (command, parsed_cmd) =
            command_execution_command_and_parsed(&command, &command_actions);
        // Ensure the status indicator is visible while the command runs.
        self.bottom_pane.ensure_status_indicator();
        let parsed_cmd = self.annotate_skill_reads_in_parsed_cmd(parsed_cmd);
        self.running_commands.insert(
            id.clone(),
            RunningCommand {
                command: command.clone(),
                parsed_cmd: parsed_cmd.clone(),
                source,
            },
        );
        if !self.should_render_exec_activity(source) {
            self.hidden_exec_calls.insert(id);
            return;
        }
        let is_wait_interaction = matches!(source, ExecCommandSource::UnifiedExecInteraction);
        let command_display = command.join(" ");
        let should_suppress_unified_wait = is_wait_interaction
            && self
                .last_unified_wait
                .as_ref()
                .is_some_and(|wait| wait.is_duplicate(&command_display));
        if is_wait_interaction {
            self.last_unified_wait = Some(UnifiedExecWaitState::new(command_display));
        } else {
            self.last_unified_wait = None;
        }
        if should_suppress_unified_wait {
            self.suppressed_exec_calls.insert(id);
            return;
        }
        if let Some(cell) = self
            .transcript
            .active_cell
            .as_mut()
            .and_then(|c| c.as_any_mut().downcast_mut::<ExecCell>())
            && cell.add_call(
                id.clone(),
                command.clone(),
                parsed_cmd.clone(),
                source,
                /*interaction_input*/ None,
            )
        {
            self.bump_active_cell_revision();
        } else {
            self.flush_active_cell();

            self.transcript.active_cell = Some(Box::new(new_active_exec_command(
                id,
                command,
                parsed_cmd,
                source,
                /*interaction_input*/ None,
                self.local_settings.tui.animations && self.local_settings.tui.effects.progress,
            )));
            self.bump_active_cell_revision();
        }

        self.request_redraw();
    }

    /// Finalizes an exec call while preserving the active exec cell grouping contract.
    ///
    /// Exec begin/end events usually pair through `running_commands`, but unified exec can emit an
    /// end event for a call that was never materialized as the current active `ExecCell` (for
    /// example, when another exploring group is still active). In that case we render the end as a
    /// standalone history entry instead of replacing or flushing the unrelated active exploring
    /// cell. If this method treated every unknown end as "complete the active cell", the UI could
    /// merge unrelated commands and hide still-running exploring work.
    pub(crate) fn handle_command_execution_completed_now(
        &mut self,
        item: ThreadItem,
        from_replay: bool,
    ) {
        enum ExecEndTarget {
            // Normal case: the active exec cell already tracks this call id.
            ActiveTracked,
            // We have an active exec group, but it does not contain this call id. Render the end
            // as a standalone finalized history cell so the active group remains intact.
            OrphanHistoryWhileActiveExec,
            // No active exec cell can safely own this end; build a new cell from the end payload.
            NewCell,
        }

        let ThreadItem::CommandExecution {
            id,
            command,
            process_id: _,
            source,
            status,
            command_actions,
            aggregated_output,
            exit_code,
            duration_ms,
            ..
        } = item
        else {
            return;
        };
        let event_command = split_command_string(&command);
        let event_parsed = command_actions
            .into_iter()
            .map(codex_app_server_protocol::CommandAction::into_core)
            .collect();
        let duration = Duration::from_millis(duration_ms.unwrap_or_default().max(0) as u64);
        let exit_code = if status == codex_app_server_protocol::CommandExecutionStatus::Completed {
            exit_code.unwrap_or_default()
        } else {
            exit_code.filter(|code| *code != 0).unwrap_or(1)
        };
        let aggregated_output = aggregated_output.unwrap_or_default();

        let running = self.running_commands.remove(&id);
        // Retain terminal hidden/suppressed markers so duplicate completion events cannot render or log again.
        let was_hidden = self.hidden_exec_calls.contains(&id);
        let already_suppressed = self.suppressed_exec_calls.contains(&id);
        let routine_success = status
            == codex_app_server_protocol::CommandExecutionStatus::Completed
            && exit_code == 0;
        // A5-HideWarn precedence fix (2026-09-29, operator-direct: "write a test proving A5's
        // hide_warnings works during replay"): a warning-marked result is ALSO `routine_success`
        // by the definition above (both require `status == Completed`), so it could otherwise take
        // the M26 `tool_activity` shortcuts immediately below - showing a concise summary from a
        // DIFFERENT, independent config surface - before ever reaching A5's own `hide_warnings`
        // check further down. Computed here, using the still-unshadowed incoming `source`/
        // `aggregated_output` (the `let (command, parsed, source) = match running {...}` rebinding
        // a few lines down would otherwise change which `source` is in scope), specifically to keep
        // A5's own configured intent from being silently overridden by an unrelated mechanism.
        // Confirmed as a real, live bug (not just a theoretical interaction) by
        // `hide_warnings_wins_over_concise_summary_during_replay` in tests/exec_flow.rs, which
        // failed before this fix and passes after it.
        let warning_marked_early = routine_success && contains_warning_marker(&aggregated_output);
        let warning_hidden_early =
            warning_marked_early && self.config.hide_tool_activity.hide_warnings;
        if from_replay
            && !self.should_render_exec_activity(source)
            && routine_success
            && !warning_hidden_early
        {
            if self.tool_activity == ToolActivity::Concise {
                self.add_concise_exec_summary(&event_command);
            }
            return;
        }
        // A call hidden at start needs completion classification when it becomes a failure or
        // warning. The explicit M135 flags below then decide whether it is muted/logged; never
        // let ToolActivity itself hide a safety-relevant completion.
        let hidden_requires_reclassification = was_hidden
            && ((status != codex_app_server_protocol::CommandExecutionStatus::Completed
                || exit_code != 0)
                || warning_marked_early);
        if was_hidden && !hidden_requires_reclassification {
            if routine_success && self.tool_activity == ToolActivity::Concise {
                self.add_concise_exec_summary(&event_command);
            }
            return;
        }
        let (command, parsed, source) = match running {
            Some(rc) => (rc.command, rc.parsed_cmd, rc.source),
            None => (event_command, event_parsed, source),
        };
        let parsed = self.annotate_skill_reads_in_parsed_cmd(parsed);
        let is_unified_exec_interaction =
            matches!(source, ExecCommandSource::UnifiedExecInteraction);
        let is_user_shell = source == ExecCommandSource::UserShell;
        let failed_ordinary_result = status
            != codex_app_server_protocol::CommandExecutionStatus::Declined
            && (status == codex_app_server_protocol::CommandExecutionStatus::Failed
                || (status == codex_app_server_protocol::CommandExecutionStatus::Completed
                    && exit_code != 0));
        // M135 A5/A8 warning marker (see `is_warning_output` in core's history.rs for the
        // full rationale): no dedicated warning classification exists elsewhere in this
        // fork, so a completed, non-failed exec result whose own output explicitly
        // contains a "warning" marker is treated as the warning class. Mutually
        // exclusive with `failed_ordinary_result` by construction.
        let warning_ordinary_result = !failed_ordinary_result
            && status == codex_app_server_protocol::CommandExecutionStatus::Completed
            && contains_warning_marker(&aggregated_output);

        if already_suppressed
            && !((failed_ordinary_result && self.config.hide_tool_activity.hide_failures)
                || (warning_ordinary_result && self.config.hide_tool_activity.hide_warnings))
        {
            return;
        }

        if failed_ordinary_result && self.config.hide_tool_activity.hide_failures {
            let first_suppression = self.suppressed_exec_calls.insert(id.clone());
            if first_suppression {
                if let Some(path) = self.config.hide_tool_activity.tool_error_log.as_deref() {
                    append_v3_tool_line(
                        path,
                        ToolActivitySeverity::Failure,
                        "exec_command",
                        &id,
                        exit_code,
                        &aggregated_output,
                    );
                }
            }
            let removed = self
                .transcript
                .active_cell
                .as_mut()
                .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
                .is_some_and(|cell| cell.remove_call(&id));
            if removed {
                let empty = self
                    .transcript
                    .active_cell
                    .as_ref()
                    .and_then(|cell| cell.as_any().downcast_ref::<ExecCell>())
                    .is_some_and(ExecCell::is_empty);
                if empty {
                    let _ = self.transcript.take_active_cell();
                } else {
                    self.bump_active_cell_revision();
                }
                self.request_redraw();
            }
            return;
        }
        // A5-HideWarn: same mute shape as A3 above, keyed to the warning marker instead
        // of the failure classification. Display suppressed here; model output (the
        // ResponseItem sent to the model) is unaffected, matching A3's contract.
        if warning_ordinary_result && self.config.hide_tool_activity.hide_warnings {
            let first_suppression = self.suppressed_exec_calls.insert(id.clone());
            if first_suppression {
                if let Some(path) = self.config.hide_tool_activity.tool_error_log.as_deref() {
                    append_v3_tool_line(
                        path,
                        ToolActivitySeverity::Warning,
                        "exec_command",
                        &id,
                        exit_code,
                        &aggregated_output,
                    );
                }
            }
            let removed = self
                .transcript
                .active_cell
                .as_mut()
                .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
                .is_some_and(|cell| cell.remove_call(&id));
            if removed {
                let empty = self
                    .transcript
                    .active_cell
                    .as_ref()
                    .and_then(|cell| cell.as_any().downcast_ref::<ExecCell>())
                    .is_some_and(ExecCell::is_empty);
                if empty {
                    let _ = self.transcript.take_active_cell();
                } else {
                    self.bump_active_cell_revision();
                }
                self.request_redraw();
            }
            return;
        }
        // Completion-only replay has no begin event to join adjacent exploration. Extend only
        // a finished, compatible group; an unrelated running group must retain orphan routing.
        if let Some(cell) = self
            .transcript
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
            && !cell.is_active()
            && !cell.should_flush()
            && !cell.iter_calls().any(|call| call.call_id == id)
        {
            cell.add_call(
                id.clone(),
                command.clone(),
                parsed.clone(),
                source,
                /*interaction_input*/ None,
            );
        }
        let end_target = match self.transcript.active_cell.as_ref() {
            Some(cell) => match cell.as_any().downcast_ref::<ExecCell>() {
                Some(exec_cell) if exec_cell.iter_calls().any(|call| call.call_id == id) => {
                    ExecEndTarget::ActiveTracked
                }
                Some(exec_cell) if exec_cell.is_active() => {
                    ExecEndTarget::OrphanHistoryWhileActiveExec
                }
                None if cell.as_any().is::<McpToolCallCell>()
                    || cell
                        .as_any()
                        .downcast_ref::<history_cell::ComputerActivityCell>()
                        .is_some_and(history_cell::ComputerActivityCell::is_active) =>
                {
                    ExecEndTarget::OrphanHistoryWhileActiveExec
                }
                Some(_) | None => ExecEndTarget::NewCell,
            },
            None => ExecEndTarget::NewCell,
        };

        let output = if is_unified_exec_interaction {
            CommandOutput::new(exit_code, String::new())
        } else {
            CommandOutput::new(exit_code, aggregated_output)
        };

        match end_target {
            ExecEndTarget::ActiveTracked => {
                if let Some(cell) = self
                    .transcript
                    .active_cell
                    .as_mut()
                    .and_then(|c| c.as_any_mut().downcast_mut::<ExecCell>())
                {
                    let completed = cell.complete_call(&id, output, duration);
                    debug_assert!(completed, "active exec cell should contain {id}");
                    if cell.should_flush() {
                        self.flush_active_cell();
                    } else {
                        self.bump_active_cell_revision();
                        self.request_redraw();
                    }
                }
            }
            ExecEndTarget::OrphanHistoryWhileActiveExec => {
                let mut orphan = new_active_exec_command(
                    id.clone(),
                    command,
                    parsed,
                    source,
                    /*interaction_input*/ None,
                    self.local_settings.tui.animations && self.local_settings.tui.effects.progress,
                );
                let completed = orphan.complete_call(&id, output, duration);
                debug_assert!(completed, "new orphan exec cell should contain {id}");
                self.app_event_tx
                    .send(AppEvent::InsertHistoryCell(Box::new(orphan)));
                self.request_redraw();
            }
            ExecEndTarget::NewCell => {
                let mut cell = new_active_exec_command(
                    id.clone(),
                    command,
                    parsed,
                    source,
                    /*interaction_input*/ None,
                    self.local_settings.tui.animations && self.local_settings.tui.effects.progress,
                );
                let completed = cell.complete_call(&id, output, duration);
                debug_assert!(completed, "new exec cell should contain {id}");
                if let Some(active) = self
                    .transcript
                    .active_cell
                    .as_mut()
                    .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
                    && !active.is_active()
                    && active.is_exploring_cell()
                    && cell.is_exploring_cell()
                {
                    // Replayed commands have completion events without matching starts.
                    active.group.calls.extend(cell.group.calls);
                    self.bump_active_cell_revision();
                    self.request_redraw();
                } else {
                    self.flush_active_cell();
                    if cell.should_flush() {
                        self.add_to_history(cell);
                    } else {
                        self.transcript.active_cell = Some(Box::new(cell));
                        self.bump_active_cell_revision();
                        self.request_redraw();
                    }
                }
            }
        }
        if is_user_shell {
            self.maybe_send_next_queued_input();
        }
    }
}
