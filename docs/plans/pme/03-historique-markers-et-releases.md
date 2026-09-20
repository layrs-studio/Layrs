# T3 — Donner du sens à l'historique

Suivi d'exécution : [journal T3](journal/T3.md). Coordination et reprise : [REPRISE.md](REPRISE.md).

Statut : en cours — 24/27 cases vérifiées au 20 septembre 2026 ; les cases composées encore partielles restent ouvertes. Priorité : P1. Référence : audit du 19 septembre 2026, actualisé par les preuves du journal T3.

## Résultat attendu

Un utilisateur retrouve une modification, nomme un point important, publie une version et reprend ou regroupe certains changements sans altérer les versions déjà identifiées. Les Steps restent des captures anonymes ; le produit ne demande pas de rédiger un commit pour chaque sauvegarde.

## Point de départ et références

- Timeline et diff par Step présents ; types Marker, Release et ReleasePublication dans `crates/layrs-core/src/scm/repository.rs`.
- `crates/layrs-graph/src/scm/` fournit parcours, bases et normalisation des têtes, pas encore le parcours complet de sélection/réécriture.
- Des tables et admissions de ces objets existent dans le nouveau store ; vérifier leur couverture avant de les étendre.
- Autorités : [ADR 0006](../../decisions/0006-step-snapshot-identity.md), [ADR 0007](../../decisions/0007-repository-views-operations-layer-heads.md), [ADR 0008](../../decisions/0008-worktrees-markers-store-baseline.md).

## Périmètre

Inclus : points nommés, versions, recherche et historique par fichier, comparaison entre états, sélection explicite, replay de changements et regroupement simple.

Reporté : langage de requêtes généraliste, réécriture interactive arbitraire, blame/bisect et gestion avancée de multiples Worktrees. Ne pas introduire un index de staging Git comme préalable à la capture automatique.

## Plan d'exécution

### T3.C — Stabiliser références et sélecteurs

- [x] **T3.C1** Définir une sélection non ambiguë par Step, Marker, Release ou Layer ; une Layer multi-head exige un choix explicite pour une opération mono-cible.
- [x] **T3.C2** Définir les formes CLI/SDK proposées, la pagination et l'ordre déterministe. Les noms d'affichage ne deviennent jamais des identités.
- [x] **T3.C3** Politique explicite [ADR0018](../../decisions/0018-immutable-reference-names-and-corrections.md) : noms exacts, Markers ambigus exigent ID, remplacement par nouvel objet + masquage explicite ; correction Release avec nouvelle version, révocation indépendante, aucune suppression en place. Deux tests natifs PASS1,14s.
- [x] **T3.C4** Définir les métadonnées affichables : titre, notes, acteurs et observations ; ne pas les incorporer artificiellement à l'identité du Step.
- [x] **T3.C5** Définir le contrat de rewrite map many-to-many et la règle de déplacement des Layers ; les Markers/Releases existants ne bougent jamais implicitement.
- [x] **T3.C6** Fournir fixtures et résolution de sélecteurs réutilisables par T1, T2, T6 et T7. Une première version accepte les IDs exacts sans attendre la recherche complète.

**Sortie T3.C :** contrat des références, sélecteurs et transformations, indépendant des écrans.

### T3.H — Livrer l'historique et les points nommés

- [x] **T3.H1** Relier création/masquage de Marker aux Views et Operations de T1.S ; conserver les cibles comme racines de rétention.
- [x] **T3.H2** Créer un manifeste Release immuable avec version, notes et preuves ; traiter publication et révocation comme événements séparés.
- [x] **T3.H3** Construire les lectures du DAG : pagination stable, parents, filtres par Layer/chemin/acteur/période et comparaison de deux états.
- [x] **T3.H4** Définir l'historique par fichier sans promettre une identité à travers tous les renommages. Afficher une continuité uniquement quand elle est démontrée, et signaler l'ambiguïté.
- [x] **T3.H5** Ajouter des projections de recherche reconstructibles ; le cache ne devient pas l'autorité des références.
- [x] **T3.H6** Captures automatiques regroupées visuellement, deux IDs distincts conservés dans le groupe déplié et Marker initial visible : Desktop PASS25,7s. Markers/Releases restent des points nommés distincts ; parcours de publication de Release Desktop PASS28,6s.
- [ ] **T3.H7** Exposer CLI/Desktop puis Web via T4, avec contrôle d'accès sur les métadonnées et les contenus.

**Sortie T3.H :** un point peut être nommé, retrouvé et utilisé pour consulter/restaurer exactement son état local. La publication distante attend T4.P.

### T3.R — Reprendre et regrouper des changements

- [x] **T3.R1** Implémenter un planner pur de replay d'un changement sur une autre base avec parent explicite pour les Steps de fusion.
- [x] **T3.R2** Implémenter un regroupement d'une sélection simple ; définir les contraintes de contiguïté et refuser les graphes non supportés avec explication.
- [x] **T3.R3** Réutiliser T2.M pour appliquer les deltas et conserver les conflits comme objets ; ne pas copier une ancienne timeline pour simuler une réécriture.
- [x] **T3.R4** Retourner nouveaux Steps, rewrite map et proposition de View ; accepter via T1.S avec attentes de concurrence et une nouvelle Operation.
- [x] **T3.R5** Afficher les Layers qui se déplacent et les Markers/Releases qui restent attachés à leurs anciens Steps. Aperçu CLI/Desktop avec têtes avant/après, identifiants conservés et correspondances ; recette Desktop et native_rewrite passent.
- [x] **T3.R6** Pour des changements publiés, conserver les objets historiques et passer par les modes de mise à jour autorisés T4 ; aucun effacement distant implicite. Recette HTTPS : squash réel, Advance refusé, admission candidate sans déplacement avec rejeu du reçu perdu, Choose administratif, membre refusé et anciennes cibles/Markers/Releases récupérés par un autre client.
- [x] **T3.R7** Rendre la transformation annulable via T1.R. Un changement de sélection ou d'état invalide le plan avant application.

