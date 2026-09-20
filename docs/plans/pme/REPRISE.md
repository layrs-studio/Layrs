# Point de reprise — travail repris, commits autorisés

**Reprise explicitement demandée après la pause.** [PAUSE.md](PAUSE.md) conserve
le dernier checkpoint arrêté. L’utilisateur demande désormais des commits
cohérents au fil du travail, incluant les modifications antérieures au fil.
Root seul gère l’index et les commits ; aucun push demandé. Premier commit :
socle intégré existant, puis correctifs et qualifications dans des commits
distincts. Secrets, caches et sorties de compilation restent ignorés.
Les huit plans restent partiellement ouverts ; un commit ne vaut pas qualification.

Défaut100MiB localisé : `remote::projection_dirty` appelle encore la capture
en mémoire via `SnapshotLimits::default()` et `read_step_files` limité64MiB.
Le parcours streaming existe déjà ; raccord et recette à faire après le commit
du socle. Première relance globale Rust : deux échecs dans
`legacy_local_data_safety`, commandes `compact` refusant le format historique
`legacy_client_documents`. Journal `tmp/resume-baseline-rust-tests.log`.
Il faut préserver la compatibilité historique ou expliciter correctement le
routage prévu, sans supprimer ces tests. Linux lancé séparément.

**PASSE ACTIVE APRÈS LES ARTEFACTS qualified2 :**

- Spool durable privé livré : Windows/Linux, reprise HTTPS dans un nouveau processus **PASS10,60s**, corruption/epoch/droits/quota. Expiration chronologique réelle puis reprise Range, client tué avant envoi et après commit/perte reçu : **TLS PASS23,81s**, nouveau processus/reçu exact. Observations clientes et migration0016 : **PG+backup PASS88,39s**, 16 checksums, pagination40Steps/10000assertions sans saut.
- Compaction physique sans suppression d’histoire Store/CLI Windows/Linux qualifiée ; budgets objets/DAG séparés, admission99000références et dépassement sans mutation **23tests PASS**. Receive inchangé optimisé : **TLS28,22s PASS**, double authentification et zéro FetchPlan/GET. Renouvellement des leases longues corrigé et qualifié Windows/Linux ; recette upgrade préparée, pas encore exécutée.
- PolicyRoot v1 backend + sauvegarde/restauration **PASS70,02s**, client TLS perte-réponse/reprise **PASS32,24s**. Création de règles v1/v2 depuis JSON : **TLS PASS34,07s** y compris réponse perdue après retrait d’upload. CLI finale **74commandes PASS16,750s**, lecture fichier maintenant streamée ; Desktop règles JSON/appliquer/restaurer **PASS1,4min** après correction de l’étiquette textarea (premier essai1,6min échoué). ACL contenu v2 enrichies **TLS PASS18,68s** ; aucun Worktree partiel, DTO restricted annoncé avant toute capture.
- TLS volume10k PASS ; premier99k échoué surbudget16k puis correctif qualifié. Deuxième99k **ÉCHEC1646,34s au clone** : initialisation115349ms et publication214522ms passées. Cause confirmée : leases300000ms sans heartbeat durant99k installations. Échec conservé dans `measurements/T8-tls-volumes-corrected-2026-09-20.json`. **Troisième99k EN COURS**, nouveau rapport `measurements/T8-tls-volumes-leases-2026-09-20.json`, log `tmp/tls-volume-leases-20260920.log`, exécutable copié `tmp/native-volume-leases-20260920.exe`, SHA256 `257af0ae3d7b80bd665f3adbef1cc80bf9c1d8d6e74c8b6dda63aeac8a7022a8`. Ne pas annoncer99k qualifié avant résultat final.
- **Streaming A/B/C/D EN COURS**, suivi détaillé [STREAMING.md](STREAMING.md). A core vérification1GiB incrémentale qualifiée (heap Rust ~3,26Mo, pas RSS ni bout en bout). B admission avec chunks externes atomique2tests PASS ; capture privée et C matérialisation en raccord par operations. Root D : graphe avec spool chunks, vérification/FileId et uploads itérés, projection sans bytes ; lecteur local `read_step_projection` compare exactement l’ancien parcours et refuse CAS corrompu **PASS1,66s**. D2 budgets serveur séparés2tests PASS. D3 admission/réception sans buffers encore en raccord ; ne pas annoncer support1GiB produit.

