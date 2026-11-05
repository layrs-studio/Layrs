# Passation de nuit — Worktrees / partial VFS — 27 octobre 2026

Arrêt demandé explicitement par l'utilisateur. Le chantier reste incomplet et
doit rester en pause jusqu'à sa demande de reprise. Ne pas interpréter ce
checkpoint comme une validation produit ou une qualification du nouvel installateur.

## Où reprendre

Branche : `feature/worktrees-partial-vfs`, dépôt local `D:\Layrs\layrs`.
Dernier commit qualifié avant le travail en cours : `8ed6b3d`
(`feat: stream exact managed image chunks`). Le checkpoint qui contient ce
document conserve les développements suivants comme **WIP**. Aucun push demandé
ni effectué pour cet arrêt. Le stash de sauvegarde initial reste conservé.

Le détail des preuves précédentes est dans le
[checkpoint managed](worktrees-partial-vfs-managed-source-checkpoint.md).
Ne pas relancer les anciennes fixtures : conserver aussi les échecs et leurs logs.

| Étape | État à l'arrêt |
| --- | --- |
| Socle, activation, lecture, capture, export | Tranches ciblées qualifiées ; pas une validation complète du produit |
| Destination, copie de l'historique, journaux Staging | Qualifiés dans les scénarios consignés |
| Catalogue multi-représentations et migration physique v2 | Qualifiés ; refus des catalogues familiaux à journal chaud conservé |
| Réservation/reprise sous propriétaires retenus | Qualifiées, y compris arrêt après vrai BUSY de fermeture source |
| Lecture exacte par morceaux de l'image courante | Qualifiée ; hydratation CFAPI non nulle d'un placeholder partiel encore à qualifier |
| Installation exacte des contenus | Code intégré, compilation passée, deux constats de revue corrigés et revérifiés ; tests nouveaux NON exécutés |
| Finalisation SQL DestinationReady | Brouillon conservé hors arbre de modules ; NON compilé, NON testé |
| Protection, copie physique du checkout, Ready/Published | À implémenter/intégrer et qualifier |
| Companions, préservation hors périmètre, expansion de sélection | À terminer |
| CLI/Desktop | Seule la commande provider status est qualifiée ; parcours restants à raccorder |
| Qualification finale de bout en bout | À faire |

## Installateur exact : fichiers et contrat

- `shared/exact_content.rs` et sous-modules `journal`, `sql`, `spool`, `publish` :
  installation identifiée par ManifestId, état durable Declared → Allocated →
  Verified → Publishing → Published, inode actif unique, refus d'adopter un
  objet orphelin ou de réécrire une pièce partielle, aucune suppression automatique.
- `activation/managed_content/exact.rs` : adaptateur fermé du
  `ManagedImportLease`. Les lectures de l'image et écritures de contenu se font
  après les fermetures SQL positives, hors des verrous du catalogue.
- `shared/provider_import/content.rs` : nouveau validateur typé d'installation ;
  le validateur de réservation seule reste strict et distinct.
- `managed_plan.rs` : enveloppe augmentée pour les journaux d'installation et
  leurs manifests. Pas d'augmentation automatique d'une réservation durable.
- `shared.rs::usage` : comptabilise aussi 8192 octets par journal exact et les
  octets de son manifest, afin que les autres réservations ne surallouent pas.
- `secure_fs::open_file_for_sync` : ouvre un fichier existant sans création ni
  troncature pour FlushFileBuffers ; l'appelant doit vérifier inode et hash.

Published ne signifie pas Worktree prêt. Les journaux, pièces de staging et la
réservation restent retenus ; protection et finalisation viennent ensuite.
Les représentations historiques absentes du stockage et de l'image courante
restent explicitement indisponibles : aucune autorité distante n'est inventée.

Deux constats de revue indépendante ont été corrigés et revérifiés :

1. L'annulation du sink renvoyait Interrupted, que write_all réessaie sans fin.
   Elle renvoie maintenant Other avec un indicateur interne ensuite traduit en
   Vfs::Cancelled. Le test correspondant existe, mais n'a pas encore été exécuté.
2. La réouverture du lease utilisait le lecteur de réservation seule et refusait
   toute progression d'installation. `open` utilise maintenant `read_admission` :
   présence de journaux exacts → validateur d'installation ; absence → validateur
   strict de réservation. `reserve_plan` conserve son contrat initial strict.

La revue indépendante ne rapporte pas d'autre P1/P2 confirmé dans son périmètre
inspecté : fermetures positives avant effets, inode actif/reprise/collisions,
débits atomiques, chunks répétés, représentations exactes, bornes et quota.
Ce constat est une revue de sources, pas une preuve d'exécution ou de crash.
Les tests source du moteur (codec/sink et deux recettes SQL ignorées) existent
mais n'ont pas été exécutés. **Aucune fixture native managed_exact_tests n'a
encore été écrite** : la mission s'est arrêtée après lecture et conception.
La fixture multichunks + réouverture Published est à écrire ; une vraie
interruption du producteur et la fixture historique à deux représentations
restent à concevoir. Un refus d'acquittement ne prouve pas cette interruption.

