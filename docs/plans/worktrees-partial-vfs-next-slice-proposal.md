# Read-only proposal following writer R7

Exported from the completed design task `layrs-stage1b-after-writer-r7-qualified-next-slice-design-readonly`. This is the original proposal, not an approved source grant. No implementation has begun. Absolute links below identify the original checkout; use their repository-relative suffix on another machine.

Transfer clarification: references below to accepted owned-host assumptions concern
the original Windows test host. Execution on another computer requires a fresh
host disposition; the exported proposal does not transfer that acceptance.

I recommend **a bound, read-only bootstrap catalog/descriptor handoff** as the next slice. Keep local range serving, native population and `ProviderPrepared` transitions outside that implementation.

This extracts useful, already validated metadata from the accepted bootstrap endpoint without inventing readiness or changing R4 durable records. The subsequent population/preparation slice needs a separate registration-ticket and post-bootstrap state contract.

### 1. Dependency decision

The current implementation supplies:

- Static inspection and expected-binding restricted recovery.
- Begin through `control_published`, publication-only consuming resume, and strict no-claims abandon.
- A `ProviderControlPublished` holder retaining the actual control guard, family/artifact pins, exact binding and immutable issuance report.
- Full SQL validation that already reconstructs the selected catalog and descriptors, but discards that reconstructed source before returning.

The relevant boundaries are visible in [the Published holder](/C:/Users/ALEXA/.t3/worktrees/Layrs/feature-worktrees-partial-vfs/crates/layrs-store-local/src/projection/provider/bootstrap/outcome.rs:15), [its readonly revalidation](/C:/Users/ALEXA/.t3/worktrees/Layrs/feature-worktrees-partial-vfs/crates/layrs-store-local/src/projection/provider/bootstrap/outcome.rs:43), and [full source reconstruction](/C:/Users/ALEXA/.t3/worktrees/Layrs/feature-worktrees-partial-vfs/crates/layrs-store-local/src/projection/provider/validation.rs:293).

The next options therefore differ:

| Option | Disposition |
|---|---|
| Bound catalog/descriptors | **Implement next:** metadata exists, exact validation exists, retained control ownership exists. No durable extension is necessary. |
| Retained local range/source capability | Later: requires a provider-specific read lease, complete native root proof, eligible phase/binding and missing-content classification. `control_published` is insufficient. |
| Native preparation/population | Design next, alongside metadata implementation: requires authenticated durable registration intent, prepared-state evidence, interruption outcomes and restart rules. |
| Product-host startup integration | Separate activation dependency. It is not required to design or qualify this metadata slice under the accepted owned-host assumptions. |
| Another bootstrap rerun | No dependency justification. Accepted tests retain their measured scopes; binary uniformity is not a reason to replay them. |

The decisive durable gap is concrete: [MountRecord](/C:/Users/ALEXA/.t3/worktrees/Layrs/feature-worktrees-partial-vfs/crates/layrs-store-local/src/projection/provider/records/receipts.rs:298) supports only `Allocating`, `ControlPrepared` and `ControlPublished`, with A/P references. It has no authenticated registration ticket or `ProviderPrepared` evidence. SQL phase vocabulary alone cannot supply those contracts, and frozen R4 `mount.v1` must not be silently extended.

R7’s accepted 156 selections close the bounded writer qualification; they do not establish these later capabilities. The differing writer-cut and inherited-handoff termination evidence, source-only rename-error branch and historical R6 failure remain exactly as accepted.

### 2. Proposed metadata contract

Proposed public surface, using the existing `ProjectionStoreError` result:

```rust
impl ProviderControlPublished {
    pub fn bootstrap_catalog(&self) -> Result<ProviderBootstrapCatalog<'_>>;
}

pub struct ProviderBootstrapCatalog<'published> {
    // Private: borrowed Published holder and owned validated source DATA.
}

impl ProviderBootstrapCatalog<'_> {
    pub fn binding(&self) -> &ProviderBootstrapBinding;
    pub fn source_generation(&self) -> u64;
    pub fn source_digest(&self) -> &str;
    pub fn selection_digest(&self) -> &str;
    pub fn catalog_id(&self) -> &str;
    pub fn source_snapshot(&self) -> &ProjectionSnapshot;
    pub fn selection(&self) -> &ProjectionSelection;
    pub fn vfs_catalog(&self) -> &layrs_vfs::Catalog;
    pub fn descriptors(
        &self,
    ) -> impl ExactSizeIterator<Item = ProviderCatalogDescriptor<'_>> + '_;
    pub fn descriptor(&self, path: &RepoPath)
        -> Option<ProviderCatalogDescriptor<'_>>;
    pub fn revalidate(&self) -> Result<()>;
}

pub struct ProviderCatalogDescriptor<'catalog> {
    // Private borrowed representation.
}
```