## Validation et critères de fin

- [x] **T3.V1** Sélecteurs exacts, noms ambigus, Layer sans tête/multi-head, identifiant introuvable et cible inaccessible.
- [x] **T3.V2** Marker masqué puis restauré, Release corrigée par une nouvelle version/ID et ancien manifeste inchangé après réouverture : `native_references` PASS1,14s. Publication/révocation sans mutation du manifeste déjà vérifiées dans la recette TLS T3.H2.
- [x] **T3.V3** Cibles toujours accessibles après compaction physique SQLite Windows/Linux : toutes lignes/Operations/Views et objets CAS retenus, Marker/Release historiques lisibles après réouverture et fsck propre (`scm_compaction`, `native_compaction`). GC serveur hors ligne Linux respecte les mêmes racines (`native_gc`, PostgreSQL et backup restauré, PASS2,65s). [Portée et reprise](../../../deploy/selfhost/LOCAL-COMPACTION.md) ; aucun pruning d'historique ni GC local.
- [x] **T3.V4** Historique paginé sans doublons/pertes pendant captures concurrentes ; filtrage de droits sans fuite de chemin ou de titre. Pagination locale et distante ancrée, snapshot d'observations validé, append concurrents exclus, curseurs Space/Principal/epoch/filtre vérifiés ; accès révoqué et hors Space refusés. Les droits distants actuels portent sur le Space entier, sans vue partielle par chemin. PG+backup PASS55,56s ; CLI45/Desktop56,4s.
- [x] **T3.V5** Replay propre/conflictuel, regroupement et rewrite map ; aucun déplacement implicite des points nommés.
- [x] **T3.V6** CLI et Desktop : nommer une version, travailler ensuite, retrouver la version et restaurer un fichier via T1. Recettes CLI native et Desktop avec marker baseline, aperçu protégé, undo/redo et transformation passent.
- [ ] **T3.V7** Second client : mêmes Markers/Releases et cibles exactes après T4.P ; Web rend le même résultat.

## Preuves associées aux cases cochées

La numérotation suit l’ordre des exigences initiales ; V désigne les critères de validation. Le [journal T3](journal/T3.md) conserve l’audit détaillé de chaque case, les commandes exécutées et les limites. Une case cochée ne clôt pas tout son sous-plan.

| Cases | Preuves disponibles |
|---|---|
| C1, C6, V1 | Sélecteurs purs typés + adaptateur Store ; 13 tests `scm_selectors`, cas absence/noms ambigus/sans tête/multi-head/hors Space. |
| C2, C4, H3, H4, H5 | `native_history` : 6 tests réels, curseurs racine/observations, identité explicite, filtres acteur/période sur la même observation, append antidaté, chemin exact ; 2 tests Store de limites/registre. [ADR0017](../../decisions/0017-local-identity-and-observation-history.md). |
| H1 | Création/masquage par View/Operation, conservation de l’historique ; `native_workspace` et `native_rewrite` vérifient les cibles inchangées. Compaction physique et GC serveur conservateur qualifiés en V3 ; suppression d'histoire hors portée. |
| H2 | Manifeste Release immuable et publication/révocation séparées ; recette TLS/PostgreSQL/Docker positive/négative PASS84,15s, guards exacts et manifeste intact. [ADR0016](../../decisions/0016-release-publication-lifecycle-evidence.md). |
| C5, R1, R2, R3, R4, R7, V5 | Planner pur, mainline de merge explicite, squash contigu, rewrite map et CAS ; 7 `native_rewrite`, conflit→résolution→undo, points nommés conservés et plan périmé refusé. |

Cases restantes : observations et filtres distants/Web réel (H7/V4/V7). Compaction physique et GC conservateur (V3), C3/V2, H6, R5/V6 et publication autorisée des transformations R6 sont vérifiés ; voir les derniers lots du journal.
## Dépendances et parallélisme

| Phase | Prérequis bloquant | Parallèle possible |
|---|---|---|
| T3.C | Accord avec T1.C sur les lectures/Operations | T2.C, T4.C, T8.C |
| T3.H local | T3.C + T1.S | T1.R, T2.U, T4.L |
| T3.H distant | T3.H local + T4.P | T5/T6 |
| T3.R moteur | T3.C + T2.M | Revue T5 et pont Git T7 |
| T3.R complet | Moteur T3.R + T1.R ; T4.P pour publication | Livraison T8 |

Propriétaire : sélecteurs, Markers/Releases, lectures d'historique et règles de réécriture. T2 possède l'application des deltas ; T1 possède l'acceptation et le checkout ; T4 publie les références. Ne pas modifier simultanément les mêmes types canoniques depuis T3 et T7 : le pont consomme les types.

Charge indicative : 4–7 semaines-personnes. Risques : réécriture des graphes non linéaires et historique de renommages. Voir [l'ordonnancement global](01-restauration-et-annulation.md#ordonnancement-global-des-huit-fichiers).
