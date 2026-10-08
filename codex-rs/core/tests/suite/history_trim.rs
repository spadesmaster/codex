#![allow(clippy::unwrap_used)]

// M135-CdxNoBlk / A9-TrimN caller-level coverage.
//
// `trim_success_pairs`/`trim_failure_pairs`/`trim_warning_pairs`
// (`codex-rs/core/src/context_manager/history.rs`) were previously proven only
// by direct unit tests against a hand-built `History`/`HistoryTrimConfig` — no
// test exercised them through the two real call sites in
// `codex-rs/core/src/session/turn.rs` that thread the live `Config`'s
// `hide_tool_activity.trim_*`/`trim_after` fields into `HistoryTrimConfig` and
// call `for_prompt_with_trim`:
//   - the top of `run_turn`, building the initial sampling request for a turn
//     (around `turn.rs:517`);
//   - inside `run_sampling_request`'s own loop, for every round after the
//     first within a turn (around `turn.rs:1626`).
//
// This test drives a real `Session`/turn through both call sites with
// `trim_success = true`, `trim_after = 1` and confirms the model-facing
// request actually loses the oldest successful exec_command pair once a
// second one exists — not just that the isolated function can do it.

use anyhow::Result;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_exec_command_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trim_success_removes_oldest_pair_at_both_real_call_sites() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let call_a = "history-trim-call-a";
    let call_b = "history-trim-call-b";

    // Turn 1, round 1 (site 1: `run_turn`'s initial `for_prompt_with_trim`,
    // history empty so nothing is eligible to remove yet). Model calls tool A.
    let round1 = sse(vec![
        ev_response_created("resp-1"),
        ev_exec_command_call(call_a, "echo a"),
        ev_completed("resp-1"),
    ]);
    // Turn 1, round 2 (site 2: `run_sampling_request`'s loop). Only call A's
    // pair exists (1 eligible, keep_after = 1) so it must still be present.
    // Model calls tool B.
    let round2 = sse(vec![
        ev_response_created("resp-2"),
        ev_exec_command_call(call_b, "echo b"),
        ev_completed("resp-2"),
    ]);
    // Turn 1, round 3 (site 2 again). Both A and B pairs now exist (2
    // eligible, keep_after = 1), so the oldest (A) must be removed from the
    // prompt this round actually receives. Model finishes the turn.
    let round3 = sse(vec![
        ev_response_created("resp-3"),
        ev_assistant_message("msg-1", "ran a and b"),
        ev_completed("resp-3"),
    ]);
    // Turn 2, round 1 (site 1 again, a fresh `run_turn` call). History still
    // holds both A and B pairs across the turn boundary (trimming only ever
    // affects what is sent to the model, never the persisted History), so the
    // same removal must reappear here from a cold call to `for_prompt_with_trim`.
    // Model finishes immediately with no further tool call.
    let round4 = sse(vec![
        ev_response_created("resp-4"),
        ev_assistant_message("msg-2", "second turn done"),
        ev_completed("resp-4"),
    ]);

    let request_log = mount_sse_sequence(&server, vec![round1, round2, round3, round4]).await;

    let mut builder = test_codex().with_config(|config| {
        config.hide_tool_activity.trim_success = true;
        config.hide_tool_activity.trim_after = Some(1);
    });
    let fixture = builder.build(&server).await?;

    fixture.submit_turn("run tool a then tool b").await?;
    fixture.submit_turn("wrap up").await?;

    let requests = request_log.requests();
    assert_eq!(
        requests.len(),
        4,
        "expected exactly 4 model requests across the two turns"
    );

    // Round 1: history is empty, neither call has happened yet.
    assert!(!requests[0].has_function_call(call_a));
    assert!(!requests[0].has_function_call(call_b));

    // Round 2 (site 2, first invocation): only A's pair exists; trim_after=1
    // must NOT remove it yet — proves the test's own eligibility accounting
    // before asserting real removal below.
    assert!(
        requests[1].has_function_call(call_a),
        "call A must still be present with only 1 eligible pair and trim_after=1"
    );

    // Round 3 (site 2, real removal): A and B pairs both exist now; the
    // oldest (A) must be gone from what is actually sent to the model, B
    // must remain. This is call site 2 (`turn.rs` ~1626) doing real trimming.
    assert!(
        !requests[2].has_function_call(call_a),
        "call site 2 (run_sampling_request loop) should have trimmed the oldest pair (A)"
    );
    assert!(
        requests[2].has_function_call(call_b),
        "the newest eligible pair (B) must be kept"
    );

    // Round 4 (site 1, fresh turn): the persisted History still has both
    // pairs; a cold call to `for_prompt_with_trim` at the top of `run_turn`
    // must independently re-derive the same removal. This is call site 1
    // (`turn.rs` ~517) doing real trimming.
    assert!(
        !requests[3].has_function_call(call_a),
        "call site 1 (top of run_turn) should have trimmed the oldest pair (A) again"
    );
    assert!(
        requests[3].has_function_call(call_b),
        "the newest eligible pair (B) must still be kept in the fresh turn's prompt"
    );

    Ok(())
}