Les artefacts qualified2 ci-dessous restent un snapshot vérifié de la passe précédente, pas de ces nouveaux travaux. Propriétaires actuels : root graphe/réception/projection locale/UI ; operations capture/store/matérialiseur ; fusion restore/undo/redo/switch/diff/aperçus ; selectors recette TLS100MiB/1GiB sans mutation production. La recette volumineuse attend leur raccord. Le streaming complet et le Worktree partiel ne sont pas encore livrés.

**Les huit plans ne sont pas intégralement terminés.** La qualification ci-dessous concerne la passe précédente ; la nouvelle passe reste active. Aucun commit, déploiement, publication, installation de MSI ou nettoyage de données utilisateur. Préserver les modifications préexistantes. Les blocs après le séparateur sont historiques.

| Plan | Livré et vérifié dans cette portée | Non terminé |
|---|---|---|
| T1 | Capture, restauration/undo/redo/recovery, worktrees, collisions et permissions qualifiés Windows/Linux | macOS non qualifié ; liens/modes Windows refusés ; UI multi-Worktrees hors portée initiale |
| T2 | Conflits riches Text/Code, blocs persistants, renommages couplés locaux et Sync ; matrice12cas Windows/Linux ; contrat natif Code1.1, pin1.0 immuable | macOS/autres architectures et formes hors sous-ensemble documenté |
| T3 | Historique/Markers/Releases, filtres chemin/acteur/dates locaux et distants, observations serveur et assertions clientes authentifiées, candidats puis Choose pour rewrite publié ; compaction physique qualifiée sans suppression d’histoire | Web réel non qualifié |
| T4 | HTTPS, équipes/Choose, PolicyRoots, assertions clientes, capacités, publication concurrente, annulations/Ctrl+C, reprise de téléchargement dans un nouveau processus, réception inchangée optimisée | Dernière qualification ACL par chemin en cours ; Worktree partiel absent ; streaming global, redémarrages exhaustifs, expiration en long transfert et Web réel |
| T5 | Revues/propositions/brouillons, versions/ancrages/commentaires/approbations/protections ; boucle sémantique riche TLS/Desktop | Qualification Web réelle |
| T6 | Preuves/Gates, runner Docker et webhooks TLS ; pending/begin, annulation et exactStep qualifiés | Qualification Web réelle ; scheduler hébergé et supervision non promis |
| T7 | Pont Git dans le périmètre documenté, cache/reprise, pilote public réel PyPA475commits/10branches/23tags, graphes/arbres/refs/fsck exacts | Progression fine SQLite et volumes supplémentaires restent des améliorations ; aucun dépôt utilisateur requis. ItsDangerous explicitement refusé sur ancien gitlink |
| T8 | Identité, HTTPS, 16 migrations, quotas/observabilité/backup, ENOSPC Linux, croissance AutomaticIdle, GC prudent Linux, compaction et leases longues Windows/Linux ; archives précédentes disponibles | Refaire packages après gel ; upgrade isolé préparé ; incidents/volumes complets, Web/macOS, signature quand identité disponible, pilote humain3–5 personnes, supervision opérationnelle |

## Preuves de la passe

- Rust workspace : **874PASS /16ignored /0échec**, `tmp/final-rust-tests-20260920-4.log`. Ignorés : services/mesures dédiés et helpers enfants ; ne pas les compter comme exécutés par cette commande.
- HTTPS complet + vrai runner Docker + backup/restore : **PASS424,23s**,708objets/52Steps. Six autres suites PostgreSQL effectives PASS ; quotas CAS concurrentiels séparés **PASS1,76s**. [Rapport](measurements/T4-T8-final-qualification-2026-09-20.md).
- Correctif FetchPlan postérieur à la suite globale : reproduction rouge page2, puis **TLS7,28s +5tests plans PASS**. Snapshot complet immuable borné10k, curseur validé avant résolution, replays exacts/epoch. Aucun changement client/WASM/migration.
- GC Linux : **PASS2,65s**,15 interruptions, quarantaine/restauration/purge, nouvel objet protégé remis au CAS et absence bloquante ; backup réel. [Mesure](measurements/T8-gc-2026-09-20.json).
- CLI distante **45commandes PASS11,949s** ; Desktop5parcours2,7min puis équipe finale~1,1min (observations, candidats, annulations et TooLate). Node47PASS, TypeScript/UI/Web/Desktop et builds, contrats SCM/SDK/Lens PASS. Tauri shell compile mais contient zéro test : preuve UI issue des E2E natifs.
- Clippy workspace/all-targets final **PASS28,29s**, format/tailles et15checksums PASS. WASM inchangé `97e72b2e6bc6630b1021ca7469df784e8d60feaaab5210a15160d5480802cf7b`.

