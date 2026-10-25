# L7 — Fraîcheur des revues et preuves sur la Step exacte

État au 24 octobre 2026 : les deux recettes complètes exécutées par le coordinateur
ont réussi, chacune avec **1 test passé, 0 échec**. Leurs journaux terminaux ont
été relus pour cette mise à jour documentaire ; les preuves et leurs limites sont
détaillées ci-dessous. La revue indépendante précédente n'avait signalé aucun
défaut bloquant de production/politique et ses corrections d'assertions sont
incluses dans la recette `native_sync_v3` réussie. La **revue indépendante Opus L7
R2 est PASS, sans blocage** : elle a vérifié les journaux bruts et les huit
empreintes du manifeste R2. Cette conclusion reste limitée au périmètre L7.
Périmètre approuvé :
[programme Worktrees/partiel/VFS](worktrees-partial-vfs.md).

## Décision conservée

Conserver l'autorisation de revue sur **toute la RepositoryView**. Ne pas réduire
automatiquement sa portée à la Layer cible ou aux chemins modifiés. La lecture
de `crates/layrs-store-server/src/v3/collaboration/gates.rs:99` et de
`review_listing.rs:125` établit que la Step proposée, la View de base, le PolicySet
et l'époque d'autorisation doivent encore correspondre. Cette lecture motive
les scénarios ci-dessous ; elle ne remplace pas leur exécution.

Une mutation de Marker change la View sans imposer de nouvelle Step ni de tête
fictive, conformément à
[ADR 0015](../decisions/0015-reference-only-sync-view-cas.md). Une éventuelle
réduction future de la portée des revues demande une décision explicite et ses
propres invariants. Aucun code de politique n'est modifié ici.

## Revue et interférences : PostgreSQL réel, Router en mémoire

`crates/layrs-server/tests/support/review_interference.rs::run` exerce deux cas,
`layer` et `marker`, dans deux Spaces distincts de la base jetable existante.
Les objets sont téléversés dans le CAS du fixture ; les Steps, définitions de
Layers et références sont admises par la façade de production. Les revues,
approbations, Runs et GateEvaluations utilisent les routes HTTP existantes via
le Router en mémoire, avec de véritables identités et données PostgreSQL.
Ce parcours est `native_sync_v3` : il ne passe pas par un serveur HTTPS/TLS et
ne lance pas Docker. Le service runner du fixture soumet directement les verdicts
`passed` aux routes de Runs ; ces verdicts sont des données de test, pas le résultat
d'une commande CI exécutée.

Les deux Layers partent d'une Step commune. A propose un arbre contenant `a/` ;
B dispose d'une Step sœur contenant `b/`. Avant l'interférence, la revue de A
est approuvée et une Proof exacte permet au Gate de passer.

- **Layer indépendante** : avancer seulement B. Vérifier que sa tête change,
  tandis que la définition et la tête de A restent identiques et que la Step
  proposée est toujours le même enregistrement canonique admis.
- **Marker seul** : ajouter un checkpoint sur la Step initiale, sans Steps dans
  la transaction et avec `layerMutations: []`. Vérifier que toutes les têtes et
  définitions de Layers restent identiques et qu'un seul Marker apparaît.

Dans les deux cas, la View et sa génération changent ; le PolicySet et l'époque
d'autorisation restent identiques. La version originale de la revue reste
immuable, mais son état doit devenir `stale`. L'essai d'intégration emploie la
**nouvelle** View/génération : il doit produire `GateRejected` avec la cause
`current Step, target View and policy require a review`, sans déplacer de tête.
Un rejet dû uniquement à un ancien CAS ne satisferait pas ce test.