// M135-CdxNoBlk / A7-TrimFail caller-level coverage.
//
// `trim_failure_pairs` (`codex-rs/core/src/context_manager/history.rs`) was
// previously proven only by direct unit tests against a hand-built
// `History`/`HistoryTrimConfig`
// (`trim_failure_pairs_keeps_newest_requested_count`,
// `trim_failure_pairs_excludes_succeeded_running_goal_and_unpaired_calls`,
// `trim_failure_pairs_independent_of_trim_success`, all in
// `history_tests.rs`) — no test exercised it through the two real call sites
// in `codex-rs/core/src/session/turn.rs` that thread the live `Config`'s
// `hide_tool_activity.trim_failures`/`trim_after` fields into
// `HistoryTrimConfig` and call `for_prompt_with_trim`:
//   - the top of `run_turn`, building the initial sampling request for a turn
//     (around `turn.rs:517`);
//   - inside `run_sampling_request`'s own loop, for every round after the
//     first within a turn (around `turn.rs:1626`).
//
// This test mirrors `trim_success_removes_oldest_pair_at_both_real_call_sites`
// above exactly, but with both exec_command calls failing (`exit 1`) and
// `trim_failures = true` instead of `trim_success`, confirming the model-facing
// request actually loses the oldest *failed* exec_command pair once a second
// one exists — not just that the isolated function can do it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trim_failure_removes_oldest_pair_at_both_real_call_sites() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let call_a = "history-trim-fail-call-a";
    let call_b = "history-trim-fail-call-b";

    // Turn 1, round 1 (site 1: `run_turn`'s initial `for_prompt_with_trim`,
    // history empty so nothing is eligible to remove yet). Model calls
    // failing tool A.
    let round1 = sse(vec![
        ev_response_created("resp-1"),
        ev_exec_command_call(call_a, "exit 1"),
        ev_completed("resp-1"),
    ]);
    // Turn 1, round 2 (site 2: `run_sampling_request`'s loop). Only call A's
    // pair exists (1 eligible, keep_after = 1) so it must still be present.
    // Model calls failing tool B.
    let round2 = sse(vec![
        ev_response_created("resp-2"),
        ev_exec_command_call(call_b, "exit 1"),
        ev_completed("resp-2"),
    ]);
    // Turn 1, round 3 (site 2 again). Both A and B failed pairs now exist (2
    // eligible, keep_after = 1), so the oldest (A) must be removed from the
    // prompt this round actually receives. Model finishes the turn.
    let round3 = sse(vec![
        ev_response_created("resp-3"),
        ev_assistant_message("msg-1", "ran a and b"),
        ev_completed("resp-3"),
    ]);
    // Turn 2, round 1 (site 1 again, a fresh `run_turn` call). History still
    // holds both A and B pairs across the turn boundary (trimming only ever
    // affects what is sent to the model, never the persisted History), so the
    // same removal must reappear here from a cold call to `for_prompt_with_trim`.
    // Model finishes immediately with no further tool call.
    let round4 = sse(vec![
        ev_response_created("resp-4"),
        ev_assistant_message("msg-2", "second turn done"),
        ev_completed("resp-4"),
    ]);

    let request_log = mount_sse_sequence(&server, vec![round1, round2, round3, round4]).await;

    let mut builder = test_codex().with_config(|config| {
        config.hide_tool_activity.trim_failures = true;
        config.hide_tool_activity.trim_after = Some(1);
    });
    let fixture = builder.build(&server).await?;

    fixture
        .submit_turn("run failing tool a then failing tool b")
        .await?;
    fixture.submit_turn("wrap up").await?;

    let requests = request_log.requests();
    assert_eq!(
        requests.len(),
        4,
        "expected exactly 4 model requests across the two turns"
    );

    // Round 1: history is empty, neither call has happened yet.
    assert!(!requests[0].has_function_call(call_a));
    assert!(!requests[0].has_function_call(call_b));

    // Round 2 (site 2, first invocation): only A's failed pair exists;
    // trim_after=1 must NOT remove it yet — proves the test's own
    // eligibility accounting before asserting real removal below.
    assert!(
        requests[1].has_function_call(call_a),
        "call A must still be present with only 1 eligible pair and trim_after=1"
    );

    // Round 3 (site 2, real removal): A and B failed pairs both exist now;
    // the oldest (A) must be gone from what is actually sent to the model, B
    // must remain. This is call site 2 (`turn.rs` ~1626) doing real trimming
    // of a *failed* pair.
    assert!(
        !requests[2].has_function_call(call_a),
        "call site 2 (run_sampling_request loop) should have trimmed the oldest failed pair (A)"
    );
    assert!(
        requests[2].has_function_call(call_b),
        "the newest eligible failed pair (B) must be kept"
    );

    // Round 4 (site 1, fresh turn): the persisted History still has both
    // pairs; a cold call to `for_prompt_with_trim` at the top of `run_turn`
    // must independently re-derive the same removal. This is call site 1
    // (`turn.rs` ~517) doing real trimming of a *failed* pair.
    assert!(
        !requests[3].has_function_call(call_a),
        "call site 1 (top of run_turn) should have trimmed the oldest failed pair (A) again"
    );
    assert!(
        requests[3].has_function_call(call_b),
        "the newest eligible failed pair (B) must still be kept in the fresh turn's prompt"
    );

    Ok(())
}