## Artefacts courants

- Windows CLI/serveur/runner/worker : `dist/native-windows-qualified2-20260920`.
- Linux x64 équivalent : `dist/native-linux-qualified2-20260920/candidate`.
- MSI Windows : `dist/desktop-windows-qualified2-20260920`.

[Manifestes, tailles, SHA256 et limites](measurements/final-artifacts-2026-09-20.json). Archives release Rust1.85, mêmes sources Rust/Cargo.lock,21fichiers chacune dont guides ; extraction et8parcours réels PASS sur les deux OS, lanceur Linux vérifié. MSI checksum vérifié mais non installé. Candidats locaux non signés, checkout modifié identifié, aucun updater automatique. Les autres dossiers dist sont historiques/incomplets, pas la livraison courante. Premier build MSI échoué au lancement WiX en sandbox, reprise autorisée réussie dans dossier neuf.

## Reprendre sans refaire le travail terminé

La passe précédente est terminée ; la passe active est décrite en tête. Priorités techniques restantes : streaming global/spool durable T4 puis volumes T8 ; contrat/admission PolicyRoots et ACL fines T4 ; compaction séparée du GC conservateur T3/T8. Ces sujets sont indépendants du pilote humain et du navigateur Web, et leurs limites sont précisées dans les plans/journaux respectifs. Toute modification ultérieure des sources invalide la qualification globale des artefacts ci-dessus : reconstruire ensuite dans de nouveaux dossiers.

Environnement : `D:/Layrs/layrs`, PowerShell. Credential Manager/Docker/WiX ont besoin de la session Windows hors sandbox (`require_escalated`, déjà accepté automatiquement pour ces fixtures). Ne jamais afficher `D:/Layrs/.test-postgres-url.txt` ni les JSON privés de fixtures. PG dédié `layrs-v3-service-20260920`, config Docker `D:/Layrs/.docker-test`, host `npipe:////./pipe/dockerDesktopLinuxEngine`. Variables privées PG + `LAYRS_TEST_DATABASE_ISOLATED=1`, puis `node scripts/test-native-ui-ci.mjs cli|desktop|desktop-team`. Pas de pnpm global ; TypeScript via `apps/studio-desktop/node_modules/typescript/bin/tsc`. Navigateur in-app indisponible (`browsers.list=[]`), aucun contournement Web ; Tauri/WebView2 natif fonctionne.

Ne pas compiler la CLI pendant son exécution sous Windows. Sources Lens épinglées : éviter formatage/modification accidentels ; la garde `scripts/check-packaged-wasm.mjs --offline` vérifie le guest sans le reconstruire. Linux hors réseau nécessite index et cache Cargo Windows montés en lecture seule pour la nouvelle dépendance ctrlc.

---

# Etat courant antérieur - 20 septembre 2026 (historique)

Ce bloc remplace les anciens etats ci-dessous. Les huit plans ne sont pas encore tous termines. Aucun commit, publication d'image ou deploiement externe.

