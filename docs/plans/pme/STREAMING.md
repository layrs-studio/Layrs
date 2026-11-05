# Gros fichiers : flux bornés de bout en bout

Lot ouvert le20septembre2026, désormais qualifié sur les parcours bornés :
capture, publication, clone, réception incrémentale et restauration100MiB/1GiB
sur HTTPS réel, avec octets et FileIds exacts. Recette1GiB1092,828s ; pic mémoire
OS du processus clients+serveur62513152octets, PostgreSQL/cacheOS exclus.
[Mesure et limites](measurements/T4-streaming-1gib-20260920.md).
Limites explicites :1GiB/fichier,4GiB de vérification et256MiB de métadonnées ;
anciennes API Vec et fusion sémantique conservent leurs plafonds. Cette preuve
ne qualifie pas une volumétrie illimitée ni toute la matrice99k fichiers.
Les archives qualified2 précèdent ce lot. Les checkpoints ci-dessous sont
historiques ou qualifient un parcours particulier ; ce résumé fait autorité
sur l'état livré et n'est pas une promesse de débit en production.

## Reprise — qualification Linux et spool privé, 20 septembre2026

Après le commit de référence3cf23b3, qualification Linux en conteneur sans réseau,
sources en lecture seule, Rust1.90 : recette1GiB **PASS113,45s**, compilation32,75s,
heap Rust supplémentaire **9 870 248octets**, mesuré avant le fsck final. Même capture/interruption/recovery/
retry/replay/réouverture/Marker historique/octets/fsck complet propre que Windows.
Image locale `sha256:e8b8d5308857e25e2660628ddb839f138bdf944d30853f49937225e637508e02` ;
conteneur supprimé automatiquement, volume de compilation rendu à fusion_native.
Cette preuve concerne le parcours local ; elle ne qualifie pas HTTPS1GiB ni RSS.

Nouveau helper commun `layrs_store_local::private_spool_file` raccordé capture,
téléchargement temporaire et spool de graphe via son wrapper : Windows DACL
protégée owner-only fournie à CreateFileW dès création, CREATE_NEW, partage nul,
delete-on-close et vérification propriétaire/ACE sur le handle ; Unix0600 puis
unlink immédiat. Aucun chemin de spool exposé à l'appelant. Tests Windows3
PASS0,01s (DACL réelle, refus de seconde ouverture, suppression à fermeture,
collision sans écrasement, lecture/seek et indépendance). Trois tests clients
spool PASS0,07s (capture/graphe/download, corruption et troncation) ; Clippy strict
Store/client libs PASS13,24s et Store tous targets PASS5,42s. Qualification Linux
transmise par fusion_native : privacy2 PASS0,00s et clients spool3 PASS0,03s
(compilation15,19s), permissions0600, propriétaire, unlink et collisions/symlink
sans écrasement vérifiés. Sources du helper gelées et prêtes pour commit root.

## Ancien checkpoint de pause — conservé pour traçabilité

Travail arrêté à la demande explicite. Aucun processus de test operations actif.
Session65768 s'est terminée normalement pendant l'arrêt contrôlé : recette locale
Windows1GiB **PASS124,95s**, pic heap Rust supplémentaire **9 791 294octets**.
Capture streamée, interruption AfterPath, recovery des octets originaux, nouvel
apply, replay, réouverture, ancienne cible Marker, lecture de tous les chunks et
fsck complet propre. Fixtures locales du test nettoyées par son Drop normal.

Première tentative1GiB (session83506) avait échoué uniquement au fsck après107,17s :
budget256MiB partagé entre chunks streamés et metadata. Correctif : budget chunks
4GiB distinct des metadata256MiB, budget reconstruction logique4GiB, cap1GiB/File,
cap64MiB/Chunk. Les petits budgets liés à max_findings restent proportionnels :
26 tests fsck PASS2,96s après correction ; aucun contrôle supprimé.

