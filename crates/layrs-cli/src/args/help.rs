pub fn usage() -> &'static str {
    "Usage:
  layrs [--space PATH] [--json] [--no-color] COMMAND

Commands:
  layrs init \"Space Name\" [--path PATH]
  layrs step
  layrs doctor
  layrs compact [--resume]
    Compact native metadata, retaining all history and objects plus a backup.
    Resume an interrupted compaction with --resume; restart history pagination.
  layrs identity [--actor ACTOR_ID [--device DEVICE_ID] [--name NAME]]
  layrs history STEP_OR_REFERENCE [--path PATH] [--actor ACTOR_ID] [--since RFC3339] [--until RFC3339] [--limit N] [--cursor JSON]
  layrs replay STEP_OR_REFERENCE [--mainline PARENT] [--preview]
  layrs revert STEP_OR_REFERENCE [--mainline PARENT] [--preview]
  layrs squash FIRST_STEP [--preview]
  layrs remote login | whoami | logout --endpoint HTTPS_URL [--ca FILE]
  layrs remote provision WORKSPACE_SLUG SPACE_SLUG --endpoint HTTPS_URL
  layrs remote publish [--integrate] | receive --endpoint HTTPS_URL
  layrs remote publish-candidates --endpoint HTTPS_URL
  layrs remote clone SPACE_ID EMPTY_DIRECTORY --endpoint HTTPS_URL
  layrs remote clone-partial SPACE_ID STEP_ID EMPTY_DIRECTORY --endpoint HTTPS_URL
  layrs partial status | capture EXPECTED_GENERATION | resume-installation
  layrs partial fsck | history [BEFORE_SEQUENCE]
  layrs partial resume-namespace | resume-refresh | resume-restore | resume-rebase
  layrs partial rebase-draft
  layrs remote partial-rebase-preview CAPTURE LAYER --output PREVIEW.json
  layrs remote partial-rebase-apply PREVIEW.json CHOICES.json
  layrs remote partial-historical-preview CAPTURE LAYER STEP PATHS.json --output PREVIEW.json
  layrs remote publish-partial CAPTURE_SEQUENCE LAYER_ID --endpoint HTTPS_URL
  layrs remote candidate-partial CAPTURE_SEQUENCE LAYER_ID --endpoint HTTPS_URL
  layrs remote scoped-review COMMAND_JSON_FILE --endpoint HTTPS_URL
  layrs remote partial-directory-preview GENERATION SOURCE DESTINATION --output FILE --endpoint HTTPS_URL
  layrs remote partial-directory-apply FILE --endpoint HTTPS_URL
  layrs remote receive-partial LAYER_ID --endpoint HTTPS_URL
  layrs remote migrate-partial LAYER_ID EMPTY_DIRECTORY --endpoint HTTPS_URL
  layrs remote partial-restore-preview REQUEST_JSON [--output NEW_PREVIEW_FILE] --endpoint HTTPS_URL
  layrs remote partial-restore-apply PREVIEW_FILE --endpoint HTTPS_URL
  layrs remote history STEP_ID [CURSOR] [--path PATH] [--actor ACTOR_ID] [--since RFC3339] [--until RFC3339] --endpoint HTTPS_URL
  layrs remote teams | team-members --endpoint HTTPS_URL
  layrs remote policy-management --endpoint HTTPS_URL
  layrs remote policy-preview POLICY_SET_ID [--output NEW_PREVIEW_FILE] --endpoint HTTPS_URL
  layrs remote policy-preview-rules RULES_JSON [--output NEW_PREVIEW_FILE] --endpoint HTTPS_URL
  layrs remote policy-read --endpoint HTTPS_URL
  layrs remote policy-replace POLICY_ID RULES_JSON [--output NEW_PREVIEW_FILE] --endpoint HTTPS_URL
  layrs remote policy-apply PREVIEW_FILE --endpoint HTTPS_URL
  layrs remote observe JSON_FILE --endpoint HTTPS_URL
    Declare step, observedAt (RFC3339), reason, idempotencyKey; identity comes from the authenticated account.
  layrs remote file STEP_ID REPO_PATH --output NEW_FILE --endpoint HTTPS_URL
  layrs remote team-create JSON_FILE | team-update TEAM_ID JSON_FILE --endpoint HTTPS_URL
  layrs remote team-archive TEAM_ID JSON_FILE | team-history TEAM_ID [AFTER ANCHOR] --endpoint HTTPS_URL
  layrs remote choose-preview JSON_FILE [--output NEW_PREVIEW_FILE] --endpoint HTTPS_URL
  layrs remote choose-apply PREVIEW_FILE --endpoint HTTPS_URL
  layrs remote releases | release-events RELEASE --endpoint HTTPS_URL
  layrs remote reviews [CURSOR] | review REVIEW_ID --endpoint HTTPS_URL
  layrs remote review-propose JSON_FILE | review-update REVIEW_ID JSON_FILE --endpoint HTTPS_URL
  layrs remote review-action REVIEW_ID JSON_FILE --endpoint HTTPS_URL
  layrs remote review-correct REVIEW_ID COMMENT_ID JSON_FILE --endpoint HTTPS_URL
  layrs remote runs [CURSOR] | run RUN_ID | compare BEFORE_STEP AFTER_STEP --endpoint HTTPS_URL
  layrs remote run-cancel RUN_ID JSON_FILE --endpoint HTTPS_URL
  layrs remote invitations [AFTER] | invitations-inbox [AFTER] --endpoint HTTPS_URL
  layrs remote invite-member JSON_FILE | invitation-decision INVITATION_ID accept|reject|revoke --endpoint HTTPS_URL
  layrs remote storage-budget [SPACE_ID] --endpoint HTTPS_URL
  layrs remote release-publish RELEASE --request-id UNIQUE_ID --expected-last EVENT_ID|none --endpoint HTTPS_URL
  layrs remote release-revoke RELEASE PUBLICATION --request-id UNIQUE_ID --expected-last EVENT_ID --reason TEXT --endpoint HTTPS_URL
  layrs operation-history [--search TEXT] [--actor ACTOR_ID] [--since RFC3339] [--until RFC3339] [--limit N] [--cursor JSON]
  layrs git import SOURCE [--ref FULL_REF ...] [--dry-run] [--timeout-seconds N]
  layrs git export DESTINATION [--ref FULL_REF ... | --snapshot STEP_ID] [--resume] [--timeout-seconds N]
  layrs git scan-cache SOURCE --output NEW_FILE [--ref FULL_REF ...] [--timeout-seconds N]
  layrs git import-cache FILE --digest sha256:HEX [--dry-run]
  layrs restore STEP_ID [--path PATH] [--preview | --expected JSON]
  layrs restore-view OPERATION_ID [--preview | --expected JSON]
  layrs undo
  layrs redo [OPERATION_ID]
  layrs recover
  layrs conflict-variant CONFLICT_ID TERM_ID --layer LAYER [--against TERM_ID]
  layrs markers
  layrs marker create NAME
  layrs marker hide MARKER_ID
  layrs marker show MARKER_ID
  layrs releases
  layrs release create VERSION
  layrs diff [STEP_ID] [--stat] [--name-only] [--window START:LIMIT] [--wrap|--no-wrap]
  layrs timeline [--limit N]
  layrs publish [--workspace WORKSPACE_ID]
  layrs sync [--workspace WORKSPACE_ID]
  layrs receive
  layrs status
  layrs login [--endpoint URL]
  layrs whoami
  layrs logout
  layrs spaces
  layrs layers
  layrs layer use NAME_OR_ID
  layrs layer create NAME
  layrs layer delete NAME_OR_ID [--yes]
  layrs layer disconnect NAME_OR_ID [--yes]
  layrs layer clear-steps NAME_OR_ID [--yes]
  layrs weave parent [--preview]
  layrs weave SOURCE --target TARGET [--preview]
  layrs weave status
  layrs weave conflicts
  layrs resolve-term CONFLICT_ID TERM_ID --layer LAYER [--preview]
  layrs resolve-file CONFLICT_ID FILE --layer LAYER [--executable] [--preview]
  layrs resolve-block CONFLICT BLOCK --layer LAYER --term TERM [--preview]
  layrs resolve-block CONFLICT BLOCK --layer LAYER --file FILE --sources TERM,... [--preview]
    Apply a prior preview with --expected-operation OP --expected-step STEP.
  layrs weave resolve PATH [--block N] --ours|--theirs|--base|--both-ours-first|--both-theirs-first|--manual-text FILE_OR_STDIN|--file FILE
  layrs weave continue
  layrs weave abort
  layrs conflict list
  layrs conflict status
  layrs conflict resolve
  layrs conflict resolve METHOD -f PATH --block BLOCK_ID
  layrs conflict continue
  layrs conflict abort

Native checkouts use remote commands for HTTPS collaboration.
Legacy-only commands (publish/sync/login/spaces/layer delete/disconnect/clear-steps,
weave resolve/continue/abort and conflict aliases) refuse unsupported native actions.
Use resolve-term/resolve-file and undo with immutable native conflict history.

Global flags:
  --space PATH     Use a local Layrs space at PATH instead of discovering from cwd.
  --json           Emit stable JSON: {\"ok\":true,\"data\":...} or {\"ok\":false,\"error\":...}.
  --no-color       Disable ANSI color in human output."
}