| Plan | Livre et verifie | Travail encore en cours |
|---|---|---|
| T1 | CLI/Desktop natifs, capture multihead, restore/undo/redo, recovery, changements fichier/dossier, .layrsignore ; symlinks Linux et reprise verifies | Performance grands arbres en mesure ; Windows/macOS symlinks refuses |
| T2 | Weave/receipts/Products atomiques ; variantes, edition texte Desktop/remplacement binaire CLI ; WASM zero import avec fuel/memoire/Job Windows ; previews sans ecriture | Supervision Lens Linux/macOS non qualifiee ; repli structurel explicite |
| T3 | Publication/revocation Release immuable, droits/preuves exactStep ; filtres Operations et observations Step, projection reconstructible ; identite locale explicite CLI/Desktop | Audit pagination et lifecycle T5 ; derniere recette UI identite en cours |
| T4 | Deux clients HTTPS/PG : publication, clone, divergence, conflits et resolution, ref-only, epochs/perte reponse PASS | Regression globale apres nouveaux lots |
| T5/T6 | Revues versions/ancrages/approbations/protections, preuves exactStep, runner Docker et webhooks TLS reels PASS ; UI partagee et E2E equipe PASS | Completer interfaces restantes et regression |
| T7 | Import/export reels, SHA1/SHA256, progression, annulation/reprise ; export2000 commits 15,246s /10processus contre278,687s /8014 auparavant, fsck exact PASS | Import206,749s mesure concurrente ; pilote depot actuel refuse car historique .layrs |
| T8 | Archives CLI Windows/Linux et MSI Windows non signe construits ; doctor lecture seule CLI/Desktop ; quotas CAS logiques PG concurrent PASS ; backup/restauration et piloteCompose local PASS | Quotas finitions, backup inclureevenementsRelease, nouvellebaseline CI, volumes ; signature/macOS/pilotehumain externes |

Agents : root integration CLI/Desktop/Web/diagnostic/identite/volumes/backupRelease ; fusion_native quotas CAS/config/metrics ; selectors finitions backend T5 ; operations gates globaux/distribution/CI/procedures exploitation. Ne pas chevaucher leurs fichiers.

Verification courante : Desktop SCM 2PASS1,2min (fusion/resolution manuelle et diagnostic), equipeTLS Release publication/revocation/admin/notifications/webhook PASS48,3s. CLI10PASS dont identite/doctorcorruption. Historique6PASS +storeidentite2PASS. Globale784PASS/1FAILfixtureFK corrigee depuis/12ignored explicites (servicesouharness) ; rerun final requis. Clippyworkspacealltargetsstrict PASS, file-sizePASS et7migrationsverrouilleesPASS. Web serviHTTPS testeHTTP ; navigateurvisuel Web indisponible.

Reprise : lire les huit journaux et messages agents. Ne jamais afficher D:/Layrs/.test-postgres-url.txt ou les .test-native-ui-fixture*.json (secrets). Fixturecourante .test-native-ui-fixture-releases-20260920.json active (root) ; ReleaseE2E publiee puis revoquee, prochainrerun preparernouvelleversion. MesuresT8 dansmeasurements/ avecfixturesfraichesconservees ; 100000fichiers+100dossiers depassent100000entrees,100MiB/1GiBrefusesavantmutation. Anciennesmentions ci-dessous seulementchronologiques. Sources ont encore change apres packaging : reconstruire finalavant annoncer une archive representantdernierlot.

---

# Reprise de l'implémentation PME

Dernière session : 2026-09-20. Travail autorisé : implémenter les huit plans, en parallèle seulement sur des fichiers indépendants. Préserver tout le travail préexistant non commité. Aucun déploiement ni publication effectué.

## Lire en premier

1. Ce fichier, puis le journal du chantier concerné et son plan numéroté.
2. `git status --short` : le dépôt contenait déjà une refonte importante non commitée avant cette session. Ne pas réinitialiser ces fichiers.
3. Les ADR 0006–0013 et `docs/architecture/scm-v1-target.md` priment sur les anciennes descriptions V1.
4. Vérifier les tests cités et l'état réel du code ; « implémenté » ne signifie pas « branché dans le produit ».

## Tableau initial ? remplac? par le point du 20 septembre ci-dessous

États : à faire / en cours / partiel testé / terminé vérifié. Une dépendance en attente ne signifie pas un abandon.

