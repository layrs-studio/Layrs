# Reprendre le programme Worktrees / partial VFS

## Complément exporteur ordinaire protégé — r26 à r27

L’exporteur conserve maintenant l’accès exclusif, l’image physique et les handles
des fichiers. Il copie les contenus courants, les bases sélectionnées et les
contenus historiques exacts, sans créer C. La revue a corrigé la résolution des
bases absentes de l’historique et l’annulation pendant les longues lectures.

Les 17 tests r27 passent : sept tests du plan ordinaire, six régressions du
planificateur existant, trois tests du scanner et une recette native de 285 s.
Cette dernière vérifie notamment une base sans historique, un contenu conservé
uniquement pour l’inverse, les copies complètes/par morceau, les refus d’écriture,
l’annulation pendant une vraie vérification, les témoins complets inchangés et
la libération des accès après fermeture. Compilation du workspace passée.

r26 est un artefact compilé/audité non exécuté : une assertion de test a été
corrigée avant lancement. L’archive conserve les deux versions et les preuves
r27 : `ordinary-export-owner-evidence-20261027-r26-r27.tar.gz`, 1 217 fichiers,
3 376 422 octets, SHA-256
`5bb1690789598e5343f2488e6094c33572e179823603d73cd0faac934c873d3b`.
Inventaire : `ordinary-export-owner-inventory-20261027-r26-r27.json`.

Suite : [pilote B → C](../../plans/worktrees-partial-vfs-ordinary-export-driver.md).
Le journal sortant doit coexister avec l’admission entrante de B ; C doit obtenir
ses propres réservations, protections, publication et admission autonome. Les
inverses hérités, publications successives et autres étapes restent ouverts.

## Complément planification depuis une source admise — r24 à r25

Le planificateur d’export authentifie maintenant l’admission entrante réelle du
worktree, son reçu et sa base. Les 19 tests de format et le test SQL de copie
passent sur r24 ; ce dernier vérifie aussi le rejeu, le refus de collision avant
écriture et les rollbacks. La revue a corrigé l’omission des reçus de restauration
dans la boucle de copie v2.

Le test natif passe sur r25 en environ 204 secondes : véritable destination
admise, capture/restauration, octets conservés uniquement pour l’inverse présents
dans le plan, refus de fausses admissions, planification sans écritures et résultat
identique après réouverture. L’échec r24 est conservé : son témoin englobait aussi
la réouverture, qui libère normalement le budget de restauration terminé. Le
test corrigé garde les témoins complets des bases autour de chaque planification
et vérifie explicitement cette libération. Le code de production est identique
sur r24 et r25 ; les vingt tests inchangés ne sont pas réexécutés.

`admitted-export-evidence-20261027-r24-r25.tar.gz` contient 1 213 fichiers vérifiés
(3 337 651 octets), SHA-256
`1793de094957a8dcf93d200cbcb3b2856f125b4ae2a07ec63e996acd91495e98`.
Inventaire : `admitted-export-inventory-20261027-r24-r25.json`. Sources, revues,
scripts, résultats et échec analysé sont inclus ; binaires, bases et fixtures
restent locaux. Aucune fixture échouée n’a été réouverte.

La prochaine étape construit le propriétaire physique et le pilote d’export
ordinaire jusqu’à C admis : [plan détaillé](../../plans/worktrees-partial-vfs-imported-restore-history.md).
Le transfert physique B → C, les inverses hérités et les publications successives
restent à qualifier ; les autres étapes du programme sont toujours ouvertes.

## Complément historique de restauration v2 — r23

Les 11 recettes r23 passent : 7 codecs compatibles v1/v2, 3 tests SQL de
métadonnées et un scénario natif sur destination admise. Une nouvelle restauration
crée désormais une nouvelle capture même à changements identiques ; l’historique
déjà engagé reste stable. Déduplication ordinaire, rejeu, réouverture, inverse local
et fsck réussissent. Le parcours natif dure environ 260 secondes.

`imported-history-evidence-20261027-r23.tar.gz` contient 607 fichiers vérifiés
(1 723 345 octets), SHA-256
`e5be607df8aacf7b66e407843dc311bd6e806dd183b2878835072c2b2ceb29d0`.
Inventaire : `imported-history-inventory-20261027-r23.json`. Sources, revue et
journaux inclus ; binaires, bases et fixtures restent locaux.