Fait : A, B capture/admission, C materializer et compatibilité anciens journaux.
Fichiers principaux : core/scm/content/stream.rs ; store-local repository/streaming,
admission, checkout/{projection,streaming}, materialize/sources, read/availability,
CAS read borné, fsck budgets ; client snapshot/spool, prepare, filesystem, workspace.
Tests : core/streaming_content, store checkout_streaming et scm_write_batch/streaming,
client native_streaming ; régressions capture/lease/types/workspace déjà indiquées.
Scans2 PASS0,05s ; batch21 PASS3,37s ; Clippy Store tous tests PASS4,54s.

Non fait : qualification Linux A/B/C et recette réseau1GiB finale ; mesures RSS
(heap Rust seule mesurée), nouveau package et upgrade. Root/selectors/fusion ont
leurs checkpoints D/restore/HTTPS séparés. Helper commun de spool Windows avec
DACL explicite encore à faire : spool capture utilise actuellement handle
share_mode0/delete-on-close, Unix0600 puis unlink, comme temporary_file download ;
ne pas affirmer que la nouvelle DACL owner-only est implémentée. Revue finale des
bornes, formatage/Clippy workspace et recette intégrée restent requis avant gel.

Reprise après autorisation de l'utilisateur :

```powershell
cargo test --offline -p layrs-core --test streaming_content -- --nocapture --test-threads=1
cargo test --offline -p layrs-store-local --test checkout_streaming --test scm_write_batch --test scm_fsck
cargo test --offline -p layrs-client-core --test native_streaming -- --nocapture --test-threads=1
cargo test --offline -p layrs-client-core --test native_streaming capture_materialize_recover_one_gib_with_bounded_heap -- --ignored --nocapture --test-threads=1
```

La dernière commande écrit plusieursGiB dans une fixture temporaire uniquement.
Coordonner les binaires Cargo et le volume Linux partagé avant toute relance.

## A — construction et vérification incrémentales (Windows qualifié)

`FileManifestBuilder::new(chunking,max_bytes,max_chunks)`, `push_chunk(&Chunk)`,
`finish()->FileManifest` gardent seulement métadonnées bornées et hasher FileId.
`FileManifest::from_chunks` reprend ce chemin sans concaténer tout le fichier.
`FileManifest::verifier()`, `push(&[u8])`, `finish()` vérifient séquence, identités
Chunk, longueurs, couverture et FileId final sans reconstruire un Vec global.
Une erreur empoisonne le vérificateur. L'admission utilise `verify_chunks` ;
readeravailability conserve ses préflights et budgets actuels puis lit/vérifie/
relâche chaque Chunk au lieu de conserver un map complet et reconstruire.

Fichiers : core/scm/content et admission, store-local/scm_v1/read/availability,
tests dédiés. Aucun changement wire, format Store, droits, profil de chunking ou
plafond de production dans A. Vérifier vecteurs canoniques, vide/binaire/chunks
répétés, manque/corruption/trop-long/troncation/erreur tardive, empreinte finale ;
mesurer grande entrée générée par chunks sans buffer complet. Le passage de A
n'est pas une preuve de capture/synchronisation1GiB.

