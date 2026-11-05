# Passation de nuit — 28 octobre 2026

L'utilisateur demande de terminer le travail en cours puis de s'arrêter pour
la nuit. Ne pas lancer de nouvelle qualification longue. Le goal complet reste
inachevé ; le mettre en pause après conservation du résultat du test actif et
du travail. Reprendre uniquement sur demande explicite.

## État conservé et résultat terminal

- Branche `feature/worktrees-partial-vfs`, dernier commit qualifié `b3ceea9` :
  protection et copie ordinaires B vers C, 18 recettes r39–r41 réussies.
- Commit local `3291ab5` : lot Ready exact r44 et archive des tentatives, marqué
  WIP car le natif échoue. Le commit suivant conserve le candidat famille et
  cette passation, également WIP/non qualifié. Aucun push effectué.
- Le lot privé C Ready possède 18 sources Rust correspondant exactement au
  snapshot r44, modulo CRLF. Les sources de publication familiale suivantes
  sont conservées dans un lot distinct. Les commits restent WIP/non qualifiés.
- r44 : trois tests purs et neuf SQL réussis. Le test natif
  `managed_ordinary_ready_replays_withheld_private_close_ack_without_promoting_b_or_family`
  a échoué après **1125,762 s**, exit101. La session ROOT **57482** est terminale
  exit1. Le témoin de lignes `projection_content` reçoit trois lignes au lieu
  de deux, après l'ACK de fermeture réussie volontairement retenu et avant le
  rejeu Ready. Aucun test ni agent ne doit rester actif après la sauvegarde.
- Revue critique SOURCE ONLY du lot famille terminée : tâche T3
  `211723d1-6734-49b3-b0f2-cdfd28077de8`. Aucun défaut concret trouvé ; les douze
  sources, le manifeste, le diff et la baseline sont inchangés aux deux contrôles.
  Aucun résultat de compilation ou d'exécution du lot famille n'est revendiqué.
  Le rapport, les entrées, le diff et les recettes proposées sont conservés dans
  `docs/continuation/worktrees-partial-vfs/ordinary-family-ready-*-20261028-r45.*`.

## Provenance Ready

r42 n'a pas été exécuté. ROOT a trouvé après la première revue une comparaison
incorrecte entre digest de sélection initial et digest SQL versionné. La revue
finale r43 vérifie le correctif et les 636 sources ; release SHA-256 :
`2877a04c23541306285ca1ab62de81ff1841de83ad84d154859c8d8490922a2c`.

r43 réussit les trois tests purs, puis échoue pendant la construction de fixture
SQL : TX et LEAF ont 34 caractères au lieu de 32. Aucun Ready writer ni test
natif n'est atteint. La fixture échouée reste conservée, sans réouverture.
La correction r44 change uniquement ces deux constantes. Construction des tests
réussie en 25,62 s, mêmes 636 sources dont un seul fichier de test modifié.
EXE r44 SHA-256 :
`437b8244141062302c16395414f1c1a02297d4cfe8656b431234e74b1e9ef787`.
Release ROOT étroite r44 :
`6b3de5b46d13ceba37ae976f4553c8fab5fb0ba7135dba636a4b989ab011bf1f`.

Les preuves sont sous
`.layrs-local/validation/provider-family-publication-20261027-r42` à `r44`.
r43 conserve 29 input bindings exacts ; r44 conserve ses propres bindings.
Les bases et fixtures ne doivent pas être rouvertes pour compléter une preuve.
Le script d'archive de réussite n'a pas été exécuté. L'archive des tentatives
non qualifiées est créée et vérifiée :
`ordinary-ready-attempts-evidence-20261028-r42-r44.tar.gz`, 1 991 fichiers,
6 012 342 octets, SHA-256
`707d891ccc625619378efdb59b1f87470fc027ecf7fcbcc8f061c537003c83c6`.
Elle contient le log d'échec et la note `provider-ordinary-ready-r44-native-failure.md`.

Premier travail à la reprise : rapprocher l'attendu du témoin `require_ready_rows`
de l'union exacte et des mappings déjà copiés. Le témoin ajoute seulement le
contenu courant, alors que `prepare_suffix` ajoute tous les FileIds requis
manquants, dont une base sélectionnée absente de l'historique. Le log montre
une ligne supplémentaire de 21 octets. Cette cause probable reste à corriger
et qualifier ; ne pas affaiblir l'égalité complète des tables. La fixture r44
échouée ne doit pas être rouverte/réutilisée. Le test famille partage ce helper.

## Lot suivant : publication familiale

Code intégré, non qualifié : lecture complète des groupes conservés et de la
protection C ; lecteur privé Ready distinct autorisant le membre familial
Initializing ou Ready ; writer physique gardant C en lecture pendant le CAS
familial, son COMMIT, sa fermeture observée et sa relecture. Le seul effet
attendu est `worktrees(C).state: initializing -> ready`. B reste Staging, C
reste Ready privé8 et n'est pas admis.

Trois nouveaux tests SQL et un test natif sont écrits. Proposition r45 :
16 recettes (3 purs, 12 SQL, 1 natif). Elles ne sont pas exécutées. Les anciennes
entrées Ready restent strictement Initializing ; aucun résultat r44 n'est
transféré automatiquement à ce lot. Le nouveau test natif partage seulement des
helpers rendus visibles dans l'ancien test Ready, sans changer son comportement.

L'observation E1 de la revue doit être traitée avant qualification : le scénario
natif retient l'ACK après une fermeture réussie puis reprend via AlreadyReady ;
il n'exerce donc pas la relecture fraîche du chemin Updated réussi. Prévoir une
couverture physique de ce chemin, sans présenter le scénario actuel comme cette
preuve. Aucun nouveau scénario long n'est lancé cette nuit.

Manifeste de revue :
`D:/Layrs/tmp/provider-ordinary-family-ready-review-inputs-r45.json`, SHA-256
`8ce883147220f782e5c0776f8085e79e2cdb79f83123ca5d5b5912f494fc2b89`.
Le diff complet, le manifeste et les recettes proposées sont dans `D:/Layrs/tmp`.
Toute modification ultérieure doit être explicitement rapprochée de la revue.

## Suite du programme, toujours incomplète

Voir `worktrees-partial-vfs-ordinary-publication-next.md` pour les symboles,
invariants et tests proposés. Points principaux :

1. B Ready/Published nécessite la partition exacte des reçus entrants/sortants,
   avec Ready toujours bloquant pour les mutations ordinaires. Ajouter des
   phases sans corriger le comptage des reçus casserait l'admission de B.
2. Le driver de publication source doit interpréter la phase réelle après
   promotion ; les lecteurs Staging actuels ne peuvent pas servir de preuve.
   L'owner ordinaire distingue COMMIT et fermeture physique terminale.
3. C9 et l'autonomie C nécessitent une reconstruction historique ordinaire,
   le handoff réel B Published, l'archive du checkout et la validation complète
   de tous les groupes familiaux, sans faux descripteur Provider.
4. Qualification encore manquante : propriétaire perdu/crash, panne réelle de
   fermeture, suppression positive/arborescence imbriquée en natif, restauration
   non vide en natif, transitions/companions/expansion, CLI/Desktop et acceptation
   complète. Les scénarios README avec ACK retenu ne prouvent pas ces propriétés.

Préférences : GPT-6.1-Sol, jamais GPT-6-Sol ; commits locaux autorisés, pas de push
ni PR ; conserver le stash original. La reprise future doit conserver tout
l'objectif, sans annoncer que ces incréments seuls terminent le chantier.