Une version 2 conserve exactement la Layer et la Step proposées, capture la
nouvelle View et reste `pending` jusqu'à une nouvelle approbation humaine.
Le diagnostic exact change selon la phase : avant cette version courante,
`rejection.message` vaut `current Step, target View and policy require a review` ;
après sa création et avant son approbation, il vaut
`review has insufficient current approvals`. Le Gate distingue explicitement
l'absence d'une revue correspondant à Step/View/politique et une revue courante
sans les approbations requises (`collaboration/gates.rs:98–121`). Les deux causes
sont désormais assertées ; un simple `outcome: failed` ne suffit pas.
L'intégration doit alors réussir, avec un enregistrement nommant cette version,
cette Step et exactement `accepted.transaction_id`. B et les Markers sont conservés.
La GateEvaluation doit réutiliser l'identifiant de la Proof initiale de A :
aucune nouvelle CI n'est nécessaire lorsque Step, recette et politique restent
inchangées. Ces assertions ont été exécutées avec succès dans la recette
`native_sync_v3` terminée et référencée ci-dessous, avec les verdicts de fixture
décrits plus haut.

## Approbation et Proof exacte du résultat recalculé

- **Sœur** : `review_interference::run` crée d'abord une Proof valide pour B,
  avec la même recette que A. Les deux Steps ont les mêmes parents mais des
  identités différentes. Gate et transaction de A doivent refuser cette Proof
  avec `missing fresh passing Proof for exact Step, recipe and policy`.
- **Prédécesseurs et combinaison** : `review_target_advance::run` utilise le vrai
  `plan_structural_merge` recompute le résultat après une intégration concurrente
  qui supprime `docs/readme.txt`. Le résultat doit conserver la suppression,
  avoir les deux anciennes propositions pour parents et une identité distincte
  de chacune. **Son arbre est identique à celui du parent qui supprime le fichier**,
  mais sa Step diffère par ses parents. Ce cas démontre la liaison à la Step exacte,
  pas la présence d'octets provenant des deux côtés. Avant la nouvelle version de
  revue, le Gate exige la revue Step/View/politique courante ; après sa création,
  il exige ses nouvelles approbations. Après approbation, les Proofs des parents
  ne satisfont toujours ni Gate ni intégration.
- **Attachement** : `review_target_advance.rs::prove` relit la Proof publiée et
  son Evidence ; il vérifie l'identité du Product, le sujet Step, les dépendances,
  la recette et le verdict exacts. La GateEvaluation du résultat recomputé doit
  nommer cette Step et seulement sa nouvelle Proof. L'intégration vient ensuite :
  `detail.integration.record.version`, `proposedStepId` et `transactionId` doivent
  désigner respectivement la version 2, ce résultat et
  `accepted.receipt().transaction_id`.
- **Intégrité déjà couverte, non dupliquée** :
  `proof_integrity.rs:108` remplace une projection de Proof par celle d'un autre
  sujet et vérifie le rejet ; `proof_integrity.rs:142` refuse également la
  publication d'une Proof forgée par Sync. Le début de ce même fixture couvre
  la disparition/corruption des objets Proof, Evidence et recette.

Ce parcours utilise `minimumApprovals: 1` et des Proofs issues de verdicts soumis
par le fixture. Il couvre **approbation plus résultat exact**, sans qualifier
l'exécution de la commande CI. Il ne faut pas le présenter comme le même scénario
que l'exécution Docker suivante.

## Résultat A/B exécuté : HTTPS réel et runner Docker

`native_clients_v3` lance le service de production sur HTTPS/TLS réel avec sa base
PostgreSQL jetable. Il appelle deux fonctions de
`support/exact_result_runner.rs` à travers les fixtures existants :

- `path_patch_scoped_drill::review_integration` appelle `scoped_recipe`. Une revue
  scoped approuvée, avec recette obligatoire, doit refuser l'intégration lorsque
  la Proof manque puis lorsqu'une Proof concerne seulement la Step de base.
  Le détail reste `approved` avec `proofStatus: pending` et la View/génération
  restent inchangées. Le précontrôle répond HTTP 400 `invalid_descriptor`
  (`scoped_review_gate_pending` en interne), sans reçu de transaction de rejet.
  Une Proof de la Step candidate exacte permet ensuite l'intégration et le replay
  du même reçu. Les verdicts de ce cas sont **soumis par le fixture**, sans Docker.
