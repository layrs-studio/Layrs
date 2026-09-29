pub mod graph_merge;
/// Pure file reconciliation. Graph planning and atomic acceptance are separate.
pub mod reconciliation;
#[cfg(not(target_arch = "wasm32"))]
pub mod wasm_host;
/// Process resource supervision; does not yet attest ambient-capability isolation.
pub mod worker;