Preuves Windows : 17 vecteurs natifs PASS ; 2 tests streaming PASS1,95s avec
100MiB puis1GiB de chunks distincts1MiB et pic supplémentaire d'allocations Rust
de3 259 336octets (ce n'est pas une mesure RSS). Admission23 PASS36,80s,
readertransactionnel2 PASS0,14s ; Clippy strict core/store-local PASS45,22s.

## B — capture locale et admission préparée (après A)

CheckoutReader conserve actuellement deux snapshots de bytes ; SnapshotBuilder
garde tous les chunks, et capture_projection lit le checkout de base uniquement
pour retrouver les chemins suivis. Introduire une capture privée sur spool avec
identité de fichier épinglée, deux scans concordants de métadonnées+digests,
limites de disque, fichiers/chunks et suppression sûre des préparations abandonnées.
PreparedSnapshot devra référencer des sources immuables plutôt que tous les bytes.

Le batch final ne doit publier aucune Operation/View avant validation de l'état
initial et fermeture complète. Les chunks devront être préparés/admis par petits
batches bornés sans changer les têtes, avec provenance/reprise explicites ; ne pas
augmenter le cap16384objets pour faire entrer16384chunks+manifest/Tree d'un1GiB
à64KiB. Examiner stockage préparé et admissions déjà durables pour préserver
idempotence/stale CAS. Les échecs peuvent laisser des objets préparés bornés, jamais
une capture partiellement visible. APIs additives avant raccord des commandes.

Contrat B retenu : nouveau profil explicite fixed1MiB pour la capture streamée,
FileId invariant ;1GiB représente1024chunks. Un seul verrou exclusif Store couvre
le staging CAS local et la transaction finale, sans réseau. Les descripteurs
externes sont bornés, uniques et liés aux admissions du digest du batch ; leurs
octets sont vérifiés un à un. Les rows Chunk sont insérées dans la transaction
avant validation et annulées avec elle si échec. Aucun intervalle entre petits
batches ne laisse les préparations exposées à un GC concurrent. Les préparations
CAS sans commit après crash restent des objets non référencés, jamais une tête.

B/C checkpoint : API `NativeScmRepository::apply_batch_with_chunks` compilée,
2 tests atomique/replay/rollback/descripteurs/refuscorruption PASS0,45s.
Capture via deux scans épinglés, spool privé borné4GiB, chunks1MiB rehashés à
lecture ; PreparedMutation retient la source jusqu'à acceptation. Les profils
en mémoire gardent leurs limites par défaut64/256MiB ; `SnapshotLimits::streaming`
est réservé aux chemins spoulés initialize/capture.

`apply_checkout_with_sources` réutilise journal et sauvegardes existants ;
projections FileId/longueur, staged files hashés pendant écriture, vérification
finale streamée et heartbeat pendant les scans/fingerprints. La récupération
reste compatible avec les anciens fingerprints BLAKE3 sans préfixe. Les données
préparées sont validées avant journal sans modifier CAS/metadata ; staging des
fichiers intervient avant claim de lease, commit CAS local reste exclusif.

Recette Windows100MiB réelle PASS10,59s : capture puis interruption après écriture
du nouveau fichier, restauration exacte de l'ancien, nouvel apply/replay,
réouverture, ancienne cible Marker intacte, lecture exacte de chaque chunk et
fsck complet propre. Pic heap Rust supplémentaire8 537 465octets (pas RSS).
Recette1GiB lancée, résultat encore attendu ; HTTPS et qualification Linux restent
à faire. Régressions Windows capture11 PASS1,89s, lease2 PASS35,76s,
types5 PASS41,62s, workspace12 PASS6,36s ; Clippy Store strict PASS2,81s.

## C — matérialisation depuis sources et digests (après A/B)

Le materializer prend actuellement before/after en Vec<CheckoutEntry> contenant
tous les bytes, puis écrit les staged files. Ajouter des sources pinnées/spools ou
itérateurs de chunks vérifiés et projections de fingerprints ; la fermeture CAS,
la projection Tree exacte et les originaux restent vérifiés. Réutiliser journaux,
backups sans écrasement, renouvellement de lease, recovery et identité du checkout.
Tests100MiB puis1GiB réels, interruptions, disque plein, fichier modifié pendant
lecture, reprise et fsck. Semantic Lens conserve une limite explicite distincte
tant qu'il requiert le contenu intégral ; ne pas lui envoyer arbitrairement1GiB.

## D — graphe et publication/réception (root après API A/B/C)

GraphObjects garde aujourd'hui toute fermeture256MiB puis reconstruit les fichiers
et un second SnapshotBuilder ; publisher parcourt ces objets en mémoire. Passer
à un index metadata + références CAS/spool, lecture lazy par Chunk, itération des
uploads et projection par FileId. Respecter auth Space/epoch, claims, closure,
capabilities fraîches, cancellation et aucune capacité writer pendant HTTP.
Ne pas modifier sans coordination les fichiers download/cache de fusion_native
ni les guards ACL de selectors. Le transport HTTP spoulé existant reste réutilisé.

Le fetch client refuse encore `has_more` ;1GiB/64KiB dépasse la page10000objets.
Consommer tous les snapshots de pages exacts avec limites d'objets/octets logiques,
vérifier omission/doublon/corruption avant admission. Un budget RAM distinct des
octets logiques doit être instrumenté, avec vraie recette HTTPS100MiB/1GiB et
RSS/peak mesuré. Erreur en fin de flux/coupure/reprise doit conserver ancien état.

## Reprise et checkpoints

1. A vert : signatures publiques, vecteurs invariants, vérification grande entrée
   bornée ; journaliser nombres/mémoire/limites avant B/C.
2. B/C verts : capture/reopen/restore/recovery locales grands fichiers avec toutes
   identités exactes et mémoire mesurée ; pas encore de revendication réseau.
3. D vert : publication/clone/receive/reprise TLS, droits et annulation réels,
   aucun saut de vérification via cache. Rejouer leases99k et packaging après gel.
4. Les limites non traitées (diff/merge sémantique gigantesques, gros graphe
   historique, stockage disque du spool) restent nommées explicitement.

### D1 — checkpoint root

`remote/chunk_spool.rs` conserve les chunks vérifiés dans un fichier temporaire
privé, supprimé à la fermeture, avec index `(ChunkId, offset, longueur)` en mémoire.
Chaque lecture recalcule le ChunkId ; corruption et troncation sont refusées.
`GraphObjects` conserve seulement les autres objets en mémoire. La publication
itère les chunks, puis les métadonnées ; `fetch_visit` verse chaque objet reçu
directement dans ce graphe sans accumuler un `Vec` de tous les objets du claim.
La vérification finale de FileId utilise le nouveau vérificateur incrémental A.

Les plafonds fichier64MiB/fermeture256MiB restent identiques. La matérialisation
et le batch final de réception reconstruisent encore des buffers complets :
ce checkpoint n'établit pas le streaming de bout en bout. Le spool temporaire
du graphe est distinct du cache de téléchargement durable ; sa disparition
n'autorise jamais à accepter une fermeture incomplète.

Premier contrôle : 13 tests client distant PASS0,17s, dont corruption/troncation
du spool, identités/fermetures et journaux. Ce résultat précède le raccord de tous
les appels `fetch_visit` ; rerun et recette TLS requis avant clôture D1.

Après raccord `fetch_visit` et vérificateur A : **15 tests PASS0,32s**. La lecture
ponctuelle CLI utilise désormais `download_remote_file` (vérification complète
avant création, écriture par chunks, `create_new`, `sync_all`). Recette CLI réelle
**74 commandes PASS16,750s**, incluant règles v2, rejeu, restoration gérée et
lecture exacte/refus de destination existante. Les limites du checkout et du
batch final restent les mêmes à ce checkpoint.

### D2 — budgets serveur séparés, qualification bout en bout en attente

`closure.rs` vérifie chaque manifeste via le vérificateur A, un chunk à la fois.
Plafonds distincts : objet canonique64MiB, fichier logique1GiB, références de
chunks16384, métadonnées de fermeture256MiB, chunks uniques4GiB. Le travail de
reconstruction par transaction est aussi borné à4GiB et compte les chunks
répétés ainsi que les candidats rejetés, avant lecture de leurs contenus.
La déduplication ne contourne donc pas le budget de vérification.

Deux tests de budgets PASS ; le parcours CLI ci-dessus reste vert. Le support
réseau1GiB attend encore B/C et le raccord D3 : ces budgets serveur seuls ne
prouvent pas sa disponibilité dans le produit.

### D3 — réception raccordée, qualification réseau en cours

La réception utilise maintenant les projections FileId/longueur et
`apply_checkout_with_sources`. Les objets du batch sont uniquement les
métadonnées ; les chunks préparés sont admis via le spool privé et les callbacks
locaux, sous la même transaction et le même journal de matérialisation.
Aucun appel réseau dans ces callbacks. Le graphe et les plans téléchargés
acceptent au plus4GiB logiques,1GiB par fichier,256MiB de métadonnées ; les API
historiques retournant un Vec gardent leurs refus explicites64/256MiB.

Après raccord : **15 tests client distant PASS0,09s**. Recette HTTPS courte et
recette100MiB puis1GiB à exécuter ; ce contrôle unitaire ne les remplace pas.
