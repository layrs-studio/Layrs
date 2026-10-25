# Offline CLI qualification boundaries

`imported_history.rs` converts a legacy checkout after two captures have replaced
its original base bytes. Legacy installation did not retain that base in the
Store. Conversion preserves the available checkout and captured history; it
cannot reconstruct an overwritten base that was never retained.

The test runs actual CLI historical reads, availability checks and content
comparisons using exact imported capture selectors under the destination
Worktree identity. Before fixture preparation, the actual CLI current-versus-
capture content comparison must refuse with `offline_compare_unavailable`:
the current base has `missingContent`, no manifest and `retained: false`, while
the capture operand remains retained. Both checkout file sets and bytes,
projection states and captured histories must remain unchanged.

The later local preparation callback supplies bytes saved by the fixture before
the original overwrite. This is **fixture-only library setup**, used to qualify
the positive CLI comparison once exact content is retained. It is not a shipped
offline repair command or proof of a user-facing online preparation workflow.
An overwritten, never-retained legacy base requires explicit authorized online
preparation using current credentials and authority; local offline commands
cannot acquire it. Existing core TLS tests own the online preparation proof.
These CLI tests do not yet qualify end-to-end online hydration.

## Explicit recovery entrypoints

Database rollback recovery and interrupted initialization currently have
Desktop-only user entrypoints on the Health page. Its read-only `inspectRecovery`
action supplies the validated `binding` passed unchanged as `expected` to:

- `recoverDatabases`, for reported family or Worktree database rollback;
- `resumeInitialization`, once initialization is the only remaining recovery need.

These actions call `execute_projection_offline` through the Desktop adapter.
There is currently no corresponding CLI database/init recovery syntax. Existing
CLI filesystem resume and reservation cancellation commands are distinct and
must not be presented as substitutes. Adding CLI parity remains separate work;
this test correction adds no commands or automatic recovery.