| Sous-plan | Dernier état | Journal / prochaine action |
|---|---|---|
| T1.C contrats locaux | Partiel testé | [T1](journal/T1.md) — façade snapshot/batch livrée ; contrat filesystem/matérialiseur reste |
| T1.S capture/switch natifs | Partiel testé | T1 — admission de snapshots acquise ; scanner/switch/surfaces restent |
| T1.R restauration/undo | À faire | T1 — dépend de T1.S |
| T1.V revert partagé | À faire | T1 — dépend de T1.R/T2.M puis T4.P |
| T2.C contrat fusion | Partiel testé | [T2](journal/T2.md) — contrat par fichier livré ; contrat graphe reste |
| T2.M fusion pure | Partiel testé | T2 — code/texte et repli livrés ; conflits canoniques/DAG restent |
| T2.U interfaces | À faire | T2 — dépend de T1.S/T2.M |
| T3.C références/sélecteurs | Partiel testé | [T3](journal/T3.md) — sélecteurs purs + lecture store livrés ; CLI/SDK complet reste |
| T3.H historique/points nommés | À faire | T3 — dépend de T1.S |
| T3.R réécritures | À faire | T3 — dépend de T2.M/T1.R |
| T4.C contrats de services | À faire | [T4](journal/T4.md) — identité commune avec T8.C |
| T4.E moteur transactionnel | À faire | T4 — implémenter vraie V3Facade |
| T4.L client V3 | À faire | T4 — dépend de T1.S/T4.C |
| T4.P preuve deux clients | À faire | T4 — dépend de T4.E/L et T8.I/H |
| T5.C contrat revue | À faire | [T5](journal/T5.md) — après T2.C/T4.C |
| T5.R revue intégrée | À faire | T5 — après moteur/identité |
| T5.G protections | À faire | T5 — après T6.P/T4.P |
| T6.C contrat preuves | À faire | [T6](journal/T6.md) — après T4.C, accord T3.C |
| T6.P proofs/gates réels | À faire | T6 — après T4.E/T8.I |
| T6.X exécuteur/webhooks | À faire | T6 — contrat puis services réels |
| T7.C mapping Git | À faire | [T7](journal/T7.md) — après T1.C/T3.C |
| T7.I import | À faire | T7 — après T1.S/T3.H |
| T7.E export | À faire | T7 — après T1.S/T3.H |
| T8.C identité/configuration | À faire | [T8](journal/T8.md) — accord avec T4.C |
| T8.I authentification | À faire | T8 — après T8.C |
| T8.H instance HTTPS | À faire | T8 — après T4.E/T8.I |
| T8.B sauvegarde/restauration | À faire | T8 — prototype possible avant intégration |
| T8.D distribution/recette | À faire | T8 — après scénarios essentiels |

## Règles de journalisation

- Avant un lot : inscrire objectif, fichiers possédés et prochaine action dans `journal/Tn.md`.
- Après un lot : inscrire comportement, commandes exactes et résultats, limites et ce qui reste non raccordé.
- Ne cocher une case du plan que lorsque toute son exigence est satisfaite. Un lot partiel reste partiel.
- Actualiser ce tableau aux points de reprise. En cas d'interruption brutale, journal et modifications locales sont la source la plus récente.
- Chaque nouvelle session ajoute une entrée datée ; conserver les résultats antérieurs, sans les présenter comme validation du code modifié ensuite.

## Organisation de cette session

- Principal : T1, contrats/façade client natif, intégration et revue.
- Agent `fusion_native` : T2, uniquement weave/code Lens et journal T2.
- Agent `selectors` : reprise T4, moteur transactionnel store-server et façade V3, journal T4.
- Agent `operations` : T8, identité/configuration, composition serveur et déploiement, journal T8.
- Tests et éditions sur fichiers partagés séquentiels ; les commandes Cargo peuvent attendre le même verrou de build.

## Dernier point de reprise vérifié — 2026-09-19

- Les deux agents ont terminé leurs lots ; aucun agent ne poursuit une écriture en arrière-plan à ce point.
- Fait : façade native de préparation/admission de snapshots (pas scanner), préconditions avant capture et au commit, moteur pur par fichier texte/code, sélecteurs et leur lecture du vrai store.
- 68 tests ciblés réussis : 8 `native_capture`, 44 graph (dont 13 nouveaux sélecteurs), 16 weave/code (dont 12 nouveaux réconciliation).
- Clippy strict réussi : `cargo clippy --offline -p layrs-client-core --test native_capture -- -D warnings` et `cargo clippy --offline -p layrs-graph -p layrs-weave -p layrs-lens-code --all-targets -- -D warnings`.
- Non exécutés : suite legacy entière, serveur/PostgreSQL, Desktop natif et E2E. Aucun résultat ci-dessus ne prouve un parcours utilisateur complet.
- Aucune modification de schéma ni migration ; aucune bascule partielle des anciennes surfaces ; aucun commit créé. Les modifications préexistantes ont été conservées.
- Reprise prioritaire : **T1.C/S, acquisition filesystem stable et contrat de matérialisation**, puis init/status/step/switch raccordés ensemble. Lire les limites de `journal/T1.md` avant d'utiliser `native_scm`.
- Parallèle possible ensuite : T2 plan de fusion DAG/objets de conflit ; T4.C/T8.C contrat d'identité et services. Éviter de multiplier les agents si le quota est faible.

