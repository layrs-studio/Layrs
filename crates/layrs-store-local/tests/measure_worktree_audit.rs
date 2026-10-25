// Include the actual example so plain `cargo test -p layrs-store-local` discovers
// its table-drift, URI, row-bound, argument and physical-accounting tests and
// type-checks its audit path. Filesystem fixtures are fresh and retained.
// The example's main is never invoked by this integration-test harness.
#[allow(dead_code)]
#[path = "../examples/measure_worktree_audit.rs"]
mod audit_example;