Validation effectuée avant l'arrêt : `cargo check -p layrs-store-local --all-targets`
a passé trois fois, dernière trace `D:\Layrs\tmp\provider-exact-install-check-r3.log`.
Ces checks incluent le moteur et l'adaptateur, mais excluent DestinationReady
qui n'est pas branché. Aucun nouveau test SQL, natif ou exécutable audité de
l'installateur n'a été exécuté ; ne pas réutiliser les résultats du producteur
de morceaux pour prétendre le qualifier.

Contrôle final de gel : `provider-exact-install-night-check.log` dans le même
répertoire de logs, **PASS, exit 0** (`cargo check --all-targets`, avec warnings).
Aucun test natif en cours. Les trois sous-agents ont terminé et figé leurs travaux.

## Brouillon DestinationReady et décisions à conserver

`provider/destination_ready.rs` avec `data.rs`, `family.rs`, `read.rs` est
volontairement non branché. Le code doit être compilé, revu et testé à la reprise.
Il n'accorde que des preuves SQL DATA, aucune preuve de checkout ou de contenu.

- Trois transactions distinctes retenues : source, destination, famille.
- Préfixe immuable exact puis suffixe local observation G+1, placement G+2,
  capture C+1, receipt et claims ; le lecteur Staging reste inchangé.
- Mapping overlay explicite par chemin/ManifestId ; ne pas deviner une
  représentation à partir du seul FileId et ne pas écraser le mapping historique.
- Claim durable `kind=provider-import`, `owner=transition_id`, opération
  `provider-import:<transition_id>`, couvrant toute l'union exacte. Une simple
  claim generation pourrait être supprimée par release_retired_generations.
- Les bytes du marqueur familial sont des DATA explicitement vérifiées contre
  le digest d'allocation, pas une capacité de fichiers.
- Deux helpers ROOT existent dans `import_validation.rs` :
  `require_copied_prefix_scope` exige la source originale et une destination
  distincte, et valide le préfixe sans interdire un suffixe Ready ;
  `copied_content_mappings` réutilise le scope exact avec companions pour exclure
  les mappings étrangers. Le simple champ content_file_ids serait incomplet.

Point ouvert explicite du brouillon : le lecteur familial exige encore un
membre destination `initializing`, même pour le rejeu privé Ready. Il faut
définir et tester le passage familial Ready dans le futur driver. Toute erreur
du finalizer après mutation exige le rollback de la transaction par l'appelant.
Les quatre fichiers ont passé rustfmt, mais n'ont pas de tests mémoire écrits.

## Reprise dans l'ordre

1. Lire cette passation, vérifier Git et les derniers constats de revue ci-dessous.
   Employer GPT-6.1-Sol pour les agents Sol ; le skill orchestrate-change et les
   profils ont déjà été corrigés. ROOT seul lance Cargo et les recettes natives.
2. Finir la revue de l'installateur, intégrer les fixtures et qualifier notamment
   annulation, débit des chunks répétés, deux représentations d'un même FileId,
   réouverture après Published/pièce complète et collisions/refus sans mutation.
3. Pour chaque recette SQL/native : revue indépendante des sources, build puis
   pins EXE/PDB CodeView/sources/startup/native, parent C: neuf et possédé, bornes
   enfant/parent, exécution sérielle et arrêt au premier échec. Ne pas nettoyer,
   rouvrir ni réutiliser une fixture échouée. Aucun kill global par nom de processus.
4. Brancher et corriger DestinationReady, écrire les tests mémoire manquants.
5. Implémenter la protection familiale exacte et la copie physique vérifiée du
   checkout. Ordre durable attendu : protection famille → Ready destination →
   fermeture source positive → relecture privée → membre familial Ready en dernier
   → source Ready → Published. Aucun accès ordinaire avant preuve autonome complète.
6. Finir l'admission ordinaire/provenance sans dépendance à l'ancien provider,
   puis companions/expansion, CLI/Desktop et qualification finale. Les barrières
   de GC et de hot-journal restent strictes tant que leurs remplacements ne sont
   pas implémentés et qualifiés.

Limites transversales : aucune garantie nouvelle de coupure de courant,
redémarrage après quarantaine BUSY, annulation forcée d'hydratation synchrone ou
publication de bout en bout. Les preuves locales ignorées sous `.layrs-local`
et les logs sous `D:\Layrs\tmp` ne sont pas transportés par un simple push Git.
