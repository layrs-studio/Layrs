# Reprise du 28 octobre 2026

L'utilisateur a explicitement demandé « reprends ! /goal resume ». La consigne
d'arrêt nocturne est levée. Le programme complet reste à poursuivre, sans push
ni PR. À la reprise, `get_goal` affiche encore `paused` ; l'API disponible ne
permet pas de réactiver ce statut. Cela ne retire pas l'autorisation de travailler.

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

## Lot C10 en source, non qualifié

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
en cours : test complet SQL C10 **PASS42,332s**, premier natif admission commencé.
Le second natif ACK/resolve suit seulement le succès du premier. Aucun PASS natif
C10 n'est encore revendiqué. Les15PASS r56 ne sont pas rapportés comme exécutions r57.
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