La lecture typée des origines est implémentée et relue, mais l’inverse importé réel,
le réexport d’une source9 et les publications positives multi-segments ne sont pas
encore qualifiés. La prochaine étape ajoute le véritable propriétaire et pilote
d’export ordinaire : [plan détaillé](../../plans/worktrees-partial-vfs-imported-restore-history.md).
Les autres étapes du programme restent actives.

## Complément annulation avant journal — r22

La recette native r22 réussit dix cas d’annulation/reprise sur une destination
admise : staging de restauration, six états d’installation partielle du contenu
et sept points d’interruption du nettoyage/de la libération. Les refus ciblés
préservent les témoins ; le réessai du preview, la restauration inverse,
l’historique et l’import initial restent valides. Détails et limites dans le
[checkpoint](../../plans/worktrees-partial-vfs-checkpoint.md) et le
[contrat de staging](../../plans/worktrees-partial-vfs-restore-staging-recovery.md).

`restore-cancel-evidence-20261027-r22.tar.gz` contient 596 fichiers vérifiés
(1 706 121 octets), SHA-256
`4f78afd2f5c63c867f070a2ff3e10994b78ec30c72f5710ea7dfcbd18f2a7801`.
Inventaire : `restore-cancel-inventory-20261027-r22.json`. Sources, revue, scripts
et journaux sont portables ; binaires, bases et fixtures restent locaux. Cette
archive ne qualifie pas un autre hôte.

```powershell
tar -xzf docs/continuation/worktrees-partial-vfs/restore-cancel-evidence-20261027-r22.tar.gz
```

La suite inclut le transport authentifié des reçus de restauration hérités et
la publication positive sur plusieurs segments :
[investigation et plan](../../plans/worktrees-partial-vfs-imported-restore-history.md).
Les autres reprises, transitions, companions, expansion et hôtes produit
restent à terminer ; le goal reste actif.

## Complément reprise des restaurations admises — r19 à r21

Le scénario natif r21 réussit les quatre interruptions après journal durable,
après quarantaine, après installation et avant commit, avec reprise bound/direct,
inspection nonmutante, refus ciblés, rejeu, inverse et conservation de l’import.
Le test SQL de bornage passe sur r20. La tentative native r20 a été refusée par
le sandbox avant création du checkout ; r21 utilise une fixture neuve avec
l’accès hôte nécessaire. r19 est conservé sans exécution. Limites détaillées
dans le [checkpoint](../../plans/worktrees-partial-vfs-checkpoint.md).

`restore-recovery-evidence-20261027-r19-r21.tar.gz` contient 1 758 fichiers
vérifiés (4 848 141 octets), SHA-256
`ad224df8beaf7c83c64c51f0f1595a2649cef23f53a6996973879b36e1a89ec7`.
L’inventaire est `restore-recovery-inventory-20261027-r19-r21.json`. Sources,
revues, scripts et journaux sont inclus ; binaires, bases et fixtures restent
locaux. Cette preuve ne vaut pas qualification d’un autre hôte.

```powershell
tar -xzf docs/continuation/worktrees-partial-vfs/restore-recovery-evidence-20261027-r19-r21.tar.gz
```

L’annulation de la fenêtre avant journal est désormais qualifiée sur r22,
y compris les états CAS partiels décrits ci-dessus. Les autres cas limites restent ouverts :
[plan de reprise du staging](../../plans/worktrees-partial-vfs-restore-staging-recovery.md).
Crashes de processus, journaux SQLite chauds, filiation inverse importée,
transitions, companions, expansion, nettoyage et parcours produit restent ouverts.

## Complément ouverture ordinaire et cycle modifiable — r16 à r18

Le jalon r16 qualifie la réouverture publique du schéma 9 et la fermeture
explicite de SQLite sur les chemins de succès et d'erreur testés. Le scénario
natif r18 qualifie préparation hors ligne, modifications, captures, lectures,
comparaison, réouverture et restaurations avec le chemin source indisponible.
Les détails et limites sont dans le [checkpoint](../../plans/worktrees-partial-vfs-checkpoint.md)
et le [contrat](../../plans/worktrees-partial-vfs-managed-admission-contract.md).
La reprise des restaurations interrompues est qualifiée séparément sur les
quatre points r21 ci-dessus ; les autres chemins de récupération, historiques
inverses importés, transitions, companions, expansion, nettoyage et parcours
produit restent à terminer. Les sections plus anciennes ci-dessous conservent l'état
historique de chaque jalon.

