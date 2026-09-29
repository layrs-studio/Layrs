# T1 — Retrouver, restaurer et annuler son travail

Suivi d'exécution : [journal T1](journal/T1.md). Coordination et reprise : [REPRISE.md](REPRISE.md).

Statut au 20 septembre 2026 : parcours T1 livré et qualifié sur Windows et Linux x86_64 dans les limites ci-dessous. Priorité : P0. Les cases reflètent les preuves du journal, pas la seule présence de code. macOS ne fait pas partie de cette qualification.

Parcours natif CLI/Desktop exécuté, 18 tests CLI de sécurité réussis, reprise avec fautes injectées et symlinks Linux vérifiés. Revert partagé reçu par le second client dans la recette HTTPS complète réussie en 64 s. Windows refuse la matérialisation des symlinks et du mode exécutable avant mutation ; macOS n'a pas été exécuté. Les aperçus de collisions et la concurrence entre Worktrees sont qualifiés. La gestion graphique de plusieurs Worktrees demeure reportée ; une restauration affectant un autre Worktree est refusée.

## Résultat attendu

Un utilisateur peut restaurer un fichier, revenir à un état du projet, annuler puis rétablir une opération et inverser un changement partagé sans perdre les modifications présentes. CLI et Desktop utilisent le même moteur. Les commandes effectivement disponibles et leurs limites sont décrites dans le journal et la documentation utilisateur ; les cases ouvertes restent des objectifs.

Ce chantier porte aussi le raccordement du client au stockage SCM V1 : c'est le prérequis local commun aux huit tâches, pas une neuvième tâche.

## Point de départ historique au 19 septembre et références

- Les Layers, Steps, scans et changements de Layer existent dans `crates/layrs-client-core/src/access_registry/`.
- Le client utilise encore `local_store.rs`, sa projection `client_documents` et l'ancien `object_store.rs` ; le nouveau stockage est dans `crates/layrs-store-local/src/scm_v1/`.
- Les types View/Operation/Worktree et le graphe existent. Leur présence ne constitue pas une commande undo utilisable.
- L'audit a reproduit un échec Windows de création du stockage via l'ancien chemin. Le diagnostiquer sans présumer de la cause et sans pérenniser ce chemin.
- Autorités : [architecture SCM](../../architecture/scm-v1-target.md), [ADR 0007](../../decisions/0007-repository-views-operations-layer-heads.md), [ADR 0008](../../decisions/0008-worktrees-markers-store-baseline.md).

## Périmètre

Inclus : init/capture/switch sur le stockage natif, matérialisation sûre, capture automatique, historique des opérations, restauration ciblée/globale, undo/redo et revert partagé.

Reporté : interface de gestion de plusieurs Worktrees. Le modèle et les verrous doivent néanmoins rester compatibles avec plusieurs Worktrees. Aucun import automatique des anciens stores de développement ; les refuser explicitement sans les modifier.

## Plan d'exécution

### T1.C — Stabiliser le contrat local commun

- [x] Cartographier chaque entrée CLI/Tauri qui ouvre ou écrit l'ancien store ; identifier les tests métier à conserver et les tests propres à l'ancien format à remplacer.
- [x] Définir une façade typée de préparation puis acceptation : lectures immuables, installation CAS, attentes sur les têtes d'Operations, transaction courte et résultat observable.
- [x] Définir le service de matérialisation partagé : plan, sauvegarde exacte, lease, journal, vérification, finalisation et reprise après panne.
- [x] Distinguer `restore-file` depuis un Step, `restore-view` depuis une View/Operation et `revert` d'un changement. Définir les erreurs d'ambiguïté et de concurrence.
- [x] Fixer les résultats structurés CLI/SDK, les identifiants, les opérations sans effet et les règles sur les fichiers non suivis.
- [x] Publier les signatures et fixtures communes avant les implémentations dépendantes. Toute extension des schémas canoniques conserve un seul propriétaire et de nouveaux tests de contrat.

**Sortie T1.C :** interfaces et invariants utilisables par T2, T3, T4 et T7, sans dépendance au format `client_documents`.

