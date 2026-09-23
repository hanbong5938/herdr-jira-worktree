#!/usr/bin/env bash
# Launch-or-focus the Jira pane as a split beside the current pane.
# `plugin pane focus` succeeds only when a jira plugin pane is already open;
# otherwise we open a fresh one.
set -uo pipefail

herdr_bin="${HERDR_BIN_PATH:-herdr}"

if "$herdr_bin" plugin pane focus --plugin han.jira-worktree --entrypoint jira >/dev/null 2>&1; then
  exit 0
fi

exec "$herdr_bin" plugin pane open \
  --plugin han.jira-worktree \
  --entrypoint jira \
  --placement split \
  --direction right \
  --focus
