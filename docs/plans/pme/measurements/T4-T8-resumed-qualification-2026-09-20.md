# Qualification après reprise — HTTPS et PostgreSQL

Mesure locale Windows debug, sources compilées après baseline `0453a88` ; la release candidate `e994526` conserve ces contrats. Cette preuve complète le rapport précédent de 424,23s sans l'écraser. Tests Linux et Desktop qualifiés séparément par les autres recettes.

## Suites PostgreSQL : PASS

Base administrateur jetable fournie depuis le fichier privé, rôle runtime non privilégié et fixtures UUID. `LAYRS_TEST_DATABASE_ISOLATED=1`, backup Docker activé pour `native_sync_v3`. Aucun endpoint ni secret dans ce rapport.

| Suite | Tests exécutés | Durée |
| --- | ---: | ---: |
| native_identity | 1 | 11,15s |
| native_invitations | 1 | 10,18s |
| native_migrations | 1 | 11,53s |
| native_retention | 1 | 7,75s |
| native_storage_budget | 1 | 7,18s |
| native_sync_v3, backup réel | 1 | 164,67s |
| v3_pg_quotas | 1 | 16,78s |

Chaque suite termine avec zéro échec et zéro test ignoré. Sessions `31965` et `99618`, logs locaux sous `tmp/final-postgres-*-current.log`. Commandes : `cargo test --locked --offline -p layrs-server --test native_identity --test native_invitations --test native_migrations --test native_retention --test native_storage_budget --test native_sync_v3 -- --ignored --nocapture --test-threads=1`, puis test `v3_pg_quotas` du package `layrs-store-server` avec les mêmes options de test.

Backup de la fixture Sync : **576 objets, 839561 octets canoniques, 52 Steps, zéro événement Release, 9 annulations de runs et 3 corrections de commentaires**. Sauvegarde18299ms, restauration24820ms, copie CAS18s, drill complet60052ms. Les contrôles de droits/epoch, observations authentifiées et assertions clientes, PolicyRoot, immutabilité et refus de corruptions font partie de cette fixture. Une exécution concurrente des autres validations était active ; ces durées ne constituent pas une mesure comparative de performance.

## HTTPS complète : PASS603,59s

Session `46379`, vrai runner Docker et backup activés. Le binaire compilé et le worker WASM sont copiés dans un dossier `tmp/fulltls-UUID` pour éviter un verrou sur le target Cargo pendant les autres constructions. Le test exact est `two_native_clients_publish_clone_receive_divergence_and_local_operations`, tous les modes `LAYRS_TEST_*_ONLY` sont retirés. Image runner épinglée `sha256:4e4c18813f7f9661cc9db25dd20e907190cc8c8a8677d29cd20b3114fab1092f`, conteneur PostgreSQL de qualification `layrs-v3-service-20260920`.

Résultat final : **1 test exécuté, zéro échec, zéro ignoré, 4 points d'entrée enfants exclus volontairement**, 603,59s. Expiration réelle avec renouvellement de capacité, reprise durable et nouveau processus client, PolicyRoot/ACL de contenu, pagination immutable, réécritures, conflits riches et renommages, annulations et concurrence font partie du parcours. Le vrai runner Docker est activé par l'image ci-dessus ; son Step historique et les refus de Gates sont vérifiés avant le backup. Le test serveur réellement tué avant dispatch/après commit est une recette ciblée distincte, documentée dans le journal T4.

| Mesure HTTPS | Résultat |
| --- | ---: |
| Fichier interrompu/repris et digest exact | 16Mio / 1495ms |
| Clones simultanés, 2 clients | 3718ms |
| Clones simultanés, 10 clients | 4725ms |
| Publications simultanées, 2 écrivains | 1377ms |
| Publications simultanées, 10 écrivains | 2894ms |
| Objets restaurés | 621 |
| Octets canoniques | 17276806 |
| Steps restaurés | 65 |
| Événements Release restaurés | 6 |
| Sauvegarde | 17979ms |
| Restauration | 16698ms |
| Fenêtre de copie CAS | 17s |
| Drill backup/restauration complet | 40788ms |

La fixture HTTPS contient zéro annulation de run et zéro correction de commentaire ; la fixture Sync ci-dessus couvre ces deux cas. Les dix publications conservent une avance et neuf divergences ensuite intégrées explicitement. Aucune production modifiée pendant ces recettes ; aucun nouveau pilote humain, aucune garantie de débit/latence et aucune preuve de transfert1Gio dans ce test. Les preuves gros fichiers et Linux restent séparées.

## Script de qualification

Complément ciblé après cette passe complète : recette du processus serveur réellement tué aux frontières objet, avant transaction et après commit avant reçu, **PASS20,87s**, Clippy strict11,58s. Pour l'objet, un préfixe Identity est constaté sur disque avant la mort pendant le GET Range ; le redémarrage réacquiert un nouveau plan/capacité avec claim/objet/principal/Space/epoch/offset exacts, puis reconstruit tous les octets du fichier512Kio. Aucun changement de production, aucune panne PostgreSQL/physique ou reprise zstd revendiquée. Voir journal T4 et mode `LAYRS_TEST_SERVER_RESTART_ONLY`.

`scripts/test-native-postgres.mjs` sélectionne désormais chaque vraie fixture par son nom exact. Les fonctions enfants du test de reprise ne sont jamais lancées directement ni comptées comme preuve PostgreSQL. Chaque fixture doit produire exactement un PASS ; un mode `LAYRS_TEST_*_ONLY` fait refuser la qualification complète. Syntaxe Node vérifiée. Les exécutions ci-dessus utilisent les commandes Cargo équivalentes ; le script complet n'a pas été relancé en doublon.