`ProviderCatalogDescriptor` would expose borrowed `path`, `file_id`, `manifest_id` and `manifest`, plus scalar byte length and executable mode. It would expose existing public core types, **not** `DescriptorRecord`, `CatalogRecord` or `SourceCommitment`. All seven [existing internal source accessors](/C:/Users/ALEXA/.t3/worktrees/Layrs/feature-worktrees-partial-vfs/crates/layrs-store-local/src/projection/provider/records/source.rs:349) remain restricted.

The contract would be:

- **Binding:** exact retained family/member/native identities, transaction, A/P/T observations and published phase. Generation remains the validated bootstrap generation, presently zero.
- **Source identity:** return the validated snapshot and selection, including exact Step/root, policy set and observed authorization epoch. Observed permissions and descriptors are DATA; they grant neither renewed remote authorization nor local content availability.
- **Eligibility:** only an actual `ProviderControlPublished` receiver. Pending, unavailable, uninitialized, terminal, mismatched or otherwise ineligible inspection refuses. No equivalent method on Withheld/Unknown outcomes.
- **Lifetime:** the view borrows Published, keeping its actual guard and non-lock pins alive. It retains no connection, Store lock, raw file pointer or content lease. Cloned catalog/manifest DATA cannot authorize writes or serving.
- **Effects:** no persistent writable database open, receipt/FORMAT mutation, registration, shared-content access or network acquisition. Existing profile-gated in-memory reference-schema construction remains part of validation.
- **Errors:** preserve current operation, database scope, original typed causes and scoped rollback identities. Readonly failures carry an unchanged `RetainedIssuance` copy; append no write entry and reconstruct no historical effects.

**Lock and validation sequence**

Acquire the existing shared lifecycle → shared owner → shared catalog gates once. Under those gates:

1. Revalidate the actual bootstrap guard.
2. Use `writer_inspect_locked` and the existing published-eligibility predicate.
3. Perform one explicit additional readonly private validation that retains the reconstructed source.
4. Require successful observable close, unchanged binding/pins and final guard/artifact checks.
5. Release every connection and operation lock before returning the view.

This deliberately bounded second read avoids changing the recovery state machine. It must preserve typed hot/sidecar/profile/close refusals and stop after observable close failure. It must not call public `revalidate()` recursively while holding its locks.

The validator change would retain its complete exact predicates, query order and bounds, while returning the reconstructed source through an internal result. The legacy `private()` wrapper would keep its current result and behavior. The pure row helper remains insufficient authority.

Restart continues through **existing explicit bound inspection/recovery and consuming publication-only resume**, then this method. No ordinary family opener, receipt adoption, initialization replay or automatic recovery is introduced.

### 3. Exact proposed source ownership

Only these existing files would need implementation changes:

| Existing file | Narrow seam |
|---|---|
| `crates/layrs-store-local/src/projection.rs` | Explicit public exports of the two new view types. |
| `crates/layrs-store-local/src/projection/provider.rs` | Explicit forwarding exports. |
| `crates/layrs-store-local/src/projection/provider/bootstrap.rs` | Declare `catalog` and export its two public types. |
| `crates/layrs-store-local/src/projection/provider/validation.rs` | Preserve the full validator while retaining its reconstructed source in an internal result; keep the existing wrapper contract. |
| `crates/layrs-store-local/src/projection/provider/bootstrap/tests.rs` | Declare the focused test module only; preserve existing pure bodies. |

Exactly two new files:

- `crates/layrs-store-local/src/projection/provider/bootstrap/catalog.rs`
- `crates/layrs-store-local/src/projection/provider/bootstrap/tests/catalog.rs`

The new production module may contain narrowly scoped `cfg(test)` observation points for its own read/return boundaries. No unnamed child modules or existing hook edits are proposed.

No change is needed to `records.rs`, `records/source.rs`, `outcome.rs`, `recovery.rs`, report types, shared locks, ordinary database helpers, VFS or client adapters. Existing holder fields are already visible to a sibling bootstrap module; existing writer-test fixtures, wrappers and catalog competitor are reusable.

The three documentation files are a **separate future documentation scope**, not part of that source ownership.

### 4. Mandatory successor contracts

The metadata slice must not be presented as a partial range reader.