Message de reprise suggéré : « Lis docs/plans/pme/REPRISE.md et les journaux, vérifie git status, puis poursuis T1.C/S à la prochaine action indiquée. Préserve les changements existants et actualise le journal après chaque lot validé. »

## Obstacles connus à revalider

- Ancien init CLI Windows : `could not create staged SQLite metadata`, OS error 1 lors de l'audit.
- Serveur normal : V3 montée avec `UnavailableV3Facade` ; clients encore V1/HTTP.
- Nombreuses fonctionnalités existent dans l'ancien modèle mais ne sont pas reliées au SCM V1 natif.
- `pnpm` absent du PATH à l'audit ; `node` et `corepack` présents. TypeScript des apps accessible par leur `node_modules/typescript/bin/tsc`.

## Point de reprise courant ? 2026-09-20

Les informations ant?rieures d?crivent les ?tapes pr?c?dentes. Ce tableau pr?vaut ; d?tails, commandes et limites dans les huit journaux.

| Sous-plans | ?tat courant | Prochaine action |
|---|---|---|
| T1.C/S | Partiel test? : scanner natif, CAS, mat?rialiseur sauvegard?/journalis?, CLI/Desktop | Types fichier/dossier et symlinks ? mat?rialiser ; matrice plateformes et concurrence |
| T1.R | Partiel test? : fichier/sous-arbre/View, undo/redo, reprise apr?s panne | Matrice multi-Worktree et surfaces restantes |
| T1.V | En cours : moteur inverse confi? ? fusion_native | Publier/recevoir le nouveau descendant |
| T2.C/M | Partiel test? : fusion fichiers/DAG, conflits canoniques, API preview | Receipt atomique, h?te born? et application |
| T2.U | ? faire | UI de fusion/r?solution apr?s acceptation native |
| T3.C/H | Partiel test? : s?lecteurs, Operations, markers/releases CLI/Desktop | S?lecteurs conviviaux, pagination/recherche |
| T3.R | En cours : fusion_native | Replay/squash, mappings et tests r?els |
| T4.C/E | Partiel test? : fa?ade production PostgreSQL/CAS | N?gatifs/politiques et recette compl?te |
| T4.L/P | Partiel test? : HTTPS, deux clients r?els PASS | CLI/Desktop, r?ponses perdues et corruptions |
| T5.C/R/G | En cours : operations | Evidence revues, approbations/protections, UI |
| T6.C/P | En cours : operations | Products/Gates et tests serveur |
| T6.X | ? faire | Ex?cuteur/webhooks apr?s autorisations scoped |
| T7.C/I/E | Partiel test? : import atomique/idempotent, export historique/snapshot, CLI | Volume, progression/reprise et pilote |
| T8.C/I | Partiel test? : configuration, identit?, comptes/device/services/r?vocation | Raccordement client et recette compl?te |
| T8.H | Partiel test? : Compose/Caddy pr?par?s et TLS int?gr? r?el | D?ploiement/pilote HTTPS de production |
| T8.B | Partiel test? : backup/restauration PG+CAS r?els PASS | Volume, r?p?tition et politique r?tention |
| T8.D | ? faire | Packaging/signature, plateformes et pilote |

Validations principales : 9 tests native_workspace, 19 scm_write_batch, 1 sc?nario CLI native_workflow ; TypeScript Desktop et compilation Tauri r?ussis. Parcours Desktop natif : **1 PASS, 20 s**, vrai WebView2/IPC et v?rification fichiers. D?marrage WebView a n?cessit? ex?cution hors sandbox. Capture ?cran inspect?e. T7 : 6 tests r?els Git PASS et Clippy strict de la crate. T4 : TLS/PG r?els, deux stores et Worktrees distincts, publication/clone/r?ception/divergence PASS ; essais suppl?mentaires en cours.