`ordinary-evidence-20261027-r16-r18.tar.gz` contient 1 744 fichiers vérifiés :
sources figées, revues, recettes et journaux de compilation/exécution, ainsi
que la révision r17 remplacée sans exécution. Inventaire :
`ordinary-inventory-20261027-r16-r18.json`. Archive : 4 768 023 octets ; SHA-256
`e3ec0c58cc3375bdc6bf8b4df9ffc28e715e8d5ba2757c3b0754f1a94b07298e`.
Exécutables/PDB, bases SQLite et fixtures natives restent exclus. Ce paquet
préserve des preuves historiques et ne qualifie pas l'hôte d'un autre poste.

Sur un poste où ces preuves sont absentes :

```powershell
tar -xzf docs/continuation/worktrees-partial-vfs/ordinary-evidence-20261027-r16-r18.tar.gz
```

Les restrictions de restauration ci-dessous s'appliquent.

## Complément admission et réconciliation — r11 à r13

Le lecteur de provenance schema9, le writer transactionnel d'admission, les
budgets et les noyaux DATA d'observation/réconciliation sont qualifiés dans les
périmètres décrits au [checkpoint](../../plans/worktrees-partial-vfs-checkpoint.md).
L'admission physique durable et l'ouverture ordinaire modifiable restent à
raccorder ; le [contrat](../../plans/worktrees-partial-vfs-managed-admission-contract.md)
précise les six raccordements nécessaires.

`admission-evidence-20261027-r11-r13.tar.gz` contient 1 720 fichiers : sources
figées, manifestes, recettes et journaux, y compris les échecs conservés.
Chaque membre a été relu et vérifié contre son empreinte dans
`admission-inventory-20261027-r11-r13.json`.
Archive : 4 757 898 octets ; SHA-256
`8b5476fe031db222eda6a3384e2e53a5c8e911c14e699ca22cd48010be000c9e`.

Sur un poste où ces preuves sont absentes :

```powershell
tar -xzf docs/continuation/worktrees-partial-vfs/admission-evidence-20261027-r11-r13.tar.gz
```

Les restrictions de restauration ci-dessous s'appliquent. Les binaires/PDB,
bases SQLite et fixtures natives restent hors archive. Ces résultats ne valent
pas qualification de l'hôte d'un autre poste.

## Complément du 27 octobre 2026

