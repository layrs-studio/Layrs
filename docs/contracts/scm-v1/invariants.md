# Layrs SCM V1 Normative Invariants

These invariants are release gates. “Must” and “must not” are normative.

## Identifiers and canonical encoding

1. Every content id is `blake3:` followed by exactly 64 lowercase hexadecimal
   characters.
2. Every canonical object id equals
   `BLAKE3(domain ASCII || NUL || canonical payload without the claimed id)`.
   The separator is exactly one zero byte and the versioned domain is unique
   per object type.
   V1 domains are `layrs-step-v1`, `layrs-tree-v1`, `layrs-conflict-v1`,
   `layrs-symlink-v1`, `layrs-tombstone-v1`,
   `layrs-repository-view-v1`, `layrs-chunk-v1`, `layrs-file-v1` and
   `layrs-file-manifest-v1`.
3. Canonical strings are UTF-8; record separators are LF; numeric values are
   base-10 without padding.
4. Canonical sets are sorted by ASCII/UTF-8 bytes and contain no duplicates.
   Metadata entries are sorted by key and metadata keys are unique.
5. Opaque ids are type-prefixed, validated, never empty and never inferred from
   display names or filesystem paths.
6. Unknown schema or canonical encoding versions are rejected, not guessed.
7. Non-identity observations must never change a Step id.
8. Step uses its specialized payload encoding. Tree, ConflictObject, Symlink,
   Tombstone, RepositoryView and FileManifest use RFC 8785 canonical JSON with
   only the claimed id property omitted and `schemaVersion` retained. Chunk and
   logical File payloads are raw bytes. Set-like arrays are sorted before
   encoding.
9. `common.schema.json#/$defs/ObjectId` and
   `common.schema.json#/$defs/RepoPath` are the single core scalar authorities;
   other schemas reference them and do not redefine their syntax.

## Repository paths

1. RepoPath is non-empty, relative and uses `/` as its only separator.
2. RepoPath contains no empty component, `.`, `..`, backslash, NUL, drive
   prefix or absolute/root prefix.
3. Every RepoPath component contains 1 through 255 Unicode scalar values.
4. RepoPath and RepoPathComponent input is accepted only if already normalized
   to Unicode NFC. Layrs rejects non-NFC input; it does not silently normalize,
   because RFC 8785 canonical JSON preserves string code points.
5. TreeEntry names use the exact RepoPathComponent contract.
6. Path containment is validated again against platform filesystem identity at
   every IO trust boundary; syntax validation alone is not containment proof.

## Chunks and files

1. ChunkId is BLAKE3 over `layrs-chunk-v1 || NUL || decoded raw chunk bytes`.
   JSON/base64, compression, encryption and transport framing are not identity
   material.
2. A Chunk envelope decodes to exactly `byteLength` bytes and re-encoding uses
   canonical padded base64 without whitespace.
3. Empty files contain zero chunks. Non-empty files contain at least one
   positive-length chunk.
4. FileManifest chunks start at offset zero, are ordered, contiguous and
   non-overlapping; their lengths sum exactly to its `byteLength`.
5. FileId is BLAKE3 over `layrs-file-v1 || NUL || reconstructed raw file
   bytes`. It is independent of chunk boundaries and chunking strategy.
6. Every referenced Chunk exists, verifies against its ChunkId and supplies the
   exact slice declared by the FileManifest.
7. FastCDC parameters satisfy `minBytes <= targetBytes <= maxBytes`.
8. FileManifestId is BLAKE3 over `layrs-file-manifest-v1 || NUL ||
   RFC8785(manifest without manifestId)`. Its identity includes `fileId`,
   `byteLength`, chunking descriptor and ordered chunk slices.
9. Multiple verified manifests may represent one FileId. Local and sync
   indexes maintain `FileId -> {FileManifestId}`; resolving a FileId must select
   a completely installed manifest and verify reconstructed bytes.
10. Rechunking or compaction creates a different FileManifestId but not the
    FileId, Tree id or Step id when raw file bytes are unchanged.
11. A CAS id maps to exactly one canonical byte sequence. FileId and
    FileManifestId are never reused as aliases for different envelope bytes.

## Steps and graph

1. A Step is immutable and contains only its schema version, parent Step ids
   and root Tree id as identity material.
2. A Step has zero or more parents; duplicate parents are invalid.
   Duplicate validation happens before canonical sorting; implementations must
   not silently deduplicate parent input.