// M135-CdxNoBlk / A8-TrimWarn caller-level coverage.
//
// `trim_warning_pairs` (`codex-rs/core/src/context_manager/history.rs`) was
// previously proven only by four direct unit tests against a hand-built
// `History`/`HistoryTrimConfig` (`trim_warning_pairs_keeps_newest_requested_count`,
// `trim_warning_pairs_excludes_plain_success_and_failure`,
// `trim_warning_pairs_ignores_zero_warnings_summary_line`,
// `trim_warning_pairs_still_matches_real_warning_next_to_zero_warnings_summary`,
// all in `history_tests.rs`) — unlike its A6/A9 and A7 siblings above, no test
// exercised it through the two real call sites in
// `codex-rs/core/src/session/turn.rs` that thread the live `Config`'s
// `hide_tool_activity.trim_warnings`/`trim_after` fields into
// `HistoryTrimConfig` and call `for_prompt_with_trim`:
//   - the top of `run_turn`, building the initial sampling request for a turn
//     (around `turn.rs:517`);
//   - inside `run_sampling_request`'s own loop, for every round after the
//     first within a turn (around `turn.rs:1626`).
//
// This test mirrors `trim_success_removes_oldest_pair_at_both_real_call_sites`
// and `trim_failure_removes_oldest_pair_at_both_real_call_sites` above exactly,
// but with both exec_command calls succeeding while printing a real
// `"warning"` marker in their own stdout (`is_warning_output` classifies on
// output text, not exit code) and `trim_warnings = true` instead of
// `trim_success`/`trim_failures`, confirming the model-facing request
// actually loses the oldest *warning-marked* exec_command pair once a second
// one exists — not just that the isolated function can do it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trim_warning_removes_oldest_pair_at_both_real_call_sites() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let call_a = "history-trim-warn-call-a";
    let call_b = "history-trim-warn-call-b";

    // Turn 1, round 1 (site 1: `run_turn`'s initial `for_prompt_with_trim`,
    // history empty so nothing is eligible to remove yet). Model calls
    // warning-marked tool A.
    let round1 = sse(vec![
        ev_response_created("resp-1"),
        ev_exec_command_call(call_a, "echo this run has a warning in it"),
        ev_completed("resp-1"),
    ]);
    // Turn 1, round 2 (site 2: `run_sampling_request`'s loop). Only call A's
    // pair exists (1 eligible, keep_after = 1) so it must still be present.
    // Model calls warning-marked tool B.
    let round2 = sse(vec![
        ev_response_created("resp-2"),
        ev_exec_command_call(call_b, "echo another warning shows up here"),
        ev_completed("resp-2"),
    ]);
    // Turn 1, round 3 (site 2 again). Both A and B warning-marked pairs now
    // exist (2 eligible, keep_after = 1), so the oldest (A) must be removed
    // from the prompt this round actually receives. Model finishes the turn.
    let round3 = sse(vec![
        ev_response_created("resp-3"),
        ev_assistant_message("msg-1", "ran a and b"),
        ev_completed("resp-3"),
    ]);
    // Turn 2, round 1 (site 1 again, a fresh `run_turn` call). History still
    // holds both A and B pairs across the turn boundary (trimming only ever
    // affects what is sent to the model, never the persisted History), so the
    // same removal must reappear here from a cold call to `for_prompt_with_trim`.
    // Model finishes immediately with no further tool call.
    let round4 = sse(vec![
        ev_response_created("resp-4"),
        ev_assistant_message("msg-2", "second turn done"),
        ev_completed("resp-4"),
    ]);

    let request_log = mount_sse_sequence(&server, vec![round1, round2, round3, round4]).await;

    let mut builder = test_codex().with_config(|config| {
        config.hide_tool_activity.trim_warnings = true;
        config.hide_tool_activity.trim_after = Some(1);
    });
    let fixture = builder.build(&server).await?;

    fixture
        .submit_turn("run warning tool a then warning tool b")
        .await?;
    fixture.submit_turn("wrap up").await?;

    let requests = request_log.requests();
    assert_eq!(
        requests.len(),
        4,
        "expected exactly 4 model requests across the two turns"
    );

    // Round 1: history is empty, neither call has happened yet.
    assert!(!requests[0].has_function_call(call_a));
    assert!(!requests[0].has_function_call(call_b));

    // Round 2 (site 2, first invocation): only A's warning-marked pair
    // exists; trim_after=1 must NOT remove it yet — proves the test's own
    // eligibility accounting before asserting real removal below.
    assert!(
        requests[1].has_function_call(call_a),
        "call A must still be present with only 1 eligible pair and trim_after=1"
    );

    // Round 3 (site 2, real removal): A and B warning-marked pairs both exist
    // now; the oldest (A) must be gone from what is actually sent to the
    // model, B must remain. This is call site 2 (`turn.rs` ~1626) doing real
    // trimming of a *warning-marked* pair.
    assert!(
        !requests[2].has_function_call(call_a),
        "call site 2 (run_sampling_request loop) should have trimmed the oldest warning-marked pair (A)"
    );
    assert!(
        requests[2].has_function_call(call_b),
        "the newest eligible warning-marked pair (B) must be kept"
    );

    // Round 4 (site 1, fresh turn): the persisted History still has both
    // pairs; a cold call to `for_prompt_with_trim` at the top of `run_turn`
    // must independently re-derive the same removal. This is call site 1
    // (`turn.rs` ~517) doing real trimming of a *warning-marked* pair.
    assert!(
        !requests[3].has_function_call(call_a),
        "call site 1 (top of run_turn) should have trimmed the oldest warning-marked pair (A) again"
    );
    assert!(
        requests[3].has_function_call(call_b),
        "the newest eligible warning-marked pair (B) must still be kept in the fresh turn's prompt"
    );

    Ok(())
}