### T1.S — Livrer init → capture → switch sur le stockage natif

- [x] Faire de `layrs-store-local` l'unique écrivain CAS/SQLite ; retirer le SQL direct et les écritures de l'ancien CAS du parcours client actif.
- [x] Initialiser Store/Space/Layer/Worktree/View/Operation et valider leurs appartenances.
- [x] Scanner avec vérification avant/après lecture et tentatives bornées si un fichier change ; ne jamais accepter une lecture instable comme capture réussie.
- [x] Préserver octets, suppression versus fichier vide, symlinks et bit exécutable ; refuser clairement les chemins non matérialisables sur la plateforme.
- [x] Exécuter hachage et préparation hors verrou exclusif long ; accepter par comparaison des têtes attendues et conserver toute divergence.
- [x] Capter avant un remplacement de fichiers, même si la capture périodique est désactivée. Refuser d'écraser un fichier non suivi sans le protéger.
- [x] Brancher la capture après période de calme et lors des commandes concernées ; coalescer les événements, éviter les captures vides et documenter quand le service est actif.
- [x] Raccorder init, status, step, diff, timeline et switch de la CLI et du Desktop au nouveau moteur.
- [x] Reproduire le problème Windows avec un petit test de stockage, vérifier le chemin natif et traiter la cause si elle y existe aussi.

**Sortie T1.S :** un projet réel s'initialise, se capture et change de Layer sans utiliser l'ancien format ; un redémarrage conserve les données.

### T1.R — Restaurer, annuler et rétablir

- [x] Offrir une prévisualisation des fichiers ajoutés, remplacés, supprimés et des collisions avant application.
- [x] Restaurer un fichier ou sous-arbre depuis un Step en créant un nouveau Step descendant ; capturer d'abord le travail courant.
- [x] Restaurer une View historique avec une nouvelle Operation ; garder les anciennes Operations et les définitions locales des chemins Worktree.
- [x] Implémenter undo/redo par nouvelles Operations, jamais en supprimant l'histoire. En cas de branches d'Operations, exiger une cible explicite.
- [x] Définir précisément quels Worktrees sont affectés ; obtenir leurs leases ou refuser sans effet partiel. Ne jamais restaurer une View qui écrase silencieusement leur travail concurrent.
- [x] Exposer les commandes et une UI avec diff préalable, cible, portée et résultat ; conserver les mêmes erreurs métier sur les deux surfaces.

**Sortie T1.R :** restauration et undo/redo locaux vérifiés, sans dépendance au serveur.

### T1.V — Inverser un changement déjà partagé

- [x] Calculer une modification inverse appliquée à l'état courant, avec parent/base explicites pour un Step de fusion.
- [x] Réutiliser le moteur T2.M ; conserver les conflits comme objets du nouveau Step.
- [x] Publier ce nouveau descendant via T4. L'undo local ne doit jamais être traduit implicitement en suppression de têtes distantes.

## Validation et critères de fin

- [x] Matrice Windows/Linux : fichier modifié/supprimé/binaire/vide et Unicode (`local_data_safety`,18 tests), fichier↔dossier (`native_materialize_types`,5), liens Linux et interruptions (`native_materialize_symlinks`,4), bit exécutable (`native_capture`/`native_workspace`), collisions de casse du pont Git sur les deux OS. Sur Windows, les types non matérialisables sont refusés avant mutation. Dernier lot Linux restauration/reprise/concurrence : 36 PASS ; macOS reste non qualifié.
- [x] Pannes injectées avant/après sauvegarde, installation CAS, transaction et finalisation ; redémarrage donnant les anciens octets, les nouveaux octets vérifiés ou `needs_repair`, jamais une réussite mensongère.
- [x] Deux écrivains sur un Worktree et deux Worktrees partageant le Store : aucune tête ni modification perdue. Deux tests réellement simultanés passent ; le Worktree resté sur une ancienne Operation exige une intégration explicite, sans écraser ses fichiers. La création interactive de Worktrees supplémentaires reste hors de ce lot.
- [x] Undo puis nouveau travail puis redo ciblé (`native_workspace`) ; restauration d'une View multi-head et undo retrouvant les octets sales (`native_concurrency`, trois tests PASS) ; chunk historique absent/corrompu refusé avant capture ou mutation (`native_restore_preview`, PASS).
- [x] Scénarios CLI en boîte noire et parcours Desktop natif pour restauration et switch. Adapter `tests/local_data_safety.rs` au nouveau format.
- [x] Revert partagé reçu par un second client sans disparition de l'historique original, après T4.P.
- [x] Mettre à jour documentation et matrice de tests ; conserver les dossiers de reproduction lors d'un échec de durabilité.

