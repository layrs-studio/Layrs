# Reprise du 28 octobre 2026

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
