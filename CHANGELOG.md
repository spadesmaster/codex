## Upcoming

- TUI tool activity is quiet by default for new and upgrading users; use `/tool-activity concise` for compact routine summaries or `/tool-activity off` for full routine output

The changelog can be found on the [releases page](https://github.com/openai/codex/releases).

## Unreleased

- Fix missing command lifecycle events when unified exec cannot create a process, preserving approval rejection and sandbox retry behavior. Thanks @Marvinthebored ([#44557](https://github.com/openai/codex/issues/44557)).