**Native preparation and population.** VFS already has a source-free [control-bound root factory](/C:/Users/ALEXA/.t3/worktrees/Layrs/feature-worktrees-partial-vfs/crates/layrs-vfs/src/windows/provider_root.rs:41): persist registration ticket before OS registration, populate, audit, then establish READY. It also has ticket/root revalidation without repopulation. The missing integration is Store’s authenticated durable record and ownership transition.

The next design should consume the Published holder and transfer its actual guard/pins into a preparation holder. It must define registration intent, known/uncertain outcomes, prepared evidence and restart refusal. No second sole owner or public raw guard is justified.

**Concrete decision requiring separate review:** storage/versioning of the registration ticket and post-bootstrap phase evidence. Recommend a separately reviewed versioned post-bootstrap record contract; do not add default fields or phases to immutable R4 v1. Whether existing schema-8 storage can hold that contract must be demonstrated before proposing any schema change.

**Local range capability.** Its eventual contract must require exact source/selection/transaction binding, authenticated complete root proof and eligible phase:

- `ProviderPrepared`: reads may be permitted with complete proof and exact source binding, without inventing an Active map.
- `Active`: additionally requires its exact valid activation mapping.
- Other, pending or contradictory states: no source capability.

A retained read lease must protect the specific local range dependencies through completion/cancellation. Missing unprepared content is `NeedsHydration`; missing content already protected by accepted claims is corruption, not an automatic fetch opportunity. No fallback to ordinary recovering accessors: the existing [shared-content path](/C:/Users/ALEXA/.t3/worktrees/Layrs/feature-worktrees-partial-vfs/crates/layrs-store-local/src/projection/shared/content.rs:263) uses ordinary lease/catalog machinery.

**Owner requests, not implementations:**

- **VFS owner:** confirm use of the existing source-free factory, durable-ticket callback and complete-proof reopen interfaces. No new primitive is currently demonstrated necessary.
- **L2 owner:** define explicit acquisition against the exact Step/root and current authority; distinguish descriptor acquisition from chunk preparation. Cache hits do not renew authority.
- **Partial adapter owner:** implement a separate closed source gate that opens only after eligible Store binding and actual root proof. Do not reuse the complete-store [VerifiedVfsSnapshot](/C:/Users/ALEXA/.t3/worktrees/Layrs/feature-worktrees-partial-vfs/crates/layrs-client-core/src/native_scm/vfs_source.rs:37) as a partial capability.

Native-I/O cancellation remains a genuine integrated acceptance obligation. It does not prevent this bounded metadata design, but owned-process deadlines must not be advertised as a production cancellation guarantee.

Managed members, companion preservation, expansion and L9 qualification remain mandatory later work.

### 5. Finite qualification proposal

These are proposed cases, not a claimed declaration inventory or released queue.

| Proposed case | Evidence and decisive assertions |
|---|---|
| Public nameability and DATA boundary | Pure, 30s: public imports/accessors; no private record exposure, source capability or guard access. |
| Exact reconstructed metadata | Pure, 30s: selected descriptors, ordering, manifest IDs, generation, selection and source commitments; existing limits remain enforced. |
| Healthy Begin and idempotent resume readouts | Owned, 180s each: real writer endpoints; exact returned metadata; repeated reads preserve receipts, databases and issuance report. |
| Cold metadata without resident chunks | Owned, 180s: real published bootstrap; catalog succeeds without content acquisition or claims. This establishes metadata independence, not range availability. |
| Descriptor/source contradiction | Owned, 180s per mutation: independent descriptor removal, extra descriptor, manifest mismatch and source/selection mismatch; exact typed refusal, no partial view. |
| Pending and ineligible state refusal | Owned, 180s per condition: private WAL/SHM, terminal appearance and changed binding; no view, retained issuance provenance and unchanged bytes. |
| Actual supported hot refusal | Owned, 240s: existing exact-owned hot helper; actual scoped `READONLY_ROLLBACK`, no persistent RW open/recovery and retained witnesses. |
| Observable readonly close failure | Owned, 180s: test-local retained statement on the actual connection; real BUSY close, original cause/close preservation, no later probe or view. Disclose test observation SQL. |
| Lock continuity and release | Owned, 240s: existing conforming competitor blocked during both read passes, succeeds after return; pin replacement refuses before delivery. |
| Two members and restart | Owned, 240s each: production-created members in one family, no descriptor/binding cross-use; fresh holder obtained through existing bound resume returns the same metadata without initialization replay. |

Order healthy controls before mutation/hot/close cases; acquisition/release control before continuity interference. Pure checks may be separately released after compilation/review.