- `runner_drill::run` appelle `combined`. Le vrai `plan_structural_merge` combine
  deux Steps sœurs qui ajoutent respectivement `a.txt` et `b.txt`, en conservant
  `dependency.txt` inchangé. L'arbre attendu contient les trois FileIds ; le
  résultat a les deux parents. Le serveur admet ce candidat avant son intégration,
  tandis que la tête reste sur B. Contrairement au cas de suppression ci-dessus,
  les octets issus des deux côtés sont effectivement requis par la recette.

Dans `combined`, les Proofs de contrôle pour A, B et un frère du résultat sont
construites à partir de verdicts `passed` soumis par le fixture. Chaque contrôle
doit passer pour son propre sujet, mais refuser le résultat combiné et laisser
la tête inchangée. Ensuite **`layrs_runner::run` exécute réellement Docker** sur
la Step combinée non intégrée : le checkout privé est exporté depuis les objets
immuables, la commande vérifie A, B et la dépendance inchangée, et exige seulement
l'interface réseau loopback. Le runner utilise `--network none`, `--pull never`
et remplace l'entrypoint de l'image par `/bin/sh`.

La Proof produite par cette exécution doit nommer la Step combinée, sa recette et
sa politique exactes. La GateEvaluation doit nommer cette Step et sa seule Proof
exacte, puis l'intégration avance B vers le résultat. Ce fixture configure
**`minimumApprovals: 0`** : il prouve l'exécution et le Gate de Proof avant
intégration, pas une approbation humaine combinée à cette exécution réelle.
Les deux parcours se complètent ; aucun scénario unique ne prouve les deux.

`layrs_runner::run` soumet déjà la completion. Le second `layrs_runner::submit`
avec la même completion teste le **replay idempotent**, pas un second Run, une
preuve indépendante ou une seconde exécution. Les contrôles de credentials
vérifient l'absence du token testé dans le log et l'absence de
`LAYRS_RUNNER_TOKEN_FILE` dans la recette, avec le chemin d'exécution sans injection
configurée. Ce n'est pas une énumération exhaustive d'un environnement hostile,
de tous les fichiers montés ou de tous les secrets possibles.

## Preuves terminales du coordinateur, programme R1

Historique conservé : avant ces exécutions, les scénarios étaient implémentés mais
non exécutés par leur auteur ; seuls la lecture des réponses, le formatage ciblé
et les contrôles statiques étaient acquis. La revue indépendante a ensuite fait
renforcer les diagnostics de refus et l'identité de transaction, et préciser la
portée des preuves. L'attente des journaux terminaux est maintenant levée pour
les deux recettes suivantes. La revue indépendante finale R2 est ensuite passée
sans blocage ; les limites de qualification décrites dans ce document demeurent.

| Recette et journal brut local | Résultat terminal | Durée du test | Compilation |
| --- | --- | --- | --- |
| [`native_clients_v3`](../../.layrs-local/validation/l7-native-clients-programme-r1.log) | 1 passé, 0 échec, 0 ignoré, 4 filtrés | 1347.76 s | 48.86 s |
| [`native_sync_v3`](../../.layrs-local/validation/l7-native-sync-programme-r1.log) | 1 passé, 0 échec, 0 ignoré, 0 filtré | 87.52 s | 25.17 s |

Dans le journal clients, la compilation se termine ligne 15 ; les lignes 43 et
65 contiennent respectivement les PASS **scoped recipe** et **real combined A/B
runner** reproduits plus bas. Le test complet nommé
`two_native_clients_publish_clone_receive_divergence_and_local_operations` se
termine `ok` ligne 66, puis le résultat terminal figure ligne 68.

Dans le journal sync, la compilation se termine ligne 13 ; les lignes 21 et 22
contiennent, dans cet ordre, `review interference scenario: layer` et
`review interference scenario: marker`. Le test complet nommé
`real_service_provision_upload_advance_replay_concurrency_fetch_and_revoke` se
termine `ok` ligne 29, puis le résultat terminal figure ligne 31. Les marqueurs
de scénario sont donc suivis du succès terminal, et non interprétés seuls comme
des preuves de réussite.

