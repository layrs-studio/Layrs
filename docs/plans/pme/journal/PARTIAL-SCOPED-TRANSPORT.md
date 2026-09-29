# Scoped patch transport and candidate admission

The v1 path patch wire and digest stay unchanged. V2 is a metadata-only request
whose digest covers intent, capture digest, exact base Step/root, View generation,
policy epoch, sorted changes, and typed fixed-1-MiB FileManifests. A session ID
is derived from that digest; PostgreSQL binds it to the authenticated principal,
Space, client/idempotency key and canonical request. Chunk possession is recorded
per session/path/ordinal only after the complete supplied body hashes to the
manifest ChunkId, even if the CAS object already exists. Scoped reservations enter
the same quota calculation as object plans before CAS writes and remain charged
until verified garbage collection can account for possible orphans.

Progress returns only chunks actually supplied in that session and requires
current read scope. A stale View or policy epoch returns no missing chunks so
the client can finalize a terminal rejection, even if upload stopped before all
bodies arrived. Finalize verifies each complete FileId with the bounded
`FileManifestVerifier`, installs manifests and reconstructed Trees, then uses the
normal scoped transaction CAS and gates. Exact terminal replay reads the durable
session and transaction receipt, so it does not need prior CAS content or the old
View. Replay still checks the current principal's projection read scope.
Final transaction admission also compares the candidate's captured View,
generation, policy and epoch before closure verification. This prevents a
candidate from being admitted after a concurrent View advance and persists a
replayable rejection without relying on missing CAS bodies.

Candidate intent admits the reconstructed Step with no View mutation. An applied
transaction atomically inserts immutable provenance keyed by candidate ID, which
is derived from the v2 request digest. Multiple source captures may share one
Step without conflating authors. Review integration accepts only a review ID and
idempotency key. The server loads its candidate, builds the exact View mutation,
checks current scoped approvals and Proof under the Space lock, then records the
candidate/review/transaction binding with the receipt in that transaction.
The protected Layer's whole-Step conflict rule is checked under that same lock;
the candidate may inherit a conflict outside its edited paths.

The transport requires a migrated server (`0017` through `0019`; `0019` moves
the scoped catalog into the extension schema without changing the baseline
fingerprint). The existing v1
base64 submission endpoint and generic restricted upload guard remain unchanged.

## Validation du 23 septembre

Recette HTTPS/PostgreSQL complète hors bac à sable : **1 PASS, 0 échec,
0 ignoré, 74,60s** (`D:/Layrs/tmp/scoped-full-https-20260923.log`). Elle couvre
100Mio envoyés par le client natif, perte de réponse/reprise/rejeu, modifications
locales ultérieures conservées, candidat sans avance, déplacement de dossier,
révocation/réautorisation, commentaires privilégiés masqués, conflit hérité
refusé, intégration après approbation et sessions périmées rejetées durablement.
Vérifications Rust/Clippy et TypeScript réussies sur le raccord initial.

La revue critique indépendante initiale a trouvé trois défauts (Gate conflits,
reçu terminal périmé, précondition View du candidat). Tous corrigés et leurs
scénarios PostgreSQL passent. La relecture indépendante finale des corrections
n'a pas pu être lancée : le moteur de collaboration a refusé les deux essais
avec `agent thread limit reached`. La racine a relu les corrections et exécuté
la recette ci-dessus ; ne pas présenter cela comme une seconde revue indépendante.

Restent : recette visuelle des nouveaux contrôles Desktop, charge1Gio/4Gio,
fusion/restauration historique et qualification des nouveaux paquets.