Use existing exact-child ownership, fresh short absolute parents and named SQLite path budgets. Retain 180/240-second outer ceilings and existing 90-second inherited interruption limits where used. No new outer watchdog, retries, direct helper selection or success skips. No new write-interruption protocol is warranted for a readonly slice.

Missing-protected-content, native population/restart, activation maps, capture and cancellation belong to the successor qualification matrix; these metadata cases do not close them. Unchanged historical writer tests need no blanket rerun.

### 6. Bounded documentation patch proposal

**Main programme — [current lines 863 onward](/C:/Users/ALEXA/.t3/worktrees/Layrs/feature-worktrees-partial-vfs/docs/plans/worktrees-partial-vfs.md:863).**

The current paragraph still records the proposal/addendum stage and says no writer is granted. Preserve it as dated history, explicitly qualifying “à cette étape de conception”. Add a current-status paragraph:

> Le writer Stage1b est désormais implémenté et qualifié dans le périmètre Windows owned-test-host accepté pour R7 : création jusqu’à `control_published`, membre de famille toujours `initializing`, reprise consommatrice limitée à la publication et abandon strict sans claims. Les 156 sélections R7 sont acceptées pour leurs assertions bornées ; les neuf tests purs conservent leur provenance R5 et l’échec natif R6 reste un échec. Cette qualification n’accorde ni préparation/population provider, ni source de lecture, capture, conversion, activation CLI/Desktop ou acceptation L5 complète.

Link the accepted disposition. Preserve first-live historical evidence and race/performance HOLD text. This updates the actual current wording rather than repeating the earlier correction mechanically.

**Storage — [status around 207](/C:/Users/ALEXA/.t3/worktrees/Layrs/feature-worktrees-partial-vfs/docs/plans/worktrees-partial-vfs-storage.md:207), API/recovery discussion and [sequence at 594](/C:/Users/ALEXA/.t3/worktrees/Layrs/feature-worktrees-partial-vfs/docs/plans/worktrees-partial-vfs-storage.md:594).**

Propose four bounded corrections:

1. Replace the current blanket “design/no implementation grant” status with the implemented inspection/restricted-recovery/writer boundary. Mark `ProjectionProviderStore`, range APIs and later phase/map APIs explicitly **proposed**.
2. Correct the receipt list and “both receipts” wording around 394–412 to **A/P/T**, including terminal dominance and exact retained T binding. No restart adoption or silent rebinding.
3. Replace the proposed `inspection_catalog(false)`/`catalog_once(false)` discovery route around 434 with filesystem-bound `FamilyAccess`, existing lifecycle/owner/catalog gates and provider-specific readonly database I/O. Document strict recovery admission, typed original causes, profile gating and close-stop behavior; ordinary openers remain prohibited.
4. Split the bundled step 2 into:
   - bound catalog/descriptor DATA;
   - reviewed registration/preparation durable contract and source-free native population;
   - eligible local-only read capability and exact Active mapping;
   - capture;
   - mandatory managed/companion/expansion work and integrated qualification.

Remove schema/dependency/control-capability grants from the list of wholly missing foundations. Retain the genuinely missing integration, cancellation and later lifecycle obligations. Keep historical failure sections intact.

**Convergence — [sequence at 471–495](/C:/Users/ALEXA/.t3/worktrees/Layrs/feature-worktrees-partial-vfs/docs/plans/worktrees-partial-vfs-convergence.md:471).**

Replace the stale sequential table with the dependency order above. Distinguish implemented VFS control/root capabilities from their unimplemented partial-Store integration. Update schema-8 “candidate/pending foundation” wording while retaining capture and post-bootstrap contract work as proposed.

Keep the four owner-assigned corrections at 492–495: expansion authority, exact base-generation preparation, availability permissions and managed recovery. None is closed or reassigned by writer acceptance.

The acceptance matrix should distinguish accepted bootstrap evidence from still-open registration/population/activation and integrated source/capture cases.

### Limits

This is a read-only proposal. No source, documentation or evidence was changed; no workload, fixture/process inspection, network access or child agent was used.

The proposed metadata API is a new public API decision requiring review. The later registration/prepared record is a separate durable-contract decision. Neither follows automatically from writer acceptance.

No implementation/runtime grant, product activation, cleanup/adoption/retry, recovery widening, rights/share/privilege change, native/VFS primitive change, schema/dependency change or source-serving authority is inferred. ROOT compilation, focused qualification, independent review and any later host/runtime release remain separate decisions. No owned process was started.