Les commits locaux `ddf31d3` et `0afbcc0` ajoutent les publications familiale et
source, la capture après Published, les lecteurs Ready autonomes et la
réouverture physique en lecture. Leur état courant est décrit par le
[checkpoint](../../plans/worktrees-partial-vfs-checkpoint.md) et le
[contrat d'admission](../../plans/worktrees-partial-vfs-managed-admission-contract.md).
L'admission ordinaire mutable, les companions, l'expansion et les parcours
produit ne sont pas terminés.

Le complément `managed-evidence-20261027.tar.gz` transporte 12 346 fichiers
de sources figées, manifestes et journaux du 27 octobre, jusqu'à l'artefact r10.
Chaque fichier a été relu depuis l'archive et comparé à son SHA-256.
`managed-inventory-20261027.json` donne les chemins, tailles et empreintes.
Archive : 33 556 498 octets ; SHA-256
`8da87e87a87f379f03b7c7f7140495c22b30a9b48eb465c10bb92be35697c3e5`.

Sur un poste où ces preuves sont absentes :

```powershell
tar -xzf docs/continuation/worktrees-partial-vfs/managed-evidence-20261027.tar.gz
```

Les mêmes restrictions de restauration et d'hôte ci-dessous s'appliquent.
Les échecs r4/r5/r6/r9/r10 restent conservés ; leur présence dans l'archive
n'est pas une acceptation. Les sources non modulées présentes dans certains
inventaires sont des brouillons, sans qualification. L'archive ne contient
ni exécutables/PDB, ni bases SQLite, ni fixtures natives. Elle ne remplace
pas `evidence.tar.gz`, qui conserve le jalon historique writer R7.

## Paquet historique writer R7

Le commit `0eba2e5` contient les sources, tests et scripts du programme. Son
premier push omettait les contrats et dossiers de revue conservés dans le
répertoire ignoré `.layrs-local/validation`. Ce complément transporte ces
documents et leurs copies de sources, sans changer le code du produit.

## Restaurer les documents locaux manquants

Depuis la racine du dépôt, sur la nouvelle machine où ces dossiers sont absents :

```powershell
tar -xzf docs/continuation/worktrees-partial-vfs/evidence.tar.gz
```

L'archive contient des fichiers ordinaires sous `.layrs-local/validation/`, avec
leurs octets d'origine. Elle ne contient aucun lien symbolique ou chemin absolu.
Ne pas extraire par-dessus un dossier de preuves déjà présent sans comparer les
empreintes : les preuves historiques doivent rester immuables.

`inventory.json` donne les chemins relatifs, tailles et SHA-256 des fichiers
transportés. Les documents historiques conservent leurs chemins absolus d'origine
et leurs empreintes. Pour lire un chemin commençant par
`C:/Users/ALEXA/.t3/worktrees/Layrs/feature-worktrees-partial-vfs/`, utiliser le
même suffixe depuis la racine du nouveau checkout. Ne pas réécrire les manifestes
historiques. Les chemins externes au dépôt désignent l'ancien environnement.

## Ordre de lecture

Le supplément `admitted-context-evidence-20261027-r14-r15.tar.gz` contient les
sources figées et résultats des lecteurs admis, du contrôle d'aliasing des
répertoires temporaires et de l'admission physique avec reprise après perte
d'accusé. Il conserve aussi le premier échec de fixture natif r14. Inventaire :
`admitted-context-inventory-20261027-r14-r15.json`, 1 167 fichiers vérifiés.
Archive : 3 214 056 octets ; SHA-256
`c6dbc24e33988cd69c977ee7fd079303385c4c322d7197a1de4599aa4f54181d`.
Les exécutables, PDB, bases et fixtures natives restent exclus. Ce supplément ne
qualifie pas l'ouverture ordinaire modifiable ni le programme complet.

Depuis la racine du dépôt, extraction des preuves historiques :

```powershell
tar -xzf docs/continuation/worktrees-partial-vfs/admitted-context-evidence-20261027-r14-r15.tar.gz
```

1. [Point d'étape](../../plans/worktrees-partial-vfs-checkpoint.md), puis
   [programme](../../plans/worktrees-partial-vfs.md),
   [storage](../../plans/worktrees-partial-vfs-storage.md) et
   [convergence](../../plans/worktrees-partial-vfs-convergence.md).
2. Après extraction :
   `.layrs-local/validation/l5-store-stage1b-consolidated-contract-r4.md`.
   C'est le contrat durable à préserver. Ses formulations de proposition
   décrivent sa date d'écriture ; les décisions ultérieures sont distinctes.
3. Les dossiers `l5-store-stage1b-writer-design-r1-*` et
   `l5-store-stage1b-writer-design-r2-*` : proposition, addendum, remplacement R2,
   plans et décisions de revue.
4. Le dossier `l5-store-stage1b-writer-r7-bdbc821b61a64108b8355ee9771f6df1`,
   notamment `freeze.md`, `review-manifest.json`, puis
   `owned-runtime-dea358a7a7a049b588aa5d23291eeb11/review-disposition-r1.json`.
5. [Proposition de suite exportée](../../plans/worktrees-partial-vfs-next-slice-proposal.md).
   Elle nécessite encore sa revue et son autorisation de mise en œuvre.

## État et limites du transfert

Les 156 sélections natives writer R7 ont une acceptation bornée jusqu'à
`control_published`, membre de famille toujours `initializing`. Ce n'est pas
l'acceptation du programme complet ni l'activation CLI/Desktop. Les échecs
historiques restent des échecs ; les résultats purs R5 gardent leur provenance.
La course de namespace et l'optimisation restent HOLD.

Le paquet est destiné à poursuivre la conception, les revues de source et
l'implémentation. Il exclut les exécutables/PDB, sorties de compilation, caches,
environnements locaux, bases et fixtures natives. Il ne permet donc pas de
revalider toutes les empreintes binaires des anciens manifestes ni de transférer
l'acceptation de l'ancien hôte au nouvel ordinateur. Les sources et résultats
transportés restent des preuves historiques, pas un nouveau test.

Ne pas lancer automatiquement les anciennes commandes de qualification : leurs
chemins, binaires, identités natives et autorisations appartiennent à leurs runs.
Une nouvelle qualification demande un build et une disposition adaptés au nouvel
hôte, des parents frais et la file explicitement approuvée. Aucun nettoyage,
réemploi des fixtures, retry, élargissement de droits ou activation produit ne
découle de ce transfert.