Configuration d'exécution confirmée par le coordinateur : image obligatoire
`busybox@sha256:73aaf090f3d85aa34ee199857f03fa3a95c8ede2ffd4cc2cdb5b94e566b11662`,
refus explicite par le shell de tout `LAYRS_TEST_*_ONLY` ou
`LAYRS_TEST_PG_CONTAINER` hérité, et utilisation exclusive de son instance
PostgreSQL jetable dédiée. Les journaux Cargo attestent les résultats ci-dessus ;
ils ne consignent pas à eux seuls cette configuration d'environnement complète.
Aucune URL PostgreSQL privée ni credential n'est reproduit ici.

Le [manifeste source R1](../../.layrs-local/validation/l7-programme-r1-source.json)
initial a été contrôlé lors de la première mise à jour documentaire : ses six
empreintes concordaient. La référence de la revue finale est désormais le
[manifeste source R2](../../.layrs-local/validation/l7-programme-r2-source.json),
dont les huit empreintes et les journaux bruts ont été vérifiés par la revue
indépendante Opus L7 R2, **PASS sans blocage**. Ce manifeste couvre une sélection
de sources de tests et de pièces de preuve, pas l'ensemble du code de production.
Il ne constitue pas une qualification globale de l'état courant du programme.

Selon la chronologie vérifiée lors de R2, le binaire clients a été construit à
**19:00:30**, avant la modification sans rapport avec L7 de `fsck/graphs.rs` à
**19:04:23**. Son succès ne couvre donc pas cette modification ultérieure.
Le binaire `native_sync_v3` inclut cette modification des graphes. Les seules
corrections de sources de la revue L7 concernaient `review_target_advance.rs` et
`review_interference.rs`, qui ne sont pas compilés dans le binaire clients ; leur
preuve d'exécution provient bien du binaire sync reconstruit avec ces assertions.
Ces résultats qualifient le périmètre L7 décrit ici, sans nouveau PASS à l'échelle
de tout le programme.

Les réserves de faible gravité restent explicites : le contrôle wrong-Proof
porte sur le code de refus, sans diagnostic plus fin revendiqué ; les contrôles
de credentials ne sont pas exhaustifs ; le refus des variables de saut héritées
est confirmé par le coordinateur mais n'est pas consigné dans les journaux Cargo.

## Deux recettes complètes à reproduire par le coordinateur

Préconditions : PostgreSQL jetable, `LAYRS_TEST_DATABASE_URL` déjà configuré
par le coordinateur vers son endpoint administrateur, privilèges de création
de base et de rôle. Le test existant crée sa base UUID, son rôle runtime et son
CAS temporaire, puis les nettoie à la fin du parcours réussi. Les nouveaux cas
ne configurent aucun endpoint utilisateur et n'écrivent aucune donnée distante
réelle. Le coordinateur sérialise Cargo et les recettes PostgreSQL/Docker.

Avant chacune des deux recettes, refuser les modes partiels et un éventuel
override de conteneur PostgreSQL ; conserver l'URL privée de la base jetable
déjà configurée :

```powershell
if (Get-ChildItem Env: | Where-Object { $_.Name -match '^LAYRS_TEST_.*_ONLY$' -or $_.Name -eq 'LAYRS_TEST_PG_CONTAINER' }) {
    throw 'Retirer les drapeaux LAYRS_TEST_*_ONLY et LAYRS_TEST_PG_CONTAINER avant cette recette complète.'
}
```

Pour la revue, les interférences et les Proofs soumises via Router en mémoire :

```powershell
cargo test --locked --offline --target-dir .layrs-local/target -p layrs-server --test native_sync_v3 real_service_provision_upload_advance_replay_concurrency_fetch_and_revoke -- --ignored --exact --nocapture --test-threads=1
```

Chaîne d'appel de `native_sync_v3` :

1. `native_sync_v3.rs:127` définit le test ignoré ; à la ligne 472 il appelle
   `collaboration_drill::run`.
2. `support/collaboration_drill.rs:612` appelle `review_target_advance::run`.
3. `support/review_target_advance.rs::run` appelle `review_interference::run`.

Les nouveaux cas ne sont pas deux tests Cargo filtrables séparément. Le journal
émet `review interference scenario: layer`, puis `...: marker`. Conserver le
journal complet et le résultat final du test ; les messages de début seuls ne
prouvent pas la réussite. Un `cargo test` sans `--ignored` ne les exécute pas.

