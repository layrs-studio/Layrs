pub fn usage() -> &'static str {
    "Usage:
  layrs [--space PATH] [--json] [--no-color] COMMAND

Commands:
  layrs init \"Space Name\" [--path PATH]
  layrs step
  layrs worktree list
  layrs worktree create EMPTY_DIR --layer NAME_OR_ID --step EXACT_SELECTOR
    Share the Store at an exact Step; the Layer must already exist.
  layrs worktree open PATH
    Validate and describe a Worktree; use --space PATH for subsequent commands.
  layrs worktree resume
    Finish interrupted creation in the selected Worktree after recovery.
  layrs vfs create EMPTY_DIR --layer NAME_OR_ID --step EXACT_SELECTOR [--background]
  layrs vfs serve ROOT
    Foreground Windows VFS host; create/serve/resume prints one nonsecret ticket, then stays running.
    Use --json and redirect stdout to a ticket file for subsequent control commands.
  layrs [--space SOURCE] vfs resume WORKTREE_ID [--background]
    Recover an interrupted creation using its retained exact ID from the ordinary source workspace.
    Uses the recorded root and original Step, including failures before root registration/locator.
  layrs vfs start ROOT [--ready-timeout-seconds 1..300]
    Start an existing root in a hidden user process. Default readiness timeout: 30 seconds.
    create/resume --background also accept --ready-timeout-seconds. Failure retains recoverable work.
  layrs vfs status --ticket FILE
  layrs vfs capture --ticket FILE --writers-quiesced
  layrs vfs export PARENT --ticket FILE --writers-quiesced
    Stop ALL Worktree writers before capture/export and keep them stopped until completion.
    The flag is your acknowledgment; it does not suspend applications. Export creates a new tree.
  layrs vfs stop --ticket FILE
    Disconnect the host and retain the root/work. Restart with serve/start; retain the new ticket.
    Layer switching, rename and restore are unsupported in VFS roots; ordinary step/status refuse.
    Requires supported Windows/local NTFS. This does not install a service or enable OS features.
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
    `layrs login`, `layrs whoami` and `layrs logout` are aliases.
  layrs remote provision WORKSPACE_SLUG SPACE_SLUG --endpoint HTTPS_URL
  layrs remote publish [--integrate] | receive --endpoint HTTPS_URL
  layrs remote publish-candidates --endpoint HTTPS_URL
  layrs remote clone SPACE_ID EMPTY_DIRECTORY --endpoint HTTPS_URL
  layrs remote clone-partial SPACE_ID STEP_ID EMPTY_DIRECTORY [--include PATH ...] [--include-tree SUBTREE ...] --endpoint HTTPS_URL
    Select exact leaves/subtrees; omitting selection preserves the full authorized clone.
  layrs remote partial-select --expected-generation N --include PATH [--include-tree SUBTREE ...] --endpoint HTTPS_URL
    Add paths to the current selection and hydrate them; partial-hydrate is an alias.
    Repeat either selection flag. Existing selected paths and local edits are retained.
  layrs partial status | capture EXPECTED_GENERATION | resume-installation
  layrs partial fsck | history [BEFORE_SEQUENCE]
  layrs partial resume-namespace | resume-refresh | resume-restore | resume-rebase
  layrs partial rebase-draft
  layrs partial offline status
  layrs partial offline check OP PATH... [--version VERSION] [--other VERSION]
    Check local availability only. OP: read, edit, capture, restore, compare-metadata, compare-content.
    Comparison checks require --other; all other checks reject it. No download is implied.
  layrs partial offline read PATH --output NEW_FILE [--version VERSION] [--max-bytes N]
    Write exact binary bytes to a new file, never stdout or an existing file. Default limit: 65536;
    maximum: 1048576 bytes. Exceeding the bound fails before creating output; no truncated success.
  layrs partial offline compare BEFORE AFTER PATH... [--content]
    Report changed file identities/modes; --content also requires local content for both versions.
    VERSION defaults to current; historical selectors: generation:N, capture:N, version-file:FILE.
    Digests come from stored history. Version files accept an exact version or successful JSON receipt.
  layrs partial offline restore-preview PATH... [--version VERSION] --expected-generation N --output NEW_PREVIEW.json
    Review destination Store/Worktree, exact before/after file identities, sizes, modes and digest.
    Writes a new typed preview artifact (maximum 1 MiB); no files are restored or history changed.
    VERSION defaults to current.
    Save the artifact outside the checkout to avoid changing its reviewed namespace.
  layrs partial offline restore-apply PREVIEW.json --expected-digest blake3:HEX
    Apply only the reviewed artifact and printed digest. Stale generation/checkout, unavailable bytes,
    changed permissions and foreign collisions are refused. Overwritten preimages remain recoverable.
    These commands use retained local content only; no credentials, downloads or automatic apply.
    Capture/Restore preparation receipts remain unsupported; restore preview/apply are separate operations.
  layrs partial offline preparations | release PREPARATION_ID | quota [--bytes N] | evict
    Eviction reclaims only unretained shared content. Releasing a preparation releases its retention.
    release also cancels a preparation still acquiring content.
  layrs partial offline cancel-reservation
    Cancel an interrupted capture reservation; keep checkout edits and accepted history.
    Other pending filesystem operations require their own resume command.
  layrs partial offline release-generation N
    Release old base retention only when no active base, capture or preparation depends on it.
  layrs partial offline family create EMPTY_DIR --quota-bytes N
  layrs partial offline family convert --family FAMILY_DIR --destination EMPTY_DIR
  layrs partial offline family resume --destination RETAINED_DIR
    Use --space SOURCE for the source partial checkout. Explicit conversion copies into a shared
    family and preserves the source; resume verifies the same source and retained destination.
  layrs partial offline family abandon --family FAMILY_DIR --destination RETAINED_DIR
    Abandon only an unready conversion. Source, destination files and private metadata are retained.
    Uses only the explicit family/destination; the original source may be changed or missing.
    Recovery and retention release never run automatically.
  layrs remote partial-observe STEP_ID PATH... --expected-generation N --endpoint HTTPS_URL
    Observe an exact historical Step; save the --json response to use as version-file:FILE.
  layrs remote partial-prepare OP PATH... [--version VERSION] [--other VERSION] --expected-generation N --endpoint HTTPS_URL
  layrs remote partial-resume-preparation PREPARATION_ID --expected-generation N --endpoint HTTPS_URL
    Explicit online preparation uses current credentials and authority. Local offline commands never connect.
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
  layrs status
  layrs layers
  layrs layer use NAME_OR_ID
  layrs layer create NAME
  layrs weave SOURCE --target TARGET [--preview]
  layrs weave status
  layrs weave conflicts
  layrs resolve-term CONFLICT_ID TERM_ID --layer LAYER [--preview]
  layrs resolve-file CONFLICT_ID FILE --layer LAYER [--executable] [--preview]
  layrs resolve-block CONFLICT BLOCK --layer LAYER --term TERM [--preview]
  layrs resolve-block CONFLICT BLOCK --layer LAYER --file FILE --sources TERM,... [--preview]
    Apply a prior preview with --expected-operation OP --expected-step STEP.
  layrs weave continue
  layrs weave abort
  layrs conflict list
  layrs conflict status
  layrs conflict continue
  layrs conflict abort

Remote commands handle HTTPS collaboration.
Use resolve-term/resolve-file/resolve-block and undo with immutable conflict history.

Global flags:
  --space PATH     Use a local Layrs space at PATH instead of discovering from cwd.
  --json           Emit stable JSON: {\"ok\":true,\"data\":...} or {\"ok\":false,\"error\":...}.
  --no-color       Disable ANSI color in human output."
}
