# Qualification HTTPS, runner et restauration — 20 septembre 2026

Cette mesure complète les précédentes recettes ; elle ne remplace notamment pas celle de 263,44 s. Exécution locale Windows, profil Rust debug, PostgreSQL dans un conteneur dédié et bases UUID isolées. Aucun changement de production pendant cette recette.

## HTTPS complet

Commande : `cargo test --offline -p layrs-server --test native_clients_v3 -- --ignored --nocapture`.

Variables actives : `LAYRS_TEST_DATABASE_URL` fourni par fichier privé, `LAYRS_TEST_PG_CONTAINER=layrs-v3-service-20260920`, configuration Docker locale et pipe Docker Desktop Linux ; `LAYRS_TEST_RUNNER_IMAGE=sha256:4e4c18813f7f9661cc9db25dd20e907190cc8c8a8677d29cd20b3114fab1092f`. Aucun secret ni URL de connexion n'est recopié dans ce rapport.

**Résultat : 1 test PASS, 0 échec, 0 ignoré, 424,23 s.** La compilation préalable a pris 22,73 s et n'entre pas dans cette durée.

| Mesure | Résultat |
| --- | ---: |
| Fichier interrompu/repris, contenu final vérifié | 16 Mio / 12 882 ms |
| Clones simultanés, 2 clients | 2 856 ms |
| Clones simultanés, 10 clients | 4 473 ms |
| Publication simultanée, 2 écrivains | 1 267 ms |
| Publication simultanée, 10 écrivains | 2 655 ms |
| Objets de l'inventaire restauré | 708 |
| Octets canoniques | 16 760 647 |
| Steps restaurés | 52 |
| Événements Release restaurés | 6 |
| Sauvegarde | 14 074 ms |
| Restauration | 15 133 ms |
| Drill backup/restauration complet | 33 605 ms |
| Fenêtre de copie CAS | 13 s |

La concurrence à dix écrivains produit une avance acceptée et neuf divergences conservées, ensuite intégrées explicitement. Le test vérifie les octets récupérés et le rejeu d'une réponse perdue, pas seulement les codes HTTP.

Parcours inclus : fusion et conflits de renommage entre clients ; revue riche refusant une résolution partielle puis intégrant la version finale réapprouvée ; Choose avec aperçu persistant, attentes périmées refusées et histoire conservée ; squash publié via admission candidate, autorité rafraîchie après changement d'epoch, perte de réponse puis retry exact ; annulation clone/receive et publication avec frontière de commit ; refus dirty/stale sans capture implicite ; vrai runner Docker sur Step historique et Gate qui ne couvre pas le Step suivant ; publications/révocations Release protégées.

Le backup compare aussi les observations de réception serveur et leur lecture reconstruite. Cette fixture contient **zéro annulation de run et zéro correction de commentaire** ; leurs recettes dédiées relèvent de `native_sync_v3` ci-dessous.

## Limites

- Mesures locales de qualification, sans engagement de débit ou de latence pour une PME en production.
- Ce PASS précède le nouveau correctif de pagination fetch/receive ; il ne le qualifie pas.
- Annulation coopérative bornée : un commit commencé n'est pas annulé rétroactivement ; une publication dont la réponse manque conserve son journal pour le retry.
- Aucune qualification supplémentaire du navigateur Web, aucun pilote humain en entreprise et aucune simulation de panne physique dans cette exécution.

## Suites PostgreSQL complémentaires

Les six suites demandées sont **PASS**, chacune avec un test effectivement exécuté, zéro échec et zéro ignoré :

| Suite | Durée |
| --- | ---: |
| `native_identity` | 5,67 s |
| `native_invitations` | 7,32 s |
| `native_migrations` | 6,82 s |
| `native_retention` | 2,88 s |
| `native_storage_budget` | 2,80 s |
| `native_sync_v3`, backup/restauration activés | 60,74 s |

`native_sync_v3` confirme les observations authentifiées de première admission, filtres inclusifs, curseur ancré pendant de nouveaux ajouts, rejeu, refus des observations locales forgées et détection des lignes corrompues/manquantes ; droits Teams, révocation par epoch, CAS concurrent, journal immuable et bornes ; Choose administratif, histoire conservée, gates et rejeu. Son backup contient **473 objets, 13 Steps, 9 annulations de runs et 3 corrections de commentaires**, 762 747 octets canoniques : sauvegarde 5 531 ms, restauration 6 754 ms, drill complet 16 559 ms et fenêtre de copie CAS 5 s. Il complète les objets et parcours différents de la fixture HTTPS précédente.

La suite de rétention est un test d'observation sans suppression : elle prouve notamment que les fences SQL ne suffisent pas à exclure la préparation CAS. Elle ne qualifie pas à elle seule le nouveau GC exclusif Linux, testé séparément par son propriétaire.

Les tests de migration exigent en plus `LAYRS_TEST_DATABASE_ISOLATED=1`. Une première invocation sans ce marqueur a été refusée immédiatement avant toute opération de migration ; les quatre suites restantes ont ensuite été exécutées avec le marqueur, sans modification du code. Les deux premières suites déjà PASS n'ont pas été rejouées inutilement.
