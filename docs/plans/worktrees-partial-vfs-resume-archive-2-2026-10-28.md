# Reprise worktrees, projection partielle et VFS — archive 2

Suite de `worktrees-partial-vfs-resume-archive-2026-10-28.md`, sections les plus anciennes.

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