## Dépendances et parallélisme

| Phase | Prérequis bloquant | Travail possible en parallèle |
|---|---|---|
| T1.C | Aucun | T2.C, T3.C, T4.C, T8.C |
| T1.S | T1.C | Moteur pur T2.M, contrats T3/T7, serveur T4.E, identité T8.I |
| T1.R | T1.S | Historique T3.H, client réseau T4.L, pont Git T7 |
| T1.V local | T1.R + T2.M | Revue T5 et CI T6 |
| T1.V partagé | T1.V local + T4.P | Packaging et exercices T8 |

Propriétaire des écritures communes : T1 pour la façade locale, le matérialiseur et leur adaptation au store. T2/T3/T4/T7 consomment ces APIs. Les modifications de `args.rs`, `engine.rs`, du registre Tauri et des écrans racines sont intégrées séquentiellement par un responsable désigné.

## Ordonnancement global des huit fichiers

Cette table est la référence de coordination ; chaque fichier précise ses sous-jalons. Une implémentation isolée peut commencer avec des doubles de test, mais son intégration n'est acquise qu'avec les dépendances réelles.

| Vague | Travaux réellement indépendants | Condition de passage |
|---|---|---|
| A — contrats | T1.C local, T2.C fusion, T3.C historique, T4.C sync, T8.C identité/déploiement | Contrats partagés stabilisés et fixtures validées ; une seule écriture à la fois par schéma |
| B — fondations | T1.S client natif, T2.M moteur pur, T4.E transactions serveur, T8.I identité | Tests propres à chaque composant ; pas de façade indisponible dans le parcours intégré |
| C — usages | T1.R, T3.H, T4.L et T8.H ; T5.C/T6.C/T7.C selon leurs contrats | T4.P prouve deux clients réels en HTTPS ; intégrations locales sur T1.S |
| D — collaboration | T2.U, T3.R, T5.R, T6.P/T6.X, T7.I/T7.E ; T1.V selon T2.M | Fonctionnalités reliées aux services réels ; approbations et preuves liées aux versions exactes |
| E — livraison | T5.G, exercices T8.B, performances et recettes transverses | Restauration après sinistre, tests multiplateformes et pilote validés |

Avec trois exécutants disponibles, une première répartition utile après les contrats est : (1) T1.S, (2) T2.M, (3) T4.E ; T8.I peut prendre le relais du premier créneau libre. Avec quatre, T8.I avance en parallèle. Ce sont des possibilités futures, pas une demande de lancer des agents pendant la rédaction des plans.

Règles communes : branches/checkouts isolés si plusieurs exécutants ; un propriétaire par migration, schéma, lockfile et point de composition ; contrat changé = prévenir ses consommateurs avant intégration. Ne pas considérer deux tâches comme indépendantes simplement parce que leurs titres diffèrent.

## Charge indicative

7–11 semaines-personnes, fondation locale incluse. Cette répartition remplace une partie des lots transverses de l'audit : ne pas additionner les deux estimations. Revoir la charge après T1.S ; les garanties de matérialisation sont le principal risque.

Les huit plans totalisent 44–72 semaines-personnes avant marge. Une réserve indicative de 20 % donne environ 53–86 semaines-personnes, proche de l'enveloppe initiale de 50–85. Ce sont des charges, pas des durées calendaires divisibles mécaniquement par le nombre d'agents : contrats partagés, intégration et validation terrain restent contraignants.
