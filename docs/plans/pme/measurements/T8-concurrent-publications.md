# Publications concurrentes natives — 20 septembre 2026

Recette Windows, PostgreSQL réel et serveur HTTPS à CA privée, binaires debug. Chaque client possède un Store, Worktree et identifiant Sync indépendants. Les clients partagent ici un compte autorisé et une petite fixture ; il ne s'agit pas d'un essai de charge de production.

| Clients publiant après la même barrière | Durée du lot Advance | Acceptés | Refusés | Têtes conservées après intégration explicite |
|---|---:|---:|---:|---:|
| 2 | 1 083 ms | 1 | 1 | 2 |
| 10 | 2 280 ms | 1 | 9 | 10 |

Les clients ont tous cloné la même tête initiale avant la barrière. Chacun capture un fichier distinct. Le serveur applique exactement une avance ; chaque client refusé conserve ses octets et son Step. Une réponse de transaction est volontairement perdue dans chaque lot : la reprise utilise le même idempotency key et le même digest. Les intégrations explicites suivantes conservent toutes les têtes ; le test télécharge chaque Step exact, vérifie son contenu et reçoit la divergence sans matérialiser une tête arbitraire ni écraser les fichiers locaux.

La durée comprend les uploads immuables et la reprise après réponse perdue, mais exclut les clones initiaux, les intégrations suivantes et les vérifications finales. Le cas 10 crée un autre Space contenant les mêmes octets initiaux que le cas 2. Ce parcours a révélé le retour CAS `AlreadyPresent` avant consommation du flux d'upload ; la correction vérifie le corps complet avant ce retour, y compris pour un objet déjà présent, sans affaiblir la validation HTTP.

[Mesures brutes](T8-concurrent-publications-2026-09-20.json), [helper](../../../../crates/layrs-server/tests/support/concurrent_publications.rs). Reproduction : définir `LAYRS_TEST_DATABASE_URL` sur une instance PostgreSQL jetable, `LAYRS_TEST_DATABASE_ISOLATED=1` et éventuellement `LAYRS_TEST_PUBLICATION_MEASUREMENT_FILE`, puis `node scripts/test-native-postgres.mjs --offline` si les dépendances sont en cache. La gate impose six suites natives exécutées, dont cette recette TLS ; ne pas utiliser une base de production.

Non mesurés ici : gros fichiers, 99 000 fichiers synchronisés, saturation disque/mémoire, latence entre machines, dix utilisateurs avec politiques différentes ou débit soutenu. La matrice T8.D reste ouverte pour ces dimensions. Aucun RPO/RTO ni débit garanti ne découle de ce rapport.