Agents actifs : principal = int?gration/s?ret? locale/CLI/Desktop ; fusion_native = T3.R/T1.V apr?s T2/T7 ; selectors = T4 client/r?silience/auth ; operations = T5/T6 apr?s T8. V?rifier leurs ?tats avant de reprendre leurs fichiers. Aucun commit ni d?ploiement externe.

PostgreSQL local : conteneur layrs-v3-service-20260920 ; URL priv?e dans D:/Layrs/.test-postgres-url.txt, ne pas afficher les identifiants. Docker Desktop d?marr? et d?pendances r?cup?r?es. pnpm absent du PATH : utiliser les scripts Node locaux. Navigateur int?gr? indisponible ; recette effectu?e via le harnais Desktop natif du d?p?t.

Les plans restent partiels. Mat?rialisation symlinks/typechanges explicitement refus?e ; Weave accept? avec receipt, UI conflits, bascule totale des anciennes surfaces, suite legacy, packaging, Linux/macOS et pilote encore incomplets. Ne pas confondre test cibl? r?ussi et recette globale.

### Mise ? jour 20 septembre ? extension format et int?grations

- Stockage natif version **2** en cours par fusion_native : Products CAS pour WeaveReceipts atomiques, baseline SQL2 explicite. Baseline1 conserv?e ; storesversion1 refus?s sans mutation, aucune conversion automatique. Ne pas remplacer un ancien .layrs pour le faire ouvrir.
- Interface remote/reviews, replay/revert/squash et pagination raccord?es ; tests CLI4+history3 PASS, Clippy client/CLI strict PASS. E2E Desktop ?largi en cours de recompilation.
- Operations poursuit runner conteneur r?el T6.X apr?s tests revues/gates/proofs/notifications et backup. Selectors poursuit publication/r?ception Layers/Markers/Releases et exactStep pour runner.

### Reprise prioritaire - integration suivante, 20 septembre 2026

Ce bloc remplace les etats de la table precedente. Aucun plan global declare termine.
- T1: capture multihead preserve les autres tetes (test PASS), CaptureReason automatique/avant operation, retry materialisation avec nouveau journal apres rollback, correctif Windows rename court/aligne. Undo/redo conflicted corrige/teste par fusion_native. Symlinks et changements fichier/dossier toujours a traiter.
- T2: Products CAS format2, acceptation Weave/recu atomique, resolutions partielles/completes testees (7 tests + stockage complet PASS). CLI weave/resolve-term et Desktop fusion en validation. Worker Lens sous supervision OS en cours; aucune attestation fictive, acceptation semantique reste refusee sans sandbox complete.
- T3: selectors noms exacts, pagination ancree Steps/Operations, recherche chemin/texte et replay/revert/squash branches CLI/Desktop. E2E Desktop restauration/undo/redo/revert/historique/autocapture PASS35.2s, screenshot inspecte.
- T4: TLS/PG reels PASS avec fusion propre recue seconde machine, compare autorise, Products negatifs. Correction epoch obsolete et references-only en cours selectors, puis transport conflits.
- T5/T6: PG revues versionnees/ancrages/protections/gates/notifications PASS; runner Docker exactStep reel PASS, Release gate positive/negative PASS. Webhooks signes TLS/503/retry/rotation/replay/crash PASS. UI administration operations en cours, revue/compare partages Web/Desktop branches, E2E UI equipe restant.
- T7: import/export Git natif et tests reels livres; volumes/progression et pilote restants.
- T8: identite comptes/device/service/revocation, backup PG+CAS restaure57objets4Steps PASS; WebDockerfile construit et Caddy validate PASS. Image pas publiee, instance pas deployee. Packaging/signature/pilote multiplateforme restants.
- Compilation temporairement instable lorsque agents ecrivent nouveaux modules worker/admin; attendre lot stable puis relancer cargo tests/Clippy. TypeScript Web/Desktop PASS apres compare/merge. Nouveau E2E fusion et CLI conflict resolution a executer.
- Owners actuels: root integration CLI/Desktop/Web et materializer; fusion_native worker/merge; selectors remote/syncserver; operations admin/runner/webhooks. Ne pas chevaucher.
