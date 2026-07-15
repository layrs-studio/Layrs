# Lens Contract V1

`lens-contract.schema.json` is the canonical wire boundary for Lens manifests,
preview and diff windows, and reconcile requests and responses.

- Every operation is pinned by `lensId`, semantic `version`, and SHA-256
  `digest` under `contractVersion: "layrs.lens.v1"`.
- Manifest `analyzer.supportedMediaTypes`, `fileExtensions`, and `capabilities`
  are authoritative. Hosts must not infer additional matching rules.
- Window ranges are zero-based, use an exclusive end, and carry the total in
  the same unit.
- Reconcile results use only `auto_resolved`, `conflicted`, or `unsupported`.
  Every advertised method includes an explicit `file` or `block` scope.
- `.layrslens` metadata reserves WASI analyzer/reconcile modules and an ESM
  viewer module, each with its own digest and resource quota. Bundle execution
  is intentionally disabled until a host verifies and sandboxes these fields.

The shared golden fixture lives at
`tests/fixtures/lenses/v1/contract.golden.json`. Rust and TypeScript package
tests serialize the same values and compare against that file.
