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
- Bundle paths are canonical `/`-separated relative paths: empty, repeated,
  trailing, `.` and `..` segments are forbidden. Loaders must also reject a
  canonicalized path that escapes the Lens directory through a symlink.
- Resource quotas are positive integers no greater than
  `9007199254740991`, the portable exact-integer maximum shared with JavaScript.
- Execution proof is minted only from loader-owned implementation bytes after
  SHA-256 verification; a manifest digest supplied by a caller is not proof.

The shared golden fixture lives at
`tests/fixtures/lenses/v1/contract.golden.json`. Rust and TypeScript package
tests serialize the same values and compare against that file.
