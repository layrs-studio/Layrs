# Reprise du 28 octobre 2026

## Point courant - fichiers de toute taille (ADR 0027) puis performances vs git, conseil r4 (Claude + conseil, 2026-11-05)

Demande : « ajoute le support des fichiers de n'importe quelle taille, PUIS ... dépasser git sur toutes les commandes sans faire de tradeoff que git ne fait pas ». Rien de commité.

Phase 1 (fichiers de toute taille), détails dans `docs/decisions/0027-fichiers-de-toute-taille.md` :
- découpe adaptative : 1 Mio jusqu'à 1 Gio, puis puissance de 2 de longueur/1024, au plus 16 Mio (`fixed_chunk_bytes`) ;
- plafonds 1/4 Gio levés partout dans le chemin natif ;
- manifeste « prouvé » (pas de re-hachage à l'admission) ;
- présence de morceau et d'arbre lue sur les métadonnées ;
- le spool oublie les morceaux déjà admis, et la capture filtre ceux des gros fichiers de base ;
- purge sûre des sauvegardes de matérialisation (dossier épinglé, sceau, Windows `open_immutable_file`) ;
- cache des morceaux d'hydratation ;
- distant : un fichier seul est hors budget, mais la pagination au-delà d'environ 156 Gio reste à faire ;
- serveur : budget d'amplification 4 Gio au-delà de 64 fois les octets distincts.

Le parcours complet passe à 1,5 et 5 Gio (`scratchpad/bigfile_e2e.sh`). Comparaison avec git sur 1,5 Gio : commit d'une édition 3,4 s contre 47 s ; `.git` 3,0 Gio contre 1,5 Gio pour le Store.

Conseil r4 (Astra 6/10, Sol 6/10, Opus 7/10, tous « ship after fixes ») : constats et décisions dans `D:/Layrs/tmp/council-r4/SYNTHESIS.md`, tous les constats confirmés étant corrigés.

Phase 2, gains, à chaque fois sans changer les garanties :
- cache persistant de projection par arbre racine (`scm_v1/projection_cache.rs`, binaire, blake3, recalcul de contrôle 1 fois sur 64, à chaque fois en debug) ;
- mémo de `projection_tree_id` par empreinte ;
- `status` en une session (`status_report`) ;
- validation des limites du Store une fois par processus ;
- schéma SQLite validé une fois par état de fichier, et `mmap` sur les connexions ;
- index de changements binaire (l'ancien JSON reste lu) ;
- handle par pack ;
- regroupement des longueurs, des présences et des manifestes admis ;
- Weave : fichiers des arbres durables non revérifiés ;
- scan : moins de travail par fichier, pas de revalidation quand le résultat égale la base ou la passe précédente.

Mesures à 10 000 fichiers (avec `LAYRS_PERF`, binaire r23) :

| commande | git | r16 | r23 |
| --- | --- | --- | --- |
| init | 43,3 s | 6,6 s | 5,9 s |
| status | 58 ms | 481 ms | 90 ms (88 ms sans perf) |
| commit 2 fichiers | 163 ms | 740 ms | 318 ms |
| nouvelle Room | 2,19 s | 2,40 s | 1,28 s |
| lecture complète juste après | 2,64 s | 19,8 s | 14,1 s |
| commit en Room | 252 ms | 593 ms | 376 ms |
| fusion en avance rapide | 85 ms | 1,79 s | 575 ms |
| fusion 3 voies | 369 ms | 2,31 s | 1,06 s |
| suppression de la Room | 1,39 s | 2,88 s | 2,9 s |
| Room physique | 2,19 s | 12,1 s | 9,1 s |

Mesure M3 (hydratation parallèle) : 8 lecteurs prennent 51 s contre 15 s en séquentiel, alors que l'hôte ne cumule que 1,9 s. Le plancher CFAPI, noyau et antivirus est d'environ 1,3 ms par fichier, et le préchargement parallèle est contre-productif ici. La « lecture complète » d'une Room purement virtuelle ne peut donc pas battre git sans décision produit : écrire les petits fichiers à la création, ou réchauffer en fond.

Tests : suites core, store, store-local, client-core, cli, vfs et weave vertes, à part les échecs déjà présents avant (`projection_recovery_routing` ×2, `scm_admission::history::worktree_cas...`). Les recettes natives VFS ne sont pas relancées.

Conseil r5 (décisions D1 à D6, synthèse dans `D:/Layrs/tmp/council-r5/SYNTHESIS.md`). Appliqué depuis :
- D1 : sondes CFAPI de vérification avant `drop` parallélisées ;
- D4 : capture en une passe pour `step`, `status` et `init` ; les vérifications avant suppression ou écriture gardent deux passes ;
- D3 en partie : un bail par worktree pour les nouvelles matérialisations (marque `leaseScope`), et plus de `lease.force()` redondant ;
- `drop` : désenregistrement et suppression par un processus détaché, lié à l'identité du dossier (P0 d'Astra) ; jamais de suppression sur place, le `drop` échoue si le dossier est occupé (P1 d'Opus) ; une nouvelle Room du même nom prend `nom-2` tant que l'ancien dossier existe.

Mesures r27 (avec `LAYRS_PERF`), contre git : `drop` 626 ms contre 1,39 s ; avance rapide 514 ms contre 85 ms ; 3 voies 1 058 ms contre 369 ms ; `commit` 293 ms contre 163 ms ; commit en Room 448 ms contre 252 ms ; `status` 90 ms contre 58 ms ; Room physique 10,6 s contre 2,19 s ; lecture complète 15 s contre 2,6 s. Les suites de tests sont vertes, à part les 3 échecs connus.

Décisions de l'utilisateur (5 novembre 2026, après r5) :
1. **Rooms virtuelles hybrides** : à la création, les petits fichiers (64 Kio au plus, avec un plafond total configurable) sont écrits physiquement, puis convertis en placeholders hydratés et in-sync ; les gros fichiers restent virtuels.
2. **`room merge` garde la capture** du travail non enregistré, accélérée par une énumération complète comparée au dernier step accepté de la Room, sans reconstruire de snapshot (D5).

Banc r28 contre git, 10 000 fichiers, médiane de 2 passages sans `LAYRS_PERF` (`D:/Layrs/tmp/vsgit/vsgit-r28.tsv`) :

| commande | git | Layrs r28 |
| --- | --- | --- |
| init | 44,7 s | 4,5 s |
| status | 52 ms | 90 ms |
| status, 2 changements | 55 ms | 122 ms |
| commit | 152 ms | 300 ms |
| nouvel espace | 2,24 s | 1,14 s |
| lecture complète | 2,89 s | 15,7 s |
| commit en Room | 250 ms | 444 ms |
| avance rapide | 79 ms | 570 ms |
| 2e agent | 74 ms | 536 ms |
| 3 voies | 354 ms | 1,03 s |
| `drop` | 1,43 s | 0,46 s |
| Room physique | 2,24 s (worktree) | 9,9 s |

Ordre prévu :
1. D2, chemin physique « dossier neuf », qui servira aussi au mode hybride ;
2. mode hybride, après la mesure M4 (`CfConvertToPlaceholder` ou transfert proactif) ;
3. D5 ;
4. packs pour les petits lots, avec regroupement automatique (équivalent du `gc`) ;
5. cache-tree (arbres reconstruits seulement pour les dossiers modifiés) ;
6. `status` minimal ;
7. sessions et transactions de matérialisation (reste de D3).

Reste à faire, par écart :
- Room physique : chemin « dossier neuf » (B5) ;
- `drop` : vérification complète, décision ADR 0025 à revoir avec le conseil ou l'utilisateur ;
- fusions : capture de la Room par l'hôte (environ 170 ms), sessions de Store, transactions groupées (B1) ;
- commit et commit en Room : écritures CAS d'objets séparés et deux passes de scan ;
- `status` : environ 30 ms de trop.

## Point precedent - packs CAS (ADR 0026) et priorites du conseil r3 (Claude + conseil, 2026-11-05)

Decision utilisateur 2026-11-04 ("let's go") : packs pour les petits objets, ADR 0026 revisee apres le conseil r2 (Astra, Sol, Opus : go avec changements).

Fait (non commite) :
- Format de Store 5 (`PACKED_FORMAT_VERSION`) cree par `init` ; 2 encore lu et cree avec `LAYRS_STORE_FORMAT=2` ; reprise d'init garde la version du marqueur ; 3/4 (projection) et 6+ restent `FutureFormat`. Les capacites (`NativeStoreOpen/Audit/Exclusive`) portent la version validee (`format_version()`, `packs_enabled()`).
- `scm_v1/cas/packs.rs` : fichier `objects/packs/pack-<32 hex>.lpk` (en-tete, enregistrements autodescriptifs, index trie, pied avec blake3 en-tete+index) ; index valide comme entree non fiable (bornes, tri strict, plages sans chevauchement, 64 Kio/objet, 1M entrees, 64 Mio d'index, 32 Mio d'enregistrements) ; lecture exacte d'un membre (en-tete d'enregistrement compare a l'entree) ; catalogue par processus (id du Store + identite de la racine) ; scrub membre par membre et, sans index valide, par parcours des enregistrements.
- `put_ordered` : un `Packer` par lot (63 objets en memoire, flux vers staging a partir du 64e, nouveau pack au-dela de 32 Mio) ; moins de 64 nouveaux petits objets = fichiers separes (publies en parallele) ; sinon scellement (index + pied, vidage B ou sync A, relecture complete comparee aux empreintes), renommage sans ecrasement, identite verifiee, puis seulement l'etape de durabilite du lot. Reutilisation d'un membre seulement s'il relit les octets attendus (A : pack et `objects/packs` synchronises en fin de lot). Inventaire des packs une fois au debut du lot.
- Lectures (`read`, `verify`, `contains`, `stream_verified_chunk`, nouveau `locate`) : packs charges, fichier separe, puis packs publies depuis ; premiere copie qui se verifie ; un fichier separe deja diffuse au flux reste definitif. `read.rs` et `fsck` passent par `locate`. `insert_object_rows` refuse une ligne existante differente. Points de faute `BeforePackPublication`/`AfterPackPublication`.
- Tests : `tests/scm_cas_packs.rs` (seuil 63/64, doublons + gros objets separes, reutilisation A et B, membre abime isole et nomme par le scrub puis reecrit, index invalide / pack tronque, Store format 2 sans pack, `per-object`, pannes avant/apres publication puis reutilisation du pack orphelin) ; 5 tests unitaires du format.

Mesures (meme jour, binaires geles `D:/Layrs/tmp/roomtest/layrs-14.exe` / `layrs-15.exe`, profils `D:/Layrs/tmp/packs-r15/`) : init 10k 51.9/50.9 s -> 15.5/14.8 s ; publication CAS 33.8 -> 2.0 s ; `.layrs/objects` 20 013 fichiers / 54 Mo -> 10 fichiers + 1 pack / 9 Mo. Reste dans init : validation des admissions 8.5 s, capture 2.4 s, insertion des lignes 1.1 s. Benchmark Rooms 10k (`bench-rooms-r15.tsv`) : commandes inchangees, 1er grep virtuel 23.0 -> 19.7 s (hydratation depuis le pack).

Tests : `layrs-store-local` complet vert hors l'echec preexistant `scm_admission::history::worktree_cas...` (une regression trouvee et corrigee : un Store de projection format 3 doit rester `FutureFormat`) ; client-core/cli/vfs/weave verts hors les 2 preexistants `projection_recovery_routing` ; recettes VFS natives 10/10 ; `native_rooms_virtual` 2/2.

Comparaison avec git 2.45.1 (configuration par defaut, meme machine et jour, mediane de 3 ; scripts `bench_vs_git.sh` / `bench_texture.sh`, donnees `D:/Layrs/tmp/vsgit/`, page `D:/Layrs/tmp/vsgit/layrs-vs-git.html`). 10k : import 13.1 s contre 43.3 s ; nouvel espace d'agent 2.0 s (Room virtuelle) contre 2.2 s (worktree) ; texture 256 Mio x 4 versions : Store 261 Mio contre 1 Gio (256 Mio apres un gc de 10 s), commit 3.1 s contre 5.5 s. git gagne les commandes courtes : status 58 ms contre 466 ms, fusions 85-369 ms contre 1.4-2.3 s, commit 163 contre 718 ms ; Store 1.6 Mo contre 45 Mo (metadonnees SQLite) ; lecture complete d'une Room virtuelle 17.3 s contre 2.6 s.

Conseil r3 (consultatif, rien d'applique) : 3 membres d'accord sur les trois chantiers suivants, qualification native verte comme porte d'entree et de sortie de chacun : (1) gros fichiers de bout en bout (plafonds verifies : 1 Gio/fichier et 4 Gio/capture cote client, admission, transfert et serveur ; 100 000 entrees ; publication partielle 16 Mio ; resolution de fusion en memoire 64 Mio/256 Mio ; aucun producteur FastCDC) ; (2) parcours produit de clone partiel avec ACL par fichier (Opus : d'abord une ADR qui unifie le modele "absent ≠ supprime" des projections et des Rooms) ; (3) progression concurrente des Rooms (chunks installes hors de la section exclusive, seul le commit et le CAS de tete sous verrou). Opus place en tete la mise a l'abri du travail (commits, CI Windows) : decision utilisateur.

## Point precedent - Rooms (ADR 0025) : index de changements, capture VFS par enumeration, Weave paresseux, conseil r1 (Claude + sous-agent Claude + conseil, 2026-11-04)

Decisions utilisateur 2026-11-04 : nom produit Room (alias `loom`, `worktree`) ; Room virtuelle par defaut (`--physical` sinon) ; detection des changements "comme git" pour les Rooms PUIS pour le dossier principal (index par Worktree, `step/status --full` pour tout relire) ; garde-fou : verification complete avant `drop` et avant tout ecrasement.

Fait (non commite) :
- Surface `room new/list/path/status/step/merge/drop` (`layrs-cli/src/native_room.rs`, `args/room.rs`, `client-core/native_scm/rooms.rs`). Emplacement par defaut `%LOCALAPPDATA%\Layrs\rooms\<space>-<hash>\<room>` si meme volume, sinon `<projet>.rooms\`. Registre `.layrs/rooms/`. Worktrees retires (`retired-worktrees.json`). Merge : avance rapide (`fast_forward_layer`) sinon Weave.
- Index de changements `scm_v1/change_index.rs` (cle stat + `verifiedAt`, marge racy 100 ms NTFS/ReFS, 2 s ailleurs ; ecrit apres acceptation ; seede par la materialisation sous handle exclusif, par `init`, par la creation VFS). Scan `capture_streaming_trusting` (hook trust recoit la metadonnee listee). Actif pour tout dossier physique.
- VFS : checkpoint par enumeration `FileIdExtdDirectoryInfo` (etat CFAPI via tag de reparse, numero NTFS) sans ouvrir les fichiers ; placeholder in-sync = base meme hydrate ; `Vouch::Content` pour fichiers prouves ; `checkpoint_verifying` (handle attributs, etat in-sync natif) pour `drop` ; renommages acceptes dans la Room (ACK_RENAME si TARGET_IN_SCOPE) ; `.layrsignore` applique ; hachage en streaming (spool 1 MiB, 1/4 GiB) ; manifests lus a la premiere hydratation ; verrou partage du Store par plage (chemin valide une fois, fichier rouvert sans parcours des ancetres) ; hote : n'herite plus des pipes de l'appelant (bloquait `room new` sous un shell capture), redemarrage si protocole inconnu, libere ses handles avant de repondre a Stop.
- `drop` : une seule capture verifiante, dossier renomme a cote puis purge detachee (`__layrs-purge-dropped-room`), retry borne du renommage.
- Weave (sous-agent Claude) : `MergeFileSource` paresseux, `max_input_bytes` ne compte que les octets lus, arbres limites aux tetes/bases ; acceptation d'un merge propre par projection (`materialize_projection`), controle "dossier propre" en streaming/index.
- `status` : booleen par comparaison de projections ; diff detaille ne lit que les chemins changes.
- Perf : `LAYRS_PERF_OUT=<fichier>` (rapports JSONL, hote VFS par requete), spans d'hydratation `vfs.read_*`.

Mesures 10k (r7 -> r11, `D:/Layrs/tmp/council-r1/bench-table.md`) virtuel / physique : room new 16.7 -> 6.2-9.6 s / 19.9 -> 10.4-13.4 ; step apres grep 132 -> 1.0 s / 9.7 -> 1.9 ; merge avance rapide 11.5 -> 1.9 / 10.6 -> 2.4 ; Weave 2e agent 213 -> 3.8 / 160 -> 4.5 ; drop 11.6 -> 3.4-5.8 / 8.5 -> 2.8-3.1. 1k : toutes les commandes Room 0.4-1.8 s. Restent mauvais : 1er grep virtuel 10k 55.7 s (physique 0.9 s), init 10k 104-185 s.

Conseil r1 (Astra xhigh, Sol max, Opus xhigh ; brief `D:/Layrs/tmp/council-r1/BRIEF.md`) — decide et fait : population VFS par lots par repertoire ; capture unique au drop ; hote libere ses handles avant Stop + retry renommage ; capture unique au merge (`prepare_named_weave_after_capture`) ; pas de prechargement des chunks de base quand l'index couvre la base ; `status` sans lecture de contenu ; diff limite aux chemins changes ; `room step/status` physiques sans l'enveloppe `execute_workspace` ; index VFS non reecrit si inchange ; spans d'hydratation (mesure M1). Plus tard (mesurer d'abord) : sondes de verification du drop en parallele ; requetes SQL par lot (projections, ids de manifest) ; ouvertures du Store par commande (17-31) ; prechargement des manifests ; init (publication CAS : fsync fichier + 2-3 syncs de repertoire par objet ; toute coalescence = decision utilisateur, la durabilite decidee ne couvrait que les fichiers projetes) ; ecriture directe des Rooms physiques ; USN ; concurrence du verrou Store. Chantier structurel suivant : population VFS a la demande (seule voie vers un `room new` instantane ; distinguer "jamais peuple" et "supprime"). Rejete : confiance a l'index pour le drop, retrait du fsync CAS, hydratation complete anticipee, reecriture du Weave, demon global, exclusions antivirus, listes de chunks lues en SQL sans manifest.

Durabilite des lots CAS (decision utilisateur 2026-11-04, "A et B") : `NativeCasWriter::put_ordered` rend tout le lot durable a sa fin, avant tout commit de metadonnees (garantie inchangee au commit). A (partout) : plus de sync du repertoire `staging` dans un lot, creations de shards sans sync (`ensure_child_unsynced`), chaque repertoire touche synchronise une fois en fin de lot (8 threads). B (NTFS) : donnees de chaque objet ecrites sans vidage du cache disque (`NtFlushBuffersFileEx` FLUSH_FLAGS_NO_SYNC), aucun sync de repertoire (ordre du journal NTFS), un seul vidage final d'un marqueur cree apres toutes les publications (journal + cache disque). `staging`/`objects` epingles une fois par lot, chemins reverifies avant de declarer le lot durable. `LAYRS_CAS_BATCH_DURABILITY=per-object|coalesced` force un mode. `put` isole inchange. Init 10k 109.5 s -> 52.9 s (publication CAS 90 -> 34 s ; 81k syncs de repertoire -> 0) ; 3k 32.8 -> 20.7 s. Validation des admissions : plus de requete "deja admis" pour les objets du lot (valides en memoire). Reste : ~1.7 ms/objet de publication (creation, relecture, renommage, scan AV a la fermeture) -> packs plus tard ; validation 10-14 s a 10k.

Plancher CFAPI (mesure M2 du conseil, binaire experimental `D:/Layrs/tmp/roomtest/layrs-exp-memory.exe`, code retire) : hydratation servie depuis la RAM = grep 1k 1.5 s, 10k 11.5 s ; hydratation normale r14 a chaud 1.55-1.6 s (1k), 23 s (10k, objets froids) ; prechargement de tout le contenu depuis le CAS dans l'hote 0.47 s (1k) / 4.6 s (10k). Donc le cout fournisseur est ~0.1 ms/fichier a chaud, le plancher CFAPI ~1.2-1.5 ms/fichier (physique : 0.09 ms). Aller sous ce plancher = eviter le callback (pre-hydratation des petits fichiers en arriere-plan apres `room new`) : decision produit (disque/CPU). Benchmark final r14 (`bench-rooms-r14.tsv`) : init 10k 54-65 s (r12 107-109), commandes Room inchangees ; suite complete (-j 4) verte hors 3 preexistants ; natives VFS 10/10.

Tests : suite complete (5 crates, `--no-fail-fast`) verte hors 3 echecs preexistants (`projection_recovery_routing` x2, `scm_admission::history::worktree_cas...`) ; recettes VFS natives 10/10 ; `native_rooms` 4/4, `native_rooms_virtual` 2/2 ; 2 recettes natives mises a jour au nouveau contrat (in-sync hydrate = base ; deplacement capture). Flakes de fixtures pid+nanos corriges (`native_offline_tests`, `native_workflow`).

## Point courant - r214/r215 : contrat checkout "comme git", switch O(K), creation 10k 248s -> 11s (Claude seul + conseil Astra, 2026-11-04)

Decision utilisateur 2026-11-04 : durabilite "comme git" pour les fichiers projetes depuis le CAS (creation + switch) : plus de fsync par fichier (mesure machine : 10k fichiers avec fsync 8-12s, sans 1.9s). Chaque fichier stage est cree en refusant tout autre ecrivain (`create_file_denying_writers`), son identite + taille + mtime apres ecriture sont journalises (evidence `staged`). Recovery : avant commit, un fichier dont le contenu n'a pas survecu n'est mis en quarantaine (deplace, jamais supprime) que si identite, taille et mtime sont inchanges ; apres commit il est garde (modification de travail). Phase 1 (originaux) garde ses syncs. Echec de staging -> journal ferme "untouched" (`abandon_untouched`), pas de needs_repair ; pre-verification limitee aux fichiers de base deplaces (`verify_displaced_content`), les octets cibles sont prouves au staging (staging parallele 8 threads via `write_file_shared` + `SharedStagingStore`). Postcheck : fichiers installes valides via leurs handles (`is_at` chemin NT exact + taille/mtime) sans reouverture (evite le scan antivirus ~10s/10k) ; fermeture des handles en parallele.

Switch : pre/postcheck limites aux K chemins touches (`touched_paths_match` : empreinte de chaque original, nouveaux chemins libres, ancetres repertoires ; `touched_paths_installed`) au lieu de 2 scans complets ; chemins non touches jamais ecrits (edition concurrente = modification locale). Projection d'un Step mise en cache par processus (Step/Trees immuables, admissions jamais retirees) ; `admitted_chunk_ids_of_files` par lots de 256 ; journal en `prepare_cached`. CAS `put_ordered` : octets possedes en vol bornes a 8MiB (test tas 100MiB : 49MB < 64MB).

Conseil Astra (gpt-6-astra, lecture seule, autorise par l'utilisateur) : 2 problemes de correction releves et corriges (partage en ecriture des fichiers stages ; identite seule insuffisante pour deplacer en recovery). Publication par sous-arbre ecartee : Windows refuse de renommer un repertoire contenant un fichier ouvert (mesure), incompatible avec le postcheck par handle.

Mesure finale r215 (EXE `.layrs-local/validation/switch-optimized-r215/layrs-r215-release.exe` SHA `e8fac86dc186f669650786a465ebbbc8ef6f300be4a11fd3f43eda14abe36638`, `switch-optimized-r215-{1k,10k}-2026-11-04.json`). 1k : worktree create 1.36s, switch K=1 0.61, no-op 0.43, switch K=100 0.95, status 0.28, step K=1 0.49, step K=100 1.5, init 10.5. 10k : worktree create 11.0s (r213 42s, r208 248s), switch K=1 2.5 (r213 6.7), no-op 2.0, switch K=100 2.9 (7.4), status 1.6, step K=1 3.2, diff K=1 6.0, init 95s, 2 agents step 8.7/4.1. Tests r215 : seuls les 3 echecs preexistants ; 3 flakes de nommage de fixture (pid+nanos) corriges (`download_cache`, `native_offline`, `checkout_streaming`).

Reste : index stat/USN par worktree (capture = scan complet 1.2-1.8s a 10k, domine switch/status/step) ; creation : ~2.3s de renames depuis le staging (ecriture directe en place pour un worktree neuf = extension du journal) ; diff (recapture complete, 6s a 10k) ; concurrence agents (verrou Store exclusif) ; switch VFS O(K) ; 100k ; lot H.

## Point courant - lots perf r212/r213 : creation worktree, CAS et scans paralleles, revues 2 et 3 corrigees (Claude seul, 2026-11-04)

Mesures release (machine ~2x plus lente que le 03/11 : le binaire r210 gele met 4.1-4.9s pour status 10k ce jour contre 1.8s hier ; meme fixture, code actuel 2.5s). 1k `docs/plans/pme/measurements/switch-optimized-r212-1k-2026-11-04.json` : init 31.6s, status 0.44, step K=1 0.62, switch K=1 1.12, no-op 0.75, step K=100 3.1 (2.0 apres CAS parallele), switch K=100 2.0, worktree create 7.2 (3.7-4.2 apres staging parallele), 2 agents step 0.64/1.18. 10k `...-r212-10k-2026-11-04.json` : init 111s (399 avant CAS parallele ; echec en r208), status 2.8, step K=1 3.3, switch K=1 9.2 (switch K=100 7.5 -> 5.9 apres r213), step K=100 4.4, worktree create 37s (r208 248, r210 137), step concurrent 6.7/3.6. Profil creation 1k : ~20s -> 3.7-4.2s.

r212 (perf) : instrumentation `perf` attribuee (`fs.*@parent`) ; `verify_path_identity` chemin rapide Windows (GetFinalPathNameByHandle) ; pins CAS racine/objects caches 250ms pour lectures verifiees seulement (publication re-pin) ; `NativeStoreReading` (CAS + registre identite ouverts une fois pour le staging) ; materialize : pre/postcheck `matches_projection` (1 passe egale, sinon capture 2 passes), creation des fichiers stages en parallele, petits fichiers verifies (FileId) puis ecrits+fsync par workers avant toute installation, phase 2 : un parent par repertoire et syncs groupes apres la boucle (recovery decide chemin par chemin ; phase 1 inchangee) ; scan checkout : classification par l'enumeration, hachage parallele 4 threads (mesure : fichiers chauds sature a 2, fichiers neufs/antivirus jusqu'a 8) ; CAS `put_ordered` (installations concurrentes, resultats et points de faute dans l'ordre) ; `ensured_shards`.

Revue lot 2 (7 findings) corrigee : verif Store des fichiers de base deplaces (`verify_changed_content` 2 cotes, + `check_restore` avant capture auto, `receive`, reprise creation) ; fallback Store dans `write_file_from` quand le spool n'a pas le chunk (5 echecs `native_restore_preview` + 1 `native_concurrency`) ; `admitted_chunk_ids_of_files` (chunks de la base seulement, plus de chargement de tout l'Espace) ; manifestes LEFT JOIN + borne 1024 ; `MAX_BATCH_ADMISSIONS = MAX_BATCH_OBJECTS`. Test CLI ajoute `missing_base_chunk_refuses_switch_before_moving_current_files`.

Revue lot 3 corrigee (r213) : H1 un handle par repertoire pendant le scan (EMFILE Unix) -> scan par lots bornes (32 repertoires termines / 2048 fichiers : hachage puis recheck+fermeture enfants avant parents ; fenetre de stabilite reduite) ; parents installes bornes a 64 (sync anticipe). M1 chemin rapide identite : objet sans reparse ni suppression en cours, comparaison en chemins NT avec lettre resolue dans les DosDevices de l'appelant ; divergence documentee : tag non-lien (cloud/ProjFS) pose sur un ancetre apres le pin non recontrole. M2 accepte partiellement : la capture ne relit plus les copies Store des chunks connus (verification a l'usage : switch/restore/receive refusent avant journal) ; `check_restore` n'exclut plus que les fichiers absents du Store. L1 `put_ordered` : receiver possede par les workers (pas de blocage si tous paniquent) ; L3 budget total charge avant lecture ; L6 receive : seuls les chemins changes ; L2/L5 commentaires ; L7/L8 acceptes. Perf r213 : capture/diff acceptes en 1 passe si egaux a la base ; `clear_obstructing_directories` limite aux chemins changes (20k verifications -> 0 au switch 10k).

Tests r212 et r213 : seuls les 3 echecs preexistants connus (`scm_admission::history::worktree_cas...`, `projection_recovery_routing` x2) ; CLI 100% vert. Flake corrige (test seulement) : fixture `tests/download_cache.rs` nommee pid+nanos (collision en parallele, 1/8 runs) -> + compteur. Non commite (sur demande).

Mesure finale r213 (EXE gele `.layrs-local/validation/switch-optimized-r213/layrs-r213-release.exe` SHA `d1eb29513cdc23b2bf74cde71a594bbcefab9e9fb2f56fe6f501a9196695cc7b`, `switch-optimized-r213-{1k,10k}-2026-11-04.json`). 1k : init 10.1s, status 0.29, step K=1 0.50, switch K=1 0.88, no-op 0.59, step K=100 1.5, switch K=100 1.4, worktree create 4.0, 2 agents step 0.56/1.03. 10k : init 106s, status 1.6, step K=1 3.4, switch K=1 6.7, no-op 4.3, step K=100 4.7, switch K=100 7.4, worktree create 42s (cout par fichier, antivirus), 2 agents step 3.6/6.8, diff K=1 7.8 (a profiler).

Reste : index stat/USN par worktree (status/switch 10k encore O(N) : 2-3 scans + projections SQL x3) ; concurrence agents (verrou Store exclusif, CAS View global) ; switch VFS O(K) ; ~30 `open_store` par creation ; mesure 100k ; lot H natives Restore non-C14 ; 3 echecs preexistants.

## Point precedent - optimisations switch/step : capture O(changement) (Claude seul, 2026-11-04)

Mesure r209 (`docs/plans/pme/measurements/switch-optimized-2026-11-03.json`) 1k : switch 23.5->2.1s, step K=1 25.7->4.6s, status 3.6->0.6s, worktree create 46.8->26s ; 10k fonctionne (bornes batch relevees : MAX_BATCH_OBJECTS 262144, visites admission 524288/1048576, spool 262144 ; test budgets aligne) mais restait O(N). Suite (r210) : `file_length_metadata` lit `file_manifests.file_byte_length` (1 requete, plus de lecture d'objet manifeste) ; `ScmReader::file_manifest_ids_metadata` et `object_admitted_metadata` (defauts = verification complete ; natifs SQL, `prepare_cached`) ; `admissible_manifest_for_file` et `validate_admission` decident l'admission durable par metadonnees AVANT de lire l'objet ; capture : chunks deja admis non mis en spool (`admitted_chunk_ids` charge avant le hachage, sans verrou pendant le scan) et batch filtre aux objets non admis (`retain_new_objects`). Resultat step K=1 : 1k 0.81s, 10k 2.15s (dont scan 1.55s) ; status 10k 1.76s. Mesure complete r210 en cours. Reste O(N) : scans de l'arbre (index stat a faire), projections (3x lecture SQL N lignes), creation de worktree (copie physique).

Revue independante perf : aucune perte/publication non verifiee. Corrige : H1 test `native_workspace` adapte (projection metadonnees ; restore avec contenu falsifie refuse sans ecrire) ; M1 `verify_changed_content` (client-core projection.rs) verifie completement AVANT journal/registration le contenu des seuls fichiers ecrits (switch/restore/rewrite via apply_projection_transition, creation worktree), en excluant les fichiers fournis par les objets crees de la transition (regression `native_rewrite` Missing("File") corrigee) ; M2 fast path CAS fsync du shard une fois par vue ; commentaires layout/lease/admission corriges.

Tests : store-local lib 509/509 ; layrs-store OK ; client-core/cli : tous les echecs restants expliques. Preexistants (non lies, codes inchanges) : `scm_admission::history::worktree_cas_can_advance_only...` (InvalidOperationHeadsTransition, echoue aussi sans changements 4/5), `projection_recovery_routing` binding+recovery (rollback journal chaud refuse volontairement depuis 34ec205/b52b95a, tests obsoletes comme r4/provider_schema). Environnement : tests paralleles (lib client-core 12, compaction, worktree_safety, maintenance passent en serie), CLI `local_data_safety` = MAX_PATH du prefixe TEMP de test (passe avec TEMP par defaut). Ne pas utiliser de prefixe TEMP long pour les tests CLI.

Mesure baseline release r208 1k fichiers (`docs/plans/pme/measurements/switch-baseline-2026-11-03.json`, script `scripts/measure-switch.mjs`) : step K=1 25.7s, switch 23.5/24.3s, no-op 17.3s, worktree create 46.8s, 2 agents step 60.6/30.9s ; init 10k FAIL `combined canonical object count exceeds 16384` (limite batch). Instrumentation opt-in `layrs_store_local::perf` (LAYRS_PERF=1 -> JSON stderr du CLI). Profil switch 1k : verify_path_identity 107k appels 19s (connection() par requete SQL), file_availability 5304 (re-hash CAS 5x/fichier) 15s, validate_admission re-parcourt tout le graphe deja admis 2x/batch ; step : cas.put stage+fsync de TOUS les objets meme existants (14k dir_sync).

Corrections (non commit, tests en cours) : (1) projection checkout par metadonnees `file_length_metadata` (contenu verifie a l'ecriture) ; (2) verification identite implicite `NativeStoreOpen::connection()` et `LeaseKeeper::tick()` limitee a 1/250ms (bind, force et appels explicites inchanges) ; (3) `cas.put` reutilise un objet publie identique octet a octet sans staging/fsync (meme durabilite que l'ancienne branche existante) ; (4) `ScmReader::file_present_metadata` (defaut = verification complete ; natifs = metadonnees) pour `file_has_verified_manifest` ; (5) `validate_admission` s'arrete sur objets admis durablement (seul writer `insert_admissions` apres validate_batch ; fsck graphs verifie l'invariant). Resultat 1k : switch 35s -> 4.1-4.7s, step 33.6 -> 4.8s, status 3.7 -> 0.7s. A faire : tests suites, revue independante, mesure 1k/10k (corriger limite batch init 10k), puis index stat, concurrence, VFS.

## Point precedent - Restore C14 complet qualifie r208 ; priorite produit : switch de branche et worktrees par agent TRES rapides (Claude seul)

r208 lot G PASS : plain C14 3128.5s, companion 3099.8s (phases ready-version, capture-version, product-surface PASS). Lot H (5 natives Restore non-C14) differe : chemins quasi inchanges, a relancer apres optimisations. Demande utilisateur 2026-11-03 : changer de branche et avoir une branche par agent doit etre TRES rapide. Analyse `docs/plans/worktrees-fast-switch-analysis-2026-11-03.md` : switch O(N) (6-9 re-hash complets + 3 re-verifications CAS), aucun index stat, verrou Store exclusif + CAS global Space serialisent les agents, creation worktree copie O(N), VFS refuse le switch, C14 25-50 validations completes/operation. Ordre : (1) mesure release `layer use`/creation/2 agents 1k-10k-100k ; (2) corrections sures switch (une capture, post-verif K, projection metadonnees, identite chemin 1x/ouverture) + dedup validations C14 ; (3) index stat/USN ; (4) concurrence par worktree/WAL ; (5) switch VFS O(K). Etape 1 : build release CLI en cours.

## Point precedent - suite verte ; Restore C14 Version (Ready + Capture locale) et surface produit, natives r208 en cours (Claude seul)

Echecs journal chaud resolus : tests obsoletes depuis durcissements voulus (34ec205 maintenance owner RO sans rollback ; b52b95a catalogue famille sans rollback infere). `provider_schema::revisions::hot_populated...` attend maintenant le refus READONLY_ROLLBACK puis rollback test-only et verifie preservation ; `r4::retained_handle...` attend refus aussi pour binding de remplacement, catalogue inchange. Groupes r2/r4/provider_schema 38/38 PASS.

Restore C14 etendu : `RestoreSource::Version` accepte la generation Ready figee exacte (abandon des changements hors ligne) et `ProjectionVersion::Capture` locale post-Ready (route produit `PreviewRestore/ApplyRestore` qui n'accepte que des Versions) ; Remote et Capture importee/Ready refuses. Lecteur D7 : fichiers de base retenus par mapping fige (pas de claim captured-content). Trou routage produit corrige : `pending_publication` (Status) et `capture_history` (History) utilisaient des gardes/chargements non scopes refusant C14 (et History refusant tout admis) -> scopes. Native C14 : phases ready-version, capture-version, product-surface (state_with_selection, pending_publication, is_imported_capture, capture_history, preparations, compare_versions).

r208 EXE `63648353372f3970b4034512c3831ca3090a3c494013939225e6b8c0496ba77d` : suite non-ignoree 509/509 PASS (premiere suite verte). Lot G (natives C14 plain/companion 5460) en cours ; lot H (5 natives Restore non-C14) a suivre. r207 lance puis arrete volontairement (non concluant).

## Point precedent - tranche Restore C14 local TERMINEE et qualifiee (r204-r206, Claude seul)

r206 recovery PASS 782.2s (budget 1800). Disposition consolidee `.layrs-local/validation/c14-local-restore-disposition.json` SHA `b7c37127c719f7afa80913d0291bda81391244774a5d14b863c94ff8ac25c0fc` (script `D:/Layrs/tmp/summarize-c14-local-restore.py`) : 39 tests distincts, 0 echec ouvert ; produit identique r204/r205/r206 (seuls tests changes apres r204), chaque resultat lie a son EXE.

Suite non-ignoree complete r206 : 507 PASS / 2 FAIL / 582 ignored. Les 2 echecs (`tests::shared::r4::retained_handle_refuses_valid_family_marker_drift_before_hot_catalog_rollback`, `tests::shared::provider_schema::revisions::hot_populated_eight_owner_restores_committed_evidence_but_reclaims_nothing`, SQLITE_READONLY_ROLLBACK 776 sur rollback de journal chaud du catalogue famille) echouent identiquement sur EXE geles r21 (27/10), r56-r77 (28/10) et r202 : hors tranche, anciens ou environnementaux (KB5132402 02/11, KB5127936 03/11 candidats non prouves). A investiguer : recover_databases -> inspect(recover) -> ouverture catalogue avec journal chaud.

Non commite (regle : commit sur demande). Prochain programme (non commence) : perf lecteur C14 (index/cache des journaux et captures par snapshot), echecs journal chaud ci-dessus, C14 Restore Version/preparation/re-export, callers CLI/Desktop, suite L0-L9.

r204 lot C PASS : plain C14 Capture+Restore 2363.8s, companion 3537.0s (budget 3600 quasi atteint -> 5400/5460 pour les deux). Phases Restore toutes PASS (garde source, apply+dedup, Inverse octets jamais captures, Staged->cancel->retry, PathQuarantined->cancel refuse->resume, AfterCommit->replay, reouverture sans source). r205 (tests seuls vs r204, EXE `926d2b10...`) lot D : 3 restore_history PASS (dont canonical_history corrige), cancellation PASS 2174.4s (budget 2400), recovery timeout 720s apres iteration 0 PASS (assertion corrigee validee) -> 1800/1860. r206 (recovery seul change) EXE `44547d2bd750931cc768d57fd7f828e026f90452d896b3cae75e1c252600c16c` : recovery en cours.

Risque perf ouvert : cout du lecteur C14 (journaux et captures re-decodes a chaque validation, plusieurs validations par operation) ; operations Restore C14 ~100-150s en debug sur companion. Natives admises globalement plus lentes que le 27/10 (cancellation 929s -> 2174s).

r204 lot A 31/31 PASS. Lot B (fixtures sur C: obligatoires : sur D: `CfGetSyncRootInfoByHandle` echoue, premier lancement invalide sans effet) : frontier PASS 399s, export PASS 303s, native9 PASS 596s (budget 900 valide) ; recovery FAIL ligne 246 = test obsolete (budget `committed` d'un Restore non-C14 termine signale `Reservation` depuis l'ajout de ce motif admis apres r21 ; fixture verifiee : seul ce budget reste) -> test corrige (attend Reservation puis acces Exclusive qui solde) ; cancellation FAIL = timeout enfant 1200s en phase 6/10 (r22 27/10 : 929s ; ralentissement general des validations admises, aussi native9) -> budget 2400/2460. Test `restore_history::canonical_history...` corrige (fixture `inherited_segments` = vrai engagement vide). Ces 3 changements sont TESTS seulement ; r205 = r204 + ces tests, a compiler apres lot C. Lot C (C14 plain/companion avec phases Restore) en cours.

Restore C14 local (sources Capture post-Ready et Inverse local ; Version/preparation fermes) integre. Revue independante Claude (lecture seule) : 1 High corrige (id d'operation non canonique aurait cree un budget `restore:` illisible par le lecteur -> store bloque ; preflight refuse maintenant avant tout effet), Low corriges (clone snapshot seulement si journaux, message `shared quota reservation pending` pour budget etranger, dispatch C14 calcule une fois sans SQL). Low acceptes : borne journal conservatrice (+9 octets/temoin), cout lecteur (journaux <=64MiB re-decodes).

r203b (avant correctifs revue, non-natives) : 27/28 PASS ; echec `import_validation::restore_history::tests::canonical_history_includes_restore_only_preimage_and_shared_capture_operations` (`import ancestry commitment differs`) PREEXISTANT, identique sur EXE r202 gele, hors perimetre, a investiguer separement.

r204 gele `.layrs-local/validation/c14-local-restore-r204` EXE SHA `fc2a0a5021f13fb31458557f0ce5bbf58f0189585913d4d94159d8c494a04a91`, sources modifiees copiees dans `source/`. Runner generique `D:/Layrs/tmp/run-recipes.py <out> <recipes.json> [--keep-going]`. Lots : A 31 non-natives (r200 + Restore/historique + 4 nouveaux tests unitaires), B 5 natives non-C14 (recovery 780, frontier 540, export 660, cancellation 1260, native9 960), C 2 natives C14 plain/companion 3660 (phases Capture puis nouvelles phases Restore `c14_denial_start_tests/restore.rs` : garde source, apply+dedup, Inverse conserve octets jamais captures, prejournal Staged->cancel->retry, PathQuarantined->cancel refuse->resume, AfterCommit->replay, reouverture).

## Point precedent - r202 resume1 termine 2/4 natives ; Restore C14 local en source (Claude seul)

r202 bilan natif : plain C14 PASS 1386.9s (session55533) ; companion C14 PASS 1368.8s (resume1) ; native9 `managed_admitted_ordinary_edit_capture_restore_without_source` FAIL101 = timeout enfant 480s (pas d'assertion) : 1re restore a ~420s, 2e (Inverse) a ~459s, tuee pendant staging. Les phases P1 (CAS epoch+1, echec BeforeCommit, edit, refus, double cancel) ont allonge le test. Restore cancellation native non executee. Disposition `root-runtime-failure-disposition.json`, resultats resume1 SHA `e90a35b4ac1f1a384f14eac2c15717ee74e3ded40391811fb1bcaa25656ab613`. Fixture recipe-03 preservee.

Correction prevue r203 : budget native9 child480->900 / outer540->960 (test seulement). A requalifier sur r203 : native9 + restore cancellation + non-natives touchees.

Restore C14 local (design `D:/Layrs/tmp/c14-local-restore-design-r203.md`) en integration SOURCE, non compile : storage.rs (append force, visit avec base), format.rs (AccessPurpose::LocalRestore, non-C14 -> General), admitted_private_io.rs (handles restore/recovery/cancel + preflight_local_restore), content_scope.rs (denial_ready, preflight), restore.rs (garde source C14, resulting_changes), engine.rs (LocalRestorePlan, borne journal, financement avant reserve, settle C14 apres finish), nouveau read/c14_restore.rs + c14_capture.rs (historique/pending/prejournal/claims restore, budgets restore), retention.rs (MAX_RESTORE_BUDGET_BYTES, require_restore_staging). Restent : recovery.rs/cancellation.rs/admitted.rs, tests, compilation.

## Point precedent - r202 interrompu par redemarrage hote, reprise resume1 en cours (Claude seul)

Session55533 tuee par redemarrage manuel menu Demarrer 2026-11-03 01:24:51 (System User32 1074), puis redemarrage Windows Update 11:06, boot 11:08. Avant interruption : native plain C14 **PASS** sur r202 (1386.905s, log `ok. 1 passed`). Companion C14 interrompu en phase `c14-local-capture-failure-cancellation` START, sans verdict (ni PASS ni FAIL) ; fixture `recipe-02` preservee non reouverte. Deux natives jamais lancees. Disposition `root-runtime-interruption-disposition.json` SHA `6826c742fe6248d41d10a69d10945fe5fb040e7d6cf5f38b69c379c76ddc7cb5`. Arbre de travail 746/746 identique au gel r202.

Reprise : `D:/Layrs/tmp/provider-c14-local-capture-run-r202-resume1.py r202-resume1`, meme EXE r202 gele, trois recettes restantes (companion, Ordinary edit/capture/restore, restore cancellation) en serie, nouveau parent `layrs-provider-c14-content-r202-resume1`, arret premier echec, SetThreadExecutionState anti-veille. Resultats `c14-content-results-resume1.json`, logs `*.resume1.log`. Apres terminaison seulement : `D:/Layrs/tmp/summarize-c14-local-capture-r202-resume1.py` exit0 ou exit1. Ne pas compiler ni lancer d'autre runtime pendant. Ne pas redemarrer l'hote (~1h30 max).

Prochain inchange : qualification r202, puis Restore local Capture/Inverse (voir investigation ci-dessous).

## Point precedent - r200 termine, quatre natives r202 en cours

r200 termine exit1 : 21/21 recettes non natives PASS. Premiere native plain C14 FAIL101 apres 783.691s sur une assertion de diagnostic : le terminal physique refuse les racines ajoutees par Capture avec `exact protection owner has additional roots`, avant le controle d'inventaire prive attendu par le test. Les deux captures, deduplications et lectures publiques precedentes ont atteint leurs assertions ; aucun PASS de la native complete. Trois natives non executees. Integrite et resultats verifies, SHA resultats `6516853a58d392089dbe193c4b33cc670983ab4555fef499c395a676a8ce5afc`. Fixture echouee preservee sans reouverture.

r201 compilation PASS 60s, 85 warnings. r202 contient la correction de cette assertion et P1 annulation Capture legacy (token lie au head courant, C14 toujours lie au Ready). 746 sources, quatre fichiers changes depuis r200. EXE `04a4d740011593a450e19a0a8aeaf10a1dd41fac1c09287a8ccb1b5a879d9545`, CodeView age190 ; host `07355159850dd65c785fb4db45fd9e69dd6b06e4233ef2ebb5f7f7494200f296` ; release ROOT `450b4cf6e2ffda63b41620f7833cef03bc70f2deb8f132ad49a1aa7f9eb13c95`.

Session55533 ACTIVE : `D:/Layrs/tmp/provider-c14-local-capture-run-r202.py r202`, quatre natives restantes en serie sur nouvelles fixtures C:, arret au premier echec. Premiere native en phase publication au dernier controle. Ne pas compiler ni lancer d'autre runtime. Apres terminaison seulement, executer `D:/Layrs/tmp/summarize-c14-local-capture-r202.py` avec exit0 ou exit1 ; script verifie (sha256, quatre recettes et session55533). Les 21 PASS r200 restent lies a leur artefact, sans revendication sur r202. P1 compile mais qualification native encore attendue.

Tous sous-agents termines. Investigation Restore C14 conservee ci-dessous ; aucun code Restore commence. Prochain : resultat r202, correction d'un echec concret ou qualification du lot, puis Restore local Capture/Inverse avec prefinancement forced-append et conservation des preimages. Objectif global non termine ; aucune pause demandee.

## Point courant - r20021testsPASS, premiere native active ; P1source pret

Session37414 toujours ACTIVE :21/21 non-nativesPASS (3nouvellesSQL,13admission,5Capturelegacy), premiere native plainC14 commencee phase c14-plain-publication. Quatre natives restent ; nepascompiler ni lancerautreruntime. Postrun D:/Layrs/tmp/summarize-c14-local-capture-r200.py avec exit0ou1 seulementapresterminal. Aucun succesnativeC14Capture revendique.

Tous agentsnatifs termines. P1legacyCapturecancel sourceintegree/rootrelue/formatPASS mais NON compilee/NONexecutee. Exactement3sources divergentdugel : retention/cancellation.rs, format/recovery/admitted.rs, session/managed_admission_tests.rs. Token prive DenialReady/LegacyHead ; legacy lieexactcanonicalcurrenthead apres inspect+requiremutable, C14writerresteferme. Native9 fait CASepoch+1 puis failureBeforeCommit/edit/refus/cancel/replay etsuiteCaptureRestore. Patch/hashes D:/Layrs/tmp/legacy-capture-cancel-pending-r200.{json,patch}. Nativechild480inchangé, duréeasurveiller ; Ordinary10nonqualifie. Compiler apresfinr200 ettraiterfailureprioritaire.

InvestigationRestoreC14 SOURCEONLYterminee surgelr200 : prochainepetitetranche RestoreSource::Capture localeC seq>=Ready etInverse local, avecresume/cancelprejournal. Version/preparationrestefermee. Nécessite LocalRestorepurpose, readerauthentifiepending/history/preimages before+after (dirtyBnoncapture doitresterretained), financementforcedappend AVANTreserve/staging/FS (Restore ne dedupjamais), budget/claim/stageRestorebranchesfermees, generationReadyfixe/importimmuable. Fichiers ROOTformat/scopes/restoreengine ; interpreterc14_capture+nouv c14_restore ; recoveryengine/recoveryadmitted/retentioncancel ; nativesplaincompanion.Aucun codeRestore commencé. Objectifnonpause/nontermine ; repriseautomatiquefinruntime.


## Point courant - r200 qualification active apres revue r198

Opus r198 termine : aucun nouveau defaut majeur protocoleC14 ; D1Low erreurapresCOMMIT possible sur contention settlement, documentee sansavalererreurs ; P1Medium legacyCapturecancel preexistant a traiter ; H1durcissement applique(ROOT) finish_budget avantvalidationfinale dansmemetx. r199 compilePASS12.17s85warnings. Gelr200746sources,18delta r194 et2petitsfichiersdelta r198(capture.rs doc etstorage.rs ordre). HostSHA ccc74565d82810122f26ae7fa2b17455a8c1e2baf9cb7160780bba71150b4b2b ; EXEage189 SHA7ae7501862c00fdc0ff5ce88b41175508a5a4c57c9b45dffc2adfe7a09e75580. ReleasetestsROOT a3ea52ced28b2c15e7d39a5d7131c681e57b9358072eda5771b0bc9d598e19cf ; runner D:/Layrs/tmp/provider-c14-local-capture-run-r200.py SHA9e419d7517c7038a0d928d9888d537b8f21f50ce44a6e5f9414eee6e0d2a6dfb.

Session37414 ACTIVE :25recettes serialfreshC stopfirstfailure,2batchSQLPASS au dernierpoint. Ne pas compiler/executerautrependant. Verifierpostrun prepare D:/Layrs/tmp/summarize-c14-local-capture-r200.py ARGexit0ou1, a lancerseulementapresterminal. Aucun gelr196/r198execute.

Worker natif c14_capture_cancellation relance SOURCEONLY surP1legacy : retention/cancellation, recoveryadmitted, sharedcancellation siutile etmanaged_admission_tests. Source live peut divergerdugelruntime ; nepastransfererresultats. Pas de fixtures/gels/logs touchesparworker ; nepascompileravantfinr200. P1 legacyhead doit etre actuel, C14 resteReadyfixe. Suite :resultatr200prioritaire, corrigerfailuresconcrets puisintegrerP1. AutonomieRestoreC14/reexport/produitL0-L9 restants ; objectifnontermine.


## Point courant - corrections Capture r198 compilees, revue annulation a lancer

Revue Opus r196 NON ACCEPTE, jamais execute : open generalwriter refusait C14 ; terminalnegative budgetcommitted ; borne apresreserve ; coutquadratique ; cancellation changedimage absente. ROOT corrige openpurpose RO seulementC14/legacy preserve, settlement succes et terminalnegative. Workers integres : preflight exact avantreserve, cap64MiB/100k avec dedup, batch snapshot1x/SQLcursor1x, orphelins seulementmainWRITE+budgetactive ; annulationCapture tokenlieadmission/identity/operation, protocolefamille exact, nobudgetreplayRO. Natives ajoutent BeforeCommit changeimage cancel/retry et AfterCommit boundcancel/replay. Followup D:/Layrs/tmp/c14-local-capture-root-followup-r196.md. F7 reste internalAccessconvention, testsOS/crash/Tree/matricecomplete absents.

r197 compileFAIL1 importchildprive ; ROOT utilise wrapperowned_test existant et rend negatifgeneration discriminant. r197b PASS55.20s85warnings. Gel r198746sources,18delta r194 et12delta r196 ; EXEage188 SHAfc666cf3f51f15cab6f7f8fe8ba7181c1d7c45f073e9b4e25208c47811b2eaff ; hostSHAb5f6dd0cef567a9d58027f86854492a622325586e0f31aa5686d0ea437742782. 25candidates=21non-native+4natives ; aucune execution/release. Runner D:/Layrs/tmp/provider-c14-local-capture-run-r198.py SHA63ed9b30700e040551be22d7173cf07aabb58c3ee1e5faec3e49de4cf7689d10, lieignoreinventory+patches maintenant. DeuxnativesC14incluentancienhelperterminal avantCapture, legacyCaptureRestore et cancellationRestore inclus.

Brief D:/Layrs/tmp/c14-local-capture-cancellation-review-brief-r198.md : revueOpus focalisee nouveauprotocoleannulation etprefunding/batch. Tous workers finis, aucunruntime/buildactif. AttendreOpus siquota, aucunremplacement. Prochain integrerrevue, dispositionROOT puis serialfreshC. Goal/programme non termine. r19443recettesagregees reste evidence precedente.


## Point courant - r196 compile/gele, revue independante a lancer

r195 compile FAIL5 imports test absents ; ROOT corrige imports format/SessionMode et largeur64 des empreintes retention dans lecteur neuf (content IDs71 inchanges). r195b compile PASS57.02s86warnings. Aucun test execute. Gel r196745sources,12fichiersdelta r194, EXEage187 SHA3117713d98744685eb06c45acf819c9bccce10cc450e60370b364db6ac2e6d75. HostSHA837130a3462697155cfb5914f4a3b4bd883ee342d2ef7ca48294a5ac7f438ba7. 21recettes candidates,18 SQL/Capture ordinaires et3natives ; runner prepare sans release donc inexecutable.

Brief D:/Layrs/tmp/c14-local-capture-review-brief-r196.md. Tous workers termines. Aucun build/runtime actif. Revue Opus seulement, attendre quota si limite. Points ouverts explicites : annulation changed-image apres Capture echouee non raccordee (cancel admis reste Restore), TreeReserved/matrice corruption/OSclose/crash non qualifies. Pas de global readiness, pas de pause/fin goal. Prochain revue, corrections bornees puis releaseROOT et execution serie freshC.


## Point courant - Capture locale C14 en integration, pas encore compilee

r194 4/4 PASS et postrun verifies ; bilan precedent 37 DATA/SQL r190 +2 admissions r192 +4 natives r194 conserve. Aucun runtime actif.

Lot suivant borne : Capture locale a generation Ready fixe, acces ferme LocalCapture ; state et lecteurs de Capture utilisent le scope admis authentifie. Generic Exclusive, Restore, preparation, observations et re-export restent refuses. Lecteur generique separe import immuable et suffixe Capture ; terminal physique garde son inventaire exact sans suffixe. Aucun changement de schema. Worker lecteur termine source seulement, worker natives plain/companion en cours. ROOT integre acces, scopes idle et state. Aucune compilation ni execution de ce nouveau lot ; revue Opus independante a lancer sur gel integre, sans substitution modele.


## Point courant - lot admission/reprises termine, autonomie mutable suivante

r194 termine exit0 et postrun `D:/Layrs/tmp/summarize-c14-ack-r194.py` PASS :4/4natives,743sources/EXE/PDB/bindings/logs verifies. ResultatsSHA `64a14eeb7015a2441101d624427ac0c94e09dde8644bcf8987a1240d6ab62805`. Plain4ACK239.327s ; Provider9observationAPI200.282s ; legacyadmissionACK141.723s ; C13Reserved391.631s. Disposition `r194/root-runtime-disposition.json`.

Bilan du lot :37 DATA/SQL PASS r190 +2 admissionsC14 plain/companion PASS r192 +4 natives PASS r194 =43 recettes distinctes qualifiees sur leurs artefacts respectifs, PAS une unique execution sur r194. Regression observations legacy corrigee et exercee sur Provider9/Ordinary10 SQL et API Provider9 ; admissions C14 via vrai adaptateur session, precancel et reprise holder, terminal physique sans B ; quatre phases ACK plain et regression C13Reserved. Les echecs r190/r192 restent conserves et expliques. Aucun runtime/build actif.

Prochaine tranche : autonomie locale mutable C14 apres admission (Capture/Restore), re-export et CLI separes. Investigateur natif `c14_mutable_admission_scope` en lecture seule pour preciser partage inventaire immuable/tailmutable, retention companions/denied, budgets et fichiers/interfaces. Ne pas simplement retirer les gardes read-only/terminal. Admission C13 avec companion mais sans denial reste a distinguer et non implicitement acquise. Limites crash/OSclose/matrice negatives restent distinctes. Programme/goal non termine et non pause ; preference Opus conservee.

## Point courant - admission C14 qualifiee r192, ACK corrige r194 actif

r192 termine exit1 : admissions C14 plain PASS514.812s et companion PASS488.013s, via vrai adaptateur session/precancel/retryholder puis quarantaine B et terminal physique sans B. Stack overflow non reproduite sur ces deux parcours ; lieu du crash r190 demeure inconnu. Integrite743sources/artefacts/bindings/logs verifiee par `D:/Layrs/tmp/record-c14-r192-partial.py`, disposition `r192/root-runtime-partial-disposition.json`, resultatsSHA `58222a95d2eb3aa62fcff6765dfdc407347baef02e526c98399087a178cd6dd2`.

Troisieme native plain ACK FAIL101 apres261.189s : observateur test demandait transaction_id a projection_provider_transitions dont PK est id. Erreur SQL avalee en false, donc aucune injection, puis expected-error assertion. ROOT corrige uniquement deux requetes du test (observer + gap reader) et ajoute phase au diagnostic ; aucune source produit changee. Fixtures precedentes intactes, aucune reouverture/reutilisation. Trois natives suivantes non executees.

r193 compile PASS19.40s86warnings. Gel r194743sources, un seul fichier delta content_plain.rs, EXEage186 SHA `5ae4771b351151d37171a2bcde39ca65789a6ae2fb313f027315511b7352184a`; host `6f6532e7aab2cae0cc4130386158efd80b7323851a03b8f314b6382c8309ed22`. Release tests `e69d3ff81a838166abb546eb91163f55dc470a462345b321d0dec453a009763f`; runner `D:/Layrs/tmp/provider-c14-ack-run-r194.py` SHA `ad9d4f5f7acbc0973c8927155c76265f7b155a25c4ffe6bf0d72ca18a811eea8`, session49702 ACTIVE. Quatre natives restantes serie/freshC/stopfirstfail. Ne pas reexecuter les deux admissions inchangees ni37DATA/SQL sans nouvelle raison ; leurs preuves restent liees r192/r190.

Prochain : finr194, integrite/results et bilan agrege sans pretendre memeEXE. Aucun agent actif, aucun build pendant runner. Preference utilisateur confirmee : attendreOpus en cas de quota, aucun remplacement Astra pour revue sans accord. Autonomie C14 mutable/re-export/callersCLI et programme L0-L9 restent ouverts ; goal ni pause ni termine.

## Point courant - r190 overflow, r192 natives en cours

r190 termine runner exit1 :37 DATA/SQL PASS, premiere native admission plain FAIL101 apres848.442s, enfant0xc00000fd stack overflow. Aucune assertion metier ni phase precise dans le log ; cinq natives non executees. Fixture echouee conservee intacte, jamais reouverte/reutilisee. Integrite743sources/EXE/PDB/bindings verifiee dans `r190/root-runtime-failure-disposition.json` ; resultatsSHA `2f79cd897668d43e17d409f79a0dcac3b26b701f0a69b759fd30c95eee16a30d`.

Investigateur natif en lecture source termine : pas de cycle trouve dans metadata_basis_floor/prefix/historical. ROOT a inspecte unwindPE et appels directs : gros TerminalSnapshot par valeur via deux scopesSQL. Correction bornee : Box construite dans callback interne, propagee jusqu'au holder ; validations/ordre/pins inchanges. Sous-chemin statique inspecte895352->248168octets, open125688->23880 ; pas un maximum global ni preuve du lieu du crash. Ajout PhaseTimer dans nativeplain/admission/terminal. Default stack et delais inchanges. Adaptateur session C14 du worker egalement integre, precancel Validating/0writer puis memeholder retry. Details `D:/Layrs/tmp/c14-admission-stack-followup-r190.md`.

r191 compile PASS. Gel r192743sources,4fichiersdelta r190, EXEage185 SHA `26d27d389a59885bd50740c91439cacff8b90c5b616363e5945299d17a9a5ca0`; host `47f8385751b956ef16ad9558af762a5369bb31b5a1835206bd57388290c1b8dd`. Release tests ROOT `ec43a4335633eedd6c6b760a98ba56122dd18be711a7de37d5f72aabae57d989`. Runner `D:/Layrs/tmp/provider-c14-admission-run-r192.py`, hash `99d86e9427fd87774c775ee76fd074bc0a64d54f02c20a7c3a312afb106549a5`, session85451 ACTIVE. Six natives serie/freshC/stopfirstfail ;37 DATA/SQL r190 conserves comme preuves r190, pas requalifies r192. Premiere nativeplain en cours ; aucun succes C14admission revendique.

Prochain : lire fin/logs phases r192. Si nouvelle overflow, localiser via derniers marqueurs puis changer hypothese ; aucune repetition identique. Si PASS, verifier gel/bindings/logs et poursuivre scope suivant. Aucun agent natif actif, aucun T3review actif ; pas de build tant que runner tourne. r18644/44PASS reste dernier lot integral termine. Goal ni pause ni termine.

## Point courant - r190 en qualification apres revue r187

Revue Opus r187 terminee NOT CLEAN : F1 High accepte (inventaire observations dans lecteur admis legacy). ROOT corrige avant runtime : comptage integral uniquement C14 ; base metadata Option calculee seulement C14 ; obsolete wrapper companion retire ; observer ACK sans panic avant close. Test SQL Provider9 ajoute, meme regression inseree dans Ordinary10, native Provider9 enrichie avec API publique observation/commit/reopen/replay. Trois corrections compilation precedentes conservees. Notes detaillees `D:/Layrs/tmp/c14-admission-root-followup-r187.md`. Rapport Opus original intact ; corrections ROOT auto-revues, aucune seconde revue independante revendiquee.

r189 compile PASS44.84s86warnings. Gel r190 : 743 sources, 28 fichiers delta r186 et exactement10 fichiers de corrections depuis r187, EXEage184. Host SHA256 `23358de3b0f33f1dada4028d223cda48a6da8417488e1130f877a73b8a510e5e`; EXE `2cfbdfa127ea895f57659a1f32ab41dc341f283d4a1d973802e58948edd2424a`. Release tests ROOT `85dd58a4ae940f734e100b74e41bbbf61c0ed8975859e06a41447677aa7edb97`, runner `78b487a08c0b04be2958becd42328dda3bc605c1ddb31f08775104c1100972e5`.

43 tests en serie sur fixtures C: neuves, session91001 ; arret premier echec. 37 DATA/SQL puis 6 natives, admission plain/companion d'abord. Dernier point :37/37 DATA/SQL PASS, premiere native admission plain active ; ne pas compiler tant que runner tourne. Script postrun `D:/Layrs/tmp/summarize-c14-admission-r190.py` prepare NON execute. r188 jamais execute. r18644/44PASS reste dernier lot complet.

Agent natif `c14_session_admission_adapter` termine : seulement session.rs et c14_denial_start_tests/admission.rs. Adaptateur explicite de session C14 et usage dans les natives, plus refusal precancel Validating/zero-writer/retry holder. ROOT a relu/integré : enum ferme, ancien Legacy inchange, lifecycle/owner/prepared/verification conserves jusqu'au retour, quarantaine inchangee. Rustfmt/check PASS ; aucune compilation ni runtime agent. Exactement ces deux fichiers live divergent du gel r190 ; patch et hashes `D:/Layrs/tmp/c14-session-admission-adapter-pending.{json,patch}`. Compiler seulement apres finruntime et traiter les eventuels echecs d'abord. Limites : plans C14 anciens insuffisamment finances refuses sans resizing ; matrice admission ACK/crash et negatives terminal encore ouvertes ; autonomie mutable/re-export/callers CLI et L0-L9 non termines. Goal ni pause ni termine.

## Point courant - r188 compile et fige, revue r187 en cours

r186 est termine et verifie : 44/44 PASS. r187b a echoue sur trois erreurs de compilation ; r187c les corrige dans deux fichiers et compile avec exit0 (88 warnings, 1m28s). Corrections exactes : import BTreeSet, conversion d'erreur par `?`, retour unitaire apres controle de capacite. JSON et patch `D:/Layrs/tmp/c14-admission-compile-corrections-r187c.*` ; aucun changement au gel source r187 examine par Opus.

Gel r188 : `.layrs-local/validation/provider-c13-history-20261102-r188`, 743 sources, 24 fichiers modifies depuis r186. Comparaison avec r187 : exactement les deux fichiers de corrections compilation. Host SHA256 `04dd4cdc9126bb05aa0a56f2689756c84cc2858d4d745e4751d729c2ad082910`, EXE `1dbddc33b343177e1fa3a8d3b4ce28ddf043af2ff27df83639db51ea5a97e973`, CodeView age183. `compile-source-binding.json` lie gel source, corrections et log de build ; cette liaison reste une attestation ROOT.

Selection r188 preparee : 35 DATA/SQL et 5 natives, toutes presentes dans l'inventaire compile. Trois nouvelles natives C14 (reprises plain, admission plain, admission companion), regression admission legacy et C13 Reserved/reouverture. Les deux natives d'admission incluent le parcours de publication complet. Candidate SHA256 `30e52b21c830875dc37ced59f22f368c785cd6f2ee72020249def81711d2abf3`. Runner `D:/Layrs/tmp/provider-c14-admission-run-r188.py`, SHA256 `ad867d2d13f04dff4e5bfb65767c8db6c307430fc2b6f26b5d241a6553741873`.

Aucune release r188, aucun test r188 execute. La revue Opus source r187 est toujours en cours selon le dernier retour ; ne pas la poller. Sa completion reveillera le thread. Prochaine action : traiter ses constats, lier explicitement les trois corrections compilation, puis release ROOT et execution serie sur nouvelles fixtures. Aucun build/runtime actif. Admission C14 runtime, callers produit et suite L0-L9 restent non termines. Goal ni pause ni termine.

## Point courant - r186 44/44 PASS, compilation admission r187b

Runner r186 termine exit0. Postrun verifier D:/Layrs/tmp/summarize-c14-reopen-parity-r186.py PASS: 740sources, EXE/PDB, bindingsreview/release/runner et logs exacts. Resultats SHA256 0b0ebfbb37fafb6ea103348934dd461441f7b070fd02e2a00244d27e3c1e9bc0. C14plain489.487s, companion497.274s, C13ACK835.394s, C13Reserved+reouverture sanssource753.22s. Aucun resultat transfere au nouveau lot.
Compilation cargo test -p layrs-store-local --lib --no-run r187b lancee apres finruntime, log D:/Layrs/tmp/c14-admission-build-r187b.log, session77108. Revue Opus source r187 task ca54c4ac-02a6-4927-b26f-6a7d0c25a8e5 en cours selon dernier outil; ne pas poll, finreveillera thread. Correctifs compilation eventuels a enregistrer separ?ment du gel source743/delta24.


## Point courant - r186 runtime actif, admission C14 r187 source prebuild

Runner r186 session28120 actif: 42/44 PASS au dernier point, toutes DATA/SQL, deux natives C14 publication sans/avec companion PASS (489.487s/497.274s); les deux C13 restent a finir. Ne pas compiler en parallele. Aucun r186 runtime-disposition final tant que runner non termine. Script postrun D:/Layrs/tmp/summarize-c14-reopen-parity-r186.py prepare mais NON execute.
Nouveau lot source r187 integre, NON compile/non execute: admission explicite C14 Provider8->9, lecteur frozen documents/companions reel, metadatafloor transporte, admission peak avant Planned, terminal physique source-independent readonly, gardes legacy/mutation/export fermees. 24 fichiers delta / 743 sources dans .layrs-local/validation/provider-c14-admission-source-20261102-r187. Disposition SHA545ff97e564f978a98b65c76554c15c038765ed2c1bf30c861c0014874adcc9b.
Trois nouvelles natives: matrice4ACK plain, admission plain et companion avec quarantaine seule DB B puis fresh C9 terminal; testbudget DATA ajoute. Tous agents natifs termines. Revue Opus SOURCE ONLY prebuild a lancer via brief D:/Layrs/tmp/c14-admission-terminal-review-brief-r187.md, rapportsuniques c14-admission-terminal-critical-review-r187-opus.json/.md. Ne pas transferer ancienEXE/qualification a ce lot. Prochain: resultat revue, finr186/postcheck, compile r187b puis freezeartifacts r188+selection exacte et releaseROOT apres correction. Pas de commit/push ni arretgoal.


## Point courant - revue r182 terminee, corrections de parite r186

Opus a repris et termine sa revue; aucun changement de modele. Les deux exemples de cibles immuables changees sont refuses par le normaliseur. Deux variantes admissibles traitees avant planning: manifeste alternatif absent des inventaires autonomes, et ancien chemin Reserved devenu lisible sans fichier dans image B. ROOT corrige les deux fichiers transition_reconcile/companions.rs et tests/companions.rs; trois nouvelles recettes DATA, aucun affaiblissement lecteur. Detail: D:/Layrs/tmp/c14-plain-denial-root-followup-r182.md. Rapports Opus conserves intacts.
r183 compile PASS49.44s, r185 PASS34.78s86warnings. r182/r184 non executes, aucun resultat transfere. Qualification r186 en preparation sur gel neuf; r18011/11PASS reste derniere preuve du parcours companion. Admission C14, callers publics et suite L0-L9 restent a faire. Goal non pause ni termine.


## Point courant - r182 revue Opus interrompue par quota

Opus task51a8f906-b67b-4631-92ca-3e656ee8e7bb failed sur quota5h; reset annonce20:50Paris2026-11-02. Revue partiellement lue, AUCUN rapport final, aucune release r182, aucun test r182 lance. Pas de remplacement modele sans accord.
Question utilisateur async en attente : autoriser GPT-6-Astra pour finir revue privee r182 OU attendreOpus. TacheOpus originale conservee pour repriseautomatique possible; si remplacee, la cancel explicitement et sortiesuniques obligatoires.
ROOT a suivi une piste unionlive/autonome : changement de cibleFileId deja refuse parnormalizer; representationManifestId alternative pour memeFileId demeure hypothese a examiner. Notes `D:/Layrs/tmp/c14-plain-denial-root-followup-r182.md`. Aucun produit modifie/build/SQL/natif pendantcefollowup. Sources/gels restent r182compile740/20/21. r18011/11PASS conserve. Pas de pause ni cloturegoal.

## Point courant - r182 compile, revue C14 sans companion a lancer

r18011/11PASS conserve (resultats ad2d9dcd0b873fddf50eb60d9f7a1323af2bc9d6a53644096605569ad31784c9). Aucun runtime actuel.
r181 FAIL101 unique E0308 return decode driverDenial, corrige Ok(...?). r181b compilePASS64s86warnings Cargo0 session28309 terminee.
Gel r182740sources/20fichiersdelta dont3nouveaux; EXEage180. Host `c827d3a1655b8e8320d28c8bc54881156b33a5fe91aee265cb5a4934547b7ed4`; EXE `2428219ad01ded171c3979ef0cc884f2e055588d8a84c672d0526452d2e82c2d`.
Route closed commit_denial+DenialPins sans s/header, ReadyLive/RetainedDenial explicite, vrais snapshots/selection/corpsC, imageB et fundingd. Metadata prospectif inclut publicationB sans s. Native ContentPlain complete puis reouverture physique apres liberationSession. T2 couts divergents ajoutes au natifCompanion existant. Tous workers SOURCE STABLE et termines.
21recettes preparees NON executees, aucune release r182. Candidate `80e36eed58ae3f9d85630986d145e7836eee8224c92dcae06b89ceabd63924f2`; runner `f3f3545310c10b9e5493b9a5c8dfdd35f58998aa45c0e475b53524e64433abfc`.
Brief D:/Layrs/tmp/c14-plain-denial-review-brief-r182.md; sorties uniques c14-plain-denial-critical-review-r182-opus.json/.md. Prochaine action revueOpus, traiter defauts concrets sans nouveaux cyclesdelegues pour petites corrections, puis releaseROOT et qualification serie sur fixturesfraiches.
Limites : absence callerproduction, admissionC14 reste refusee, ACKmatrixv2/freshowner incomplets, aucun L0-L9 completion. Pas de pause/fin du goal demandee ni appliquee.

## Point courant — r178 terminé, préfinancement complet en intégration

**r178 : 15/15 PASS**, session4327 terminée exit0. Douze DATA/SQL, native C14
complète313,283s, C13 reprise ACK688,396s et unchanged Reserved403,942s.
Intégrité postrun vérifiée :735sources gelées exactes, EXE/PDB, liens de release
et logs. Résultats SHA256
`fd50e20ddff8498f5f77bcbaf9e881b7467f44fd0d8eb5baa64b1db83560805b`.
Disposition `r178/root-runtime-disposition.json`. Le parcours C14 qualifié est
celui **avec companion réel** ; pas de route C14 sans companion, admission
réussie ou autonomie physique C14 revendiquée. r176 FAIL reste conservé.

Lot suivant NON compilé : ProspectiveCompanionCost DATA dans source_companion
(worker c14_source_receipt_v2), borne du reçu64KiB avant B et du financement s
sans faux Marker/ExactBinding/Ready. ROOT raccorde le coût dans denial_plan,
start/staging, helper shared source_denial, opener content et native. Préflight
U+R+absent_d+q+s ; ancien None refuse un plan avec companion sans coût fermé.
Native ajoutée au parcours existant : seuil moins1 refuse avant B, seuil exact,
ACK famille puis reprise sans double d, Planned durable sous quota réduit sans
writer, actual publication s borné par coût retenu initialement. Le runner r178
était gelé et n’a exécuté aucun de ces nouveaux edits. Aucun build/runtime actif.
Attendre SOURCE STABLE du worker avant compilation r179 ; nouvelle revue
critique après intégration du calcul prospectif, puis qualification ciblée.

## Point courant — correction Ready r178 en qualification

**Mise à jour r178 : native C14 complète PASS313,283s.** Les douze régressions
courtes passent aussi. Ready privé/famille C, B Published, replay0writer,
corps/reçus v2 exacts, corruptions corps absente/substituée/extra refusées sous
rollback, admission SQL sans mutation et admission physique Validating0writer
sont réellement exercés. Session4327 poursuit les deux C13 ; aucun résultat
C13 r178 revendiqué avant leur fin. L’ancienne mention « pas encore C14
qualifié » ci-dessous est remplacée par cette preuve bornée, sans admission
réussie/autonomie physique C14 ni clôture globale.

r176 terminé exit1 : **27 PASS**, native C14 FAIL101 après218,034s à
`Ready data snapshot or capture commitments differ`, deux C13 non lancées.
Échec et fixture conservés, jamais rouverts. Diagnostic ROOT : C14 engage
la sélection fraîche, alors que Ready DATA et suffixe comparent encore celle
de la source ; garde denied inconditionnelle également présente.
Trois fichiers corrigés : Ready DATA utilise la sélection du corps scellé,
suffixe recalcule le digest sélection/génération/snapshot frais en v2, mode
companion v2 reconstruit le corps complet depuis la vraie observation et exige
sa liaison exacte. Le corps SQL et son namespace restent requis ; v1 inchangé.

r177 erreur de compilation sur `.record()` d’un record déjà décodé, corrigée.
r177b PASS20,29s Cargo0,85warnings. Gel r178735sources, trois fichiers changés,
âge178, host `4391b20947a49cacde54002a8df35c09b9d6cc3eb18ce0becddffd6af150f58f`.
EXE `992e48c43a9eb5a3415b65aed9be162ab43af31f8bbc118a210eb18bcad4845b`.
ROOT review `tmp/c14-ready-selection-root-review-r178.md`; release
`33af6970756b94df5475cbc30e652a6cc4f7674656d640fad2e877adcbd606dd`.
**Session4327 en cours** : douze régressions Ready/prefix/DATA PASS puis native
C14 en cours ; deux natives C13 à suivre. Aucun autre build/SQL/natif parallèle.
Runner `tmp/provider-c14-ready-selection-run-r178.py`, résultats dans
`r178/c14-content-results.json`. Pas encore de C14 Ready/Published qualifié.

## Point courant — revue r174 corrigée, qualification r176 en cours

Opus r174 terminé : F1 moyen (admission v2 refusée après ouverture writer),
F2 faible (corps lu avant rôle/état/FORMAT), lacunes de tests séparées. Rapport
`tmp/c14-content-coverage-critical-review-r174-opus.json`, SHA256
`8679f703967acec32f263c933cd357116c32fad1bde78780225d54877bbe4604`.
ROOT a corrigé directement ces petits findings, sans nouvelle délégation.
F1 : garde au début du commit physique, dans clean_admission et admit_ready
avant toute mutation/budget ; F2 : liens légers vérifiés avant corps.
Native réelle étendue : admission SQL et physique refusée sans mutation,
step Validating/0writer ; trois corruptions du corps Ready C sous savepoints
annulés, erreurs exactes et aucune réparation, full reader après rollback.

Compilation r175 PASS32,85s Cargo0,85warnings, session17448 terminée.
Gel r176 : 735 sources, cinq fichiers changés depuis r174, âge177.
Host `072d0b379bd5a3893887bcda468b12a68afb98cf17bcd29bfc814cf4d927265e`.
EXE `9f925945c5bc2ce1059d81f1e08999e78395843e460421c3d837fbb181ef6fdb`.
ROOT delta review `tmp/c14-content-guards-root-review-r176.md` ; release
`a5eb9ca0afd132e2a5ff7827a6eaf003f55766ea2f430541c7c615d6a7371f80`.
Runner `tmp/provider-c14-content-guards-run-r176.py`, **session61561 en cours** :
26 DATA/SQL PASS puis native Staging en cours au dernier point. 30 recettes
au total ; suivent C14 complet, C13 reprise ACK et C13 unchanged Reserved.
Pas de native C14 Ready/Publiée ni refus d’admission qualifié pour l’instant.
Ne pas exécuter un autre build/SQL/natif pendant cette session. Logs/results
dans `r176/c14-content-results.json`. Stop premier échec ; pas de fixture reuse.

## Point courant — 2 novembre, contenu C14 r174

Ce bloc remplace les états « en cours/non compilé » plus bas. **Compilation
r173b PASS38,05s**, Cargo0,85warnings, session42875 terminée. Gel r174 :
735 sources, 13 fichiers changés depuis r172, RSDS âge176. Host SHA256
`909b900fa9b069c6c4dbdfe708abaa36d3cfcf4f9bdf8118f5e69b55181be612`, EXE
`b1d8bb12b7d5b2d55bbe8b38e543ec68b9d4c22cfa312b9547771635dad90891`.
**Aucun test r174 exécuté, aucune release d’exécution encore émise.**

Intégration : opener et wrapper content C14 fermés, vrai anchor Session acquis
avant Planned, corps scellé revalidé contre SQL/observation, d conservé dans
le contexte famille complet, reçu v2 source et Ready, lectures protection B
strictes, inventaire et coût du corps courant dans Ready8. Les lecteurs
Admitted/Ordinary denial restent explicitement refusés. Nouvelle native réelle
begin→allocation→copie→Staging→C Ready privé/famille→B Published, puis replay
0writer ; README modifié devient Reserved dans cette seule observation.
Pas de succès runtime revendiqué sur cette nouvelle chaîne.

Brief : `tmp/c14-content-coverage-review-brief-r174.md`. Revue indépendante à
lancer sur ce gel avec sorties uniques `tmp/c14-content-coverage-critical-review-r174-opus.{json,md}`.
15 recettes candidates (12 DATA/SQL, Stage DATA receipt, nouveau parcours C14,
régression C13), runner préparé `tmp/provider-c14-content-coverage-run-r174.py`.
Après revue : ROOT corrige les findings concrets, recompile/gel nouveau si
nécessaire, puis release et qualification exacte sérialisée en fixtures fraîches.
Tous sous-agents natifs ont terminé ; aucun build/runtime actif.

Limites inchangées : d+q initial n’inclut pas s ; matrice ACK v2/corruption
Ready encore absente ; admission/autonomie physique/re-export, caller produit,
lease serveur et reste L0–L9 non terminés. r170 13/13 et r172 4/4 PASS restent
des preuves séparées. `get_goal` affiche toujours blocked ; ROOT n’a pas changé
ce statut et l’API disponible ne permet pas resume. Travail autorisé poursuivi.

## Point courant — 2 novembre, gel Staging r170

**r172 terminé : 4/4 PASS, session85322 exit0**. SQL CAS et agrégat PASS,
reprise BStaging/CCopied sous quota futur réduit PASS101,897s (un seul writer C),
refus de capacité avant B toujours PASS99,416s. Disposition dans
`r172/root-runtime-disposition.json`, release
`e635610619ef44629fec6e48bede9044f9d87b14b0d6e264077552497f5fce4b`.
Deux revues Opus limitées avaient repris automatiquement et écrit le même
rapport r170. Résolu sans modification de sources : première revue récupérée
EXACTEMENT depuis file_change T3, hash de925b4b…a093 rétabli au chemin original
et conservé sous `tmp/c14-staging-critical-review-r170-first.json`. Deuxième
conservée sous `...-second.json` hash f7142a2a…4c9a. Aucun HIGH/MEDIUM dans les
deux. Nouveaux résiduels du second : corruption du corps après Staging non
exercée et ancienne sélection ACK/recovery non transférée à r170 (explicitement).
À l'avenir, annuler même les anciennes tâches limitées avant remplacement,
et utiliser des sorties uniques pour chaque revue.

Travail suivant en cours, NON compilé/exécuté : source_companion reçoit une
variante fermée derive_with_coverage/planned_receipt_with_coverage et garde
legacy explicite ; Ready DATA reçoit new_companion_with_coverage. Le raccordement
content doit conserver vrai ProviderTransitionSource acquis AVANT Planned :
il prouve connexion/lifecycle et historique complet, non remplaçable par le
plan/union seuls. Caller peut le retenir séparément, puis le fournir au nouvel
opener C14 après libération de PendingDenialStart et conservation de l'image.
Ne pas reconstruire cet anchor après Planned ni supprimer son contrôle.

**Dernier état : r170 runner terminé exit0, 13/13 PASS**, y compris document
substitué et deux legacy. Résultats SHA256
`78a0d8eb0e324591ecf9f1a1c8c3f9e1486de05c381fbf6b2fbbc1545b053666` ; disposition
`r170/root-runtime-disposition.json`. **r171 compile PASS29,89s**, Cargo exit0,
85 warnings, log `tmp/c14-staging-gap-build-r171.log`. Aucun runtime/build actif.
**Gel r172** : 734 sources, 3 fichiers changés depuis r170, PDB age174.
Host `3d32dbbc3b174c2ff2f8fe2958c8484a228627ce3111cb01e7c78c13139fd1b7`, EXE
`02241d6c24fb24258011b7ff1d27539ffc8b69ca0d8332ae5290f933d3d8bd13`.
LOW1/2 corrigés en source : après BStaging durable, reprendre le seul CAS C
sans réadmettre q futur ; native ACK B sous quota réduit (un writer C, famille
et B inchangés) ajoutée. LOW3 : deux négatifs SQL du CAS et de ses bornes ajoutés.
Quatre recettes ciblées compilées, non exécutées. Brief
`tmp/c14-staging-gap-review-brief-r172.md`, runner préparé
`tmp/provider-c14-staging-gap-run-r172.py` (SHA256
`9ea9f1ec8a63800577b89733a955b5e26aea05667cf1afe459a41fe7ce55c537`).
Vérification ROOT des trois petites corrections, conformément à la règle T3
« Fix small review findings yourself; start no delegate for them ». Pas de
nouvelle revue indépendante revendiquée : celle d'Opus reste liée à r170.
Qualification ciblée des quatre recettes après décision ROOT, sans réexécution
générale des 13 anciens PASS. LOW4 reste ouvert pour content C14.

**Mise à jour après reprise automatique Opus : revue r170 terminée**, aucun
HIGH/MEDIUM. Quatre LOW : capacité future réimposée dans le gap BStaging/CCopied,
absence de son test sous quota réduit, négatifs preflight CAS absents, garde
legacy v2 seulement implicite (plausible, audit non exhaustif).
Rapport SHA256 `de925b4bef316a8d02f11c0cc2a51e87d3984e2fc88300d46f8aef2ba6f3a093`.
ROOT a renforcé le runner (host/PDB/review/13 candidats liés avant création de
fixtures), puis émis la release bornée
`8a311822cb0ca24267a960d5a85e1c75178fcddc218940e823c9b159c93c2998`.
**Runner r170 session89641 en cours**, aucune acceptation globale. Les huit
premières recettes sont PASS, notamment vrai Staging, ACK B/C, capacité avant B.
Attendre le résultat final et arrêter au premier échec comme prévu.

Modifications live post-gel NON compilées/exécutées : suppression du contrôle
prospectif q uniquement après BStaging durable, reprise native après ACK B sous
quota réduit, deux SQL négatifs du preflight CAS/agrégat. Le test d'agrégat utilise
du padding SQL explicitement synthétique pour le seul helper arithmétique,
jamais comme preuve source complète/native. Fixtures rollback et source relue.
Ne pas transférer les PASS r170 à ces changements. Le finding legacy plausible
reste ouvert ; il sera traité dans le raccordement C14 fermé avec gardes C13.
Tous agents natifs terminés. Notes de raccordement content dans
`tmp/c14-after-staging-integration-notes-r170.md` enrichies : d traverse déjà
le contexte famille exact, aucun double vecteur/double débit à introduire.

Cette section prévaut sur les notes historiques ci-dessous. **r168 est qualifié
sur ses 17 recettes : 17 PASS**, runner session78936 terminé exit0 après revue
Opus. Résultats `provider-c13-history-20261102-r168/c14-sql-results.json`, SHA256
`1e1d9c03043c6176e5216b83d0a3fde6037601b5ed8c8b93fe0d9065d38b4cc3`.
Revue `tmp/c14-capacity-critical-review-r168.json` : aucun HIGH/MEDIUM, 4 LOW.
Release ROOT `b187cbb02eef2ed3e851dd3b297cf19975922672c4ba02304d5943cd664c8f3a`.
Les deux recettes différées de r166 (ambigu/history) passent en r168.

Le delta suivant est compilé mais **pas exécuté**, et doit être revu avant
release runtime :

- `stage_denial` réel : SourceCopying/CCopied vers dual Staging, puis reprises
  BStaging/CCopied et dual-Staging sans writers inutiles. Source actuelle,
  corps, union, financement d et famille complète relus à chaque frontière.
  Refus C ahead et corps absent/substitué ; aucune réparation. Snapshot source
  retenu jusqu'au COMMIT/close/readback C. Aucun C reserve/companion/Ready.
- Préflight aggregate du CAS source avant B et sous writer ; contrôle d+q
  prospectif avant nouvelles écritures, omis pour dual-Staging déjà durable.
- R168-3 : Session du guard et Session d'allocation liées au vrai root source
  par registration encodée/catalog identity et revalidation, avant effets.
- Natives R1 enrichies : Planned durable sous quota réduit, parent remplacé
  sous le même chemin, guard/Session étrangers. SQL capacité : id/phase,
  autocommit, doublon opération et installations négatifs supplémentaires.
- Sept nouvelles natives Stage : succès, ACK B, ACK C, quota, C ahead,
  corps absent, corps substitué. Compteurs writers et full readbacks.

r169 a échoué sur la visibilité du nouveau helper ; corrigée. **r169b compile
PASS38,94s, Cargo exit0, 85 warnings**, log `tmp/c14-staging-build-r169b.log`.
**r170 : 734 sources, 11 chemins depuis r168, RSDS age173.** Host SHA256
`afbca5ade059f5d8d42d65de97aa636f3b6da866421efa5ca70380c977c74393` ; EXE
`2dd06a74d7d747c6772f670e496bf47a94e03db631b4755d50e646c82c0bbd36`.
Treize recettes ciblées dans `c14-staging-candidate-recipes.json` : sept Stage,
R1, SQL capacité, transition SQL, current coverage et deux legacy Staging.
Brief `tmp/c14-staging-review-brief-r170.md` ; runner préparé mais non exécuté
`tmp/provider-c14-staging-run-r170.py`. Pas de release runtime r170 à cette note.
Tous les agents natifs ont terminé ; aucun build/test actif.

La revue Opus r170 a été lancée puis a échoué immédiatement :
`Claude API rate limit reached. Try again later.` Aucun verdict reçu, aucun
résultat inventé ni release runtime. Tâche `713bb258-0c31-421a-a425-c62626ae8ef3`.
Le gel et le brief sont conservés pour une nouvelle revue quand le fournisseur
est disponible ; aucun autre modèle substitué sans instruction.

Après l'heure annoncée de reset (15h50 Paris), nouvelle tâche de revue
`61b08307-74ab-4961-864a-242c30def1f7` : même échec fournisseur immédiat.
Pas de nouvelle tentative en boucle. Revue r170 toujours absente. Vérification
ROOT finale : 734 sources vivantes et gelées concordantes ; runner syntaxe OK,
SHA256 `8b70700aebe19a7a6daa905196d960c980417301ec1112e538bdfd867f1e18ba`.
Préparation de la suite, lecture seule :
`D:/Layrs/tmp/c14-after-staging-integration-notes-r170.md` (receipt v2, d+q+s,
constructeur C14 fermé, gardes legacy maintenues). Aucun test r170 exécuté.

Suite : revue intégrée indépendante, correction de findings s'il y en a,
release ROOT puis qualification ciblée sur racines C fraîches, arrêt au premier
échec. Ensuite raccorder C14 content/protection/Ready/admission/autonomie et
le reste L0–L9. Le sizing companion B `s` reste distinct de d+q, sans faux proof.
Pas de qualification panne OS close/powerloss ni autorité serveur continue.
Pas de commit/push ; stash original conservé. Le suivi `get_goal` est apparu
`blocked` à la reprise du contexte ; aucun appel pause/complete/blocked n'a été
fait par ce tour et le travail autorisé continue (l'API ne permet pas resume).

## Travail parallèle après r166 — capacité prospective et refus recovery

La revue Opus du gel r166 est terminée, distincte des modifications suivantes du
worktree. Son runner (`tmp/provider-c14-copy-run-r166.py`, SHA256
`28b53a6914d12f8463514fd58065543a287fa0fcb5f5745df70209779876adc1`), sans release
à sa préparation, a reçu la décision ROOT après revue. Rapport JSON SHA256
`95f596754e85e2868d3f20fe8eb85341abfe780d2c0300e3479a7b7bd692c6c7`, release
`d742ec65c3b627af72884fe05496e4f788bc6d9d639626e31c8a178ac85b95ac`.
**Runner session14350 terminé exit0 : 36/36 recettes retenues PASS.** Preuve
`provider-c13-history-20261102-r166/c14-sql-results.json`. Deux explicitement
différées (négatif alias avec unwrap confirmé, histoire dont taille minimale non
garantie) : aucune des deux n'est PASS. Qualifie begin complet, allocation/FORMAT,
copie C/replay, altération Planned/image/preuve, legacy, ACK source/famille/C,
orphan release, conservation du Planned après perte owner et SQL sélectionné.
Ces résultats ne qualifient pas les nouveaux changements worktree.

- R1 partiel implémenté : helper SQL read-only lié au plan complet ; avec usage
  contenu U, réservations R, document B d et quota C q, vérifier U+R+d+q si B
  absent, U+R+q si B exact présent. Enveloppe physique R+d+q ou R+q sur famille
  et vrai parent C. `PendingDenialStart::prepare` retient désormais un véritable
  ExportParentGuard fourni par le caller, lié au chemin/identité du plan.
  Contrôles dans prepare, avant reçu dans begin, puis sous BEGIN IMMEDIATE avant
  B. `require_retained`, recovery et reconnaissance du Planned durable restent
  sans cette exigence prospective. Les futurs writers doivent recontrôler.
- Deux SQL ciblés ajoutés : limite exacte/-1 avec CAS+sweep+journal+réservation
  étrangère et B compté une seule fois ; plan changé et collision refusés sans
  effet. Aucun n'est exécuté à cette note.
- F2-R157-1 : quatre natives du vrai recovery opener ajoutées. Orphelin sans
  reçu, reçu en conflit avec intent, corps sans intent, seconde opération B
  étrangère à la release. Départ par vrais gaps begin/ACK ; corruptions de
  fixture explicitement nommées. Zéro writer et témoins source/catalog/reçu
  inchangés exigés. Corps sans intent qualifie le full inventory en amont,
  pas directement le EXISTS interne. Aucun runtime encore.
- r167 : E0583 chemin du nouveau sous-module test, corrigé par #[path].
  **r167b compile PASS49,29s, Cargo exit0, 85 warnings**. Log
  `D:/Layrs/tmp/c14-capacity-recovery-build-r167b.log`. Aucun gel/exécution de
  ce binaire. La native supplémentaire d+q est maintenant écrite et formatée :
  `c14_denial_start_rechecks_source_and_future_destination_capacity_before_funding`.
  Quota −1 avant prepare puis après prepare, seuil exact, vrai ACK famille puis
  retry B déjà présent (sans double compte), Planned replay, vrai mauvais guard
  parent. APIs usage/set_quota réelles ; aucun claim de panne disque physique.
- Trois corrections r166 ajoutées APRÈS r167b : propagation du candidat strict
  par and_then(valid), historique Cancelled avec locator DATA borné assez long
  pour dépasser les bytes avant MAX_ROWS, scopes copie/readback exigeant famille
  Initializing (Absent permis seulement avant registration, jamais après succès).
  Caller test passé à registering=false ; aucun changement au gel r166.
  **r167c compile PASS36,95s, Cargo exit0, 85 warnings**, après runner terminal.
  **Gel r168 créé : 732 sources, neuf chemins depuis r166, PDB age172.** Host
  `3fdd641778f7a4a37f65dff1c2f1c12224332fc20f68de5c18ecca0e9647b968`, EXE
  `c90ff68514f27cdd4be02b16daaeb5eba8f37856e84ae542396517a07169a1d8`.
  17 recettes ciblées compilées, aucune exécutée. Brief
  `D:/Layrs/tmp/c14-capacity-review-brief-r168.md`. Revue ciblée indépendante
  capacité/recovery/corrections avant décision ROOT d'exécution. Aucun runtime
  ni build actif à cette note. Staging est la prochaine intégration produit ;
  la condition préalable de vrais succès begin/allocation/copie est satisfaite
  par r166, sans qualification de la nouvelle capacité d+q.

Limite R1 explicite : la réservation companion B `s` est une troisième charge.
Le constructeur actuel exige vrai Staging et identité C future ; aucun faux
proof n'est construit pour calculer `s`. Il faut une primitive de sizing DATA
distincte au raccordement C14 content. Le présent contrôle d+q ne qualifie pas
la capacité complète du parcours companion, ni une garantie entre verrous.
La garde parent ne remplace pas les exclusions famille/control de l'allocation.

## Point courant — 2 novembre, gel r166 (prévaut sur r162)

**Goal actif ; plan global L0–L9 incomplet.** Aucun runtime en cours à cette
note. Aucun commit/push ; stash original préservé.

- Revue r162 terminée : aucun nouveau produit HIGH/MEDIUM, couverture négative
  allocation insuffisante. Qualification r162 : **DescriptorOnly orphan recovery
  PASS18,892s**, puis allocation **FAIL101 avant begin** (6,512s). La fixture
  CurrentAlias avait deux descriptors originaux de même FileId et ManifestIds
  différents ; le produit refuse exactement cette ambiguïté. Reste non exécuté.
- Toutes les fixtures positives sont désormais DescriptorOnly : README
  fixed1MiB dès bootstrap, immuable distinct, préparation publique. Alias ambigu
  devient un négatif explicite ; Nonresident ne manque que la préimage exacte.
  Aucun relâchement du contrat de pin ou de mapping produit.
- C14 `register_and_copy_denial` ajouté : scope fermé du même owner, source SQL
  retenue, vrai B/current body/plan/image, famille Initializing, C EmptyIngress
  puis CopiedPrefix avec corps exact. Replays family/C ACK et sans writer.
  Legacy garde ses refus ; lectures directes du module devenues strictes.
- Corrections r162 : preuve copiée explicitement égale à l'allocation réelle ;
  owner utilisable seulement après begin acquitté et reçu nettoyé. Préflight
  réserve MAX_BYTES de croissance de l'intent avant B dans la limite 16MiB.
- Cinq nouveaux négatifs : owner non soldé, Planned modifié, routes legacy,
  Copying proof attendu modifié, image native modifiée. Trois natives copie
  et régressions legacy retenues. C déjà enregistré reste non couvert.
- Compilation r163 PASS1m02 puis r165 terminée en 1m03 avec EXE et 85 warnings.
  Wrapper PowerShell r165 exit1 sur NativeCommandError stderr Cargo ; aucun
  diagnostic de compilation error dans le log. r164 intermédiaire non exécuté.
  **r166 : 730 sources, six chemins depuis r162, PDB age170**. Host
  `24cf02dc8aa106ff624ea0dc7d4bdd613130601765e579e13751dc91712972be`, EXE
  `44e93522a058bd562ba9d9d9c94186e1aa0376de098d6aa4b27a29fd97aa3231`.
  38 candidats compilés, aucun exécuté ; doublon orphan DescriptorOnly omis.
  Brief : `D:/Layrs/tmp/c14-copy-review-brief-r166.md`. Revue indépendante
  et décision ROOT requises avant le runner, pas de release à cette note.

Prochaine validation : vrai begin réussi DescriptorOnly, allocation puis copie,
avant d'empiler Staging. Arrêt au premier échec, aucun fixture rouvert/réutilisé.
Preuves anciennes distinctes : r157 16 DATA/FS/SQL PASS ; r154 cinq DATA PASS.

Suite produit identifiée : `stage_denial` avec matrice réelle BCopying/CCopied,
BStaging/CCopied, BStaging/CStaging ; BCopying/CStaging doit refuser. Conserver
les gardes legacy, snapshot source jusqu'au close C et corps réellement relus.
Ne pas élargir SourceCurrentDeniedCoverage (Copying seulement). Préflight futur
quota C et capacité physique avant effets à ajouter ; recontrôle au writer.
Content C14 exige un constructeur coverage-aware, pas la suppression des gardes
C13 : deux obligations B distinctes, document et preservation companions.

Restent F2 négatifs recovery, share-all du reçu, probes FS, OS-close/crash/power-loss,
F7 GC/rétention, historique Published/Reserved, Stage/Ready/admission/autonomie,
caller client/autorité distante réelle et reste du plan global. Pas de clôture
de C14 ni du goal.

## Point courant — 2 novembre, gel r162 (prévaut sur r157)

**Goal actif, aucune étape globale déclarée terminée.** Aucun runtime actif
à cette note ; aucun commit/push, stash original préservé.

- C14 allocation explicite implémentée : `allocate_denial` et
  `publish_format_denial` passent par une autorité fermée conservant le même
  propriétaire. Toutes les frontières physiques restent vérifiées. Nouveau
  `denial_transition` : source Planned/Copying réellement relue, B et document
  exigés, plan/union/coût redérivés depuis le template Planned immuable,
  destination non enregistrée. Binding Copying avec replay sans writer,
  rollback/close/readback stricts. C SQL reste vide ; registration/copie à venir.
- Lectures managed_journal désormais strictes, annulation comprise. Préflight
  roundtrip intent+body avant B ; commentaire metadata-floor corrigé.
- F3 natives renforcées : ImmutableSource courant et range-watcher armé au
  close famille après COMMIT, pin effectivement vérifié, préimage absente,
  floor metadata−1 distinct du total, plan dérivé altéré, véritable Staging
  via writers capture. Deux nouvelles natives allocation/FORMAT (dont ACK
  SourceCopying retenu). F2 succès/replay vérifient directement le cleanup du reçu.
- r154 : cinq pures PASS après revue. Preuve `provider-c13-history-20261102-r154/c14-sql-results.json`.
- r157 : revue Opus terminée sans produit HIGH/MEDIUM. Quatre receipt DATA/FS
  PASS ; première native orphan recovery FAIL à prepare, avant financement.
  Cause démontrée en source : descriptor README `single()`, préparation publique
  `fixed1MiB`, donc ManifestId exact absent malgré FileId égal. Aucun fixture
  rouvert. Les douze codec/SQL indépendants, non exécutés au premier arrêt, ont
  ensuite tous PASS dans un runner séparé sur racines fraîches. Preuves
  `provider-c13-history-20261102-r157/c14-sql-results.json` et
  `c14-sql-followup-results.json`. Aucun F2 natif complet qualifié à ce point.
- Correction fixture : mode explicite descriptor-only (README fixed1MiB AVANT
  bootstrap, immutable distinct, vraie préparation des deux, puis edit README,
  pin exact sans alias) et native dédiée de reprise orpheline. Mode alias conservé
  séparément. Le produit continue de refuser une préimage dont seule une autre
  représentation est résidente sans producteur courant : aucune relaxation.
- r158 compile E0308 proof non Clone ; correction copie DATA par codec et
  validation exacte. r159 PASS54,24s ; **r161 PASS1m05s**,85 warnings.
  r160 était un gel intermédiaire non exécuté/non revu ; r162 le remplace pour
  qualification. r162 :730 sources,sept chemins depuis r157,PDB age168 ; host
  `83fd68ad4af15e8ab27ab2a033378260dee057e9a4c59521509aa1b4d3928d34`, EXE
  `09c3a03c51624e16e0dcc7bda266e93002c112b8e380002c50ac9e7fdfcc0e60`.
  27 candidates compilées non exécutées ; 16 r157 PASS réutilisés sans répétition.
  Brief `D:/Layrs/tmp/c14-allocation-review-brief-r162.md`.

À faire ensuite : revue du delta allocation, qualification sur fixtures neuves,
puis `register_and_copy` denial-aware sous source/image/B réellement retenus,
utilisant `destination_coverage::read_source` + `initialize_empty_with_current`.
Ne pas retirer les gardes legacy. `PendingDenialStart` conserve le template
Planned ; ses scopes explicites lisent séparément la phase actuelle.

Lacunes encore ouvertes r157 : négatifs complets recovery (reçu absent, intent
en conflit, corps sans intent, seconde réservation), probes fichiers non
observées, pin share-all du reçu détectant sans prévenir suppression externe,
OS-close/crash/power-loss non qualifiés. F7 GC/rétention et capacité C physique,
historique Published/Reserved, lookup authentifié d'une représentation résidente
alternative si requis, caller client/autorité distante, Ready/admission/autonomie
C14 puis reste L0–L9 demeurent ouverts. Le collector client est une observation,
pas un lease serveur retenu jusqu'aux COMMIT.

## Point courant — 2 novembre, gel r157 (prévaut sur l'historique)

**Goal actif ; C14 et le plan global restent incomplets.** Aucun runtime actif
à cette note. Aucun commit/push ; stash original conservé.

- r152 : revue Opus terminée, F1/F4/F5/F6 tenus en source. Sélection élargie
  aux régressions des lecteurs (N2), **33 DATA/SQL PASS**, runner session54916
  terminal exit0. Preuve : `.layrs-local/validation/provider-c13-history-20261102-r152/c14-sql-results.json`.
  Cette preuve reste liée à l'ancien EXE, pas aux sources suivantes.
- r154 : revue Opus terminée, F3 fermé au niveau source pour l'admissibilité
  logique du plan C avant B. Aucun nouveau défaut produit HIGH/MEDIUM trouvé.
  Lacunes à combler : fichier courant immuable et absence de range-read après
  B, refus préimage non résidente, négatifs discriminants et capture pending
  atteinte réellement. Capacité physique C, historique Published et GC ouverts.
- F2 intégré : reçu compact `pending-denial-start.json` publié avant B, lié
  aux A/P et inode réels ; il donne seulement une preuve de libération. Nouveau
  recovery opener distinct de l'Active normal, sans revalidation du checkout.
  Snapshot privé complet retenu : transaction+document absents → release
  exacte de B ; Planned réel exact → garde B. Close positif et readback avant
  cleanup du reçu. Trois natives authored : perte du propriétaire+edit checkout,
  Planned conservé, ACK de release refusé puis reprise sans writer.
- r152 N1/N3/N4/N5/N6 corrigés : garde native après vrai Planned, gardes stage
  dès entrée, transport limité au vrai SourceCopying et preuve C exacte,
  assertions de causes discriminantes. Famille B reste au futur caller.
- Compilation r155 : E0277 conversion d'erreur du verrou ; corrigée.
  **Compilation r156 PASS,53,72s**,85 warnings. Gel r157 :729 sources,
  20 chemins changés depuis r154, PDB age166 ; host SHA256
  `79b9eab5e11344f5d14c3e3207a40bbc69a211066e7ad023e5c518a857052b67`, EXE
  `101df159393caa41d4e6407e675307ef384e3d7aff79252791874f4cd345345b`.
  27 recettes candidates compilées, **aucune exécutée dans ce lot**.
  Brief : `D:/Layrs/tmp/c14-recovery-review-brief-r157.md`.

Prochaine étape : revue critique F2 intégrée avant qualification, puis tests
affectés sur racines fraîches, arrêt au premier échec. Ne pas rejouer toute
l'histoire. Le reçu qualifie au mieux l'interruption de processus ; aucune
garantie power-loss/directory-flush. Orphelins anciens sans reçu refusés, aucun
temp adopté. F7 rétention/GC, caller public et C14 Ready/admission/autonomie
restent à terminer, puis le reste L0–L9.

## Travail en cours — après le gel r152

Le gel r152 reste immuable et sa revue Opus est en cours. Travail vivant F3 :
entrée Capture distincte dans `transition_reconcile/source_sql`, reconcile
Capture retenu sans rehash sous gates, nouveau `managed_journal/denial_plan`
avec constructeur fermé et reconstruction du plan complet. `PendingDenialStart`
retient maintenant observation et union proposées, refuse le plan non admissible
avant réservation, épingle seulement l'union redérivée et répète les comparaisons
dans les scopes SQL avant effets. Kernel C14/selection et sizing prospectif en
intégration parallèle. Les natives utilisent désormais une union réelle avec
préimage historique rendue résidente par préparation publique, et des refus
quota zéro/manifest omis/observation changée. **Compilation r153 PASS,43,30s**,
85 warnings ; aucun test exécuté. Gel r154 de 723 sources,14 chemins changés
depuis r152, PDB age165 ; host SHA256
`7cfdff2632bc3c530c2610f5dc3fd87cfb84ab35563b51b548c336da6437f943`, EXE
`7e35a6b28feaf5b2cedec4601853e61ca397458c6a6590a8cc8295a0398bd621`.
Cinq recettes pures et trois natives sélectionnées. Brief de revue ciblée :
`D:/Layrs/tmp/c14-plan-review-brief-r154.md`.
F2 récupération durable n'est pas implémenté ; aucune activation produit.

## Point courant — 2 novembre, intégration après revue r149

Cette section prévaut sur l'historique ci-dessous. **Goal actif ; C14 incomplet.**

- La revue Opus r149 est terminée : `D:/Layrs/tmp/c14-pending-start-critical-review-r149.md`.
  F1 confirme un défaut : financer B avant de vérifier l'absence de capture
  en attente et les bornes prospectives peut bloquer définitivement Planned.
  Correctif partagé lecteur/préflight et recettes de régression intégrés,
  compilation réussie ; runtime et contre-revue en attente.
- F5 : remplacement des lectures familiales non strictes dans ce démarrage,
  afin de ne pas reprendre de snapshot après erreur. Intégré, à qualifier.
- ROOT a ajouté les gardes C14 aux anciens chemins allocation/observation/
  binding/Staging (F4), avant leurs effets. La lecture du financement réel
  est séparée du contrôle de capacité prospectif (F6), pour préserver
  l'interprétation de l'état durable après baisse du quota.
- Transport courant écrit : `destination_coverage`, initialisation atomique
  avec le document, lecteurs EmptyIngress/CopiedPrefix/Staging et inventaire
  exact hérité + courant. Rejeu sans réparation ; gardes Ready conservées.
  Quatre recettes SQL de transport, trois d'inventaire et une d'intégration
  ajoutées ; **non exécutées** à ce point.
- Compilation r150 : échec E0308 dans `require_destination`, conversion
  d'erreur corrigée. **Compilation r151 PASS en 29,77 s**, 86 warnings.
  Gel r152 : 719 sources, 21 chemins changés depuis r149 ; EXE SHA256
  `05f01390cac1cf27585010fcde1bcb885b49ca66114ec580db21eee8e9eb3afd`,
  PDB age164 ; host SHA256
  `07e7b5753829bed982ff198f29062fffce38ddd22be37557ac749fae8b8e5286`.
  Vingt recettes DATA/SQL sélectionnées, trois natives de démarrage à qualifier.
- F2 reste ouvert : le contexte perdu entre réservation famille et Planned
  nécessite une preuve durable de reprise/libération exacte. F3 reste ouvert :
  le plan C complet et son coût courant doivent être redérivés avant financement.
  F7 : contrat de rétention/GC des réservations encore à définir avant caller public.
- Les PASS r149 ci-dessous sont des résultats réellement exécutés mais
  antérieurs à la revue indépendante ; le prérequis de revue annoncé par
  leur host-disposition n'a pas été respecté. La revue ne les autorise pas
  rétroactivement et ils ne qualifient pas les corrections en cours.

Prochaine étape : revue ciblée r152 avant nouvelles exécutions, brief
`D:/Layrs/tmp/c14-current-review-brief-r152.md`. Aucun runtime actif à cette note.
Pas de commit/push ; stash original préservé. Le reste L0–L9 reste à accomplir.

Piste F2 étudiée en lecture seule : reçu compact de libération, publié avant B
dans provider-control, avec binding exact/opération/coût recomputables et inode
propre ; jamais le gros corps avant financement. Une entrée de récupération
distincte est nécessaire : la réouverture Active ordinaire refuse déjà l'orphelin.
Sous owner natif/A/P/binding/attempt et gates exclusives, retenir le lecteur source
complet prouvant transaction ET corps absents, supprimer seulement la réservation
exacte sans installation, fermer positivement, relire strictement puis enlever
le reçu épinglé. Ne pas exiger l'image actuelle (elle peut avoir changé). Si le
corps/intent est durable, conserver B et seulement régler le reçu après preuve
réelle. Ne jamais convertir ce reçu compact en proof de financement ou autorité
d'insertion. Le mécanisme PendingReceipt existant aide mais ne qualifie pas la
perte de courant ; les anciens orphelins sans reçu restent sans preuve de release.
Cette piste est une conception, pas une récupération implémentée ou qualifiée.

Piste F3 étudiée en lecture seule : `prepare` doit recevoir le vrai
`TransitionObservationData` et passer par un reconcile Capture de l'image complète
(ajouter une entrée source_sql Capture sans fabriquer un proof Publication).
Réutiliser `OwnedCompanionData::derive/require_union`, puis un kernel C14 explicite
avec coverage et digest de sélection de l'observation ; le kernel v1 reste strict.
Redériver baseline à metadata zéro, extraire l'enveloppe depuis quota, calculer
`DeniedCoverageCost` et le floor incluant le courant une fois, redériver avec
l'enveloppe et comparer tout le plan. Ajouter un sizing prospectif DATA :
`destination_intent(Source Planned)` produit une phase interdite et ne peut servir
au calcul avant allocation. Épingler seulement l'union redérivée après admission,
puis refaire les comparaisons dans le scope SQL précédant la réservation B.
Pas de cycle : le corps ne dépend ni du quota ni du digest final du plan.
F3/TG3 restent non implémentés ; ne pas prendre les fixtures quota0 pour un plan C admissible.

## Point courant — 2 novembre, C14 démarrage financé r149

Cette section prévaut sur les suivantes. **Goal actif ; pas d'acceptation globale.**

- Le gap same-owner « réservation B durable / Planned absent » est maintenant
  raccordé par `managed_journal/denial_start.rs::PendingDenialStart`. Contexte
  retenu avant la première mutation, image et pins offline conservés, admission
  complète source/prefix/famille ; réservation exacte avant corps+Planned.
- Le lecteur familial possède une entrée prospective explicite. Les entrées
  existantes restent strictes. Seule l'opération liée à l'allocation/plan/corps
  exacts est reconnue ; le proof réel reste dérivé exclusivement des lignes SQL.
- La réservation déjà durable se rejoue après lecture complète et fermeture
  positive, sans writer familial. Planned déjà durable se rejoue sans aucun
  writer. Les erreurs de fermeture interrompent la chaîne avant toute nouvelle
  sonde ; les scopes existants sont fermés.
- r146 compilation PASS. r147 n'a pas de résultat terminal exploitable : sa
  session a disparu et aucun processus Cargo/rustc/test n'était actif à la reprise.
  **r148 compilation PASS, 41,05 s**, puis gel r149 de 714 sources.
- **r149 : huit SQL PASS et deux natifs PASS** : parcours normal 9,277 s ; deux
  refus d'ACK successifs famille puis B et replay final sans writers 9,023 s.
  Les observateurs confirment zéro probe après ACK refusé et zéro ouverture B
  après refus familial. Les témoins des DB confirment les phases sans écriture.
  Aucun runtime actif après session ROOT58524 terminée code0.
- Gel : `.layrs-local/validation/provider-c13-history-20261102-r149`.
  Host SHA256 `ecb29bbc8f4a85622c4f0a1392cc3151cbbdf133a804c3025e45b9d21c9a08d3` ;
  EXE SHA256 `c4d216bea6c03521f88c622481aae6e8010d0200329102a9f097ca5e5632a13b`,
  PDB age163. Résultats `c14-sql-results.json` et `c14-native-results.json`.

Limites : observation locale normalisée de fixture, pas de lease serveur ; un
fichier natif courant et aucun C créé ; ACK retenu après fermeture SQLite réussie,
pas panne OS/crash ; reprise avec même owner uniquement. Le nouveau démarrage
n'est pas exposé par un caller produit. Les tests ne qualifient pas Ready/admission.
Les revues r139/r142/r145 n'ont fourni aucune acceptation indépendante (limite
fournisseur puis notifications de reprise annulée). Nouvelle revue intégrée r149
à déléguer sur le gel, sans demander de réexécuter les tests ROOT.

Prochain travail : transport du corps courant (distinct des hérités déjà copiés),
inventaire exact destination et propagation du coût réel dans les dérivations
du plan/Ready/admission/autonomie ; puis raccordement du vrai caller avec autorité
retenue. Préserver les refus v2 des modes encore non pris en charge. Le reste
du plan L0–L9, dont client/CLI, reprise fresh-owner et qualification finale, demeure.
Pas de commit/push ; stash original préservé.

## Historique — 29 octobre, C13 r142 / C14 r145

Cette section prévaut sur les suivantes. **Goal actif, aucune acceptation globale.**

- **C13 r142 PASS natif, 590,105 s**, session ROOT74117 terminée code0.
  Le fichier inchangé devenu Reserved est conservé en compagnon, C est publié
  puis rouvert sans owner B par `ManagedReadyInspection`. Correction de
  C13R136-001 ; variante Tree couverte en DATA seulement. r140 : 19 DATA PASS.
  Les trois natifs r136 restent PASS (629,724 / 679,830 / 63,420 s) ; pas de
  réexécution inutile de ces parcours sur les modifications C14.
  Preuve : `.layrs-local/validation/provider-c13-history-20261029-r142/c13-native-results.json`.
- **Contre-revue C13 r142 et revue transport C14 r139 : FAILED**, limite API
  Claude. Aucun résultat de revue ni acceptation indépendante. Ne pas poller
  ces tâches terminales, ne pas substituer un autre modèle sans demande.
  Nouvelle revue du financement C14 r145 également FAILED sur la même limite
  (`41f2669c-6eb6-43a5-86d9-e079f155281a`). Aucun reviewer actif ; poursuivre
  l'intégration autorisée sans nouvelle boucle de tentatives Opus immédiates.
  Le message fournisseur précise : limite de session, réinitialisation annoncée
  à **17 h Europe/Paris le 29 octobre**. Ne pas retenter avant cette échéance.
- **C14 r139 : cinq DATA et neuf SQL PASS**, transport des documents hérités
  sous prefix-v3, corps réels liés aux observations et refus legacy v2.
  Preuves conservées dans le gel r139 ; l'activation C14 reste fermée.
- **C14 financement r145 : compilation r144 PASS (22,63 s), huit DATA et cinq
  SQL PASS.** Nouveau `shared/source_denial` : réservation B fixe, dérivée du
  corps complet et de l'allocation/plan/famille exacts, indépendante de la
  phase et de C physique. Coût = colonnes document réelles + borne codec intent
  B et scalaires + scalaires réservation. Rejeu exact, pas de débit/réparation.
  Les lecteurs Transition/Publication reconstruisent les bindings depuis les
  corps SQL effectifs ; le classifier familial exige les montants présents.
  Le test Planned/Cancelled utilise deux bases SQL jetables et le vrai proof
  source pour refuser absence, montant altéré et rollback du financement.
  Les trois autres tests de réservation couvrent capacité exacte/moins un,
  comptabilité globale, collisions et préconditions transaction/FK/cancellation.
  Le floor C possède un coût courant fermé dérivé du corps réel ; les anciens
  callers refusent v2 sans ce coût. Les hérités restent dans copied metadata.
  Aucun caller actif v2, aucune preuve native ou atomicité inter-bases revendiquée.
- Gel r145 : 712 sources, EXE/PDB age161. Host SHA256
  `77c8cc7127470098104456ca2b610646dffdf3523f025287b691aea4e9023c71` ; EXE SHA256
  `106a8b91fd51f689bf1f90aea9db62e3dda6d77434b7b8ebd4b0cc44d3e955fc`.
  Résultats : `.layrs-local/validation/provider-c13-history-20261029-r145/c14-data-results.json`
  et `c14-sql-results.json`. Aucun runtime actif à ce point.

Prochaine intégration concrète : gap réservation B durable / Planned absent.
Exiger un contexte prospectif exact retenu par le même owner, sans accepter un
namespace seul ; COMMIT et fermeture familiale positive avant writer B ; arrêt
immédiat après ACK incertain ; corps+Planned atomiques puis full readback.
L'investigation de raccordement `c14_derived_storage_guard` est terminée, lecture
seule. Proposition et points à trancher :
`D:/Layrs/tmp/c14-pending-denial-start-handoff-r145.json`. Retenir les pins offline
avant réservation ; ne pas repasser par une admission familiale stricte au retry
du gap. Si Planned existe déjà après ACK B refusé, utiliser le vrai proof complet
et non le contexte prospectif. Le fresh-owner sans corps demeure hors prise en charge.
Ensuite : vrai caller, document courant/inventaire destination, propagation du
coût, Ready/admission/autonomie C14 ; client/CLI et reste L0–L9 toujours à finir.
Pas de commit/push, stash original préservé. Les tests ciblés ne valent pas
acceptation globale ni qualification crash/échec OS de fermeture.

## Historique — après revue C14 r134

Cette section prévaut sur les états historiques ci-dessous. Goal actif.

- **C13 r136 : premier natif PASS, 629,724 s.**
  `c13_real_accepted_edit_denied_present_companion_reaches_c_ready_and_b_published`
  a terminé avec code0. Le runner ROOT65400 poursuit le scénario same-owner
  à onze points de refus d'ACK, puis le cas de préimage froide manquante.
  Ne pas démarrer Cargo/SQL/natif tant que cette session n'est pas terminale.
  Preuve : `.layrs-local/validation/provider-c13-history-20261029-r136/c13-native-results.json`.
  Aucun résultat final des deux autres recettes n'est encore revendiqué.
- Revue Opus C14 r134 terminée : aucun défaut Critical/High/Medium ; deux Low.
  Rapport `D:/Layrs/tmp/c14-source-critical-review-r134.json`. F1 : des lecteurs
  legacy pouvaient décoder v2 sans exiger son corps ; F2 : test policy/epoch absent.
  ROOT a ajouté les refus v2 explicites aux lecteurs destination, Ordinary et
  admission, et exige prefix-v3 pour un reçu v2 hérité dans ancestry. Nouveau
  test policy/epoch avec corps et référence recalculés. Ces changements restent
  **non compilés/non exécutés** à ce point ; ne pas transférer les PASS r134.
- Transport C14 hérité intégré par GPT-6.1-Sol : rowset14 DenialObservations,
  domaine distinct, engagement des documents complets triés, bornes existantes
  1M lignes/1GiB et 64MiB/document avant allocation. IDs issus exclusivement
  des reçus entrants authentifiés ; document du transfert courant exclu pour
  éviter le cycle de digest. Copie avant les reçus, replay exact et collision
  sans réparation. Ordinary6 autorise le scope vide sans table supplémentaire.
- ROOT a raccordé `require_observation_rows` et `transition_observation/stored.rs` :
  liaison aux lignes snapshot/sélection authentifiées, génération/digests,
  Step/root, policy/epoch et sémantique ; recontrôle du namespace visible/refusé
  partagé, aliases, ancêtres, frontières et scope. Aucune autorité distante créée.
  Deux tests DATA ajoutés (policy/epoch et namespace historique).
- Sept fichiers transport, six recettes (une DATA, cinq SQL) préparés par l'agent,
  rustfmt/check et diff-check PASS seulement. Compilation ROOT puis DATA ciblées,
  cinq SQL transport et régressions legacy concernées à exécuter une fois r136
  terminal. Revue C13 r136 Opus encore en attente, ne pas la sonder.
- C14 n'est pas activé : restent caller réel, financement complet des documents,
  document courant et inventaires source/destination, Ready/admission/autonomie.
  Les gardes correspondantes restent fermées. Aucun commit/push ni acceptation globale.

## État courant — C13 r121 / intégration C14, 29 octobre

Cette section remplace les mentions « en cours » ci-dessous. Goal toujours actif.

### Dernière suite r122–r129 (prioritaire)

#### Suite r130–r134 — état le plus récent

##### Suite r135/r136 — prioritaire

- r134 natif terminal **FAIL 23,764 s**, session ROOT33027 code1. Le parcours
  dépasse les fixtures précédentes et révèle un défaut produit dans
  managed_plan::derive_kernel : un fichier Reserved présent, déjà retenu par
  le compagnon, était encore exigé dans l'image de checkout C.
- ROOT corrige : seul un owner reconciled typé autorise une entrée Reserved,
  sans writable, sans fichier checkout et sans suppression. Le kernel legacy
  garde le refus. Le nouvel exemple DATA dérive réellement observation+owner,
  vérifie compagnon/union/budget, maintien Reserved et absence de Capture.
- r135 compilation **PASS 19,00 s** ; 13 managed-plan DATA **PASS 0,120 s**,
  10 companion DATA **PASS 0,360 s**, incluant le nouveau cas. Le premier
  sélecteur de groupe companions était erroné : aucun test de ce groupe lancé
  avant correction du nom en companion_codec_recipes (r135b), sans rerun des13.
- R136 gel vérifie exactement deux changements depuis r134 : managed_plan.rs
  et transition_reconcile/tests/companions.rs. Tous les fichiers C14 sont
  inchangés et restent sous la revue Opus r134 (task suffix9a204f63).
  Rétention `.layrs-local/validation/provider-c13-history-20261029-r136` ;
  host SHA256 `4022e176dc18d13e4513be2df21509b848b2d8235aca0b100ca3984a7bd8f9bf` ;
  EXE SHA256 `6bedf9cac4fae000e8936a50a0bccba39e7cb9487be6cd1b1b290de5c30e5299`.
  Runner natif r136 lancé, session ROOT65400, série3, stop premier échec. Aucune autre exécution
  Cargo/SQL/native tant qu'il n'est pas terminal. Aucune qualification globale.

- Agent source C14 terminé : sept fichiers, lecteur complet Provider des
  documents v2 et primitive apply_source_intent_with_coverage. Documents
  dérivés + Planned dans la même transaction main Write ; replays et phases
  exigent le corps SQL présent, jamais réparé. Préflight 4096 documents,
  64MiB chacun, inventaire exact et comparaison aux semantics/Step/root actifs.
  Bootstrap/Preparation/Active/ActiveCapture restent stricts ; prefix-v3 refusé.
  Caller réel, destination/transport, Ordinary et financement restent à faire.
- r130 compilation FAIL : helper digest_token privé et valeur cancelled
  déplacée dans test. ROOT utilise raw_digest après strip_prefix, comparaison
  empruntée dans le test. r131 compilation PASS 29,02 s.
- r132 : sept SQL PASS (3 nouveaux writer, 4 historiques intention/publication).
  Test preflight FAIL car STRICT rejetait déjà TEXT dans BLOB malgré
  ignore_check_constraints. Attente corrigée pour vérifier ce refus effectif.
  r133 compilation PASS 15,27 s. r134 quatre SQL restants PASS
  (0,123 / 0,422 / 0,119 / 0,271 s). **Onze SQL ciblés acquis** ; les sept premiers
  conservés avec preuve que seul le test preflight a changé. Workspace all-targets
  **PASS 14,39 s**. Aucun autre rerun nécessaire pour ces sources inchangées.
- Rétention r134 : `.layrs-local/validation/provider-c13-history-20261029-r134`,
  705 sources, EXE SHA256
  `1df62aa4b0dd182e1ef13f1a58e260715058bf2a331131bc41987b793dc89f35`.
  Host JSON SHA256 `3d2f29918a3e5905e7b8ba04d5440b68d33a47fca705207291e068bfb21f863d`.
  Lire `root-current-disposition.json` pour la portée actuelle : les phrases
  historiques cumulées dans host-disposition.scope ne décrivent pas toutes r134.
- ROOT a autorisé les trois natifs C13 diagnostiques r134 après ces contrôles.
  Runner lancé en série, nouvelles fixtures C:, stop premier échec. Revue critique
  indépendante de l'intégration SOURCE C14 à lancer sur ce gel. Aucune acceptation
  globale ou qualification C13/C14 ; goal reste actif. Ne pas démarrer un autre
  Cargo/SQL/natif tant que ce runner n'est pas terminal.

- Deux agents GPT-6.1-Sol ont terminé les durcissements F1/F2/I3 : wrapper
  DerivedDeniedCoverage scellé, garde SQL main Write/query_only, budget commun
  fresh-visible+denied (sans compter deux fois les sélections/descripteurs).
  r122 compilation **PASS 38,54 s** ; r123 DATA : 7 tests record distincts PASS
  (accidentellement compilés deux fois), 16 observation PASS, 32 codecs PASS.
  **Cinq SQL PASS**, notamment BEGIN DEFERRED/savepoint/query_only sans écriture.
  Artefacts : `.layrs-local/validation/provider-c13-history-20261029-r123`.
- ROOT a corrigé le chemin explicite de binding/tests.rs. r124 compilation
  PASS 23,79 s. r125 : test de reconstruction PASS ; test de substitutions FAIL
  car le token substitué rootTreeId était identique à la fixture. Le test choisit
  désormais une valeur différente et vérifie que la mutation est réelle.
  r126 compilation PASS 16,29 s ; r127 **deux tests de binding PASS** (0,072 s).
- r127 natif C13 **FAIL 26,478 s**, même assertion Source, sans lancer les deux
  suivants. Diagnostic précis dans layrs-vfs/evidence.rs : Immutable exige
  partial+in_sync+modified_bytes=0 ; un fichier entièrement hydraté est NeedsHash
  même readonly. ROOT a laissé unchanged.txt froid côté placeholder ; ses octets
  sont déjà présents dans CAS par la préparation publique. Aucun garde produit
  ni assertion affaibli. Anciennes fixtures r121/r127 laissées intactes.
- r128 compilation PASS 14,25 s. Gel r129 vérifie un seul fichier modifié depuis
  r127 (fixture C13), 702 sources, EXE SHA256
  `9d12e2950659fcc7c4dd0f29df3177c03dfa6997620160c9379cc589e186e212`.
  Runner `tmp/provider-c13-history-native-run-r129.py r129` terminé code1,
  session ROOT29619 : **FAIL 26,230 s**. L'assertion Source est désormais passée ;
  le préfixe préparatoire échouait car son count captures était encore 1 pour
  deux Captures. ROOT a remplacé les compteurs préparatoires par ceux de la preuve
  réelle (génération, captures, descripteurs, mappings). Non recompilé. Les deux
  autres recettes n'ont pas été exécutées.
- Agent `c14_derived_storage_guard` travaille désormais sur l'intégration
  Provider SOURCE : lecteur complet des documents, inventaire exact des v2,
  nouvelle primitive Planned atomique. Ownership validation.rs/transition.rs,
  transition_sql.rs et tests/helper dédiés. Aucun caller actif, transport,
  Ordinary ou destination autorisé dans ce lot. Prefix-v3 reste refusé tant
  que le transport n'est pas raccordé. Aucun build ni test chez cet agent.

**Aucun runtime en cours. Attendre le handoff de l'agent avant prochain build.**
Les éditions source C14 peuvent continuer. Aucune qualification C13 n'est encore
acquise ; ne rouvrir aucune fixture précédente.

- Revue Opus r109 terminée : les deux blocages de reprise r102 sont corrigés
  en source. ROOT a corrigé le refus des représentations Capture historiques
  et déplacé la fixation des pins après validation de la publication. Les
  recettes couvrent désormais onze ACK retenus, les contrôles négatifs hors
  driver, une Capture remplacée et les producteurs de préimages hors ligne.
- Revue Opus r115 terminée : aucun défaut critique/haut/moyen, deux faibles
  (construction non scellée du document, transaction non garantie en écriture).
  Garder le cycle acyclique : prefix-v3 ne contient que les documents hérités.
  Les lecteurs actifs, l'écriture atomique Planned et le transport restent à
  raccorder ; les barrières historiques n'ont pas été levées.
- r118 compilation lib **PASS 41,07 s**, workspace all-targets **PASS 20,28 s**.
  r116/r117 avaient révélé deux erreurs locales du nouveau helper de liaison
  (chemin de module et conversion de résultat), corrigées avant r118.
- r119 codecs : transition 9 PASS, import 22 PASS / 1 FAIL. Cause vérifiée
  dans HEAD : ancien test rejetant Ordinary10, alors que 9 et 10 sont admis.
  Test corrigé avec cas positifs 9/10 et refus 6/7/8/11 ; aucun garde produit
  affaibli. r120 compilation **PASS 13,00 s**.
- r121 **32 codecs PASS** (9 transition, 23 import), artefact/699 sources
  conservés sous `.layrs-local/validation/provider-c13-history-20261029-r121`.
  EXE SHA256 `ee2b87b56351e7eca367424fbc281156f50b44e602a4694d811754a4808c4180`.
  Le premier natif C13 a réellement été exécuté : **FAIL 24,365 s**, assertion
  « unchanged file must exercise the immutable Source reader ». Fixture
  incorrectement writable ; corrigée à false. La représentation de la Capture
  historique a aussi été ajoutée à l'inventaire fourni et liée au mapping
  effectivement relu. Ces deux corrections ne sont pas encore compilées.
  Runner arrêté au premier échec ; deux autres natifs NON EXÉCUTÉS. Session
  ROOT96504 terminée code1. Ne pas rouvrir sa fixture, ne pas relancer r121.
- C14 fermé : codecs intent/receipt v2 et prefix v3 intégrés et testés ;
  producteurs encore legacy/None. Helper require_intent/require_observation
  compilé r118 ; ses deux nouveaux tests sont écrits, pas encore exécutés.
- Durcissements C14 en intégration : DerivedDeniedCoverage scellé, writer
  exigeant une transaction main Write et query_only=0 ; fixture dérivée du
  normaliseur et nouveau test SQL. Agent terminé, format vérifié, non compilé.
  Agent de budget commun fresh-visible + denied encore en cours.

Prochaine action : intégrer les handoffs, compiler une seule fois, retenir le
nouvel artefact, exécuter les DATA/SQL affectés puis les natifs C13 avec de
nouvelles fixtures. Aucun build/runtime en cours à cet instant. Ne pas confondre
les revues source et les PASS de primitives avec une qualification C13/C14.

## Suite active — correctifs de reprise C13 r109, 29 octobre

Cette section remplace les états « en cours » des sections historiques suivantes.

- Suite r111/r112 : le cas natif de préimage originale absente est maintenant
  écrit et compilé (pas exécuté). Il exige une cause I/O NotFound, zéro nouveau
  writer, les trois bases byte-identiques et aucun appel d'hydratation implicite.
  r110 a détecté l'emploi incorrect de DatabaseFile comme Path dans le témoin ;
  ROOT a utilisé son accesseur path(). r111 compilation **PASS 23,80 s**.
- R112 **trois SQL PASS** (0,069 / 0,069 / 0,071 s) : réservation fixe B+C,
  namespace de racines parcouru progressivement, insertion/rejeu de suppression.
  Artefact et 691 sources retenus sous
  `.layrs-local/validation/provider-c13-complete-20261029-r112` ; EXE SHA256
  `22edb66e2882c151ef2a2d41a0613cd1f7f3ca87376d611ec89d1a5b58e16ca0`.
  `c13-sql-source-release.json` autorise uniquement ces trois primitives et
  `c13-sql-results.json` contient les résultats. Aucun natif C13 exécuté.
- Contre-revue Opus5.5/xhigh r109 en cours : taskId
  `node:delegated-task:command%3Amcp%3Abcb659e8-99b8-40bf-8ee3-f3319081559e%3Adelegate-task%3A9f84f9dc-1161-462a-9eb0-9c094585fe6f`.
  Ne pas poller ; résultat asynchrone. Gel r109 inchangé ; le seul delta r111
  est la recette froide dans c13_complete_ready_tests.rs, retenue avec r112.
- Agent natif GPT-6.1-Sol `c14_codec_storage_integration` chargé d'intégrer les
  seuls codecs/primitives de stockage C14 et leurs déclarations de modules.
  Pas de changement des lecteurs stricts ni de raccordement actif C14 à ce stade.
  Intégration terminée : quatre nouveaux fichiers, deux déclarations étroites.
  r113 compilation a révélé une durée de vie manquante de snapshot dans
  require_shared_namespace ; ROOT l'a corrigée. r114 compilation **PASS 48,22 s**.
  Sept tests DATA **PASS** via `tmp/run-c14-data-r114.py`, ROOT28470 terminal0 ;
  tests 25,54 s, dont 100000 chemins à exactement 8MiB et refus du dépassement.
  résultats sous `.layrs-local/validation/provider-c14-data-20261029-r114`.
  R115 : **quatre tests SQL C14 PASS**, 0,071 / 0,070 / 0,120 / 0,070 s.
  Insertion/rejeu/rollback, refus d'un corps substitué/non canonique, refus du
  BLOB surdimensionné avant décodage, transaction/FK/annulation sans effets.
  Artefact et 695 sources : `.layrs-local/validation/provider-c14-primitives-20261029-r115`.
  Le contexte actif/Planned, Ready, transport et barrières d'acceptation ne sont
  pas raccordés par ces tests de primitives.
- Cartographie C14 par explorer `c14_versioned_links_map` terminée : garder
  les modèles internes communs mais employer des wire types fermés séparés pour
  intent/receipt v1/v2 et prefix v1/v2/v3, avec champs obligatoires non nuls dans
  les nouvelles versions et domaines de digest distincts. Pas d'Option tolérée
  sur l'ancien wire, pas de Value/flatten permissif. 10 plan literals, 10 receipt
  literals et 23 closure literals à mettre à jour ; les producers doivent
  sélectionner les nouvelles versions. require_transition doit interdire tout
  changement de schema_version pendant CAS. Prefix v3 compte des DOCUMENTS.
  **Éviter le cycle** : denialObservations du préfixe source inclut seulement les
  documents hérités ; le document de la transition actuelle lie déjà ce préfixe
  et entre dans plan-v2/receipt-v2, puis dans le préfixe du prochain export.
  Les lecteurs devront comparer le corps réel à transaction/source/identités/
  Step/root/epoch/observation et maintenir les refus legacy jusqu'au raccordement.

- Revue Opus r102 reçue et lue : deux P1 de reprise same-owner, un P2 de
  couverture par alias FileId et un P3 de contrôles de test dans le driver.
  Rapports `tmp/c13-complete-ready-critical-review-r102.{json,md}`.
- GPT-6.1-Sol a corrigé le driver dans managed_content : choix de phase durable,
  réemploi des pins, conservation des DATA Ready, reconstruction terminale
  validée sans writer, durée d'emprunt de l'image séparée de celle des pins.
  Les assertions/replays cfg(test) ont été retirés du parcours de production.
- ROOT a ajouté les préimages descriptor-only à la preuve CAS hors ligne avant
  réservation, avec vérification de l'ensemble conservé lors des reprises.
  Le scénario natif utilise maintenant cinq FileIds distincts, dont une source
  inchangée, et des préimages rendues résidentes par préparation publique puis
  hydratation réelle en processus enfant. Il appelle `commit_companion`.
  Une deuxième recette refuse successivement les ACK après installation,
  journal checkout, B Ready, B Published et reprend avec les mêmes pins.
  Ces deux recettes sont compilées mais **NON EXÉCUTÉES**.
- r108 compilation refusée : durée de vie manquante dans un ancien test et
  mauvais argument du callback de préparation dans le nouveau scénario.
  ROOT a corrigé les deux. r109 store-lib-no-run **PASS 41,44 s**.
  Logs conservés `tmp/c13-recovery-lib-no-run-r108.log` et `...-r109.log`.
- Gel r109 : `tmp/c13-recovery-integrated-r109/manifest.json`, 80 fichiers,
  13 modifiés depuis r102 ; SHA256
  `2e85427369269dd49eaa0dcea0b16fb1514e70273a2f9aa6acb21b292401f9e1`.
  Workspace/all-targets ROOT89919 terminal0 **PASS 24,85 s**, log
  `tmp/c13-recovery-workspace-check-r109.log`. `git diff --check` PASS.
- Collecteur client r107 : compilation **PASS 19,79 s**, puis **21/21 tests TLS
  locaux PASS en 2,45 s**, aucun serveur de production. Résultats et bindings
  `.layrs-local/validation/provider-observation-20261029-r107/`.
- À faire immédiatement : contre-revue Opus des correctifs, finir qualification
  SQL/native C13 et ajouter le refus natif d'une préimage froide absente. Aucun
  résultat C13 natif ni correction runtime encore revendiqué. C14/host/CLI et
  programme L0–L9 restent ouverts. Goal **active**, aucun push ni nouveau commit.

## Suite active — C13 compilé, données vérifiées, collecteur client intégré

- ROOT49288/r102 terminal0 : store-lib-no-run **PASS46,505s**, workspace
  all-targets **PASS26,034s**. Aucun runtime natif/SQL C13 encore exécuté.
- Revue Opus5.5/xhigh démarrée via claudeAgent, aucune demande d'approbation :
  taskId `node:delegated-task:command%3Amcp%3Abcb659e8-99b8-40bf-8ee3-f3319081559e%3Adelegate-task%3A7eb42b45-f356-492e-bbee-b7ed582c12d8`.
  threadId identique avec préfixe `thread:`. Ne pas la poller ; attendre son
  résultat. Cible figée r102, sortie prévue
  `tmp/c13-complete-ready-critical-review-r102.json`. ROOT corrige le dépôt,
  jamais le gel r102. Aucun résultat de cette revue encore reçu.
- R103 : EXE/PDB et691sources retenus dans
  `.layrs-local/validation/provider-c13-complete-20261029-r103`.
  EXE88184832octets SHA256
  `f4be3b2a2d5450fe578458f56b874beffc3b6f212ba849c11b3fbe89264618c2`,
  CodeView âge142. Binding artefact seulement, pas de release native/SQL.
- Sélection DATA initiale r103 : mauvais chemin de module, **zéro test lancé**,
  échec correctement conservé. R103b corrige les noms d'après `--list` du vrai
  EXE (`companion_codec_recipes`). Deux PASS puis un échec de scénario invalide.
  R103c exécute les neuf autres DATA indépendants : **neuf PASS** ; aucun SQL.
  Fichiers résultats et logs dans le répertoire retenu r103.
- Le scénario fautif changeait un target sous le même Step/root immuable.
  Première tentative de correction ROOT r104 également refusée comme preuve :
  elle supposait des Captures cumulatives, alors que chaque Capture est un diff
  complet contre sa génération. Build PASS12,810s, test FAILED conservé.
  Aucun changement de règles de production pour accommoder ces mauvais cas.
  R105 remplace ce scénario impossible par
  `c13_v2_accepted_deletion_refuses_changed_immutable_base_and_retains_authorized_diff` :
  refus exact du target substitué, réobservation valide, diff de suppression
  autorisé exact, capture historique inchangée et préimage dans l'union.
  ROOT34891 terminal0 ; build19,49s et test **PASS**. Log
  `tmp/c13-corrected-deletion-data-r105.log`. Seul companions.rs de tests diffère
  du gel r102 côté store. Les11autres PASS restent valides. Ne pas revendiquer
  un cas positif accepted SatisfiedDeletion impossible dans ce contrat same-Step.
- Collecteur client corrigé précédemment revu désormais intégré : trois fichiers
  sous client-core/native_scm/remote. Manifest d'entrée
  `35bae32f11b6d3f31d19d340c886b984484146a31259b6e1522203e5d68b0861`.
  ROOT6674/r106 terminal1, deux erreurs compiler : conversion String→RemoteError
  et tableau temporaire emprunté dans un test. ROOT a corrigé .map_err(core_error)
  et la liaison locale du tableau. Rapport r106 conservé, aucun test TLS lancé.
  **ROOT71776** nouvelle compilation client-lib-no-run r107 en cours ; log
  `tmp/provider-observation-client-lib-no-run-r107.log`. Aucun autre Cargo/SQL/natif.
  Après compilation : tests ciblés du collecteur sur serveur TLS127.0.0.1,
  puis workspace check si nécessaire ; pas d'accès à un serveur de production.
- C14 codecs/stockage restent en brouillon isolé, non intégrés. C13 Store doit
  encore passer SQL/natif après traitement de la revue. Client host/CLI,
  transport durable C14 et suite L0–L9 restent nécessaires ; goal **active**.

## Reprise effective du 29 octobre — C13 r101 intégré, compilation en cours

L'utilisateur a demandé « Okay reprends ! » et des revues Opus 5.5 ponctuelles
en parallèle. Goal vérifié **active**. Cette demande lève la pause ci-dessous.

- r100 terminal exit0 reçu : **six PASS**, copie partielle native975,108s.
  Packager exécuté une fois : archive701fichiers/11670323octets, SHA256
  `59c311fd5bbb9f71bc5cbdfc1ddbebc6a7dee6a1cc7efe43617f8b072e781ab5`.
  Archive/inventaire sous docs/continuation/worktrees-partial-vfs. Aucun rerun.
- Proposition C13 interrompue reprise par ROOT : snapshot de nuit intégralement
  conforme, 79 chemins (78 Rust), 49 modifiés depuis la proposition source-B53.
  Tous les chemins des53 conservés ; rustfmt --check et git apply --check PASS.
  Gel `tmp/c13-complete-lease-ready-source-proposal/manifest-r101.json`, SHA256
  `b806f17060b0b252be720490c16c5b3caba5f45d2ed2881e0b931f2f1df667f2`.
  Patch exact HEAD `proposal-r101.patch`. Ces sources sont maintenant intégrées
  au dépôt principal, en plus des cinq fichiers de tests r100. Les fichiers du
  bundle restent immuables pour la revue ; corrections ROOT dans le dépôt puis
  nouveau gel ultérieur. Ne pas relancer le script d'intégration.
- `py D:/Layrs/tmp/integrate-and-build-c13-r101.py` lancé UNE fois :
  **ROOT10981** compilation store-lib --no-run en cours, suivie du workspace
  all-targets seulement si elle réussit. Rapport
  `tmp/c13-complete-ready-build-results-r101.json` et logs voisins.
  Aucun runtime C13 exécuté. Aucun autre Cargo/SQL/natif simultané.
- Opus5.5 disponible via `claudeAgent`, modèle `claude-opus-5-5` ; revue
  indépendante ciblée sur le nouveau raccordement, le maintien des pins et
  les erreurs/ACK. Préférence explicite utilisateur pour ce modèle de revue.
- C14 stockage : brouillon de nuit conservé, toujours non intégré/non compilé.
  C13 ne qualifie pas C14, l'hôte client/CLI, la reprise avec nouvel owner ni
  le réexport sortant Ordinary9/10 des compagnons. Programme L0–L9 reste actif.

Suite compilation : ROOT10981 terminal exit1, Cargo101 après28,018s ; quatre
erreurs conservées dans le log r101. ROOT a corrigé export read_ancestry,
conversion CauseResult, clone du chemin de test emprunté et dispatch strict/
publication dans ProviderContext (pas de faux proof pour le parcours Strict).
Gel intégré suivant `tmp/c13-complete-ready-integrated-r102/manifest.json`,
79 fichiers ; SHA256 `ae0ef70270a1130bd1e3556b58cc8ecf79f1f42e367feb4bafff695f5a93aa67`.
Exactement quatre fichiers changés depuis r101 ; rustfmt --check PASS.
`py D:/Layrs/tmp/freeze-and-build-c13-r102.py` lancé une fois, **ROOT49288**.
Rapport `tmp/c13-complete-ready-build-results-r102.json` ; compiler store-lib puis
workspace all-targets, aucun test. Revue Opus5.5 doit porter sur ce gel immuable,
et utiliser HEAD comme contexte pour les fichiers absents, pas sur un ancien lot.

## Arrêt demandé pour la nuit — 29 octobre 2026

L'utilisateur a explicitement demandé : « Alright arretons nous pour ce soir ».
Mettre le goal en **pause** après sauvegarde ; NE PAS reprendre sur notification
de fin d'un test ou d'un ancien sous-agent. Attendre une demande de reprise.
Aucun push/PR ; HEAD reste `6b1f5632bd7f6092226d57e3c813033a82bcfc98`.

- **r100 : cinq SQL PASS** (21,191 / 16,336 / 87,936 / 54,274 / 43,674 secondes).
  Le sixième et dernier scénario natif de copie partielle est encore en cours,
  handle ROOT25207. Il est laissé finir seul et enregistrer son résultat : aucun
  test supplémentaire ne suit. Ne pas le relancer ni rouvrir ses fixtures.
  Résultats : `.layrs-local/validation/provider-ordinary-checkout-boundary-partial-20261028-r100/ordinary-checkout-boundary-partial-results.json`.
  Aucun terminal observé à l'arrêt ; packager r100 NON exécuté. À la reprise,
  récupérer le terminal/logs, puis archiver succès OU échec sans masquer l'état.
- **C13 : arrêt confirmé** (`interrupted`, aucun run actif, 00:01:08 UTC)
  après task_cancel pour la tâche
  `33e27463-7cc9-443b-b7c9-d266c63aeccc` (lease complet et vrai C Ready).
  Ses fichiers partiels restent dans `tmp/c13-complete-lease-ready-source-proposal/`.
  Aucun manifeste final/patch final ni compilation/acceptation revendiqué.
  Snapshot de 103 fichiers, relu dans l'archive et sans dérive pendant sauvegarde :
  `docs/continuation/worktrees-partial-vfs/c13-lease-ready-interrupted-draft-20261029.tar.gz`,
  SHA256 `9bc56fdc450a4043101fdbbca2b1097454d57add6982334bfc7c127a7fff7596`.
  Reprendre cette proposition existante, ne pas redémarrer toute l'implémentation.
  Vérifier ses changements et l'intégration des trois correctifs ROOT avant build.
- **C14 : brouillon SQL de stockage écrit, NON formaté/compilé/exécuté** dans
  `tmp/provider-denied-coverage-storage-proposal/` : six Rust (trois records déjà
  figés + module provider, stockage exact et quatre recettes SQL nouvelles).
  Transaction appelante obligatoire, FK pour insertion, taille lue avant BLOB,
  validation corps/référence, premier INSERT et replay exact sans réparation.
  Aucun raccordement Planned-v2, receipt-v2, prefix-v3, copie, preuve live ou
  fence d'acceptation encore effectué. Les quatre recettes sont ignorées et NON
  exécutées ; ce module reste une proposition DATA, pas C14 terminé.
  Snapshot brut sept fichiers avec base-inputs, sans dérive :
  `docs/continuation/worktrees-partial-vfs/provider-denied-storage-unverified-draft-20261029.tar.gz`,
  SHA256 `9781a027671ed4921adfa1f84b933bb7ff43f726b5d44c0d1d2a2e74a2857a53`.
  À la reprise : relire le diff, formater/vérifier, finir les liens protocolaires
  sur la base cumulative C13 avant qualification. Ne pas traiter l'archive brute
  comme une release source validée.
- Inventaires et checkpoint détaillé :
  `docs/continuation/worktrees-partial-vfs/night-stop-checkpoint-20261029.json`.
  Les modifications du dépôt principal restent cinq fichiers de tests r100 et
  les documents/archives. Aucune production Provider proposée n'est intégrée.

La priorité suivante reste le parcours C13 complet compilé et testé, puis C14
et le reste du programme L0–L9. Les anciens lots acquis ne sont pas à rejouer.

## Dernier checkpoint — checkout r98 cinq PASS ; r99 build/check PASS ; r100 en cours

Checkpoint local **6b1f5632bd7f6092226d57e3c813033a82bcfc98**, 23 fichiers,
stash original préservé ; aucun push/PR. Goal confirmé **active**.

- **r91 : 7/7 PASS**, archive déjà vérifiée et commitée. Ne pas relancer.
- **ROOT86893/r94 terminal exit0 : 5/5 PASS** : SQL44,665s, fresh575,541s,
  DeclareACK576,062s, OwnedACK577,891s, retained652,120s.
  Packager r94 exécuté UNE fois : archive708fichiers/7162733octets, SHA256
  `ac0284032d0ca7dfd16face21f009716d37f9e3a2d8c92dfc921f23a209eaa94`.
  Archive et inventaire sous docs/continuation/worktrees-partial-vfs restent
  à commiter. Ne pas réexécuter runner/packager ni rouvrir les fixtures.
- Revues r92/r95 et corrections ROOT r93/r93b/r96 conservées. r95 relève deux
  défauts de témoins corrigés : cause structurée des refus allocation-gap et
  garde zéro-writer autour de la réouverture elle-même. Pas de nouvelle revue
  pour ces petits correctifs. Les cinq nouveaux tests sont commités.
- **r97 build et workspace/all-targets PASS**, logs terminaux : 1m01s/21,19s.
  Ancien handle ROOT59569 désormais absent ; aucun rebuild nécessaire.
- **r98 release exécutée UNE fois**, SHA256
  `a45d7641add7813d056fcd218391a57d704ecb9cedaa89be8a81cefb97a6dc89`.
  672 sources retenues ; seul fichier de tests append-only différent de r94.
  EXE82693632octets SHA256
  `5c5c1adf1b14c91c8fc366947840385fcc265cf18f52e6b05d62cf0469176e57`,
  CodeView621a98b1-e6bc-410a-9a0f-44ddcc208f5e age140.
  **ROOT99703 terminal exit0**, runner provider-ordinary-checkout-late-run-r98.py r98,
  lancé une fois avec accès Windows autorisé. Cinq nouveaux natifs uniquement :
  Verified ACK, Publishing ACK, rename-gap, stage allocation-gap, file allocation-gap.
  Verified ACK **PASS1206,615s**, Publishing ACK **PASS1194,779s** ;
  rename-gap **PASS1258,106s**, stage allocation-gap **PASS1118,518s**.
  file allocation-gap **PASS1149,415s** ; cinq PASS. Terminal observé le28/10
  23:43UTC (29/10 local). Ne pas rejouer les cinq scénarios acquis.
  Préparation première recette1046,296s ; PID enfant72876 observé vivant avec
  CPU917,234s durant cette préparation. Aucun blocage/timeout diagnostiqué.
  Aucun Cargo/build/SQL/natif concurrent. Les deux allocation-gap qualifient
  un refus préservant les objets, PAS une adoption/réparation réussie.
  Packager r98 exécuté UNE fois après terminal : archive702fichiers/9605282octets,
  SHA256 `41cb195a597cbb63eb071fe1e70730055fcc8f64bb77657bac34c9cbbd2c83c3`.
  Archive et inventaire sous docs/continuation/worktrees-partial-vfs, à commiter.
  Lecture seule des trois logs PASS → tmp/checkout-r98-first-three-phase-diagnostics.json.
  Coût principalement setup : 1030–1079s, dont installation des trois manifests
  440–468s et construction Provider→B admis 300–315s. Pas de diagnostic OS/lock,
  ni mesure comparative contrôlée : phases imbriquées non additionnables.
  Packager r98 retient cette analyse après vérification de ses hashes de logs ;
  AST et rétention PASS. Ne pas rouvrir les fixtures ni rejouer ces trois.
- Proposition SQL de borne C10 livrée par ordinary_checkout_progress_sql sous
  tmp/ordinary-checkout-c10-floor-proposal/ : T−1/T, construction avant effets,
  contrôle positif Ready8 puis rollback. Candidate SHA256
  `a6c069075b8ff114a87b8bc4545f24e87bff49170f0e98c49528ad4d97fb8f22`.
  Intégrée par ROOT après lecture du diff, vérification des bornes indépendantes,
  du contrôle Ready8 et du tail de négatifs/rejeux inchangé. Rustfmt/check PASS.
  root-integration.json conserve hashes et acceptation SOURCE ONLY. Lors de cette
  intégration, seul ce fichier de tests différait des 672 entrées retenues r98.
  Non compilée, non exécutée. Après ROOT99703 terminal, compiler puis exécuter
  le nouveau SQL et les quatre anciens SQL utilisant le setup refactoré.
  Ne pas relancer les natifs inchangés. Ne pas confondre cette source avec r98.
- Delta suivant ajouté par ROOT : test natif de véritable écriture partielle,
  `managed_ordinary_fresh_checkout_partial_write_retains_owned_inode_and_restarts_copy`.
  Hook local au thread sous cfg(test), dans le sink du driver Owned : Write::write_all
  écrit réellement la moitié de CURRENT puis reçoit une erreur I/O typée. Aucun
  TEMP fabriqué/modifié manuellement. Témoins : trois closes SQLite acquittées,
  journal File Owned, préfixe réel non vide, pin conservé puis libéré, réouverture
  sans writer ni réparation, réécriture depuis zéro, même inode publié et replay.
  Aucune panne OS/crash/perte d'owner n'est revendiquée ; petit fichier courant.
  Rustfmt/check cinq fichiers PASS ; compilation et runtime NON exécutés.
  Gel tmp/ordinary-checkout-boundary-partial-source/ : quatre anciens fichiers
  modifiés + un nouveau module test, manifest SHA256
  `fe1c0755b1252e8279a909147bbd00f08b4b73296bda9625ccc4224d0a097f79`.
  Les anciennes recettes fresh checkout depuis fn snapshot restent identiques
  après normalisation LF. Revue native indépendante checkout_partial_copy_review
  terminée, SOURCE ONLY, sans défaut actionnable ; cinq hashes début/fin conformes.
  Rapport entièrement lu :
  tmp/provider-ordinary-checkout-boundary-partial-source-review.json.
  Après r98 terminal et revue traitée : build/check puis cinq SQL (nouvelle borne
  et quatre anciens callers au setup modifié) et ce seul nouveau natif.
  Scripts préparés, AST PASS :
  `py D:/Layrs/tmp/build-checkout-boundary-partial-r99.py --r98-terminal-observed`
  puis `py D:/Layrs/tmp/prepare-and-release-checkout-boundary-partial-r100.py --root-accept-source-review`.
  R99 lancé UNE fois après r98 terminal+archive, **ROOT25872 terminal exit0** :
  build/store-lib-no-run PASS52,85s puis workspace/all-targets PASS24,48s. Aucun test
  dans cette commande. Préparation/release r100 exécutée UNE fois, **ROOT93289 terminal0**.
  R100 retient673sources, cinq fichiers revus différents de r98, six recettes.
  Release SHA256 `539e46b3885271ee6509313f6ec30baac1871e5228b8f995c2d2193405db11dd` ;
  EXE82814464octets SHA256 `e0a903f03f7907b416a1932f1a047c72d8716c0fae866590b98ab89369025495`,
  CodeView621a98b1-e6bc-410a-9a0f-44ddcc208f5e age141.
  Runner `py D:/Layrs/tmp/provider-ordinary-checkout-boundary-partial-run-r100.py r100`
  lancé UNE fois avec accès Windows autorisé, **ROOT25207 actif** : borne C10
  PASS21,191s, private Ready SQL PASS16,336s, family publication SQL démarrée.
  Aucun autre Cargo/SQL/natif simultané ; packager r100
  NON exécuté, à lancer seulement après observation du terminal. Ne pas relancer.
  Ils exigent r98 cinq PASS, sources figées conformes et revue sans finding ;
  la rétention r100 exige aussi archive r98 et build/check r99 terminaux PASS.
  R100 retiendra 673 sources et six recettes. Aucun test C13 dans ce lot.
  Si r98 échoue, conserver l'échec, ne pas contourner ces assertions.
  Packager `package-checkout-boundary-partial-r100.py` également préparé,
  AST PASS, NON exécuté ; exige résultat r100 terminal avant usage.
- Proposition C13 T3 **terminée**, thread lu jusqu'à position94/hasMorefalse.
  Bundle tmp/c13-companions-source-proposal/ : 17 fichiers, 16 Rust, rustfmt PASS.
  Manifest SHA256 `62b21e4638d7d1aa42a51f67c13387b56eb9383285c5ff6403cf4771552844aa`.
  Dépôt principal inchangé. Codec v2, owner dérivé du reconcile, image complète,
  plan DATA et primitives SQL proposés. **U1–U7 non achevés** : raccordement actif,
  installation/protection, publications, Ready, réexport9/10, autonomie, qualification.
  Les vecteurs d'intentions/représentations dans l'en-tête64Kio sont une limite
  fonctionnelle à garder explicite ; aucun débit/quota augmenté implicitement.
  Ne pas accepter ce bundle comme C13 terminé ou comme preuve runtime.
  Proposition d'origine archivée sans intégration :
  c13-companions-source-proposal-20261028.tar.gz,115471octets,25entrées,
  SHA256 `b276be0f3485597acb46f38b99c50f2bb2f4b08f16addd20373e92973e6a744a`.
  Archive et inventaire dans docs/continuation/worktrees-partial-vfs, à commiter.

Prochaine tranche : raccordement C13 au parcours Provider réel et préservation
avant Ready ; conserver les garanties Ordinary existantes. Les tests r98 acquis
ne seront pas répétés sans changement pertinent. Le reste du programme L0–L9
reste à poursuivre ; aucune réussite de lot ne clôt le goal complet.

Suite C13 terminée via T3, GPT-6.1-Sol/high, shared avec dépôt READ ONLY,
sorties uniquement tmp/c13-provider-driver-source-proposal/ :
taskId node:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3A9c871aa5-1484-4824-a6d5-65b0d88f354f.
threadId thread:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3A9c871aa5-1484-4824-a6d5-65b0d88f354f.
Mission : U1–U4 et autonomie Provider-produced U6, test intégré réel ; réexport
Ordinary9/10 U5 explicitement différé. Limite v2 64Kio conservée dans cette tranche,
avec refus précoce testé et limitation documentée ; aucune refonte DDL en parallèle.
Premier appel cellule803 refusé AVANT création child : option `effort` inconnue.
Capabilities reconsultées ; nouvel appel utilise `reasoningEffort: high`, accepté.
Statut completed, thread entièrement lu jusqu'à position79/hasMorefalse.
Livraison encore **partielle, driver non implémenté** : 23 fichiers cumulés,
manifest SHA256 `0033a04e8831186c79b0d3b8581e1d36d1f5083a56470194c0a672ff4186835c`,
patch SHA256 `fd41a6a42d91e0ad4ebdf702421ee3716c9a327a65d96af53803516efc28e1ba`.
Lecture héritée v2 et union exacte raccordées dans la proposition ; deux recettes
non exécutées ajoutées. Rustfmt/check22Rust PASS. Pas d'intégration ni compilation.
Blocages CONCRETS : aucun caller produit Provider8 de conversion ; aucune
observation d'autorité couvrant la sélection entière acquise côté client ;
protections/mappings source avant FK du reçu et nouveau Ready restent à raccorder.
Voir C13-PROVIDER-DRIVER-HANDOFF.md dans ce nouveau bundle. Ne pas redéléguer la
même mission sans traiter ces dépendances ; aucun Cargo/build/SQL/natif exécuté.
Bundle cumulatif retenu sans intégration :
`c13-provider-driver-source-proposal-20261028.tar.gz`, 149566 octets, 32 entrées,
SHA256 `9fe82ed476e8eab82b7c9ef1ca49755024c6aad58d010ea8f35e894e98074ae4`.
Les 23 sorties et les bases Git ont été vérifiées avant archivage ; les 31 fichiers
du bundle ont été relus dans l'archive. Inventaire adjacent, à commiter. Premier
essai d'archivage arrêté avant toute écriture sur un décompte erroné des fichiers
annexes (8, pas 7), puis corrigé. Aucune qualification runtime C13.

Investigation source C13 également terminée et lue entièrement par ROOT :
`docs/continuation/worktrees-partial-vfs/c13-source-ownership-design-20261028.json`.
Décision d'architecture pour l'implémentation suivante, NON implémentée/qualifiée :
réservation fixe et racine exacte de rétention appartenant à B ; C reste seul
installateur et propriétaire des débits. Aucun emprunt au budget Capture ou C,
aucune augmentation du quota. Les validateurs doivent reconnaître explicitement
ce nouveau groupe et conserver les contrôles de complétude et les fences GC.
Après C privé ET famille Ready, insérer atomiquement dans B les mappings exacts,
compagnons, claim, reçu et passage Staging→Ready ; puis fermeture reconnue,
relecture et passage Ready→Published séparé. Le préfixe historique reste inchangé.
Cela corrige le contrat temporel de la proposition non intégrée : protéger B et C
avant l'un ou l'autre reçu, mais persister les lignes de chaque base avant SON
reçu. Les deux bases n'ont pas besoin de leurs lignes avant le reçu de C.
Le prochain diff devra modifier explicitement ce texte de proposition et tester
les états B réservé seul, B protégé/C non Ready, C Ready/B Staging et B Ready.
Pas de changement DDL nécessaire identifié ; les mappings limités à l'en-tête
ne respectent pas les contrats v2 actuels. L'exemple de formule Q_B dans le
rapport n'est PAS une formule acceptée : son calcul déterministe, ses bornes et
son coût doivent être établis avant les effets et vérifiés dans le prochain lot.
Ne pas refaire cette investigation ni la revue des cinq sources test inchangées.

Investigation C14 ciblée terminée par programme_remaining_inventory :
tmp/c14-authority-integration-map.json, SHA256
`c5872ab4271f7b78909944c6e6c7146edc1e481f0ad31a3a03334257041b04d8`.
Chemin existant : NativeRemote bootstrap → projection_page complète →
projection_access pour tous anciens/nouveaux chemins et absences → manifest
metadata seulement → fence finale. fetch_plan ne compare pas automatiquement
l'epoch de Binding.observed ; contrôle explicite requis. Aucun lease serveur
persistent ; HTTP avant gates Store puis revalidation de l'ancrage source.

Implémentation de cette dépendance client terminée, GPT-6.1-Sol/high, T3 async :
taskId node:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3A1deb3b3b-eba7-437b-9940-6c5d91845df4.
threadId thread:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3A1deb3b3b-eba7-437b-9940-6c5d91845df4.
Sorties uniquement tmp/provider-transition-observation-source-proposal/ ;
ownership client-core remote provider_observation + tests + wiring/helper étroit.
Dépôt READ ONLY, aucun Cargo/build/test/SQL/natif/network/fixture/process polling.
Collecteur réel avec constructeur privé, deux stratégies d'epoch, absence refusée
séparée, aucun téléchargement de chunks, aucune mutation Store/remount, tests HTTP
écrits mais non exécutés. Notification reçue ; thread lu jusqu'à position62,
hasMorefalse. Bundle trois Rust : API réelle observe_provider_transition et
revalidate_provider_transition_scope, 21 tests HTTPS écrits, rustfmt/check et
rejeu mémoire du patch PASS ; aucun build/runtime. Handoff entièrement lu.
Patch SHA256 `cba3ebbeaa62a7707ef7482a2e68fcb668146fdd81b4b05b7ede36d9a6eaeb85`.
Revue critique native indépendante `provider_observation_critical_review` terminée,
SOURCE ONLY, sortie tmp/provider-transition-observation-critical-review.json.
Un P1 de compilation POBS-001 : match_indices produit (usize,&str), sentinel char
incompatible. Deux notes de couverture, pas de contournement d'autorisation
concret trouvé. Empreintes début/fin et base conformes ; aucun runtime.
ROOT a traité le petit correctif sans redélégation dans une candidate distincte
tmp/provider-transition-observation-corrected-proposal/ : seul changement produit
est le sentinel string "/". Tests renforcés : 100001 absences triées uniques sans
HTTP, renouvellements cohérents modifiés (descripteurs/digest/ranges) passant les
validateurs de plan avant refus du collecteur ; bearer vérifié, digest de fixture
opaque distinct du wire ; nom du test cache limité à l'epoch réellement testée.
21 fonctions, cas supplémentaires non exécutés. Rustfmt/check et Git apply --check
PASS ; compilation/runtime restent requis. Original et revue inchangés conservés
sous reviewed-input. Manifest corrigé SHA256
`35bae32f11b6d3f31d19d340c886b984484146a31259b6e1522203e5d68b0861` ; patch
`8a409e6ce5feae17449d2b623feda66fae44719d536a806b4f51ad28852c7121`.
Candidate corrigée archivée et relue une fois, 14 entrées/82456 octets :
provider-observation-corrected-proposal-20261029.tar.gz, SHA256
`f6517656cc73d738bcf290c9e4833465d70b3c49c5266bba2ce3d494b4234d7c`.
Inventaire adjacent, à commiter. Original et revue inclus ; aucune qualification.
Ne pas réappliquer l'ancienne candidate ni refaire sa revue inchangée. Pas de
build/test concurrent avec r98 ; préserver le delta r99/r100 déjà figé avant
intégration des propositions Provider/client.

Implémentation source B terminée via T3 GPT-6.1-Sol/high, dépôt READ ONLY :
taskId node:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3A4d8663ee-8893-4192-ba0f-08c0ec276920.
threadId thread:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3A4d8663ee-8893-4192-ba0f-08c0ec276920.
Mission exacte tmp/c13-source-protocol-implementation-task.txt ; sorties uniquement
tmp/c13-source-protection-source-proposal/. Raccordement réel réservation/racine B,
preuve privée/famille complète, financement et publication atomique source,
préservation du préfixe et des gardes C Ready. Pas de Cargo/build/test/SQL/natif.
Notification reçue, thread lu jusqu'à position113/hasMorefalse. Bundle cumulatif
53 fichiers dont 52 Rust ; original23 conservés, rustfmt/check PASS, aucun build.
Manifest SHA256 `601c7e851214849c418c04d9852c8ccd1622243c8899a921e9e9c93aa3516c69` ;
patch SHA256 `c1e53502d0691c1d27727435c555f0d5a70fece7f6f33aab20bbe3b098290987`.
Handoff lu entièrement : validateurs privés/famille et modes complets, réserve
B distincte et budget exact R19 sans plancher16MiB, protection CAS/image, writer
source atomique et scans autonomes raccordés. Les constructors C Ready et le
lease legacy gardent leurs refus de nouveaux compagnons ; conversion non finie.
Reste précisément le lease conservant l'image complète/reconcile et le Ready
compagnon réel ; pas de permission pour contourner les anciens guards.
Bundle53 retenu et relu UNE fois :
`c13-source-protection-source-proposal-20261029.tar.gz`, 312784 octets, 74 entrées,
SHA256 `078ebf7c45cb0f55a823b14b36b026942238828dc86600637360b2695d8eb5f8`.
Inventaire adjacent ; toutes les 53 sorties et bases Git vérifiées. Rétention
de proposition, sans acceptation compilation/runtime. Handoff JSON et recettes
également lus entièrement ; trois tests réels écrits, autres recettes prospectives.
Deux missions distinctes préparées pour T3 background, sorties sans conflit :
`tmp/c13-source-protection-critical-review-task.txt` (revue Astra/xhigh du lot53,
sortie `tmp/c13-source-protection-critical-review.json`) et
`tmp/c13-complete-lease-ready-implementation-task.txt` (Sol6.1/high, raccordement
concret image complète/lease/Ready/publis, sortie cumulative
`tmp/c13-complete-lease-ready-source-proposal/`). Cette dernière doit joindre les
appels de production et écrire une recette intégrée, pas livrer seulement des
interfaces. Elle conserve les 53 sources et fusionne la vue locale neuf fichiers.
Client host complet, denied-absence durable, reprise avec nouvel owner et réexport
Ordinary9/10 restent explicites hors tranche. Aucun Cargo/test concurrent autorisé.
État exact des délégations dans leurs résultats T3 ; ne pas les relancer sur
la seule base de ce paragraphe préparatoire. Goal confirmé active au 29/10 local.
Les deux délégations ont été acceptées et lancées :
revue taskId node:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3Afcb60a92-64ce-431e-87f8-1e3dc34fbec3 ;
implémentation taskId node:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3A33e27463-7cc9-443b-b7c9-d266c63aeccc.
Les threadId correspondants remplacent le préfixe node: par thread:.
Revue fcb60 terminée, thread lu jusqu'à position114/hasMorefalse ; implémentation
33e274 encore active (steering confirmé), aucun nouveau lancement ni polling.
Rapport retenu dans docs/continuation/worktrees-partial-vfs/c13-source-protection-critical-review-20261029.json.
Trois findings concrets : CR001 P1 snapshot FS après ACK refusé, CR002 P1 INSERT
suppression8valeurs/7colonnes, CR003 P2 collection4096racines pouvant retenir64GiB.
ROOT a corrigé dans `tmp/c13-source-protection-review-fixes/`, sept fichiers en
overlay du lot53, original inchangé : reader strict, sept placeholders, parcours
streaming avec callback de validation avant racine suivante et seuls IDs conservés.
Les trois callers sont raccordés ; deux recettes SQL nouvelles écrites (première
insertion/replay/conflit et streaming/refus avant payload suivant), NON exécutées.
Rustfmt/check et apply --check sur le lot53 PASS. Manifest SHA256
`b1d05862814f29e011b6c64e126bd4cb2096efd9da1af80cb139ad5ce26a8fcd` ; patch
`943972e50f11ab092d29b16fbc8bc9d9f781e1d42d174b2f76464365d03d9413`.
Correction transmise par steering à33e274 pour fusion cumulative ; pas de nouvelle
revue déléguée pour ces corrections. Compilation/runtime et validation intégrée
restent à faire. W3/W4 doivent parler de close SQLite, pas de close OS observée.

ROOT a corrigé une dépendance réelle distincte dans
`tmp/provider-observation-prefix-domain-fix/` : le normaliseur calculait toujours
source_prefix_id avec le domaine v1, même pour les préfixes v2 valides ; il appelle
désormais prefix.digest(), versionné. Deux fichiers, une régression DATA v1/v2
via le normaliseur réel ; v1 reste inchangé. Rustfmt/check et apply --check PASS,
aucune compilation/exécution. Manifest SHA256
`a6ece02687428485849bb451c51a14e128b3c11f96ccf1d6258a228b41f5ef43` ; patch
`e0257ead1139be9a9fc0366e090a3a2140a8f05839ac860911019793af7db340`.
Petit correctif relu par ROOT sans nouvelle revue déléguée. L'implémenteur lease
reçoit ces deux fichiers par steering pour fusion cumulative ; dépôt principal
et gel r99/r100 restent inchangés. Ne pas considérer ce fix comme C14 durable.

ROOT a écrit une proposition distincte de vue locale :
tmp/provider-transition-source-anchor-proposal/, neuf fichiers dont deux nouveaux.
API publique ProviderActiveSession::transition_source(cancel) →
ProviderTransitionSource<'session>, puis revalidate(cancel). La vue emprunte la
session et conserve la preuve complète, relâche les gates avant HTTP, refuse
capture/transition pendante et compare l'état complet après nouvelle lecture.
Ne fabrique ni prefix_id ni image_digest : le futur driver doit encore les
tirer de la closure/checkpoint réelle et les revalider. Aucun grant writer.
Helper from_publication factorisé dans PrivateCaptureMutationValidation ; le
délégué source B a reçu les interfaces et doit préserver le nouveau companion
proof dans read_live/require_same_state. Fusion étroite de validation.rs à faire.
Manifest SHA256 `b715fce9b14728026def9cf64fcbe81fb237d80b2a8fcc01611c3ebcd315e7bf` ;
patch SHA256 `fd1a62a97267204fd1001245418133eddc08f03f1f00e14f5a944b8fd56e257a`.
Huit Rust rustfmt/check PASS ; projection.rs garde son format historique avec
un réexport ajouté. Git apply --check PASS après correction des headers new-file
du générateur. Aucune application au dépôt, compilation/exécution/revue indépendante.
Recette native écrite : lecture sans writer/hydratation, déconnexion/annulation,
vraie Capture intervenante → ancienne vue refusée/nouvelle acceptée ; la preuve
d'image reste séparée. Child3600/outer3660 proposés non mesurés. Voir HANDOFF.md.
Archive de cette proposition locale créée et relue UNE fois, 14 entrées/38261 octets :
provider-transition-source-anchor-proposal-20261028.tar.gz, SHA256
`3143877c4671fa8d7ce98cb76fd77620a0b3a30648601b5738a00a4bd313913b`.
Inventaire adjacent, à commiter. Rétention de source partielle, aucune acceptation.
Revue native indépendante de cette vue locale terminée sous
`provider_transition_source_review`, SOURCE ONLY, aucun finding. Rapport lu et
conservé sous docs/continuation/worktrees-partial-vfs/provider-transition-source-anchor-review-20261029.json.
Ne remplace pas la revue critique du driver complet après jonction des capacités.
Investigation bornée C14 terminée par `programme_remaining_inventory` : stockage durable
exact des denied-absence paths, contraintes codec/payload/scans/replay/autonomie,
sans se limiter à un hash ou à un refus permanent. Sortie attendue
tmp/c14-durable-denial-contract.json, lu entièrement, retenu sous
docs/continuation/worktrees-partial-vfs/c14-durable-denial-contract-20261029.json.
ROOT retient l'architecture suivante, protocole durable NON encore implémenté : document canonique complet
dans projection_observations existante, liens compacts obligatoires intent/receipt
v2 et rowset import-prefix v3. Les limites 100k/8MiB restent partagées ; document
<=64MiB, plans/SourcePublished<=256KiB et receipts<=64KiB. Pas de nouveau DDL ni
champ optionnel silencieux v1, pas de cycle document→plan→document. Le document
doit être copié/authentifié et rester un fence après admission jusqu'à observation
explicite suivante ; old offline Observation conserve son lecteur strict séparé.
Ce travail n'est pas clos par un simple refus, ni par un digest sans les chemins.

Première couche C14 écrite par ROOT dans `tmp/provider-denied-coverage-record-proposal/` :
trois Rust (module + codec + sept tests DATA), `TransitionDeniedCoverageRecord`
dérivé du normaliseur réel, corps complet canonique, référence compacte, contrôle
des ancrages/version/image/autorité, lecture corps+clé+ref, chemins/alias/bornes
avant copie et désérialisation bornée. 100k chemins/8MiB partagés avec namespace
visible, document64MiB ; aucun grant, writer, version de plan/reçu ou fence actif.
Tests de pleine capacité, tamper, canonicalité, normaliseur et annulation écrits
NON exécutés. Rustfmt/check et apply --check PASS ; main et tous autres bundles
inchangés. Manifest SHA256 `8e21ffab7ab08be56d3108a99dede4529387b4f59b9c859e4f56f07369fd0bfd`,
patch `cc7f278c561ee72053aa3b1a89fdbde21bbe107ac070cc191270b3547ef44022`.
Archive relue UNE fois, 8entrées/19982octets :
provider-denied-coverage-record-proposal-20261029.tar.gz, SHA256
`a98e2cfa0e6373bd7e5a171518eba75f47032a91e20aa8bf62a6f0b5379f2999`.
Inventaire adjacent ; reste toute intégration v2/v3, Planned+document atomique,
SQL scoped, copie, Ready/admission/réexport et fences courantes. Ne pas annoncer
la persistance achevée et ne pas élargir le chantier C13 en cours à cette couche.

Goal **actif**, programme complet ; aucun push/PR. Base des revues `e2a1ac4`.
r66 admission fraîche et r69 publication fraîche : cinq PASS chacun, archives
vérifiées. Reprise fraîche Staging : revue r70 sans défaut, build/check r71 PASS.
Runner r72 **ROOT17382** terminé exit1 : cinq régressions PASS, premier natif
FAIL101/Windows0xc00000fd après642,474s (stack overflow), second non atteint.
Archive vérifiée. Cet échec est désormais suivi des deux succès natifs r79.

Revues **r73/r74 terminées sans finding**, threads lus au résultat final.
Garde Ready et recette C10→D10 compilées r75 ; instrumentation de diagnostic
Staging uniquement test compilée/check r76. r77 terminé : SQL Ready PASS,
natif FAIL ; traces localisent le débordement à open/validated-read puis
validate/observation. Correction privée Box<Validated> compilée/check r78,
réduction de pile compilée mesurée. Runner **ROOT22841/r79 terminé exit0**,
deux natifs Staging PASS599,491s et598,976s, pile inchangée ; archive vérifiée.
Réexport C10→D10 : r80 refus sandbox avant fixture ; r81 FAIL1011189,026s sur
un témoin de budget trop précoce, avant admission D. Archive conservée. ROOT a
corrigé le témoin avec comparaison exacte du nettoyage autorisé source/famille.
Lot familial revu r82 (remarque P3 corrigée), build/check r83 PASS. **ROOT73332/r84
terminé exit0**, sept PASS : quatre courts, réexport1309,558s, famille609,067s et
famille ACK600,052s. Archive vérifiée. Le passage privé frais et les corrections
Ready retenues passent les revues r85/r86. Compilation r87 échoue sur un motif
de test incomplet ; correction ROOT d'une ligne, build37,27s et check14,40s r87b
PASS. **ROOT38516/r88 terminal exit1** : quatre SQL PASS, premier natif échoue
dans le témoin des reçus hérités, avant le driver privé frais ; quatre natifs
non atteints. Archive conservée, témoin corrigé en source. Lot r89 revu sans finding,
build/check r90 PASS. ROOT75810/r91 actif : deux SQL et deux natifs privés frais PASS,
retained privé ACK PASS559,386s, famille Updated en cours. Checkout r92 revu,
R92-01 corrigé en source par ROOT ; compilation en attente. Ne relancer
aucun runner/script déjà exécuté ; les sections finales donnent l'état exact.
Les sections finales donnent hashes, chemins, périmètres et scripts déjà exécutés.
Le reste de ce document conserve la chronologie des preuves, y compris échecs.

L'utilisateur a explicitement demandé « reprends ! /goal resume ». La consigne
d'arrêt nocturne est levée. Le programme complet reste à poursuivre, sans push
ni PR. À la reprise, `get_goal` affiche encore `paused` ; l'API disponible ne
permet pas de réactiver ce statut. Cela ne retire pas l'autorisation de travailler.
Mise à jour : après la nouvelle demande explicite « resume (sans arrêter le goal) »,
`get_goal` confirme maintenant **active**. Le périmètre reste le programme complet.

## Ready et publication familiale

Baseline sauvegardée : `3291ab5` (Ready WIP), `9ab84e0` (famille WIP).

- Échec Ready r44 confirmé dans le témoin de test : il omettait la base
  sélectionnée, absente de l'historique copié mais membre de l'union protégée.
  `ReadyContent` fournit désormais explicitement base/historique/courant. Le
  témoin vérifie l'union exacte, le préfixe historique seul et l'ajout exact
  base+courant. Toutes les comparaisons complètes de tables sont conservées.
- La recette familiale possède deux scénarios sur fixtures distinctes : ACK
  retenu après fermeture réussie, et succès `Updated` avec relecture fraîche,
  chacun suivi de `AlreadyReady` sans writer. Ce second scénario couvre
  l'observation E1 de la revue r45, sous réserve de réussite à l'exécution.
- Build libtest r46 réussi (50,32 s), `cargo check --workspace --all-targets`
  réussi (29,73 s). Les avertissements existants restent présents.
- r46 : 3 tests purs PASS, premier SQL familial FAIL lors du nettoyage.
  Le corps avait déjà fermé la transaction ; la fixture extérieure exécutait
  un second ROLLBACK. Correction des deux recettes SQL concernées : la fixture
  conserve la responsabilité de fermeture. Aucun changement produit.
- Build r47 réussi (9,42 s). Snapshot exact : 639 sources, 180 chemins de revue,
  18 recettes. EXE SHA256
  `f074c4d01a7c9ff47f303dfd881586ec746447c965584b40916aba5fc286fe45`.
  Release source/artefact ROOT r47 SHA256
  `aa272b8107fb222bd8cc9d80be8838ddbbb5a598be7331b76f61ee8d5cb6b89c`.
  Revues indépendantes produit r43/r45 conservées ; corrections de tests
  vérifiées par ROOT. Ne pas présenter cela comme une nouvelle revue indépendante.
- **r47 terminé, 18 PASS** : 15 tests courts (3 purs, 12 SQL), natif privé Ready
  611,529 s, natif famille ACK/rejeu 745,666 s, natif famille Updated/relecture
  785,795 s. Session ROOT `78672` terminée exit0. Ne pas relancer ce parent.
- Archive vérifiée : `ordinary-ready-family-evidence-20261028-r46-r47.tar.gz`,
  1 338 fichiers, 3 822 723 octets, SHA256
  `ffad3ec7ebb04ca48de61bd9bb9a873ab163bb8de0b98adde33f45875380428d`.
  Inventaire correspondant dans `docs/continuation/worktrees-partial-vfs`.

Preuves sous `.layrs-local/validation/provider-family-publication-20261027-r46`
et `r47`. Runner `D:/Layrs/tmp/provider-ordinary-ready-family-run-r47.py`.
Chaque exécution utilise un parent C: neuf. Les fixtures échouées r44/r46
restent conservées et ne sont pas rouvertes. Ces résultats qualifient les seules
portées annoncées ; aucun résultat B Ready/Published/C9 n'en est déduit.

## Suite en préparation, indépendante des artefacts r47

Le commit local `218e625` conserve les trois corrections de tests r47 et cette
note, sans inclure le nouveau lot B Ready/Published. Aucun push.

Le nouveau lot est intégré en source :
partition bornée exacte des reçus, primitives SQL B Ready/Published, nouveaux
lecteurs C/famille sensibles à la phase B réelle, driver physique à deux commits,
nettoyage err-only sans nouvelle validation SQL et invalidation des clones après
rollback échoué. Tests SQL et natifs écrits, pas exécutés. La fermeture positive
de l'owner B reste séparée des résultats de progression ; C reste non admis.

Revue critique T3 GPT-6-Astra xhigh terminée sans finding actionnable ; rapport
`D:/Layrs/tmp/provider-ordinary-source-publication-critical-review-r48.json`.
La cellule ROOT `51` est terminée ; le résultat complet du thread a été lu.
Manifeste de 24 sources :
`D:/Layrs/tmp/provider-ordinary-source-publication-review-inputs-r48.json`, SHA256
`be48500220fdd4f84a8c5c94d17960095afde65734c6928e5a16a2fdc0ccb976`.
Diff SHA256 `7fb83cdbce76fb303667d6ce2c15ab7136692ddbdd3c9a6a253a079f9c06474f`.
Les sources sont restées figées pendant la revue. Recettes r48 proposées : 3 purs,
13 SQL et 2 natifs (nettoyage 240/300, publication B 3600/3660 non mesurés).
Après fin r47, compilation libtest r48 en échec E0603 : import privé depuis le
module partagé. ROOT conserve la preuve fraîche fournie par le driver et la
réauthentifie via require_private, sans exposer l'entrée privée ni réutiliser
une preuve obsolète. Le digest de fixture ci-dessous est également corrigé.
Build r49 réussi (61 s), workspace/all-targets réussi (32,79 s).

ROOT a trouvé et corrigé un défaut dans son nouveau cas natif de nettoyage :
`SET digest='invalid-uncommitted-digest'` viole le CHECK SQL longueur71 avant le
chemin testé. Ce digest est remplacé par `blake3:` +64zéros de longueur71 ; le
rollback et l'assertion d'erreur exacte sont conservés. Les sources originales
revues r48 sont reconstruites depuis le diff épinglé et vérifiées contre leurs
hashes, puis conservées dans le lot r49 avec le diff exact des deux corrections.

Qualification r49 terminée, session ROOT `63777` exit1 : 16 recettes courtes
PASS (3 pures,13 SQL dont publication B en43,002s), puis timeout natif nettoyage
child240 à240,052s, kill/reap confirmés ; publication B non atteinte.
Ne pas relancer ce parent ni rouvrir ses fixtures. Le lot produit est sauvegardé
dans `3717c59` (`wip: preserve ordinary source publication candidate`).
EXE SHA256 `dc237db7d62e0d90b30ccbca76e7721d0ad841f7188cc6dfabc36fa04c4e6dcd`.
Release ROOT r49 SHA256 `743bfdca56d45d8e08060f69e9e02a16a8325e456b20b004fddcef89fdedf14e`.
644 sources,186 chemins sélectionnés,18 recettes. Les lacunes close/rollback OS,
bornes maximales et owner-loss signalées par la revue restent explicites.

r50 conserve le produit r49 identique et ajoute seulement des checkpoints de
progression au test nettoyage, avec child1800/outer1860 diagnostiques. Build
libtest réussi. EXE SHA256
`c2aa43f9f65e0141180680cdba05db033ed41c9bbfc278e01bea7dabebbd89d8`.
Release r50 SHA256
`3d98ca6149bd8cdb43a3f1ea479df33a15d93b35e6f926ea9a7ba7ff1e6b88f6`.
Exécution r50 échoue immédiatement (0,677s) sur accès refusé au parent
`C:/Users/ALEXA`, avant création de fixture ; résultat conservé.

r51 réutilise exactement le binaire et les644sources r50 avec un parent C: neuf
et l'exécution autorisée hors sandbox. Session ROOT **97632**, runner
`D:/Layrs/tmp/provider-ordinary-source-publication-run-r51.py`.
Session97632 terminée exit0 : nettoyage **PASS356,977s**, publication B
Ready/Published et replays **PASS1438,083s**. Ne pas relancer ce parent.
Les16tests courts r49 ne sont pas présentés comme des exécutions r51.

## Lot C10 : historique de préparation et qualification r56/r57

Décision : admission Ordinary8→10 avec témoin complet local des triples, sans
changer le schéma9 Provider. Voir la note `ordinary-autonomy-evidence-gap`.
Travail en cours : permit9/10 et témoin borné, budget explicite, reader C8 autonome
et historique10, writer SQL8→10 et handoff BPublished, family terminal fullwalk,
propagation de la vraie version source dans C10→D, gates de restore/cancellation.
Ces sources évoluent séparément du binaire r51 retenu. L'adaptateur physique et
deux recettes natives sont écrits ; ROOT a enregistré le nouveau module de test.
Compilation libtest r53 en échec : visibilité du validateur d'union, type
ImportRowset/TransitionRowset et branche OrdinaryFrozen manquante. Corrections
ROOT ; r54 PASS. Les régressions ajoutées ont révélé un appel de test à une
méthode privée en r55 ; remplacé par le pattern Ordinary explicite. **r56 libtest
et `cargo check --workspace --all-targets` PASS**. Aucun test/runtime C10 exécuté.

Revue critique SOURCE ONLY r52 terminée (T3 GPT-6-Astra xhigh), cinq findings.
Task id `node:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3A35761828-6071-4260-8e0c-b99237f909a8`.
47 sources conservées, manifeste
`D:/Layrs/tmp/provider-ordinary-admission10-review-r52-inputs.json`, SHA256
`081d1dd8b2338c8f2db0f00c2bc7b26339920c3a87b34c8f0202773b1c5c01b5` ; diff SHA256
`4da0d5cfab66dbe62a7fa1edb66b201fb97692a02445666f961e6bf7bb93c35c`.
Snapshot exact dans `D:/Layrs/tmp/provider-ordinary-admission10-review-r52-source`.
Rapport reçu `provider-ordinary-admission10-critical-review-r52.json`.
ROOT a corrigé : visibilité, tri des triples indépendant de l'ordre ManifestId
de la racine, ancêtre10, dispatch historique conversion9|10 et deux gardes
ordinary/reclamation9|10. Régressions dans ordinary_tests et import_validation/tests :
ordres opposés FileId/ManifestId, lectures capture/publication/origine restore,
refus Generation sans scope, C10 interprété comme ancêtre avec reçu D déclaré
et refus d'une closure altérée. Ce dernier cas ne qualifie pas une admission D
réelle ni une chaîne physique D→E.

Contre-revue SOURCE ONLY r56 terminée : cinq corrections confirmées en source,
aucun nouveau défaut concret établi ; intégrité des59sources conforme début/fin.
Cellule ROOT156 terminée ; thread lu jusqu'au résultat final position88.
Manifeste `D:/Layrs/tmp/provider-ordinary-admission10-review-r56-inputs.json`, SHA256
`b01f5c4cf6334cd6e8b3e6e118ede0331c62837c24d361053034984562987f44` ; diff SHA256
`c92c9aad736b271d809cb2fc50fa25f681f463ad905f3fc8aeae7e027590fe77`.
Rapport reçu et lu, acceptation ROOT limitée à l'exécution des recettes :
`D:/Layrs/tmp/provider-ordinary-admission10-critical-review-r56.json`.

Worker `/root/ordinary_admission_schema` a terminé séparément le driver physique
`managed_export/admission.rs`, registration dans `managed_export.rs`, trois
helpers identity-only dans protection/checkout/managed_checkout_fs et les recettes
natives dédiées (helper dans source_publication_tests, nouveau admission_tests).
Ces fichiers, hors revue r52, ont été intégrés puis examinés dans r56.
Pas de réacquisition owner C ; holder emprunte allocation/protection/checkout.
Nouveau B RO indépendant et famille RO traversent C COMMIT/fermeture ; les lectures
fraîches suivent seulement toutes les fermetures positives. Resolve même holder
sansB/writer ; nouvelle ouverture après perte d'owner reste à faire.

Artefacts r56 retenus dans
`.layrs-local/validation/provider-ordinary-admission10-20261028-r56` :650sources,
EXE SHA256 `89b263834d7bf4ab9e76c7e55cce4fda2d2e2c3a6c08aa39646e9257826c3985`,
CodeView age127, EXE/PDB et9sources startup liés. Release ROOT r56 SHA256
`119fc59deb8eb5d66252f37caaaa3f688cd8f2a9a14afea7009370f2bb0294ca`.
Runner `D:/Layrs/tmp/provider-ordinary-admission10-run-r56.py`, session ROOT57640
terminée exit1 :15premières recettes PASS, puis assertion de test incorrecte
comparant l'union complète dans deux ordres différents (17,806s). Natifs non atteints.
Correction ROOT test-only : trier le vecteur attendu par ManifestId pour comparer
l'ordre réel de la racine ; aucune ligne perdue, aucune modification produit.
Build libtest r57 PASS ;650sources comparées au lot r56, seul ce test diffère.
EXE r57 SHA256 `2500ee6d6ac2bef1447535750f400d75e414434672823e6c1d3bfb30a2c1057e`,
release ROOT SHA256 `3e62ff590f1ebd0b5e5e363a1ba1d67443c5e06c53c4177f46b41f72fb5fcc98`.
Runner `D:/Layrs/tmp/provider-ordinary-admission10-run-r57.py`, session ROOT23741
en cours : test complet SQL C10 **PASS42,332s**, premier natif admission
**PASS1580,483s**, second natif ACK/resolve commencé. Ne pas conclure à un PASS
du second scénario avant sa fin. Les15PASS r56 ne sont pas rapportés comme exécutions r57.
Les scripts de préparation/release ont déjà tourné ; ne pas les relancer ni
rouvrir les fixtures. Ne pas compiler ou lancer d'autres tests pendant les natifs.

Suite fonctionnelle après qualification C10 : réutiliser le scénario existant
`session/managed_admission_tests.rs::managed_admitted_ordinary_edit_capture_restore_without_source`
sur une nouvelle destination C10. Le helper `with_admission_hooks` conserve
actuellement tous les pins jusqu'à sa sortie ; faire retourner seulement les
chemins/digests DATA après teardown avant toute nouvelle ouverture mutable.
Rendre indisponibles les chemins sources de cette fixture seulement, après
contrôle des chemins absolus et sans suppression ; vérifier capture, lecture
héritée, restauration/inverse et intégrité de l'admission B. Ne pas prétendre que
le resolve avec holder retenu r56 couvre cette réouverture, ni perte d'owner/crash.

La recette r51 nettoyage localise le coût principal : publication destination de
10,163s à297,008s, admission terminée305,217s, recette terminée356,885s. Le natif
B publication est maintenant terminal PASS. Archive r49–r51 créée et vérifiée :
`ordinary-source-publication-evidence-20261028-r49-r51.tar.gz`, 2014fichiers,
5895640octets, SHA256
`c3e18a26146fd1e1026225970e3ed6dad9fe5768e8eda57c5b4c52f93003ef66`.
Le script `package-ordinary-source-publication-20261028-r49-r51.py` a déjà été
exécuté, ne pas le relancer. Il conserve le timeout r49 et le refus d'accès r50.

Les entrées existantes Staging restent strictes ; les nouvelles entrées
réauthentifient la phase réelle B. Les données Staging reconstruites servent
uniquement aux calculs et ne prouvent pas une phase stockée.

Le programme reste incomplet : admission C10/autonomie,
reprise après perte d'owner/crash, couverture native complémentaire, transitions,
companions/expansion, CLI/Desktop et acceptation complète. Voir la passation
nocturne et `worktrees-partial-vfs-ordinary-publication-next.md` pour le contrat.

## Suite r58 en source pendant les natifs r57

Commits locaux : `31d08d2` conserve le lot C10 revu et l'archive B publication ;
`27fda1f` conserve la correction test-only de l'ordre des manifestes et le PASS SQL.
Aucun push. Les scripts/builds r56/r57 ne doivent pas être relancés.

Trois missions natives terminées, aucun build/test/SQL par les agents :
`ordinary_c10_autonomy_recipe` (implémentation recette),
`ordinary_ready_reopen_scope` (investigation),
`ordinary_terminal_reopen` (implémentation production). ROOT a enregistré le
module de test et intégré la résolution terminale au scénario d'autonomie.

Nouveaux types `managed_ready_reopen::OrdinaryReadyInspection` (C8, owner Shared)
et `OrdinaryTerminalResolution` (C10 inchangé, owner Exclusive), chacun `open`
et `require`. C10 expose `admission_digest` et `admitted`. Aucun B ni writer ;
les holders doivent être détruits avant l'ouverture ordinaire mutable.
Le détachement consommant dans `managed_admission/read.rs` déplace l'ExpectedCheckout
et le CheckoutJournal réellement authentifiés ; vérifie inventaire terminal exact,
frontières, backend/FK, témoin complet et enveloppe inchangée. Les manifestations
canoniques viennent du catalogue familial complet. Les scopes SQL ferment
positivement avant les preuves physiques, puis une nouvelle lecture compare.
La nouvelle admission C8 fraîche avec writer n'est pas implémentée dans ce lot.

Recette proposée, non compilée/exécutée :
`session::managed_ordinary_autonomy_tests::managed_ordinary_c10_reopens_and_captures_restores_after_all_holders_and_ancestors_release`
(child3600/outer3660). Retour de DATA seulement après teardown complet ; chemins
A/B déplacés uniquement après contrôle canonique et exclusion C/famille. Résolution
C10 fraîche sans A/B, puis libération du holder et ouverture publique C, captures
héritées/locales, restauration/inverse, B privé inchangé avec véritable handle et
verrous frais. Le B privé doit rester accessible ; aucun saut silencieux du contrôle.
Après mutations, bytes checkout d'origine rétablis avant le refus terminal attendu,
puis ouverture ordinaire autorisée. Aucun crash, xClose réel défaillant ou D→E.

Contre-revue SOURCE ONLY r58 terminée, cellule ROOT198 clôturée. Thread lu
jusqu'au résultat final position133. Un finding medium R58-F1 : la capture
initiale Ready de B est vide, le test lui demandait à tort des bytes README.
Aucun défaut production concret établi. Huit sources conservées dans
`D:/Layrs/tmp/provider-ordinary-terminal-review-r58-inputs.json`, SHA256
`956681a52b5fd7b7b8f24f64202fa58cedd74b3be668779c20fe2e2d4fa57b0c`, diff SHA256
`c8f29e0581a0fa3e0f5d518614de1937df644c62196ea365ac8e6294d702c18e`.
HEAD de revue `27fda1f480e5986972bf628398fd51dc35c037ed`. Rapport reçu et lu :
`D:/Layrs/tmp/provider-ordinary-terminal-critical-review-r58.json`.
Pas de compilation avant fin des natifs ROOT23741. Les sources r58 restent
séparées du binaire retenu r57. Le script d'archive
`D:/Layrs/tmp/package-ordinary-admission10-20261028-r56-r57.py` est préparé mais
NON exécuté ; attendre la fin du runner avant archivage et vérification.

ROOT a corrigé R58-F1 dans le test uniquement : reconnaissance de la séquence
Ready B réelle, identité B/importée et changes vide ; metadata/origines comparées
pour toutes les captures, streaming seulement des replacements réels, présence
explicite des bytes HISTORY et CURRENT. Aucune capture BASE inventée.
Le test bloque aussi la base privée B avec un handle Windows read/share_mode(0)
pendant toutes les opérations C ; une tentative de lecture doit échouer32.
La comparaison indépendante du baseline B a lieu avant ce blocage et après
libération finale du handle. Checkouts A/B toujours indisponibles.

Contre-vérification r59 en cours, cellule ROOT208 ; portée étroite sur ce seul
test et le guard. Huit fichiers conservés, sept inchangés par rapport à r58.
Manifeste `D:/Layrs/tmp/provider-ordinary-terminal-review-r59-inputs.json`, SHA256
`516a36965bf189c017b92b828c7fe20d43295da20ea42ccf04f0d97c9da73f93`, diff SHA256
`4b12cbb803f328fbe0d8bb1c6b65e25bf9d868e3e08017560e261fa7b8d45262`.
Rapport attendu `D:/Layrs/tmp/provider-ordinary-terminal-correction-review-r59.json`.
Ne pas modifier les huit fichiers pendant cette revue.

Séparément, ROOT a étendu le cas SQL dans `destination_ready/ordinary_tests.rs`
(hors huit fichiers r59) : détachement terminal C10 positif sous le vrai catalogue
familial, append d'une capture locale canonique dans un SAVEPOINT, lecteur générique
acceptant la provenance puis refus par l'inventaire terminal, aucune écriture
de réparation et rollback exact. Non compilé/exécuté ; à qualifier avant le natif
d'autonomie coûteux. Les nouvelles sources restent indépendantes du binaire r57.

## État courant : admission qualifiée, autonomie native r60 en cours

Session ROOT23741 terminée **exit0**. Les trois recettes r57 passent : SQL
42,332s, admission native 1580,483s, ACK retenu/résolution avec holder conservé
1515,529s. Cela ne prouve pas une réouverture après teardown ni un crash.
Archive r56/r57 créée et relue avec vérification de tous les hashes :
`ordinary-admission10-evidence-20261028-r56-r57.tar.gz`, 1464 fichiers,
4460786 octets, SHA256
`d6ece1d7879f753a2442a323073d16694d550a827837917f5be4ea4dacf6b66f`.
L'inventaire correspondant conserve le premier échec d'assertion r56.
Le script d'archive a maintenant été exécuté ; ne pas le relancer.

Contre-revue r59 terminée, thread final lu jusqu'à position34/hasMore=false :
`thread:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3A7fb2e0dd-9df4-47fd-a813-de6b205695da`.
R58-F1 corrigé en source, aucun nouveau finding. Les huit fichiers concordent.
ROOT accepte cette revue de source, sans lui attribuer de résultat runtime.

Build libtest r60 **PASS36,91s** ; `cargo check --workspace --all-targets` PASS.
Artefacts et 652 sources conservés dans
`.layrs-local/validation/provider-ordinary-terminal-20261028-r60`.
EXE SHA256 `c08e2b9d849c8d5044677ebf674b40ec86c7043715e0138c446824c491727e45`,
CodeView age129, neuf checksums de startup vérifiés. Exactement neuf fichiers
Rust diffèrent de r57 : huit revus r59 et le test SQL ajouté par ROOT.
Release ROOT `ordinary-terminal-source-release.json`, SHA256
`04c56c515ceb17a8540eadaf2575cad3a7c67329f99f8a99c5b35ec8ec1f09ce`.
Ne pas relancer `release-ordinary-terminal-r60.py` : ses créations sont exclusives.

Runner `D:/Layrs/tmp/provider-ordinary-terminal-run-r60.py r60`,
**session ROOT48378 en cours**, parent C neuf, séquentiel stop premier échec :
1. SQL C10 avec détachement terminal et refus sans réparation d'un suffixe
   canonique : **PASS38,249s**.
2. Natifs C10 fraîchement rouvert, capture/restauration sous indisponibilité A/B
   et refus de lecture de la base B : **EN COURS**, child3600/outer3660.

Aucun nouveau Cargo, SQL ou autre natif avant fin de cette session. Ne pas
rouvrir la fixture. Les durées sont observées, jamais des garanties maximales.
Le visuel demandé est affiché dans le chat, source conservée hors dépôt dans
`D:/Layrs/outputs/avancement-worktrees-20261028.html`. Il n'arrête pas le goal.
L'investigateur `ordinary_ready_reopen_scope` poursuit en lecture seule la
recherche d'une capacité source B fraîche pour le futur writer d'admission C8.

## Admission fraîche r61 : source figée en revue critique

Le commit local `2ccf824` conserve le candidat terminal r60, ses tests et
l'archive d'admission r56/r57. Aucun push, stash original conservé.
Le script `D:/Layrs/tmp/package-ordinary-terminal-20261028-r60.py` est préparé,
NON exécuté ; il refuse l'archivage tant que le runner n'est pas terminal.

Investigation B fraîche terminée : pas de holder existant complet. Réutiliser
l'enveloppe FS ManagedReadyPins, contrôler explicitement l'identité du control
contre FORMAT, acquérir lifecycle puis owner B Shared, et AdmittedPreflight RO
positivement fermé. Aucun into_handle, Lease::admit, format::Access ou settlement.

Deux agents natifs terminés, source seulement. Nouveau lot de dix fichiers :
`OrdinaryAdmissionSource::open`, `OrdinaryFreshAdmission::open(&source,C,cancel)`
impose B avant l'acquisition Exclusive C ; `commit` et `resolve` renvoient un
digest ou failure step/cause/held. B réel Published et famille sont retenus à
travers COMMIT et close C ; resolve ne consulte ni SQL ni identité B. Le driver
retained, Provider et la résolution terminale C10 restent indépendants.
ROOT ajoute le getter FORMAT étroit, l'enregistrement des tests et un observer
cfg(test), local au thread, RAII, avant read_inner/snapshot et avant identité B.
Aucune modification d'auto-extension, de VFS ou de configuration SQLite globale.

Deux recettes proposées dans `managed_ordinary_fresh_admission_tests.rs` :
- `managed_ordinary_fresh_admission_accepts_advanced_published_b_after_all_publication_holders_release` ;
- `managed_ordinary_fresh_admission_withheld_c_ack_resolves_without_b_reads_or_identity_checks`.
Chacune ignored child3600/outer3660, non mesurée. La seconde prouve une absence
instrumentée de frontière B, pas son absence physique sous pins retenus. Aucun
crash ou échec OS réel n'est revendiqué. Rustfmt isolé PASS ; **non compilé**.

Revue GPT-6-Astra xhigh en cours, cellule ROOT251 (delegate_task mode wait600000).
Manifeste `D:/Layrs/tmp/provider-ordinary-fresh-admission-review-r61-inputs.json`,
SHA256 `4a0d4eee00aa355fbba1c5b0eb5b3be03bc55d3d2a90599e12807a7f5761e21f` ;
diff SHA256 `8f8de85779f42f5bc4c0ca230c8359225b117011986cd1d4a779086e48169756`.
HEAD figé `2ccf824561a511a304332839071576007b143cb5`, dix sources copiées dans
le snapshot adjacent `-source`. Ne pas modifier ces sources ni HEAD pendant revue.
Rapport attendu `D:/Layrs/tmp/provider-ordinary-fresh-admission-critical-review-r61.json`.
Ne pas relancer le script de préparation ; suivre le next de la délégation et
ne pas la poller. Session native r60 ROOT48378 toujours distincte et en cours.

### Résultat terminal r60 et correctif de recette hors périmètre r61

ROOT48378 terminé **exit1** : SQL PASS38,249s, natif FAIL101 après989,252s.
Échec dans l'assertion `managed_ordinary_autonomy_tests.rs:438` : le refus
terminal attendu existe, mais le catalogue familial refuse déjà les racines C
supplémentaires créées par capture/restauration, avant le contrôle d'inventaire
privé. Erreur exacte : `exact protection owner has additional roots`.
Les assertions précédentes du natif ont donc été traversées ; sa réouverture
publique finale et les derniers contrôles B ne sont PAS qualifiés.

Archive r60 créée/vérifiée : `ordinary-terminal-evidence-20261028-r60.tar.gz`,
694 fichiers,2074692 octets, SHA256
`3c8243ccab13820f6814ed6afa85f4a392815a2f13c017fb36f85309ce664908`.
Ne pas relancer le script d'archive ni rouvrir la fixture échouée.
ROOT corrige uniquement le test d'autonomie : assert_eq sur cette erreur exacte,
en conservant bytes d'origine avant refus et toutes les assertions suivantes.
La recette SQL qualifie séparément le refus d'inventaire privé. Ce fichier est
hors des dix sources figées r61 ; aucun fichier de revue ni HEAD ne change.
Build libtest r62 lancé après terminaison du runner, log
`D:/Layrs/tmp/provider-ordinary-fresh-admission-build-r62.log`.

Build r62 terminé **exit101**, un seul E0308 dans
`managed_ready_reopen/ordinary_source.rs:144` : `require_ready_source` renvoie
directement Result<(),ProjectionStoreError> au lieu de convertir en Cause.
Correction mécanique prévue après la revue figée : `require_references(...)?;`
puis `Ok(())`. Ne pas modifier ce fichier pendant r61. Aucun autre diagnostic
compilateur d'erreur ; ne pas transformer ce build en PASS. Prochain build r63.

## Correction R61-F1 et validation suivante

Revue r61 reçue et thread lu jusqu'à position75/hasMore=false, rapport
`D:/Layrs/tmp/provider-ordinary-fresh-admission-critical-review-r61.json`.
Un finding medium confirmé : après ACK C refusé sans échec physique de close,
les lecteurs RO famille/B ordinaires effectuaient encore leurs snapshots FS
de conclusion. Ces snapshots pouvaient remplacer la cause ACK, et le premier
observer ne voyait pas ces opérations. Aucun autre défaut concret établi.

Correction ROOT : nouveau `read_cancellable_strict`, utilisé par les quatre
scopes du nouveau driver frais seulement. La fermeture réelle reste obligatoire ;
toute erreur callback/close est ensuite propagée avant le snapshot final.
Les succès conservent les mêmes comparaisons ; les lecteurs existants gardent
leur comportement de classification/recovery. La cause d'origine reste dans
Cleanup.primary si la fermeture réelle échoue aussi.
Le hook cfg(test) observe maintenant `DatabaseFile::snapshot` ET `verify` avant
FS, en plus de l'entrée RO et de l'identité source. Les contrôles positifs et
le refus après ACK sont étendus. Deux nouvelles recettes SQL courtes dans
`database_io/tests/strict_read.rs` : erreur callback imbriquée ; erreur callback
avec vraie fermeture SQLite BUSY produite par statement retenu. Cette dernière
met le handle en quarantaine jusqu'à sortie du child, sans retry ni simulation
de panne OS. Aucune configuration globale SQLite/VFS ajoutée.

La conversion E0308 r62 est corrigée mécaniquement via `?; Ok(())`.
Build r63 FAIL101 sur comparaison d'un Box<ProjectionStoreError> à une chaîne
dans le nouveau test uniquement ; correction via `.to_string()`.
Build r64 lancé, log `D:/Layrs/tmp/provider-ordinary-fresh-admission-build-r64.log`.
Ces corrections nécessitent une nouvelle contre-revue, pas une acceptation r61.

Build r64 **PASS33,65s**, workspace/all-targets **PASS14,07s**. Aucun test exécuté.
Contre-revue ciblée r65 lancée, cellule ROOT275, GPT-6-Astra xhigh ; attendre son
résultat et suivre son next, sans poller la tâche. Manifeste13 fichiers :
`D:/Layrs/tmp/provider-ordinary-fresh-admission-review-r65-inputs.json`, SHA256
`0af2628d4b38d6df1cba08f4a55df3718143eb5977523b2c658ec158febe8cf5` ;
diff SHA256 `8e33a2b6f245dfc730a9439b22671f9822c50789cb617f3adf1c256eef7b7152`.
Neuf fichiers changent depuis r61/HEAD pour les extensions de périmètre. Tous
les13 fichiers et HEAD2ccf824 restent figés. Rapport attendu :
`D:/Layrs/tmp/provider-ordinary-fresh-admission-correction-review-r65.json`.

Artefacts r66 préparés, **liaison seulement, aucune release ni exécution** :
`.layrs-local/validation/provider-ordinary-fresh-admission-20261028-r66`,657sources,
EXE SHA256 `9e1d64645593e64e61695034158a8df36de7177b9368643ccacd3892f5afd34d`,
CodeView age130, neuf startup checksums et EXE/PDB vérifiés. Script
`prepare-ordinary-fresh-admission-artifacts-r66.py` déjà exécuté, ne pas relancer.
`release-ordinary-fresh-admission-r66.py` préparé, syntaxe parsée, NON exécuté.
Après lecture/acceptation du rapport r65 uniquement, ROOT pourra l'appeler avec
`--root-accept-source-review`. Il refuse les sources modifiées depuis le manifeste.
Programme proposé : les2 régressions SQL courtes, autonomie C10 corrigée, admission
fraîche succès, admission fraîche ACK/résolution. Parent C neuf, séquentiel stop
premier échec. Le runner n'est créé que lors de la release. Aucune session native
ni compilation encore active ; tous les échecs/builds précédents sont conservés.

## État courant r66 : source acceptée, qualification en cours

Contre-revue r65 reçue/lue intégralement jusqu'à position55/hasMore=false :
`thread:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3A11af785e-d59c-4985-9ee4-620ba55e1b30`.
R61-F1 corrigé en source, zéro nouveau finding ;13 fichiers, HEAD/branche et
hashes début/fin concordent. ROOT accepte cette portée pour les cinq recettes
définies ; le rapport ne prétend aucune exécution ou acceptation produit.
Script de release r66 maintenant exécuté (ne pas relancer), SHA256 release :
`7f5ad4198d3743569591f99e4f19645ae727843b305ada99219d10927a1d6e6d`.

Runner `D:/Layrs/tmp/provider-ordinary-fresh-admission-run-r66.py r66`,
**session ROOT7553 en cours**, parent C neuf, arrêt au premier échec :
1. `strict_nested_read_preserves_callback_failure_without_concluding_filesystem_probes`
   **PASS0,534s** ;
2. `strict_nested_read_preserves_primary_and_actual_busy_close_without_concluding_probes`
   **PASS0,118s** ;
3. autonomie C10 corrigée : **EN COURS** ;
4. admission fraîche succès et B avancé : non atteinte ;
5. admission fraîche ACK/résolution sans probes B : non atteinte.

Pas de nouvelle compilation/SQL/natif concurrent avant terminaison du runner.
Le test BUSY couvre une vraie erreur sqlite3_close provoquée par statement
retenu, pas une panne OS ni un crash. L'investigateur ordinary_ready_reopen_scope
étudie en lecture seule la future réouverture de B SourceReady et publication
avec C8/famille Ready, sans réadopter l'allocation ni fabriquer l'image source.
Cette investigation n'est pas une implémentation ni une qualification de reprise.

## Suite source pendant r66 : reprise physique B Ready → Published

Commit local `a2a5549` conserve le candidat admission fraîche, la correction
strict-read et les preuves archivées r60. Aucun push. Goal toujours actif.

L'investigation est terminée : `ordinary_transition::apply_published` vérifie
déjà le vrai B Ready, son frontier et le reçu C exact sans reconstruire l'image
B. Le driver retained est trop couplé à l'ancienne allocation pour servir à une
réouverture fraîche. Le prochain adaptateur possède donc un nouvel owner B
Exclusive, puis C Shared, avec vrais préflights et vérifications physiques.
La capacité doit être recalculée depuis le lecteur autonome C complet
(floor + Jmax, remaining net des octets checkout), jamais fournie par l'appelant.
Les scopes C/famille utilisent le lecteur strict ; le close B positif précède
tout readback. Une résolution explicite relit B Published sans writer ; elle
refuse B Ready ou indisponible. Staging est explicitement hors de ce petit lot.

Deux agents GPT-6.1-Sol réalisent des sources disjointes :
`ordinary_terminal_reopen` possède l'adaptateur/les capacités ;
`ordinary_c10_autonomy_recipe` possède le helper et les recettes natives.
Le helper devra s'arrêter réellement après Ready, fermer B, laisser tous les
holders sortir, puis retourner seulement des DATA. Le hook existant
`before_published` ne suffit pas : il poursuit aujourd'hui vers Published.
ROOT intègre, relit et qualifie. Aucune compilation/SQL/natif supplémentaire
pendant ROOT7553 ; son EXE et ses 657 sources retenues restent immuables.
Ces nouveaux changements ne sont pas encore compilés, revus ou qualifiés.

Progression r66 : l'autonomie C10 corrigée **PASS821,111s**. Les assertions
finales de réouverture publique et d'intégrité B sont cette fois atteintes.
Le scénario admission fraîche avec B avancé est maintenant en cours ; le
scénario ACK/résolution reste à exécuter. Session ROOT7553 toujours exclusive.
Script d'archive `D:/Layrs/tmp/package-ordinary-fresh-admission-20261028-r66.py`
préparé et parsé uniquement ; ne l'exécuter qu'après terminaison du runner.

## Candidat reprise Ready r67 : revue source en cours

Les deux agents d'implémentation ont terminé. ROOT a enregistré le module natif,
relu le diff et renforcé les contrôles positifs d'observation des snapshots et
identités des trois bases. Rustfmt ciblé et diff-check passent. Aucun build ni
test du nouveau lot pendant r66. Les huit fichiers Rust changés et huit fichiers
de contexte sont figés dans
`D:/Layrs/tmp/provider-ordinary-fresh-publication-review-r67-source`.
Manifeste `.../provider-ordinary-fresh-publication-review-r67-inputs.json`, SHA256
`3a658f04430e640b8bcd7db97f5a00beeeef2ded66ff20e90f5576af50fc51c5` ; diff SHA256
`b58e8e0b2e8e70b0ef8e153876a393a073384383252bf09a382de55665d7c38c`.
HEAD attendu `a2a55497ee2dad70ca8098e59f5faf257ade2321`.
Revue critique GPT-6-Astra xhigh lancée, cellule ROOT312 ; rapport attendu
`D:/Layrs/tmp/provider-ordinary-fresh-publication-critical-review-r67.json`.
Suivre le résultat de l'appel en attente, sans polling de la tâche déléguée.

L'investigation du lot suivant Staging→Ready est également terminée et conservée
dans [la proposition dédiée](worktrees-partial-vfs-ordinary-fresh-staging.md).
Elle décrit une image B fraîche réelle et la réutilisation des kernels existants,
sans faux Access ni allocation adoptée. Ce lot n'est pas implémenté ; ne pas le
confondre avec le Ready→Published actuellement en revue.

Progression runtime r66 : admission fraîche après libération de tous les
holders et suffixe cohérent B **PASS785,869s**. Le dernier scénario
`managed_ordinary_fresh_admission_withheld_c_ack_resolves_without_b_reads_or_identity_checks`
vient de démarrer dans ROOT7553. Les quatre premières recettes r66 passent ;
ni le lot complet ni l'archive r66 ne sont encore terminés.

Revue critique r67 terminée : aucun défaut concret, 16 inputs et 10 dépendances
supplémentaires concordants au début/à la fin. Thread lu jusqu'à position55,
hasMore=false :
`thread:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3Ad89f62d8-f972-4b89-8ff2-3c3bc204938f`.
Rapport JSON ci-dessus lu intégralement. ROOT accepte cette revue de source,
sans résultat de compilation ni runtime du candidat. Cellule312 terminée.

Après terminaison de ROOT7553 seulement : build libtest et check workspace
prévus sous logs `provider-ordinary-fresh-publication-build-r68.log` et
`provider-ordinary-fresh-publication-check-r68.log` dans `D:/Layrs/tmp`.
Scripts `prepare-ordinary-fresh-publication-artifacts-r69.py` et
`release-ordinary-fresh-publication-r69.py` créés et parsés, NON exécutés.
Ils vérifient les sources r67, les 659 inputs attendus, les deux logs de build,
les cinq succès r66 et exactement huit changements avant de lier puis libérer
trois recettes SQL existantes et les deux natives nouvelles. La release exige
l'argument explicite ROOT `--root-accept-source-review`.

Visuel actualisé montré inline : `D:/Layrs/outputs/avancement-worktrees-r66.html`,
HTML preview inspecté puis html_render, sans serveur. C'est un instantané
(avant retour de la revue r67), pas un suivi automatique.

## r66 terminé : cinq recettes PASS, preuves archivées

ROOT7553 est terminé **exit0**. Dernier scénario ACK/résolution **PASS746,214s**.
Ordre complet : SQL strict0,534s ; SQL close BUSY0,118s ; autonomie C10
821,111s ; admission fraîche B avancé785,869s ; ACK/résolution sans B746,214s.
Le PASS porte sur ces recettes exactes et leurs limites, pas sur le programme.

Archive créée et relue avec vérification de chaque empreinte :
`docs/continuation/worktrees-partial-vfs/ordinary-fresh-admission-evidence-20261028-r66.tar.gz`,
711 fichiers,2106092 octets, SHA256
`0e13f4e378a8119793930f60059b0d9d6baa76d91c01bbc32503b6a96df30999`.
Inventaire adjacent `ordinary-fresh-admission-inventory-20261028-r66.json`.
Le script d'archive est déjà exécuté, ne pas le relancer. EXE/PDB et fixtures
restent locaux et ne sont pas archivés ; aucun ancien état de fixture rouvert.

Compilation r68 lancée après terminaison native, **session ROOT85188**,
`cargo test -p layrs-store-local --lib --no-run`. Aucun test lancé. Vérifier
son résultat avant workspace/check, liaison r69 et release ROOT.

## État courant r69 : candidat compilé, exécution en cours

Build libtest r68 **PASS32,43s**, workspace/all-targets **PASS12,89s**.
ROOT85188 et ROOT7413 terminés exit0. Aucun changement Rust depuis r67.
Artefacts r69 retenus :659 sources, neuf startup checksums, EXE/PDB concordants,
CodeView age131. EXE SHA256
`3ca51aa2fbdb45aae668135da2702f99ff831c1f792c05240b6335c624f43b99`.
Les scripts de préparation et de release r69 sont maintenant exécutés ; ne pas
les relancer. Les 16 inputs r67 et 10 dépendances supplémentaires concordent ;
les huit seuls changements depuis r66 sont ceux revus. Release ROOT SHA256
`25e314dcf8cf97fa4a7e70d97740a3aee99b267bd8ec86f58e583f3220ecee57`.

Runner `D:/Layrs/tmp/provider-ordinary-fresh-publication-run-r69.py r69`,
**session ROOT2313 en cours**, parent C neuf, séquentiel stop au premier échec :
1. publication SQL Ordinary complète : en cours ;
2. lecteur autonome Provider positif : non atteint ;
3. refus autonome suffixe/journal/phase/débit : non atteint ;
4. publication fraîche B Ready→Published puis replay : non atteinte ;
5. ACK B refusé puis résolution Published nécessitant B : non atteinte.

Pas de Cargo/build/SQL/natif concurrent avant terminaison. Les sources de la
future reprise Staging peuvent avancer séparément du binaire retenu r69, en
respectant la proposition dédiée et une nouvelle revue. Tous les agents natifs
sont terminés ; aucune tâche T3 en attente. Goal actif ; pas de push/PR.

Checkpoint local `e2a1ac460fb422b46b784796ce00f521443f6695` : candidat r67/r68,
archives r66 et documentation conservés, stash original intact. Après lancement,
les trois premières recettes SQL r69 passent :24,177s ;0,873s ;1,226s.
Le natif `managed_ordinary_fresh_publication_advances_released_ready_and_replays_published_without_writer`
est en cours dans ROOT2313 ; le natif ACK B/résolution n'est pas encore atteint.
Ne pas relancer le runner ni les scripts de préparation/release. La passation
présente ce dernier état ; le binaire retenu reste celui accepté avant commit.

## Reprise Staging en implémentation pendant r69

Tour goal précédent = progrès réel (r66 cinq PASS, archive, commit candidat,
r69 trois SQL PASS). Goal revérifié actif. ROOT2313 confirmé encore vivant par
son handle ; pas de redémarrage ni autre exécution native/build simultanée.

Trois agents GPT-6.1-Sol, sources disjointes :
- `ordinary_fresh_image_parts` : `capture/scan.rs` et `managed_export/image.rs`,
  extraction des kernels physiques sans contourner les anciens ContentScope ;
- `ordinary_terminal_reopen` : nouveau `managed_export/staging_reopen.rs`,
  wiring et interfaces minimales source exclusive/C inspection ;
- `ordinary_c10_autonomy_recipe` : helper arrêt réel avant B Ready et nouveau
  `session/managed_ordinary_fresh_staging_tests.rs`.

API convenue : `OrdinaryStagingPromotion::open(B,C,transaction_id,cancel)` puis
`promote_ready(self,close,cancel)` ; erreur garde `step/cause/held`, succès expose
Ready/receipt/digest DATA. Construction strictement Staging. Aucun replay ou
resolve de cet objet qui traiterait Ready comme Staging : après ACK incertain,
libération explicite du holder puis driver distinct `OrdinaryReadyPublication`
sur le vrai état. Les anciennes APIs Shared/Ready/Provider restent inchangées.
Tests proposés : image B réellement modifiée puis restaurée avant acquisition
réussie ; succès Ready exact ; ACK B refusé, arrêt des probes et ouverture fraîche
du driver Ready après teardown. Pas de claim crash/OS close ou arbre complexe.

Script `D:/Layrs/tmp/package-ordinary-fresh-publication-20261028-r69.py` préparé
et parsé, NON exécuté ; attendre la terminaison de ROOT2313, même en échec.

Progression r69 : natif publication fraîche et replay **PASS812,201s**.
Dernier natif ACK B/résolution en cours dans ROOT2313. Les trois SQL passent.

## Candidat Staging r70 intégré, revue source en cours

Les trois agents ont terminé. Dix sources Rust changées : noyaux scan/image,
driver `OrdinaryStagingPromotion`, wrapper `OrdinaryExclusiveSource`, getters
fermés C, helper/tests et enregistrement du module. ROOT a préservé explicitement
l'ingress PortableWindows, ajouté les contrôles positifs de snapshots/identités
sur les trois bases et les vraies lectures B/C/famille après close positif.
Rustfmt ciblé et diff-check passent. Aucune compilation ni recette r70 exécutée.

Manifeste29 fichiers (10 changés,19 contexte) :
`D:/Layrs/tmp/provider-ordinary-fresh-staging-review-r70-inputs.json`, SHA256
`985ff127c0a1fb4d9931b7442e2f56d72f3cff29e9f615401e2a3d5a1282133b` ; diff SHA256
`c8ce7873ac05dae2c60d5fab84d36e1f7cef2ede39a425d778fcf7492687e937`.
Snapshot `.../provider-ordinary-fresh-staging-review-r70-source`, HEAD e2a1ac4.
Le script de préparation a d'abord rencontré un mauvais chemin de contexte
`writer_support.rs` (module en réalité dans `database_io/tests.rs`) ; snapshot
partiel conservé et complété uniquement après égalité exacte de chaque fichier
déjà présent. Manifeste/diff créés exclusivement une fois, aucun input altéré.

Revue critique GPT-6-Astra xhigh lancée, cellule **ROOT372**, rapport attendu
`D:/Layrs/tmp/provider-ordinary-fresh-staging-critical-review-r70.json`.
Suivre l'appel en attente, sans polling de tâche déléguée. Les29 sources et HEAD
restent figés. Après fin native r69 seulement, compilation du candidat possible ;
aucune nouvelle exécution native sans comparaison/release ROOT correspondante.


## Fin r69 et qualification Staging r72 en cours

Le goal reste ACTIVE. ROOT2313 est terminé, exit0 : cinq recettes r69 PASS.
Durées : SQL24,177s /0,873s /1,226s, publication fraîche812,201s,
ACK B puis résolution Published711,188s. Archive créée et entièrement relue
par hashes (sans réouvrir de fixture) :
`ordinary-fresh-publication-evidence-20261028-r69.tar.gz`,702 fichiers,
2082137 octets, SHA256
`f84e1ff5e4a72611488e3fbcaaca499b3d67191136c4df7703b8ef06f75e35fc`.
Inventory adjacent. Ne pas relancer le packaging r69 déjà exécuté.

La revue critique r70 est terminée sans défaut concret ; thread lu jusqu'au
message final/hasMore=false. Rapport
`D:/Layrs/tmp/provider-ordinary-fresh-staging-critical-review-r70.json`.
La compilation r71 passe36,26s, le check workspace/all-targets13,54s.
74 warnings du lib-test, aucune erreur. ROOT accepte les seules sept recettes
définies dans `ordinary-fresh-staging-source-release.json` :29 sources revues,
7 sources supplémentaires,661 inputs build live/retenus concordants, exactement
10 fichiers Rust changés depuis r69. EXE SHA256
`2f8fd4ce31aa1ea0093c9db8f18a3b4c05a2b511ed5594349cc400009d176a55`,
CodeView age132. Release SHA256
`afe79b5c7455cced80165829ff84bad9d2c8701aa12b7e8ed10eeb069261bdb8`.

Runner `D:/Layrs/tmp/provider-ordinary-fresh-staging-run-r72.py r72`,
session **ROOT17382**, artefacts
`.layrs-local/validation/provider-ordinary-fresh-staging-20261028-r72`.
Parent neuf `C:/Users/ALEXA/AppData/Local/Temp/layrs-provider-ordinary-fresh-staging-r72`.
Trois tests purs scan/hash/cancel PASS0,411s /0,015s /0,015s ; refus capture
ordinaire protected/unknown/alias/read-only PASS0,323s ; SQL publication
ordinaire PASS23,821s. Premier natif fresh Staging/drift/succès en cours,
second natif ACK/Ready handoff non atteint. Aucun autre build/test/SQL concurrent.
Ne relancer ni préparation/release ni runner. Les sources futures ne changent
pas le binaire retenu. Bounds3600/3660 proposés, toujours non mesurés ici.

Deux agents natifs GPT-6.1-Sol avancent sans exécution :
- `ready_quiescence_gap` : audit read-only de l'absence de garde opérationnelle
  pending/history dans l'ancien adapter fresh Ready. Aucun contre-exemple
  produit démontré ; possible refus de corruption à préciser. Staging r70
  utilise déjà verify_source et sa garde quiescence complète.
- `source10_nested_export_recipe` : uniquement nouveau test C10→D10 avec ajout
  imbriqué et suppression, helpers source-publication/autonomy minimaux. ROOT
  possède session.rs. Aucun changement production autorisé à cet agent.

Le nouvel export doit obtenir une vraie observation writable d'absence via
observe_absent_paths, capturer les vrais octets/effacement, puis exporter les
preuves réelles. Pas de grants SQL ni capture forgée. Namespace preview/apply
reste une lacune séparée (scan non scoped refuse9/10 et history non exportable).
Les archives r69 et docs/candidat Staging restent à préserver en commit local ;
pas de push/PR, stash original intact. Aucun jalon ne termine le programme.


## Deux deltas suivants figés en revue r73/r74

ROOT17382 continue le natif Staging r72 ; les cinq premières recettes PASS,
pas encore de résultat du premier natif. Les nouveaux deltas ne changent pas
le binaire retenu. Ne lancer aucun build/SQL/test concurrent.

L'audit GPT-6.1-Sol a confirmé un contre-exemple SOURCE : B SourceReady correct
plus `projection_install(1,X'')` pouvait atteindre la publication, faute de
garde des opérations en attente. Aucune production de cette combinaison par
une API normale ni perte de données observée n'est démontrée. ROOT a corrigé
quatre fichiers : `import_validation.rs`, `import_validation/source.rs`,
`ordinary_transition/publication.rs`, `destination_ready/ordinary_tests.rs`.
Le helper require_live_source_quiescence relit l'ascendance puis réutilise
require_quiescent_history, sans prétendre prouver une image. Il est appelé
par read_publication_state et apply seulement si l'état stocké ACTUEL est
Ready, avant rejeu ou effet. Published historique garde ses évolutions B
cohérentes. Cinq savepoints testent install/refresh/namespace/restore/rebase
pending, admission immuable encore valide, refus des lecteurs/écrivains Ready,
absence de changements de lignes/total_changes. Sources seules, non compilées.

Revue critique r73 en attente, cellule **ROOT404** (mode wait600000).
Manifeste12 fichiers/4changés :
`D:/Layrs/tmp/provider-ordinary-ready-quiescence-review-r73-inputs.json`, SHA
`72cbc76bdd2733ba3bc68ca4a6eea2a1eaf319dee4aad231834f09a527c6570a` ; diff SHA
`8dea99d60f8f9fd441aa541f02cfa76a95ad4e8e0f375d5b270dbb39178a7f14`.
Rapport attendu `provider-ordinary-ready-quiescence-critical-review-r73.json`.
Premier appel delegate refusé pour option inconnue reasoning_effort (aucun
enfant créé) ; capacités relues après erreur, appel correct reasoningEffort
xhigh, GPT-6-Astra. Suivre son next, pas de polling task/thread en attente.

Agent source10 terminé. Nouveau test
`session/managed_ordinary_source10_export_tests.rs`, helper DATA-only
released_source10 dans autonomy_tests, module enregistré par ROOT session.rs.
Aucun autre changement production. C10 public/observation absence explicite,
addition imbriquée/capture puis suppression README/capture, export complet
vers D, admission10, libération des holders, D puis C publics vérifiés.
Assertions sur les origines B/C, octets des captures héritées, quatre manifests,
deux dossiers, un fichier et suppression ; tables C stables sauf source/receipt.
Proposition child5400/outer5460 non mesurée. Pas de D sans ancêtres, D→E, crash
ou défaut réel de fermeture qualifié. Direct rustfmt seulement, pas de build.

Revue critique r74 en attente, cellule **ROOT411** (mode wait600000).
Manifeste18 fichiers/3changés :
`D:/Layrs/tmp/provider-ordinary-source10-export-review-r74-inputs.json`, SHA
`a1c6e84506eca97916dc4115d14713d205b6cfd4ecce53e4733a1e6641dc5fef` ; diff SHA
`112b160e35f8edb1d69f19fed8dbae24f3e74c9dde1253dee5f09769c58ea3bb`.
Rapport attendu `provider-ordinary-source10-export-critical-review-r74.json`.
Les deux ensembles et HEAD e2a1ac4 restent figés jusqu'aux checks finaux.

Script `D:/Layrs/tmp/package-ordinary-fresh-staging-20261028-r72.py` créé et
parsable, NON exécuté ; attendre la terminaison effective ROOT17382. Le script
de génération prepare-staging-package-script-r72.py est déjà exécuté.
Agent ready_quiescence_gap réutilisé en read-only pour cartographier la prochaine
reprise avant C/famille Ready, sans faux ManagedDestinationAllocation. Aucun
autre agent natif actif. Goal actif ; pas de commit pendant les revues figées.


## Échec natif r72 conservé ; diagnostic r77 lancé

ROOT17382 est terminal exit1. Cinq tests PASS, premier natif exit101 en642,474s :
le child Windows a quitté0xc00000fd, «has overflowed its stack». Aucun point
de phase n'était journalisé après RETAINED_PROVIDER_FIXTURE (émis dès la
création de fixture). L'échec ne prouve donc pas que open/promote ait été atteint.
Second natif ACK non exécuté. Fixture échouée jamais rouverte/réparée/réutilisée.
Archive r72 créée et vérifiée :717 fichiers,2178496 octets, SHA256
`e69a40e9ba7d48acd58307c6954fd63289d2eacba85ca102e50f5af0a80cfa08`.
`ordinary-fresh-staging-evidence-20261028-r72.tar.gz` et inventory adjacent.
Ne pas relancer le packaging déjà exécuté.

Les revues r73 et r74 sont terminées sans findings, intégrité début/fin PASS.
Threads lus au dernier message, hasMore=false :
- r73 child c0f55cb2-75d8-43b4-9b9b-4b8a098d8512, cellule404 fermée ;
- r74 child44db4426-cf28-4f76-9071-29a5394746f5, cellule411 fermée.
Build combiné r75 PASS20,42s. Le nouveau test source10 est compilé, non exécuté.
Aucun agent natif ni délégué restant. Dernière investigation read-only : aucun
cycle nouveau trouvé dans image/build/validate ; grands retours/frames debug
possibles. Famille Initializing→Ready identifiée comme prochaine entrée fraîche,
documentée dans le plan fresh-staging ; aucune implémentation ajoutée.

Diagnostic offline ROOT : dumpbin /unwindinfo sur l'EXE r72 retenu (aucune
exécution du binaire/fixture) puis parse des288601 entrées. Résumé local
`D:/Layrs/tmp/ordinary-r72-stack-frames.json`. Frames nommées validate291960,
open154088, closure test111816 octets ; certains monomorphs de setup >200k.
Ce n'est ni une pile dynamique ni une preuve du site du crash. Premier parseur
ne séparait pas les fonctions anonymes et surestimait des frames ; corrigé
avant le résumé retenu/release. Raw dumpbin local exclu des archives volumineuses.
Environnement ROOT RUST_MIN_STACK absent, r72 ne l'avait pas enregistré ; aucune
valeur particulière du child r72 n'est déduite. Pas d'augmentation de pile.

ROOT ajoute seulement des traces cfg(test) dans staging_reopen open/validate
et quatre marqueurs de phases dans la première recette. Comparaison mécanique :
retirer exactement ces lignes/helper retrouve intégralement le source r72.
Aucune logique, limite, autorisation, assertion, cleanup ou budget changé.
Build diagnostic r76 PASS10,13s, check workspace/all-targets PASS5,23s.
Les sources runtime suivantes restent exactement r73/r74, sans autre modification.

Release diagnostic r77 :662 inputs live/retenus, exactement9 fichiers changés
depuis r72 (4garde Ready +3tests source10 +2traces), sources revues concordantes.
EXE SHA256 `1d2cdbd0dc3674b80da6cc109fd28446aa109fc7f0f6e524b2ac90f2b1e4740a`,
CodeView age134. Release SHA256
`b1cbe86fcd701cb26a0d9ace6ee96578ef4ea81eb5e581daeaee10cd5625b21c`.
Runner `D:/Layrs/tmp/provider-ordinary-staging-diagnostic-run-r77.py r77`,
**ROOT81989**, artefacts
`.layrs-local/validation/provider-ordinary-staging-diagnostic-20261028-r77`.
Deux recettes uniquement : SQL Ready avec cinq nouveaux refus pending, puis
premier natif Staging diagnostiqué. Parent C neuf, pile héritée enregistrée,
child3600/outer3660, arrêt premier échec. Aucun build/test/SQL concurrent.
Ne relancer ni préparation/release ni runner. Aucun succès natif/correctif de
stack revendiqué. Source10 et ACK Staging exclus de cette exécution.

Préserver ces sources/archives/docs dans un commit local de continuation,
pas de push/PR ; stash original conservé. Goal actif, programme non terminé.

Checkpoint local effectivement créé : `8dbeba239a7bf08abc0cf9430ccf6621582c9cbb`,
24 fichiers, index/worktree propres juste après commit, stash contrôlé inchangé.
r77 SQL Ready avec les nouveaux refus **PASS28,109s** ; natif diagnostique en
cours ROOT81989, seul marqueur encore disponible à17:49UTC = création fixture.
Ne pas conclure que le nouveau driver est atteint avant les traces suivantes.

Investigation offline terminée (aucun agent restant), résumé
`D:/Layrs/tmp/ordinary-r72-callchain-summary.json`, graph et50 ranges adjacents.
Chaîne open/read_validated/validate mesurée statiquement : trois read_inner
imbriqués transportant Validated occupent508552 octets +120408 with_progress ;
subtotalavec test/open/callbacks/validate1275888 octets, puis admission ajoute
276352 avant ses propres callees. Ce sont des frames qui peuvent coexister,
pas une capture de la pile fautive ni une mesure de sa limite réelle.
Correction candidate après confirmation de phase : faire traverser aux scopes
stricts un résultat petit (`Box<Validated>` ou `()` avec DATA local), conserver
toutes les propagations de close/erreur, aucun emprunt de preuve hors transaction.
Aucune de ces corrections n'est encore implémentée. La pile reste inchangée.
Script packaging r77 créé et parsé, NON exécuté :
`D:/Layrs/tmp/package-ordinary-staging-diagnostic-20261028-r77.py`.
Attendre la terminaison du handle puis archiver succès ou échec, sans fixture.

Tour goal suivant : ROOT81989 revérifié vivant, ancien binaire diagnostic
toujours seul en exécution. ROOT prépare le delta minimal dans un seul fichier
`managed_export/staging_reopen.rs` : validate et read_validated retournent
`Box<Validated>`, construit après toutes les validations. Les trois readers
transportent ainsi un pointeur au lieu de dupliquer les records dans leurs
slots de retour. Aucun ordre SQL/close, owner/pin, contrôle ou budget modifié.
Rustfmt passe ; pas encore de compilation/mesure/runtime de ce candidat.
Le site exact du débordement r72 reste à confirmer par les traces r77.


## r77 terminé, candidat Box r78/r79 en qualification

ROOT81989 terminé exit1 : SQL Ready PASS28,109s, premier natif FAIL101688,67s.
Les traces établissent setup-released, precancel, drift-open/source/C/image,
puis exact-open/source/C/image/validated-read et validate/observation avant
stack overflow. La chaîne de validation fraîche est effectivement atteinte.
Archive r77 vérifiée725 fichiers2173486 octets SHA256
`7ee36f854c09a133603a3c3a02cb045987d8dbcf5f0fcddbf3800217a7584c15`.
Pas de fixture r77 rouverte, aucun succès natif inféré de ces traces.

Box<Validated> compile r78 PASS18,63s, check workspace/all-targets PASS5,09s.
Comparaison indépendante offline du binaire r78,24 plages désassemblées :
`D:/Layrs/tmp/ordinary-r78-callchain-comparison.json`, graph adjacent.
Même chaîne compilée imbriquée : subtotal1275888→548464 octets, gain727424
(57,01%). Trois read_inner508552→20872 ; progress120408→4632 ; callbacks
58744→1720 ; read_validated30056→1544 ; open154088→115480 ; validate reste
291944. Pas une pile dynamique/borne complète : callees plus profonds, bridges,
retours et libtest ne sont pas tous inclus. Réduction compilée, pas runtime PASS.
Attention : ordinary-r78-callchain-map.json est une ligne JSON~30Mo ; ne jamais
Get-Content/head ce fichier brut. Filtrer avec Python et limiter les résultats.

ROOT accepte uniquement la petite représentation privée mécanique : retirer
deux signatures Box, le constructeur Box et son commentaire retrouve exactement
r77. Aucun ordre de preuve/SQL/fermeture, owner/pin ni limite de pile changé.
Self-review ROOT, pas de nouvelle revue critique indépendante revendiquée pour
ce delta mécanique ; reviews r70/r73/r74 restent le contexte protocolaire.
Release r79 SHA256
`18e478d0a2ce5feefb9c03bdf83accabd61d1d6dccaf2eb980f108a36c20bf6f`.
EXE `695389de06e1c912136bbd871a831ba3010007c67b06aab9b47b3ebd9b7cf51a`,
CodeView age135,662 inputs live/retenus, exactement1 fichier changé depuis r77.

Runner `D:/Layrs/tmp/provider-ordinary-staging-box-run-r79.py r79`,
**ROOT22841 actif**, artefacts
`.layrs-local/validation/provider-ordinary-staging-box-20261028-r79`.
Deux natifs Staging exacts : drift/succès puis ACK/Ready handoff, chacun parent
neuf, child3600/outer3660, arrêt premier échec. Premier en cours, second pas
encore atteint. Aucun autre build/SQL/test simultané. Ne relancer aucun script.
Packager `D:/Layrs/tmp/package-ordinary-staging-box-20261028-r79.py` créé/parsable,
NON exécuté ; attendre terminaison ROOT22841, archiver résultat même échoué.

L'agent d'investigation est terminé ; aucun agent/revue restant. Son plan
précis pour PrivateReadyParts puis lecteur/holder distinct family-publication
est ajouté au document fresh-staging. Ne pas élargir terminal/admission Ready.
Code Box, docs courants et archive r77 restent non commités après8dbeba2.
Source10 natif et second ACK Staging restent non qualifiés. Goal actif.

## Reprise suivante — premier natif r79 PASS, publication familiale en source

ROOT22841 confirme le premier natif r79 PASS599,491s : image B modifiée
refusée, puis image exacte authentifiée et B promu Ready après libération de
tous les anciens holders. Le second scénario ACK est maintenant actif.
Ne pas reconstruire ni lancer un autre test avant terminaison du même handle.
Le packaging r79 reste à faire après terminaison ; aucun script relancé.

Trois contributions source séparées, sans exécution concurrente :

- `ordinary_family_ready_sql_parts` a extrait PrivateReadyParts et deux
  nouvelles lectures distinctes : DATA privée provisoire, puis lecture complète
  soutenue par B réel et le catalogue familial complet. Les lecteurs terminaux
  conservent leur obligation Ready et leurs contrôles stricts. Il ajoute la
  couverture SQL dans ordinary_tests et corrige une comparaison d'union qui
  dépendait à tort de l'ordre FileId au lieu de l'ensemble exact ManifestId.
- `source10_nested_export_recipe` implémente le driver physique frais
  OrdinaryFamilyReadyPublication, les ressources C distinctes et leur factory
  depuis B Exclusive. Pas d'adoption d'une ancienne allocation/proof ni d'upgrade.
- `fresh_family_recipe` prépare deux recettes natives et un arrêt de setup
  réellement PrivateReady : succès Updated/relecture/rejeu et ACK retenu/reprise.

ROOT révise l'intégration. Ces nouveaux fichiers ne sont pas encore compilés,
revus indépendamment ou qualifiés. Le lot fait seulement passer le membre C
Initializing→Ready lorsque C privé est déjà Ready et B reste Staging. La reprise
Staging→Ready, la publication B et l'admission restent des actions distinctes.
Goal actif ; aucun push/PR. Le réexport natif C10→D10 reste en attente.

## r79 qualifié, réexport C10→D10 r81 et candidat familial r82

ROOT22841 terminé exit0. Les deux natifs Staging passent, respectivement
599,491s et598,976s. Le second conserve la cause ACK, refuse les probes suivants,
retient les pins/owners puis permet le handoff Ready après libération explicite.
Archive r79 vérifiée721 fichiers2159560 octets, SHA256
`418996d68d3ccecb1cfd81e509fe969cbd7cecb2ffa9e3f73d441c91a079434c`.
Packager exécuté, ne pas relancer. Pas de hausse de pile ; aucune qualification
de panne OS réelle ou de crash déduite des ACK retenus.

Réexport source10 : les18 chemins de revue r74 correspondent exactement aux
sources du binaire retenu r79 ;662 inputs retenus revérifiés. Aucune source du
nouveau lot familial n'entre dans ce binaire. r80 termine exit1/101 en0,078s :
refus d'ouverture pinned-directory C:/Users/ALEXA, os error5, avant fixture.
Ce résultat est un défaut d'accès du bac à sable, pas une exécution du parcours.
Preuves conservées sous `.layrs-local/validation/provider-ordinary-source10-20261028-r80`.

r81 utilise un parent neuf et l'escalade d'accès explicitement acceptée.
Release source/artefact SHA256
`eff620e8a4ce898cfa00f6a821e8bf082921b1cc800e7bd393f71ecba60d9f98`.
Runner `D:/Layrs/tmp/provider-ordinary-source10-run-r81.py r81`, **ROOT40925 actif**.
Artefacts `.layrs-local/validation/provider-ordinary-source10-20261028-r81`.
Une seule recette native, child5400/outer5460 proposés non mesurés, nouveau
parent C:, arrêt au premier échec. EXE retenu r79/age135 inchangé. Ne pas lancer
de compilation/SQL/natif concurrent ; ne jamais rouvrir la fixture r80.

Les trois implémenteurs natifs sont terminés, fichiers figés. Revue critique
T3 GPT-6-Astra xhigh r82 démarrée en mode wait, cellule ROOT492 (600000ms).
Manifeste40 fichiers/13changés :
`D:/Layrs/tmp/provider-ordinary-fresh-family-review-r82-inputs.json`, SHA256
`62a903bad05b08a0820a2874f7f15e2b161f71bced02f1ae43b604c653c0252b`.
Diff SHA256 `a81d0f6cd5fc9a327510fa96408d9c00a7b9c656956836533e6ec4c8dbfd859e`.
Ne pas modifier les sources ni HEAD pendant la revue ; docs non liées possibles.
Nouveau lot non compilé/non exécuté : uniquement extraction SQL stricte,
reconstruction physique source/famille et recettes. ROOT garde l'intégration,
la compilation, la release et la qualification. Goal actif, programme ouvert.

Packager r80/r81 préparé et parsé, NON exécuté :
`D:/Layrs/tmp/package-ordinary-source10-20261028-r80-r81.py`.
Attendre terminaison explicite ROOT40925 avant archivage (succès ou échec).
Il vérifie les deux releases/résultats, conserve le refus initial et archive
une seule copie des662 sources identiques ; EXE/PDB/fixtures exclus.

La cellule ROOT492 a rendu après son wait600000ms : revue r82 encore en cours,
aucune question/approbation en attente. Directive T3 : terminer ce tour et laisser
la fin de revue réveiller ROOT, sans polling. Task suffixe
`77458e04-b3b0-4b15-8b87-5d361effe67d`, thread
`thread:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3A77458e04-b3b0-4b15-8b87-5d361effe67d`.
Lire le résultat final complet au réveil. ROOT40925 encore vivant à18:30:49UTC,
aucun résultat natif source10 disponible alors. Tous les agents natifs ont fini.
Goal toujours actif ; aucun commit durant cette revue (HEAD doit rester figé).

## Revue r82 terminée, précision de preuve corrigée

Thread complet lu jusqu'au résultat final, hasMore=false. Aucun défaut concret
de protocole, permission ou intégrité identifié en source. Unique finding
R82-CV-01/P3 : la recette annonçait un succès OS alors que l'observateur établit
seulement le retour réussi de Connection::close/SQLite. ROOT a corrigé l'en-tête
et le texte d'assertion, sans toucher au comportement. Comparaison mécanique
des40 sources : seul ce fichier diffère, exactement ces deux substitutions.
Rustfmt check PASS. Pas de nouvelle délégation pour cette correction textuelle.
Rapport `D:/Layrs/tmp/provider-ordinary-fresh-family-critical-review-r82.json`.
La revue est source-only ; compilation, durées et tests restent non établis.

Le tour précédent a produit du progrès concret (deux natifs r79 PASS, archive,
nouveau lot familial intégré et revue lancée). Ce tour confirme ROOT40925 vivant,
consomme la revue achevée et corrige sa remarque ; goal actif sans réduction de
périmètre. La sérialisation interdit encore Cargo/SQL/natif tant que r81 tourne.

Archive source-only r82 vérifiée47 fichiers199971 octets, SHA256
`333e08d8bd06f4c3638aa2da88f57d7880a46acff2b6120454fc7fba7d45aa93`.
Elle conserve les40 sources revues, le rapport/manifeste/diff, puis le fichier
de recette corrigé et sa comparaison mécanique. Pas de qualification runtime.
Checkpoint local prévu pour ces14 Rust (13famille +Box Staging), quatre docs
et les archives/inventaires r77/r79/r82 ; aucun push, stash original inchangé.
L'agent `fresh_private_ready_investigation` examine uniquement en lecture source
le prochain prédécesseur : B Staging/C Staging/protection+checkout complets vers
C Ready privé. Aucun code de ce prochain lot commencé.

Checkpoint créé : `f01f6b9518b71a3cfc309be3a2911c51599f2e79`,24 fichiers,
worktree/index propres immédiatement après commit, stash inchangé, aucun push.
Investigation private Ready terminée : résultats consignés dans
`worktrees-partial-vfs-ordinary-fresh-private-ready.md`. Ajouts précis identifiés :
lecteur SQL finalizable RO, lecture exacte de manifests par ID, C Exclusive dès
factory et driver strict séparé. Les anciens ReadyContext retenus demandent
aussi un audit de leurs contrôles post-erreur et namespace sous gate ; ne pas
confondre ce suivi avec une preuve de panne exécutée. Aucun agent natif actif.

Préparation future r83 parsée, NON exécutée :
`D:/Layrs/tmp/prepare-and-release-ordinary-fresh-family-r83.py`.
Elle exige r81 terminal PASS, deux logs build/check r83 réussis, même HEAD et
delta exact r82+correction texte ; attend666 inputs et13 chemins changés depuis
r79. Six recettes prévues : deux régressions Provider terminal, autonomie
Ordinary, SQL familial étendu, puis deux natifs frais. Si le build exige des
correctifs, adapter la release avec une nouvelle comparaison explicite ; ne
contourner aucune assertion. Aucun build/check exécuté à ce stade.

## Échec témoin r81 archivé, build r83 et qualification combinée r84

ROOT40925 terminé exit1 ; source10 FAIL1011189,026s à l'assertion all-table
« C export changed projection_budgets ». Le budget committed de la dernière
capture était présent dans la baseline avant acquisition propriétaire, puis
absent à SourcePublished. Admission D et réouvertures n'ont pas été atteintes.
Trace source confirmée : begin_managed_export appelle format::access Exclusive,
qui settle le budget committed en l'absence d'intent live ; le settlement
libère exactement la réservation familiale puis supprime le singleton privé.
Pas de défaut produit établi par cet échec ; fixture non rouverte.

ROOT corrige uniquement le témoin source10 : discovery Shared doit conserver
toutes les tables ; avant begin, le budget unique est lié à capture:state.digest
et committed, sa réservation doit appartenir au bon owner/opération. Après
begin, toutes les tables privées et familiales doivent correspondre exactement
à la suppression de ces deux lignes. La comparaison complète ultérieure garde
le droit d'ajouter seulement intent/reçu. Aucun tableau ignoré ou assertion
historique supprimée. Self-review ROOT ciblée, pas nouvelle revue indépendante.
Delta inclut le rangement rustfmt d'imports et de lignes d'assertions.
Archive r80/r81 vérifiée691 fichiers2105179 octets, SHA256
`9f3d5ed33eb7f12f884c4733422c0ac3926933d6d09a355053fb93d05cad2dd3`.

Build libtest r83 PASS30,80s, ROOT20321 terminé exit0 ; workspace/all-targets
check PASS9,52s. Les nouveaux sources familiaux compilent sans correction Rust.
Préparation r83 NON exécutée reste remplacée par r84 explicite : r81 n'a pas
PASS, et source10 possède désormais son propre correctif de témoin borné.

r84 host audit/retention créé,666 inputs. Le premier script de release s'est
arrêté sur décodage Windows cp1252 d'un source UTF8 après retention, avant
runner/release/fixture. Un script de continuation utilise UTF8 explicite,
vérifie l'audit et les artefacts déjà retenus, puis finit sans les recréer.
Scripts déjà exécutés : prepare-fresh-family-and-source10-release-r84.py,
prepare-and-release-ordinary-fresh-family-r84.py (partiel),
finish-family-release-script-r84.py, finish-ordinary-fresh-family-release-r84.py.
Ne les relancer pas.

Release r84 SHA256
`ff6e1c462351dfd039a23fda46ee38d84156e50b6fbcb8c4a4ee1d387613dcc4`.
EXE SHA256 `48ec4b8a3419dfa6552f6419c2178e259c61c9519e70da9d5181f89cdbe37dd9`,
CodeView age136.666 sources live/retenues concordantes au lancement,14 chemins
changés depuis r79 =13familiaux revus+correction texte et1testsource10 corrigé.
Runner `D:/Layrs/tmp/provider-ordinary-fresh-family-run-r84.py r84`, **ROOT73332**,
artefacts `.layrs-local/validation/provider-ordinary-fresh-family-20261028-r84`.
Sept recettes séquentielles : Provider terminal RO PASS1,338s ; corruption
Provider PASS1,278s ; autonomie Ordinary PASS22,008s ; SQL familial étendu
PASS21,454s ; source10 corrigé en cours ; puis deux natifs familiaux. Source10
child5400/outer5460, famille child3600/outer3660, chaque parent neuf, stop first
failure. Aucun résultat natif r84 revendiqué. Accès hôte autorisé via escalation.

Trois agents source-only, sans build/SQL/process/fixtures :
`ordinary_family_ready_sql_parts` possède ordinary.rs/destination_ready.rs,
shared/content.rs/shared.rs et ordinary_tests.rs pour le lecteur finalizable RO
et les manifests exacts ; `source10_nested_export_recipe` possède nouveau driver
private_ready_reopen et ressources C Exclusive/factory ; `fresh_family_recipe`
possède nouveau test fresh_private_ready et hook checkout-complete/registration.
Les preuves r84 sont retenues et indépendantes de ces nouveaux edits live.
Goal actif, aucun push. Commit f01f6b9 conserve la baseline précédente.

Packager r84 préparé/parsable, NON exécuté :
`D:/Layrs/tmp/package-ordinary-fresh-family-20261028-r84.py`.
Attendre terminaison du handle ROOT73332, puis archiver tous les résultats
atteints même en cas d'échec ; ne pas confondre les sources live suivantes
avec les666 sources retenues du binaire actuellement exécuté.

## Reprise privée fraîche intégrée et gelée r85 — SOURCE ONLY

Les trois agents sont terminés. ROOT a examiné le lecteur SQL finalizable,
l'accès exact ManifestId, le pilote C Exclusive, le hook checkout-complete et
les deux recettes. Aucun build/test/SQL supplémentaire : ROOT73332 reste actif.
Les contextes actual Staging/Ready sont distincts et le rejeu Ready impose
explicitement la famille Initializing. Les tests négatifs journal incomplet,
substitution de manifeste et corruption familiale sont des recettes SQL ; les
deux natives proposées couvrent succès/rejeu et ACK SQLite retenu, pas une
fermeture native xClose/OS échouée ni un crash.

`prepare-ordinary-fresh-private-ready-review-r85.py` exécuté une fois :
50 sources gelées, 13 fichiers modifiés, comparaison au source retenu r84.
Manifeste `D:/Layrs/tmp/provider-ordinary-fresh-private-ready-review-r85-inputs.json`
SHA256 `ed6167d858eec42dda2038e75ed821d43a62071af4266114d82dee5e6a0b3c1f` ;
diff SHA256 `9f9d8b873a8274c6486f266c15331f3e3b95791cbb2a88d66ec62aa1a5f8f00e`.
Revue critique T3 GPT-6-Astra xhigh demandée, cellule557, mode wait600000.
Rapport prévu `provider-ordinary-fresh-private-ready-critical-review-r85.json`.
Ne pas modifier les50 sources avant résultat et vérification finale de la revue.
ROOT a exécuté `rustfmt --check` sur les13 fichiers intégrés : PASS, puis
`git diff --check` : PASS. Aucun résultat de compilation n'en est déduit.

Le document `worktrees-partial-vfs-ordinary-fresh-private-ready.md` précise
également un audit ultérieur de ReadyContext retenu : lecteur nonstrict faisant
un snapshot après callback Err, et boundary finale inconditionnelle encore sous
gate. ROOT a confirmé le contrôle de flux en source, sans exécution. Ne pas
confondre cette lacune de l'ancien chemin avec la qualification future r85.

## Résultats reçus à 19:12 UTC — r84 source10 et revue r85

ROOT73332 a rendu le réexport C10→D10 corrigé **PASS1309,558s**, exit0, puis
a lancé `managed_ordinary_fresh_family_refuses_changed_b_image_then_updates_only_member_and_replays_without_writer`.
La suite globale r84 n'est pas terminée ; ne pas archiver un succès global ni
compiler avant la terminaison du handle. Le second natif familial reste à suivre.

Revue critique r85 terminée sans défaut concret actionnable. Rapport JSON créé,
50 sources et8 dépendances supplémentaires vérifiées sans dérive. Thread
`thread:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3A0a20efaf-9a07-4bae-9a67-f2f1be8aa895`
lu jusqu'au résultat final, position95, hasMorefalse. Cellule557 fermée. Aucun
build/runtime r85 ; ne pas transformer cette revue source en qualification.
Le gel de revue est terminé. Conserver snapshot/manifeste/diff/rapport avant
les prochains changements, puis préparer compilation et qualification ciblée.

L'utilisateur demande si les durées sont normales. Réponse : recettes natives
complexes, mais pas d'explication mesurée suffisante pour déclarer20minutes
nécessaires. Le lanceur child.rs n'impose ni CPUthrottle ni attente dans le corps;
son sleep50ms ne fait que surveiller le processus. La consultation du PID59712
est arrivée après sa fin (aucun compteur CPU obtenu). Aucun profilage exécuté.
Ajouter des durées par grandes phases aux prochaines recettes, sans relancer
une fixture passée ni affaiblir les assertions. Distinguer setup, contrôles
produit, témoins complets et replay ; le coût dominant reste à établir.

## Travaux source indépendants après r85

Archive source r85 créée et relue :
`ordinary-fresh-private-ready-source-review-20261028-r85.tar.gz`,64fichiers,
262338octets,SHA256`b987609f2c3ca4749e5fe5b783c758d5dfb172f4197517df22cd17adb15aabde`.
Contient les50sources,8dépendances supplémentaires et supports de revue ; aucun
binaire/fixture/résultat runtime. Packager exécuté, ne pas le relancer.

Agent `retained_ready_failure_order` : ownership ready.rs et les deux recettes
retained privateReady/familyReady, correction strict failure ordering et probes
ACK. Agent `native_phase_timings` : ownership process_profile/tests/child.rs,
source_publication testhelper et fresh_private_ready tests, chronomètres monotones
test-only. Tous deux SOURCE ONLY ; ROOT73332 continue seul le runtime r84.
Le premier tour de l'agent retained a échoué sur capacité GPT-6.1-Sol après
investigation ; une reprise du même agent/profil a été demandée, aucun changement
de modèle. Les contributions restent à intégrer/revoir/qualifier.

## Lot intégré r86 et prochaine qualification préparée

Les deux agents sont terminés et leurs fichiers sont gelés. ROOT a examiné le
diff ReadyContext, les témoins ACK et l'instrumentation. `rustfmt --check` sur
les six fichiers intégrés passe. Aucun build, test ou SQL exécuté pour ce candidat.
ReadyContext utilise des lecteurs stricts, des contrôles d'identité sous le
verrou catalogue et des contrôles physiques complets avant/après les scopes
réussis. Une erreur se propage avant toute sonde optionnelle. Les deux anciennes
recettes ACK observent les routes C/famille et exigent zéro sonde après l'ACK
refusé ; la recette Updated observe la relecture familiale fraîche.

Chronomètres test-only : `LAYRS_TEST_PHASE phase=... event=START|END
elapsed_seconds=... unwinding=...`. 141 lignes ajoutées dans trois fichiers,
aucune ligne originale supprimée/remplacée. Aucune closure supplémentaire ni
donnée de fixture retenue par PhaseTimer. Les durées imbriquées se chevauchent ;
END n'est pas un verdict de succès.
Analyseur offline `D:/Layrs/tmp/analyze-native-phase-log.py` créé et exécuté sur
le log source10 r84 terminé : aucun événement de phase, résultat correctement vide.
Il ne lance aucun processus produit ni n'ouvre de fixture.

Revue critique r86 T3 GPT-6-Astra xhigh en cours, cellule588, mode wait600000.
Manifeste `D:/Layrs/tmp/provider-ordinary-ready-ordering-review-r86-inputs.json` :
60 sources, six fichiers modifiés depuis r85, SHA256
`d9684350c299384cc3006d42c1a60efa45e58671d5a4a996de32a86ccce6275d`.
DiffSHA256`da515be2617eaa26598a129bbf76020797f8ea195259e3f9dec16423fd520c34`.
Rapport prévu `provider-ordinary-ready-ordering-critical-review-r86.json`.
HEAD toujours f01f6b9 ; ne pas committer/modifier les 60 sources pendant la revue.

ROOT73332 a désormais six PASS : quatre tests courts, réexport1309,558s et premier
familial609,067s. Dernier familial ACK en cours. Aucun build concurrent. Après
confirmation de terminaison du handle et résultat de revue, archiver r84 avec
son packager existant, puis compiler libtest et workspace/all-targets r87 (logs prévus
`provider-ordinary-fresh-private-ready-build-r87.log` et `...-check-r87.log`).

Scripts préparés/ASTPASS mais NON exécutés :
`D:/Layrs/tmp/prepare-and-release-ordinary-fresh-private-ready-r88.py`
(argument `--root-accept-reviewed-private-ready`) et
`D:/Layrs/tmp/package-ordinary-fresh-private-ready-20261028-r88.py`.
La release attend les sept PASS r84, les revues r85/r86 sans finding, les builds
r87 réussis, HEAD f01f6b9, 669 sources et exactement 17 fichiers modifiés depuis
r84. Neuf recettes : deux strict-read SQL, private Ready SQL, family/finalizable
SQL, deux natives privées fraîches chronométrées, retained private ACK,
retained family Updated et retained family ACK. Quatre SQL child120/outer180 ;
fresh3600/3660 ; retained3000/3060. Aucun scénario natif r88 lancé. Revoir les scripts si finding,
correction de compilation ou changement de HEAD avant release ; ne pas contourner
les checks ni qualifier les sources live avec le résultat du binaire r84.
Préflight ROOT effectué : les 669 sources courantes présentent exactement les
17 changements de l'union des manifestes r85/r86 par rapport aux sources r84.
Ce contrôle de contenu n'établit pas leur compilabilité.

## r84 terminal, r86 reçu, compilation corrigée et r88 lancé

ROOT73332 terminal exit0 : sept PASS. Second familial ACK/rejeu600,052s.
Packager r84 exécuté :695fichiers,2256804octets,SHA256
`2eea7bcdd155122cbb2853b10e4e2a474c05bd37021695ba424b627ef77309aa`.
Archive `ordinary-fresh-family-evidence-20261028-r84.tar.gz` et inventaire adjacent.

Revue r86 terminée sans finding actionnable, thread lu jusqu'à position78,
hasMorefalse ; cellule588 fermée. Identifiant :
`thread:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3Ab6951e9e-e662-4728-8e36-e5e922b39123`.
Archive source r86 exécutée/vérifiée :73fichiers,294357octets,SHA256
`0de57eee7b3f312a2f790bb36485dc64d142050d8ad9119e899cb44af92e5433`.
Les deux agents natifs sont terminés. Aucun sous-agent encore attendu.

Build r87 ROOT81353 terminal101 : E0027 à ordinary_tests.rs996, motif Published
omettant parent_identity. Après fin et archivage de revue, ROOT remplace seulement
`Published { inode }` par `Published { inode, .. }`. Le test construit toujours
le même état Verified et garde toutes ses assertions. Comparaison mécanique
d'une substitution,59autres sources sélectionnées identiques, rustfmt PASS.
Rapport `ordinary-private-ready-compile-correction-r87.json` SHA256
`4c5a366d6b50ac7ea01c12c292b7b3b84a726f051f41f2503d084b8857352b58` ;
diff SHA256`a96eaf768001b0d8923a37f0531d51d0303577f3dacaab18cdb3e2629ab2a5f0`.
Le log échoué r87 est conservé, jamais écrasé. Aucun protocole modifié par ce fix.

Build corrigé r87b ROOT88959 terminal0,37,27s. Workspace/all-targets ROOT12687
terminal0,14,40s. Logs `...-build-r87b.log` et `...-check-r87b.log` sous tmp.
Script release r88 adapté au fix et à ces logs puis exécuté une fois : host audit,
EXE/PDB,669sources retenues,17deltas vsr84, deux revues+fix ROOT+runner liés.
EXE SHA256`8d793beea8af3d4d1611ccedbffe22f68e19c051687daffe810fff00fb506e57`,
CodeView age137. Release SHA256
`23fba8be8c7155b0758f0c4dd9c9bb07915b49c2b511bdaea6707ae38126d0f0`.

Runner `D:/Layrs/tmp/provider-ordinary-fresh-private-ready-run-r88.py r88`
lancé avec accès hôte autorisé, **handle ROOT38516 actif**. Quatre SQL PASS :
strict callback0,695s ; strict réel BUSY close0,176s ; private Ready9,227s ;
family/finalizable27,644s. Premier natif fresh private Updated/replay chronométré
en cours, puis fresh ACK, retained private ACK, retained family Updated et ACK.
Ne lancer aucun autre build/SQL/natif avant terminaison de38516. Chaque recette
possède un parent neuf. Ne jamais rouvrir une fixture passée ou échouée.

Lire uniquement les logs pour les premières durées :
`.layrs-local/validation/provider-ordinary-fresh-private-ready-20261028-r88/`.
L'analyseur de phases accepte les chemins explicites des logs, jamais les fixtures.
Packager r88 préparé et parsable, NON exécuté ; attendre terminal et conserver
aussi un éventuel échec avec tous les résultats atteints. Aucun résultat natif
r88 acquis à ce checkpoint. Goal actif ; aucun push/PR ; stash original conservé.


## r88 échec de témoin archivé ; progression checkout SQL r89

Checkpoint local `91c566ab6cf778f731167e7adc1334fc1a46ce73` conserve r85/r86,
correction de compilation, archives r80/r81/r84/r85/r86. Aucun push/PR ; stash
original inchangé. Goal actif, programme Worktrees/partial VFS complet ouvert.

ROOT38516/r88 terminal exit1 : quatre SQL PASS (0,695s / 0,176s / 9,227s /
27,644s), premier natif FAIL101 après556,326s. Les quatre suivants ne sont pas
atteints. Échec dans `close_at_complete_checkout` : le témoin exigeait zéro
ligne dans la table des reçus alors que C hérite du reçu d'entrée de B. Aucun
appel au driver frais, aucune clôture de l'ancien owner encore atteints.
ROOT conserve l'égalité exacte des lignes reçus B/C et exige zéro reçu pour
la transaction B→C courante. Ce correctif ne modifie pas le protocole produit.
Ne pas réutiliser ni rouvrir cette fixture.

Packager r88 EXÉCUTÉ : archive
`ordinary-fresh-private-ready-evidence-20261028-r88.tar.gz`,703fichiers,
2650961octets,SHA256
`2c6ebd9299bab22a47f1b1fb7822d89e2aafd5f5b1375811a99fa69401f38051`.
Inventaire adjacent ; EXE/PDB et fixtures exclus. Quatre PASS et échec conservés,
sans verdict global PASS. Analyseur de phases exécuté sur le log terminal.

Mesures séquentielles de préparation r88 : Provider→B139,632s ; prepare16,682s ;
begin3,571s ; plan8,956s ; allocation/registration70,298s ; copy metadata4,772s ;
stage14,881s ; reserve6,932s ; install232,010s ; protect15,719s ; checkout41,963s.
Les phases parentes se recouvrent ; ne pas les additionner aux enfants. END
est aussi émis en unwind et n'est pas un verdict. Aucun temps de reprise privée
Ready mesuré : son driver n'a pas été atteint. Aucun CPU fiable obtenu.
ROOT a séparé le groupe allocation en allocate/commit-allocation/publish-format/
register-allocation, sans changer opérations ni assertions. Coûts possibles
identifiés en source : revalidations image répétées, traversées d'ancêtres et
pins/no-follow/ACL. Aucune cause dominante mesurée ni optimisation revendiquée.

Agent `ordinary_checkout_progress_sql` terminé SOURCE ONLY, cinq fichiers gelés.
Nouvelle API `read_ordinary_checkout_progress` retourne None ou journal typé
partiel/complet après B/C Staging, union/protection/famille et budget futur C10.
Finalizable refuse toujours absent/partiel. Aucune autorité physique. Recette
étendue : query-only, absence, Owned, Verified, complet, annulation, erreurs
marqueur/manifeste/floor/source, journal étranger et corruptions familiales.
Les deux modifications ROOT de témoin/chronométrage portent le lot à sept
fichiers Rust. Revue critique r89 en cours, manifeste60sources SHA256
`153fc804bfd947a2d0921357f11b89fd278905b928ef5db11ab362a673adb866`, diffSHA256
`19420536cce3720deccc8ef28a21b64ff52d1b6001f433a2238a8bc06d077603`.
Snapshot `D:/Layrs/tmp/provider-ordinary-checkout-progress-review-r89-source`.
Build libtest r90 ROOT88929 terminal0,16,62s ; workspace/all-targets terminal0.
Logs `provider-ordinary-checkout-progress-build-r90.log` et `...-check-r90.log`.
Ne pas modifier sources/HEAD pendant revue. Nouvelle qualification requiert
nouveau binaire retenu/release/runner/parent ; ne pas relancer r88.

Suite de reprise physique identifiée mais NON implémentée : ouvrir B/C frais
Exclusives sur SourceStaging/C8Staging protégés, journal absent ou partiel,
puis reprendre le moteur checkout avec scopes stricts et clôtures observées.
Ne pas adopter ManagedDestinationAllocation. Ressources propres sans prétendre
VerifiedCheckout avant complétude. None exige espace initial physique exact,
distinct du journal SQL. Les transitions Owned/Verified/Publishing/Published
existantes sont réutilisables ; Declared avec objet présent non durablement
Owned reste refusé et nécessite une résolution séparée. Ne pas déclarer la
reprise crash globale achevée avec ce seul lecteur SQL. Le chemin checkout
retenu requiert aussi correction des post-probes inconditionnelles, comme Ready.


Preparation suivante (NON executee tant que revue r89 non acceptee) :
`D:/Layrs/tmp/prepare-and-release-ordinary-private-ready-r91.py`
avec `--root-accept-r89-review`, et
`D:/Layrs/tmp/package-ordinary-fresh-private-ready-20261028-r91.py`.
AST des deux scripts PASS. Le premier attend HEAD91c566a, les60sources gelees,
zero finding r89, build/check r90, exactement7deltas sur669inputs vsr88.
Il cree audit/runner/release immuables r91. Sept recettes prevues : deux SQL
Ready/progress affectes, deux natifs fresh private, retained private ACK,
retained family Updated et ACK. Les deux strict-read r88 inchanges ne sont
pas relances. Le packager exige resultat terminal et archive aussi un echec.


## r89 accepte en source, r90 compile, r91 actif

Revue critique r89 terminee sans finding; thread lu jusqu'a position64,
hasMorefalse. 60sources +8dependances stables; seules3notes documentaires ont
change hors perimetre pendant revue. Lacunes distinguees : seuil C10 non isole
par recette de frontiere, phases Declared/Publishing et Copying canonique non
directement couvertes ici, qualification physique absente. Aucun bypass source
confirme. Archive source r89 executee/verifiee :74fichiers,275818octets,SHA256
`11f616b8216bbc6007c95be235ca3e6d0bfc66bcac01c1a2d521abf8b71de951`.

Build r90 16,62s et workspace/all-targets5,28s PASS. Release r91 executee :
669inputs,7deltas depuis r88, EXE/PDB identity age138; SHA256 EXE
`a762bdbdc61c90e747e164720e331cc0cb1b661e1af6fa879b510176f09a69a5`.
Release SHA256`1c014a07ce4030baa96fc2e3b8091f8474ef521dc6b60e8c63d75d7c87d6e79e`.
Runner `D:/Layrs/tmp/provider-ordinary-fresh-private-ready-run-r91.py r91`
actif sous **ROOT75810**, acces hote autorise. SQL private9,494s PASS,
family/progress40,422s PASS. Premier natif frais Updated/replay en cours,
puis fresh ACK, retained private ACK, retained family Updated et ACK.
Aucun build/SQL/natif concurrent. Aucun script r91 execute ne doit etre relance.
Packager r91 prepare seulement; attendre terminal. Ne jamais rouvrir fixtures
anciennes. Les edits sources suivants ne modifieront pas le binaire retenu r91.


## Checkpoint local 0fa9954 ; premier natif privé r91 PASS ; r92 source gelée

HEAD `0fa9954d7208117bcaa84b2a883265e3995da501` conserve les sept Rust r89,
trois notes et archives r88/r89. Worktree propre juste après commit ; aucun
push/PR, stash original inchangé. Les modifications suivantes sont hors r91.

ROOT75810 reste actif. r91 : SQL9,494s et40,422s PASS ; premier natif privé
fresh Staging→Ready/refus B/C drift/replay sans writer PASS585,368s. Second
fresh privé ACK en cours ; retained private ACK, family Updated et family ACK
restent à suivre. Ne pas compiler/tester en parallèle. Lire seulement logs,
jamais les fixtures. Analyseur de phases exécuté une fois sur le premier natif
terminal : `r91/phase-durations-first-native.json`,43phases. Total interne585,300s ;
setup547,621s ; open4,582s + finalize11,512s ; replay avec témoins10,757s ; reste
10,828s (refus/contrôles/handoff familial). Phases choisies disjointes : aucune
somme parents+enfants. Cela mesure ce scénario unique, pas une borne ni une
optimisation. Visuel affiché dans le chat, `D:/Layrs/outputs/test-time-breakdown-r91.html`.

Deux agents natifs GPT-6.1-Sol terminés SOURCE ONLY :
`implement_fresh_checkout` et `fresh_checkout_recipes`. API
`OrdinaryCheckoutPopulation::open(B,C,tx,cancel)` puis consuming
`populate_checkout(close,cancel)` ; Progress expected/journal/effect
Updated|AlreadyComplete DATA ; Failure step/cause/held. B et C Exclusive dès
ouverture, C CAS séparé, SQL actual Staging/familleInitializing/protection/funding,
pending stage/dir/TEMP pins conservés dans failureholder, strict scopes et
readback avant FS, complet vérifié physiquement sans writer. Pas de Ready,
publication ou admission. Namespace initial contrôlé mécaniquement sans faux
journal observé. Reconstruction et moteur dupliqués plutôt qu'extraction.
Retained checkout corrigé en strict readers/result? avant post-probes ; protection
physique avant/après gates, identité sous gate.

Nouveaux fichiers : managed_export/checkout_reopen.rs,
managed_ready_reopen/ordinary_checkout.rs,
session/managed_ordinary_fresh_checkout_tests.rs. Exports/wiring et hook de
fixture après genuine protect_content. Trois recettes :
`managed_ordinary_fresh_checkout_refuses_drift_then_populates_absent_and_replays_without_writer`,
`managed_ordinary_fresh_checkout_withheld_declare_ack_releases_then_reopens_and_completes`,
`managed_ordinary_fresh_checkout_withheld_owned_ack_releases_then_reopens_and_completes`.
Les fixtures ferment B et relâchent tous les anciens holders avant fresh open ;
les reçus historiques B/C restent intégralement comparés. Limites child3600/
outer3660 non mesurées. Flat README uniquement ; pas native Verified/Publishing,
rename-gap, TEMP partiellement écrit, suppression positive ou nested directories.

ROOT a complété `destination_ready/ordinary_tests.rs` : allDeclared,
Publishing, C Copying canonique sans journal refusé, unique clé opaque refusée ;
query-only/dumps/total_changes conservés. ROOT a étendu le natif retained
`managed_ordinary_checkout_tests.rs` : ACK Declare original, zéro probes après
ACK, contrôles positifs des routes observées ; wording SQLite-close précis.
Ces modifications sont uniquement source, NON compilées/exécutées.

Lot r92 gelé :14Rust changés dont3nouveaux,74sources sélectionnées. Manifeste
`D:/Layrs/tmp/provider-ordinary-fresh-checkout-review-r92-inputs.json` SHA256
`5e649c67ac6a67477f37d76992fc99fc416763a1dc91697eae7a9bc7b2e36978` ; diffSHA256
`47ff4a36ca908ddcfa877ffd22ac85b312bf5b145ce6c512712368b54a0ecb21`.
Snapshot `provider-ordinary-fresh-checkout-review-r92-source`, comparaison
normalisée vs source retenue r91. Revue critique Astra xhigh démarrée, cellule
functions.exec677 en attente de delegate_task(mode=wait,timeout600000). Ne pas
poll task_status ; suivre résultat/next. Aucun autre sous-agent natif actif.
Ne pas modifier sources/HEAD avant fin de cette revue. Rapport attendu
`provider-ordinary-fresh-checkout-critical-review-r92.json`.

La résolution du gap Declared + objet FS présent sans ownership durable reste
ouverte : aucun droit d'adopter ou supprimer cet objet n'est ajouté. Les pannes
réelles de fermeture, crash/perte d'owner, budget C10 à la frontière et le reste
des lots L0–L9 restent à qualifier. Goal actif ; aucun jalon ne le clôture.


## r91 deuxième privé frais PASS, session toujours active

ROOT75810 : deuxième natif privé ACK/rejeu PASS571,530s. Total atteint :
quatre PASS (deux SQL + deux natifs frais). Retained private ACK maintenant en
cours, puis retained family Updated et ACK. Ne pas relancer ni compiler avant
terminaison de ce handle. Analyseur exécuté sur le deuxième log terminal :
`r91/phase-durations-second-native.json`. Aucune fixture ouverte par ROOT.

Observation processus en lecture seule pendant la préparation du privé ACK :
`r91/process-cpu-samples-private-ack.json`, chemin EXE exact et StartUtc liés.
PID70512 :35,28125sCPU sur35,591s murales entre deux observations ; PID68440,
wrapper,0sCPU sur le même intervalle. Ce relevé suggère une consommation proche
d'un cœur logique sur cet intervalle ; il n'attribue pas le coût à une fonction
et ne constitue ni profil de pile, ni preuve de cause I/O, ni gain mesuré.

Revue r92 toujours en cours, sources/HEAD inchangés ; trois notes documentaires
actualisées hors périmètre. Pas de compilation de r92. Goal actif.


## Revue r92 achevée ; correction ROOT r93 test-only ; r91 cinq PASS

ROOT75810 poursuit le runner retenu r91. Le retained private ACK passe en559,386s ;
famille Updated puis ACK restent. Aucun autre Cargo/build/SQL/natif lancé.

Rapport r92 terminé : un finding P2 R92-01 dans la recette ACK-Owned. Le handle
TEMP mutable partage READ/WRITE/DELETE ; une deuxième ouverture WRITE/share7 est
compatible et ne pouvait prouver sa rétention. Aucun autre défaut produit concret
établi par la revue, sans garantie exhaustive. ROOT remplace ce témoin par une
demande READ/share0 : refus tant que le holder conserve le handle, succès après
drop explicite, même identité TEMP. Aucune permission/sharing production modifiée.
Les contrôles B/CAS restent ceux de pins immutables. ROOT renforce aussi G5 avec
témoins positifs B/C/famille par frontière et G6 avec comparaison structurée de
Cleanup.primary → Validation(Invalid(ACK)), plutôt qu’une simple sous-chaîne.

Deux fichiers de tests changent après le gel r92 ; les72 autres sources du gel
restent identiques. `rustfmt --check` PASS. Correctif auto-revu ROOT, non revu
indépendamment, non compilé/non exécuté. Source-only : aucune qualification du
partage Windows ou de la reprise n’est encore revendiquée pour ce nouveau témoin.

Archive originale r92 : `ordinary-fresh-checkout-source-review-20261028-r92.tar.gz`,
84fichiers,334787octets,SHA256
`94e11bf1a5c9a66ba0696c2f265befb9d07a541f5adc6c04a2ca330ccc7a30a3`.
`package-ordinary-fresh-checkout-review-r92.py` exécuté une fois ; ne pas relancer.

Correction : `D:/Layrs/tmp/provider-ordinary-fresh-checkout-root-correction-r93.json`,
SHA256 `2790cd2bb49c76894136f84b058fe2297ff556eae03e747f2ce432c7240254be`.
Diff6770octets,SHA256
`15b4837a32c4d405b520bb48eea497720859dcf2033bac0640aac22ca8d04908`.
Snapshot74sources : `provider-ordinary-fresh-checkout-root-correction-r93-source`.
`record-ordinary-fresh-checkout-root-correction-r93.py` déjà exécuté une fois.

Préparé, AST PASS mais NON exécuté :
`D:/Layrs/tmp/prepare-and-release-ordinary-fresh-checkout-r94.py`.
Attend r91 terminal7PASS+archive et build/check r93. Lie672sources,14deltas,
revue r92+correction, binaire/PDB,1SQL+3fresh checkout+1retained checkout.
Ne pas lancer tant que ROOT75810 actif. Le script exigera le HEAD0fa9954 courant.
Les phases Verified/Publishing/rename-gap/partialTEMP et objets Declared sans
ownership durable restent des lacunes distinctes ; programme/goal toujours actif.


## Investigation suivante : coupures de copie, SOURCE ONLY

Agent natif `checkout_remaining_recovery` terminé, aucun fichier modifié, aucune
exécution/fixture. Aucun défaut concret établi dans les chemins étudiés. Après
gel/rétention du binaire r94, tranche proposée sur la vraie fixture un fichier :

- ACK SQLite close#4 : Verified, TEMP complet avec inode journalisé, final absent.
  Reprise fraîche attendue : deux writers, puis replay sans writer.
- ACK close#5 : Publishing, TEMP complet, final absent, parent exact. Reprise :
  un writer, même inode, puis replay sans writer.
- Refus AdmissionPoint::BeforeWritableOpen#6 : rename et sync déjà réussis,
  SQL encore Publishing, TEMP absent, final avec ancien inode. Reprise : un
  writer Published. Ne qualifie pas la panne entre rename et sync.
- Négatifs allocation-gap authentiques : refus BeforeWritableOpen#2 après stage
  créé ou #3 après TEMP créé. Après drop, objet présent/SQL Declared doit être
  refusé et conservé, sans adoption ni suppression ni nouveau writer.

Chaque recette doit observer journal/arborescence/inodes après drop explicite,
conserver la cause primaire, comparer B/famille inchangés et C Staging sans
receipt/admission. Les nombres de fermetures dépendent de cette fixture exacte ;
ils ne remplacent pas les témoins SQL physiques. Owned autorise seulement l’inode
exact et une réécriture depuis zéro. Verified exige les octets exacts ; Publishing
exige exactement un nom TEMP/final et le parent exact, puis rehachage.

Mid-copy : hooks DB insuffisants. Le driver donne directement fs::File au
stream64KiB et CURRENT est minuscule. Un futur scénario authentique demande
image >64KiB dès le setup et coupure test après écriture, ou sink préfixe/erreur.
Exiger0<len(TEMP)<attendu, ligne Owned, inode identique, reprise depuis zéro.
Écrire manuellement un TEMP partiel après Owned ne prouverait pas une interruption
du driver. Une corruption de même longueur en Verified/Publishing peut être
refusée au hachage dans populate, pas nécessairement dans open ; exiger zéro
writer jusqu’au refus, sans imposer le mauvais point de rejet.

Références : managed_export/checkout_reopen.rs317/385/414/492,
activation/managed_checkout_fs.rs380/495/668/820,
activation/managed_image/stream.rs91 ; aucune nouvelle autorité de reprise créée.


## Inventaire actuel du programme après Ordinary — lecture indépendante

Investigation native `programme_remaining_inventory` terminée en lecture seule,
sans build/SQL/fixture/modification. Cet inventaire consolide les derniers états ;
les « reste » historiques ne doivent pas provoquer des travaux déjà accomplis.

1. Terminer r91/r94 puis reprises checkout Verified/Publishing/rename-gap/partial
   TEMP et refus allocation-gap. Établir aussi les entrées fraîches amont
   Copying/allocation/install/protection, sans reconstruire une capacité
   ManagedDestinationAllocation depuis des chemins.
2. Interruptions réelles : perte owner/crash, commit/close incertain, journaux
   chauds, quarantaine et coupures install/protection. Réutiliser les barrières
   des deux crashes Capture déjà qualifiés. Isoler la frontière exacte C10.
3. Companions C13 : préserver créations inconnues, modifications readonly ou
   révoquées et suppressions hors checkout ; protection/reçu avant readiness.
   managed_plan.rs699 refuse encore les companions non vides. Recettes offline,
   Capture/Restore et échec de préservation selon convergence.md434/526.
4. Expansion Provider C14 : ancien travail/sélection préservés, nouveau map,
   même Step/root, epoch retenu ou réobservation complète ; révocation ancien
   chemin même avec cache hit et reprise par phase (convergence.md350/497/527).
   L2 ordinaire HTTPS déjà acquis ne qualifie pas cette transition VFS.
5. Préparation autorisée de contenu froid Provider : acquisition explicite,
   dépendances Capture→Generation/map et autorité avant acceptation, offline
   ensuite ; callbacks sans HTTP. session.rs238 expose actuellement connect,
   disconnect,checkpoint,export,capture. Convergence C3/activation-checkpoint95.
6. Lifecycle/GC/matrice FS : éviction/quotas/pins/fermeture, rename/delete,
   handles/mmap/watchers/flush/disque plein/callback annulé/déconnexion en vol.
   La fence GC de provider.rs174 sur layouts9/10 reste à conserver ou remplacer
   par un contrat qualifié, jamais simplement enlever.
7. Parcours produit Provider CLI/Desktop et vrais hosts : création/activation,
   préparation/capture/conversion/expansion/récupération et états accessibles.
   args/partial_provider.rs5 expose Status seulement ; main.rs61 déclare le
   profil sur ce trajet. Réutiliser product-host-checkpoint35, qualifier nouvelles
   routes/binaire ; UI distantes/partielles restent distinctes.
8. L0/L6 : mesures comparables froid/chaud, 1/2agents, code/assets/mixte,
   1k/10k/99k, dispersion/CPU/disque/réseau/verrous. Réutiliser pilotes32fichiers
   PASS et baseline release1k/unagent ; candidat1k et baseline2agents incomplets.
   Optimisation ancêtres HOLD sans gain prouvé, pas un préalable fonctionnel.
9. L9 : recette intégrée deux variantes/captures/offline/crash/restart/conflit,
   pression disque/ACL/migration/refus incompatible, contrôles pertinents et
   runbooks/revue finale. Réutiliser preuves de modules inchangés à périmètre égal.

Déjà acquis dans leurs périmètres : L7 HTTPS/Docker+Router/PostgreSQL avec revueR2
(plan principal1202), WebView/Git natif1214, Capture Provider et deux crashes
(capture-checkpoint48), admission/autonomieC10, publication fraîche, Staging,
réexportC10→D10 et famille. Ne pas les réannoncer comme non implémentés.

Limites externes actuelles : second volume Windows absent, symlink privilège1314,
Linux/FUSE selon prérequis/recette propre, macOS non qualifié, opérateur humain
indépendant si revendiqué. Aucun de ces points ne bloque les autres travaux
fonctionnels Windows. Aucun pourcentage global de fin fiable n’en est déduit.

Scripts r94 préparés NON exécutés : prepare-and-release-ordinary-fresh-checkout-r94.py
et package-ordinary-fresh-checkout-20261028-r94.py ; AST PASS. Le retained checkout
garde child2400/outer2460 ; les fresh gardent3600/3660. Après ROOT75810 terminal,
archiver r91, compiler libtest puis workspace/all-targets en série dans logs
provider-ordinary-fresh-checkout-{build,check}-r93.log. Traiter toute erreur avant
release. Ne pas modifier les sources gelées74 avant ce build sans nouveau correctif
traçable. HEAD0fa9954 doit rester pour le script r94 tel que préparé.


## Proposition des cinq recettes tardives terminée, hors worktree

Agent fresh_checkout_recipes terminé. Dossier :
`D:/Layrs/tmp/ordinary-checkout-late-recipes-proposal/`.
Candidat `managed_ordinary_fresh_checkout_tests.rs`, patch `late-recipes.patch`,
pins `base.sha256.json`. +558/-0, base intégralement préservée. Rustfmt PASS,
aucun build/test/SQL/natif ni modification du worktree. ROOT a parcouru les
témoins ; revue indépendante et compilation restent à faire après intégration.

Base SHA256715ddb14338022940820e7414aa08ef74ec2956bd9254a4f529849c59cadfed4.
Candidat SHA2567a46519469be832b3c957b06d2cb9f5e2a3eed7249f15fd374494694bbd3431c.
Patch SHA256f33689a3243c2bf46caa1c04f2723234cd5f83e8025def0d91a9f09bf96a5582.

Trois recettes positives VerifiedACK4,PublishingACK5,BeforeWritableOpen6 rename-gap
et deux négatives BeforeWritableOpen2/3 allocation Stage/TEMP. Writers de reprise
2/1/1 ; étapes CommitReturned pour ACK, WriterOpenAttempted pour refus avant RW.
Le témoin Stage share0 repose sur les droits FILE_ADD_FILE/ADD_SUBDIRECTORY du
handle créateur effectivement conservé, pas sur un simple pin d’attributs.
Il reste non exécuté. Les trois reprises exigent SQL/inode/parent/bytes exacts,
replay sans writer, B/famille inchangés et C Staging. Les gaps exigent refus et
préservation sans adoption/nettoyage ; ils ne prétendent pas terminer l’export.

NE PAS appliquer avant rétention du binaire r94 : le build courant doit rester
lié aux74sources r93. Après rétention, intégrer sous vérification baseSHA, relire
le diff, revue critique source puis prochain build seulement après runtime r94
terminal. Aucun nouveau hook produit, mid-copy/crash/native-close hors portée.


## r91 terminal, r93b compilé, r94 lancé ; r95 corrigé et intégré r96

Les7recettes r91 passent, archive créée/vérifiée une fois. Ne pas les relancer
sans changement justifiant la régression. Le build r93 échoue101 sur une seule
annotation de type (7diagnostics). `correct-ordinary-checkout-inference-r93b.py`
EXÉCUTÉ une fois après fin r95. JSON SHA256
a6504ef3b65199fdd39c10528558ef3c65582298a4183a86d156966426ddf457.
Build r93b ROOT28582 PASS37,38s ; check ROOT1448 PASS13,78s. Logs conservés,
échec r93 inclus dans bindings. Aucun autre changement produit de correction.

`prepare-and-release-ordinary-fresh-checkout-r94.py` EXÉCUTÉ une fois, puis runner
`provider-ordinary-fresh-checkout-run-r94.py r94` lancé sous **ROOT86893** avec
accès hôte autorisé. SQL progress/family PASS44,665s ; premier fresh checkout
en cours. Restent fresh DeclareACK,OwnedACK puis retained checkoutACK. Une seule
exécution active, aucun autre build/SQL/natif. Pas de fixture r91 réouverte.

r94 EXE82587648octets SHA256
72d7e25655bc21ca5df7a486896878fa0771368780e62f620b3c1b58d89ae66b,
CodeView621a98b1-e6bc-410a-9a0f-44ddcc208f5e/age139. Release SHA256
669c97d204533804845bb157bcb65e6d16a80342db48a96f38ca275689139296.
672sources,14deltas vsr91,5recettes. Packager r94 PRÉPARÉ/NON exécuté.

r95 terminé, cellule733 fermée, thread lu jusqu’à70/hasMorefalse :
thread:delegated-task:command%3Amcp%3A615e0744-3c02-40fd-a5ee-4bbd79d181f5%3Adelegate-task%3A1eb77803-230c-4060-968d-dd19c1e5f6d1.
Deux findings test P2 : refus quelconque accepté après allocation (F1), reopen
partiel hors garde writer (F2). Archive originale88fichiers342016octets SHA256
9ae2227a38a73867bc10fdb69f8f3b1593bea13fa704847b5676fe9a5f0ea8cf.
`package-ordinary-checkout-late-review-r95.py` EXÉCUTÉ une fois.

ROOT a intégré le candidat puis corrigé F1/F2, sans nouveau delegate : F1 exige
Cleanup.primary→Validation(Invalid) avec ensemble fermé des deux motifs namespace;
F2 garde BeforeWritableOpen autour de fresh-resume-open, compte zéro, puis retire
le garde avant les2/1writers attendus. Aucune instruction produit modifiée.
`integrate-corrected-checkout-late-recipes-r96.py` EXÉCUTÉ une fois, rustfmt PASS.
JSON correction SHA2566e06222636b5896ef865c2c32378592e4c1c843b80560435d2a1d6b92be17e31.
Diff2940octets SHA25679d69ccf0d2d2e7364d3065b0e0c20dde1a3105d7cfa39034017488c8dbf6fa2.
Fichier59411octets SHA256da0439cdf6f3653cfdc5881ff79df991252dfeddf55a225cc1e09236a9f28d9a.

Contrôle post-intégration : UN seul input parmi672diffère du binaire r94, le
fichier de tests append-only ; son préfixe r94 est byte-exact. Produit et anciennes
recettes r94 inchangés. Les5nouvelles recettes attendent compilation après ROOT86893
terminal, puis qualification ciblée sur un prochain binaire retenu. Les petits
correctifs sont auto-revus ROOT, pas indépendamment revus ni encore exécutés.

Consigne utilisateur réaffirmée : ne pas tourner en rond. Conserver les acquis,
chaque étape ferme un manque explicite ; pas de nouveau cycle de revue générale
ni réexécution des suites inchangées. Avancer aussi C13 companions pendant
les attentes : investigation native bornée en cours, pas de nouveaux tests lancés.


## C13 : prochain lot fonctionnel, investigation terminée

Agent programme_remaining_inventory terminé, SOURCE ONLY sans édition/exécution.
Réutiliser `provider/transition_reconcile` déjà câblé : classification overlay,
recovery-only, révocation, suppressions, companions hérités et représentations
multiples. source.rs27 lie closure/histoire/mappings, image.rs33 exige l’image
complète ExportCheckpoint ; ces objets DATA ne fournissent aucune autorité native.
Ne pas réécrire ce kernel. Réutiliser codecs v1 records/import.rs488/566,
validation import_validation/scoped.rs456, copie des3tables copy.rs63,
installation/protection exactes managed_content/exact.rs118 et ready_data.rs156.

Blocages actifs : managed_image.rs432 refuse recovery-only avant hashes et474
readonly modifié ; managed_plan.rs524 exige fresh==initial,205/699 refuse hérités,
935 construit empty. Ready/autonomous/catalogues contiennent aussi refus empty.
Ne pas retirer seulement ces guards : dériver un OwnedCompanionData authentifié
avec header/rows/digests/manifests/coûts depuis reconcile et le checkpoint retenu,
hasher tous les fichiers avec modes prouvés (règle unknown Windows capture/scan340).
Installer union complète overlay+bases+historique+companions+préimages ; la recherche
install_exact est actuellement limitée à image.files(). Garder companions hors
checkout et hors Capture autorisée. Protéger avant Ready, mappings/claim/receipt
dans transaction et validation autonome après arrêt Provider.

La publication source écrit aussi un reçu avec FK companion_id→header
(provider/schema15,transition_sql/publication113). Persister/valider les rows
source avant ce reçu et compter leur metadata/protection/mapping. Budget complet
avant effets, sans augmenter quota durable. La simple copie destination serait
insuffisante. L’observation fraîche d’autorité doit être traitée explicitement,
pas déguisée en suppression du contrôle fresh==initial.

Deux intentions kernel sans codec durable aujourd’hui : FromAcceptedCapture
(base effective déjà Absente) et SatisfiedDeletionIntentData (transition_reconcile60–90).
v1/scoped654 exige une base présente ; ne pas forcer ces variantes dans v1. Définir
leur codec versionné/provenance/compatibilité et tests avant de leur permettre Ready.
Header v1 n’accepte que sources6/7/8 : origine Ordinary9/10 demande extension
explicite ; companions hérités d’origine8 peuvent rester verbatim.

Une tranche Provider8→Ordinary9 à sélection fixe et variantes v1 est réalisable
d’abord, avec refus conservateur avant effets des variantes non encodables. Ce
refus est uniquement un état intermédiaire autorisé par le contrat, pas une
substitution à la fonctionnalité finale ni un motif de clôturer C13/le goal.
Les codecs et nouveaux companions Ordinary9/10 restent dans le périmètre global.

Recettes d’intégration nécessaires : conversion mixte avec inconnus/readonly/
suppression recovery-only séparés de checkout/Capture ; déconnexion Provider puis
open/capture/Restore Ordinary préparé ; interruption après protection et commit
privé avant ACK ; préservation impossible/autorité changée/codec non supporté avec
source conservée ; réexport companion préservant payloads/bases/mappings/claims.
Réutiliser tests DATA transition_reconcile et admissions/restores existants,
sans repartir de zéro ni refaire une revue générale de tout le programme.
