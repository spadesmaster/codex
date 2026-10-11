//! Retain dynamic tool state in either renderer; the app emits only finished rows to terminal history.

use super::command_lifecycle::ToolActivitySeverity;
use super::command_lifecycle::append_v3_tool_line;
use super::command_lifecycle::contains_warning_marker;
use super::*;

fn dynamic_tool_name(namespace: Option<&str>, tool: &str) -> String {
    namespace.map_or_else(
        || tool.to_string(),
        |namespace| format!("{namespace}.{tool}"),
    )
}

fn dynamic_tool_output(
    content_items: Option<&[codex_app_server_protocol::DynamicToolCallOutputContentItem]>,
) -> String {
    content_items
        .unwrap_or_default()
        .iter()
        .map(|item| match item {
            codex_app_server_protocol::DynamicToolCallOutputContentItem::InputText { text } => {
                text.as_str()
            }
            codex_app_server_protocol::DynamicToolCallOutputContentItem::InputImage { .. } => {
                "<image content>"
            }
            codex_app_server_protocol::DynamicToolCallOutputContentItem::InputAudio { .. } => {
                "<audio content>"
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl ChatWidget {
    pub(super) fn on_dynamic_tool_item(&mut self, item: ThreadItem) {
        let started = matches!(
            &item,
            ThreadItem::DynamicToolCall {
                status: codex_app_server_protocol::DynamicToolCallStatus::InProgress,
                ..
            }
        );
        self.defer_or_handle(
            item,
            |interrupts, item| {
                if started {
                    interrupts.push_item_started(item);
                } else {
                    interrupts.push_item_completed(item);
                }
            },
            Self::handle_dynamic_tool_item_now,
        );
    }

    pub(crate) fn handle_dynamic_tool_item_now(&mut self, item: ThreadItem) {
        let ThreadItem::DynamicToolCall {
            id,
            namespace,
            tool,
            status,
            content_items,
            success,
            ..
        } = &item
        else {
            return;
        };
        let id = id.clone();
        let in_progress = matches!(
            status,
            codex_app_server_protocol::DynamicToolCallStatus::InProgress
        );
        if in_progress && self.hides_routine() {
            self.hidden_dynamic_calls.insert(id);
            self.request_redraw();
            return;
        }
        if in_progress
            && (self.config.hide_tool_activity.hide_failures
                || self.config.hide_tool_activity.hide_warnings)
        {
            self.deferred_dynamic_calls.insert(id);
            self.request_redraw();
            return;
        }

        let failed = matches!(
            status,
            codex_app_server_protocol::DynamicToolCallStatus::Failed
        ) || *success == Some(false);
        let output = dynamic_tool_output(content_items.as_deref());
        let warning = !failed && contains_warning_marker(&output);
        let name = dynamic_tool_name(namespace.as_deref(), tool);
        let was_hidden = self.hidden_dynamic_calls.remove(&id);
        self.deferred_dynamic_calls.remove(&id);

        if failed && self.config.hide_tool_activity.hide_failures {
            if let Some(path) = self.config.hide_tool_activity.tool_error_log.as_deref() {
                append_v3_tool_line(path, ToolActivitySeverity::Failure, &name, &id, 1, &output);
            }
            self.request_redraw();
            return;
        }
        if warning && self.config.hide_tool_activity.hide_warnings {
            if let Some(path) = self.config.hide_tool_activity.tool_error_log.as_deref() {
                append_v3_tool_line(path, ToolActivitySeverity::Warning, &name, &id, 0, &output);
            }
            self.request_redraw();
            return;
        }
        if !failed && !warning && (was_hidden || self.hides_routine()) {
            self.add_concise_tool_activity_summary(format!("Completed: {name}"));
            self.request_redraw();
            return;
        }

        if let Some(cell) = self.transcript.dynamic_calls.get(&id) {
            cell.update_from_item(item);
            if !cell.is_active() {
                self.transcript.dynamic_calls.remove(&id);
            }
        } else if let Some(cell) = history_cell::DynamicToolCallCell::from_item(item) {
            self.flush_answer_stream_with_separator();
            if cell.is_active() {
                self.transcript
                    .dynamic_calls
                    .insert(cell.call_id().to_owned(), cell.clone());
            }
            self.add_to_history(cell);
        }
        self.request_redraw();
    }

    pub(super) fn finish_dynamic_activity(&mut self) {
        for (_, cell) in self.transcript.dynamic_calls.drain() {
            cell.mark_interrupted();
        }
    }
}