Pour les revues scoped sur HTTPS et l'exécution Docker du résultat A/B :

```powershell
$env:LAYRS_TEST_RUNNER_IMAGE = 'busybox@sha256:73aaf090f3d85aa34ee199857f03fa3a95c8ede2ffd4cc2cdb5b94e566b11662'
cargo test --locked --offline --target-dir .layrs-local/target -p layrs-server --test native_clients_v3 two_native_clients_publish_clone_receive_divergence_and_local_operations -- --ignored --exact --nocapture --test-threads=1
```

L'image épinglée ci-dessus est celle déjà présente et choisie par le coordinateur.
Ne pas remplacer ce pin par un tag mobile et ne pas tirer une image pour cette
recette. **`LAYRS_TEST_RUNNER_IMAGE` est obligatoire** : en son absence, le fixture
saute `runner_drill` et un résultat global positif ne qualifie pas le runner.
Aucun `LAYRS_TEST_*_ONLY` ne doit être défini, même avec la valeur `0`, notamment
`LAYRS_TEST_CONTENT_ACL_ONLY` ou `LAYRS_TEST_EMPTY_TREE_EXPORT_ONLY`.

Chaîne d'appel de `native_clients_v3` : le test complet appelle
`content_acl_drill::run`, puis `path_patch_drill::run` et
`path_patch_scoped_drill::review_integration` pour `scoped_recipe` ; plus loin,
`runner_drill::run` appelle `combined` lorsque le pin d'image est présent.

La qualification de cette seconde recette exige **à la fois** le succès terminal
du test complet et les deux lignes suivantes dans le même journal :

```text
L7 scoped recipe missing/wrong-Step refusal, unchanged heads, exact-candidate Proof and replay PASS
L7 real combined A/B runner, unchanged dependency, network none, exact-Step Proof gate PASS
```

Une ligne de départ, un processus toujours en cours, un succès avant le passage
du runner ou une seule de ces lignes ne suffisent pas. Conserver les journaux
complets, la commande et le pin, sans recopier l'URL PostgreSQL privée ni un token.

Validation de cette mise à jour limitée à la documentation : lecture des preuves
terminales, comparaison du manifeste initial et contrôles statiques du document.
Aucun Cargo, test natif, PostgreSQL ou Docker n'a été lancé par son auteur ; les
exécutions réussies ci-dessus sont celles du coordinateur. Le code et les tests
restent gelés. La revue finale indépendante Opus L7 R2 est PASS, sans blocage.
La présente mise à jour documentaire enregistre ce résultat transmis par le
coordinateur, sans nouvelle exécution ni nouvelle lecture système pendant le
créneau de gel ; le coordinateur contrôlera le document après ce créneau.

## Surface sans checkout et Limits

Aucune API supplémentaire n'est nécessaire pour ces scénarios. Les surfaces
existantes comprennent `POST /v3/spaces/:space_id/compare` (deux Steps exactes,
budgets serveur), la projection paginée et `fetch-plans`/downloads autorisés,
les `path-patches` ancrés sur Step/View/génération/politique et limités à 1024
changements, puis les sessions de patch et revues scoped pour les candidats.
Les déclarations de routes se trouvent dans `src/v3/router.rs` et
`src/collaboration.rs` du serveur ; aucun nouveau contrat n'est introduit.

Le parcours Docker couvre les fichiers réguliers de cette recette et sa dépendance
inchangée ; il ne qualifie pas un export CI universel de tous les types d'entrées,
ni une API générique de construction de candidats de fusion. La régression isolée
d'export d'arbre vide est un contrôle distinct et ne remplace pas le parcours
complet sans `*_ONLY`. La sauvegarde/restauration est **hors L7 et non revendiquée**.
Ces limites ne justifient pas d'inventer une API étendue.
Pas de réécriture en changesets, de validation seulement après intégration, de
correction globale de politique, de migration, de modification du câblage large
des tests ou de lancement Cargo concurrent. La revue indépendante reste à la
charge du coordinateur ; aucun sous-agent supplémentaire n'est lancé ici.