3. The Step graph is acyclic. Admission of an edge that creates a cycle fails
   atomically.
4. Every parent Step and root Tree of an accepted Step exists locally and is
   verified.
5. A Step belongs to no Layer. Space authorization is determined by reachable
   Space references.
6. A Step with ConflictObject entries is valid, persistable, synchronizable and
   usable as a parent.
7. Rewrite and supersession relations are stored on Operations, never by
   mutating Steps.
8. A rewrite map is explicit, many-to-many and references existing old Steps
   and verified replacement Steps.

## Merkle objects

1. A Tree is hierarchical. Every entry name is one immediate normalized path
   component.
2. Tree entry names are non-empty, unique and canonically sorted.
3. Entry names must not be `.`, `..`, absolute, contain `/`, `\`, NUL or escape
   the checkout root after platform normalization.
4. Each TreeEntry has exactly one target whose id type matches its target kind.
5. Every accepted Tree has a complete verified closure of Trees, logical Files,
   at least one installed FileManifest per FileId, Chunks, Conflicts, Symlinks
   and Tombstones.
6. Object publication is immutable. Existing bytes at an id are accepted only
   after exact digest and length verification.
7. An accepted metadata record never references a temporary or unverified
   object.
8. A crash may leave an unreferenced CAS object; it must not leave an accepted
   Step with a missing closure.

## Layers and Repository Views

1. A Layer contains no head field and owns no Step.
2. All authoritative Layer heads exist only in immutable RepositoryViews.
3. A Layer head value is a duplicate-free set with zero or more Step ids.
4. Multiple heads are valid divergence. No implicit operation may discard one.
5. A Layer head set is an antichain: no head is an ancestor of another head in
   the same set.
6. Every head Step referenced by a View exists and is verified.
7. RepositoryView collections are canonical, sorted and duplicate-free.
8. A RepositoryView contains Worktree state but never checkout paths,
   credentials, network cursors or caches.
9. Mutable current-head tables, if present, are disposable projections and
   must be fsck-equivalent to the selected current View.

## Operations

1. Repository initialization creates the first View, first Operation and at
   least one current Operation head atomically.
2. Every later Operation has at least one parent and exactly one resulting
   RepositoryView.
3. Operation parent edges are acyclic and duplicate-free.
4. Every local state change that affects undo creates a new Operation; prior
   Operations and Views remain immutable.
5. Undo, redo, restore and revert create Operations. They never update history
   in place.
6. An acceptance transaction compares the complete expected Operation head
   set. A mismatch cannot fall back to last-write-wins.
7. Concurrent states are either recomputed or integrated while preserving all
   prior heads.
8. The local Operation DAG, technical recovery journal and server event log are
   separate schemas and sources of truth.
9. Slow filesystem, Lens and network work occurs outside the exclusive SQLite
   transaction.
10. The repository may have several global Operation heads, but each initialized
    Worktree has exactly one `activeOperationId` and therefore one current View.
11. Integration creates a new multi-parent Operation and atomically updates the
    integrated global heads and all affected Worktree active pointers.

## Worktrees and capture

1. A Worktree has its own identity and explicitly references one Store and one
   Space.
2. After initialization a Worktree has exactly one `activeOperationId` that
   resolves to an Operation of the same Space.
3. A Worktree path is local metadata, validated by filesystem identity and not
   content-addressed or synchronized as repository state.
4. Active Layer, base Step and materialized Tree are Worktree state in a
   RepositoryView, not Layer fields.
5. Before changing materialized content, Layrs captures outstanding stable
   changes or refuses the operation.
6. Acknowledged capture means CAS objects are durable and Step/View/Operation
   metadata is committed.
7. A file changed while being read is retried or omitted with an explicit
   error; mixed bytes are never accepted as a stable capture.
8. A capture producing no repository state change creates no empty Operation.
9. Concurrent Worktree advancement of one Layer preserves multiple heads.

## Materialization and recovery

1. Every modified path is validated as contained and free of symlink/reparse
   escapes before IO.
2. Original bytes and nonexistence states are durably journaled before the
   first user-visible replacement.
3. Replacement files are written to new temporary files, flushed, verified and
   atomically installed where supported by the host filesystem.
4. Success is acknowledged only after the resulting Worktree View is committed
   and recovery state is safely finalized.
5. Recovery decides from durable evidence and never assumes an external side
   effect occurred.
6. Failure restores byte-exact originals or produces an explicit
   `needs_repair` state without discarding backups.

## Markers, Releases, Proofs and Gates

1. Every Marker and Release targets exactly one existing Step.
2. Marker/Release metadata does not alter the target Step.
3. A Release is an immutable manifest with no publication state or publication
   timestamp. Correction creates a new Release.
4. Publication creates an immutable ReleasePublication; it never mutates the
   Release. Revocation is a separate event.
5. Removing visibility from a View does not delete the Marker, Release or Step.
6. Marker, Release, ReleasePublication, Proof and Gate references participate in reachability and
   authorization checks.
7. A Marker or Release never follows a rewrite automatically.

## Cross-Space isolation

1. Physical CAS deduplication does not grant discovery, read or reference
   authority in another Space.
2. Every Layer, RepositoryView, Operation, Worktree, Marker, Release and
   ReleasePublication relation has matching Space ids.
3. A Step referenced by a Space has its complete Step-parent and Merkle closure
   admitted and reachable in that Space.
4. Cross-Space reuse requires an explicit authorized admission/import Operation
   and an auditable server event.
5. A Marker, Release or Layer in one Space cannot directly reference an object
   that is reachable only in another Space.
6. StepObservation without `spaceId` grants no Space reachability. When
   `spaceId` is present, the observed Step is admitted in that Space.
7. StepObservation without `operationId` claims no historical origin. When it
   is present, the Operation exists and introduces or reaches the Step; if
   `spaceId` is also present their Space ids match.

## Sync and server

1. Native objects are transferred in dependency/topological order or accepted
   through an atomic prepared batch with equivalent closure validation.
2. The server independently verifies ids, closure, authorization and expected
   reference generation.
3. Uploaded immutable objects remain valid when a later reference update
   conflicts.
4. A reference generation mismatch returns divergence; it never overwrites the
   accepted state.
5. Pending work remains pending until explicit durable acknowledgement.
6. Recovery and retry are idempotent by stable operation/idempotency keys.
7. No Git object, ref or service is required by the core or server.

## Store baseline and crate ownership

1. The new store initializes only its new format baseline.
2. Previous development formats are detected and refused without mutation.
3. `client_documents` and JSON-document projections are not an authority in the
   new baseline.
4. `layrs-store-local` is the only local CAS writer.
5. `layrs-client-core` performs no direct SQL and implements no second object
   store.
6. `layrs-sync` transports core objects but does not define their identity.
7. All critical metadata mutations use foreign-key-checked short transactions.
8. Schema definitions and migrations are ordered and checksummed.

## Fsck, repair, retention and GC

1. Fsck validates SQLite integrity, foreign keys, canonical ids, DAG acyclicity,
   View closure, current projections, CAS bytes and all root closures.
2. Repair creates and verifies a durable metadata backup before mutation.
3. Repair quarantines evidence; it does not invent missing object bytes.
4. GC starts only from an explicit root inventory.
5. Roots include current and retained Operations/Views, Layer heads, visible
   Markers/Releases, Worktrees, pending sync, Proofs/Gates, journals, leases and
   manual pins.
6. Marking follows every Step parent and full Merkle closure.
7. Newly installed orphans are protected by a grace period.
8. GC never races active readers, acceptance transactions, fsck, uploads or
   materialization leases.
9. Operation retention is initially unbounded and may be reduced only by an
   explicit auditable prune operation.
10. Pruning roots and sweeping bytes are separate operations.

## Minimum acceptance scenarios

1. `edit -> status` automatically captures a Step without a commit command.
2. A crash after capture acknowledgement preserves the Step and its closure.
3. Layer switching captures before materialization and is undoable.
4. A Marker names a Step without changing its id.
5. A Release remains on its original Step while a Layer advances or rewrites.
6. A conflicted Weave produces a durable conflict Step and completes.
7. Restart preserves every conflict side and partial resolution.
8. Undo and redo create inspectable Operations.
9. Two Worktrees and two offline devices can produce preserved multi-head
   divergence.
10. A fresh self-hosted server accepts a Space without Git or proprietary
    infrastructure.
11. Fsck detects a missing object, invalid DAG edge and View/projection mismatch.
12. GC cannot remove anything reachable through any declared root.
