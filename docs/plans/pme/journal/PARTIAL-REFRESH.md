# Rafraîchissement du Worktree partiel — checkpoint du 22 septembre 2026

## État actuel

Moteur implémenté : `refresh_projection(expected_generation,target,write_file)`
et `resume_refresh(root)`. Le callback fournit un spool local préalablement
téléchargé ; aucun réseau sous verrou Store. Intention SQLite durable avant
staging, puis journal complet avant mutation. Chaque fichier préparé est borné
(1 GiB par fichier, 4 GiB au total), vérifié contre son FileId et synchronisé.
Handles, identités et parents sont revérifiés avant rename sans remplacement.

Réception des modifications/suppressions, chemins imbriqués, révocation
Materialized→Reserved et réattribution Reserved→Materialized. Les anciens octets
restent dans `.layrs/projection-quarantine`, privée et journalisée. Aucun
effacement de copies déjà lues n'est promis. Les fichiers locaux modifiés ou
supprimés bloquent l'opération avant déplacement ; collisions conservées.
Une collision externe pendant la reprise conserve le pending et bloque
l'ouverture normale ; aucune adoption par simple égalité des octets.

Snapshot, historique du refresh et retrait du pending sont atomiques. Les
captures historiques restent lisibles. Epoch monotone ; les anciennes absences
autorisées sont oubliées si la politique ou la base distante observée change. Le fsck vérifie les
journaux canoniques bornés contre les snapshots exacts avant/après, et compte
les opérations. Installation, namespace, publication et restauration pending
excluent une reprise concurrente.

Corrections : Windows accepte la métadonnée exécutable d'un fichier réservé ;
le fixture partagé utilise un suffixe atomique contre les collisions de noms
entre tests parallèles. Clippy `needless_borrow` corrigé.

## Preuves et validations en cours

Qualification finale ciblée : **Windows 11 PASS, 0 ignoré, 2,03 s ; Linux
11 PASS, 0 ignoré, 3,01 s ; Clippy Windows strict lib/tests PASS, 5,03 s**.
Le test Linux de symlink est réellement exécuté ; Windows a son test distinct
de métadonnée exécutable réservée. Régression supplémentaire : une permission
sur un chemin absent ne survit pas au changement de Step/racine distante,
même avec politique inchangée. Journal Linux :
`tmp/refresh-linux-final-20260922.log`. Sources gelées pour commit parent,
après **Clippy Linux strict lib/tests PASS, 16,42 s**
(`tmp/refresh-linux-clippy-20260922.log`). Recettes intégrées client/serveur
à suivre dans le journal parent.

- Windows première série : 7 tests PASS, 0 ignoré, 1,20 s. Sept frontières de
  panne, révocation/regrant, dirty/suppression locale, collision/inode remplacé,
  spool corrompu, fichier→chemin imbriqué, captures préservées, epoch refusé.
- Clippy strict Store lib/tests PASS (6,54 s), avant ajout du fsck historique.
- Tests supplémentaires ajoutés : corruption historique, dossier→fichier
  refusé sans modifier les enfants, exécutable réservé Windows ; Linux ajoute
  symlink de dossier refusé. Nouvelle exécution après compilation restauration.
- Docker Desktop arrêté a été relancé. Caches Linux seuls incomplets
  (`serde_json`, puis `ctrlc`) ; résolution metadata offline réussie avec
  index/cache Windows montés RO. La recette Linux finale ci-dessus utilise ces
  mounts, reste sans réseau, et supprime le conteneur à sa sortie.

## Limites et reprise

- Checkout propre requis : publier/restaurer les changements avant réception.
- Dossier physique→feuille refusé faute de propriété durable du dossier entier.
  Enfants intacts ; fichier→chemin imbriqué fonctionne et est testé.
- Seuls fichiers réguliers matérialisés. Conflits, symlinks et exécutables
  Windows doivent rester Unavailable côté client.
- Avant Ready, reprise abandonne l'intention sans toucher au checkout ; stages
  privés conservés. Après Ready, reprise exacte depuis journal.
- Windows : `cargo test -p layrs-store-local --lib projection::tests::refresh`.
  Clippy : `cargo clippy -p layrs-store-local --lib --tests -- -D warnings`.
- Linux : image `layrs-linux-tests:validation`, volumes
  `layrs-linux-test-build-20260920` / `layrs-linux-test-cargo-20260920`, source RO,
  `CARGO_TARGET_DIR=/build`, Rust 1.90, cache/index
  `C:/Users/ALEXA/.cargo/registry/{cache,index}` montés RO, `--network none`.
- Client/réseau et recette HTTPS suivis par parent ; pas revendiqués ici.

## Checkpoint historique avant implémentation

2026-09-21, en cours. Propriétaire : moteur Store local ; client réseau raccordé
par root, namespace local par operations. Aucun changement serveur dans ce lot.

API prévue : `refresh_projection(expected_generation, target, write_file)` et
variante avec injection de panne, `resume_refresh(checkout_root)`. Les octets
réseau sont téléchargés et vérifiés avant l'appel ; le callback lit un spool local.

Garanties visées : refus des éditions divergentes avant déplacement, contenu
révoqué retiré vers une quarantaine privée durable (aucune promesse d'effacement),
absence de remplacement d'un fichier non possédé, journal avant mutation du
checkout, déplacements sans écrasement avec identités de fichiers épinglées,
reprise exacte et collisions conservées. Captures historiques préservées.
Publication pending et opération namespace interrompue bloquent le refresh.

À faire : moteur/journal SQL, transitions et reprise, tests Windows/Linux,
qualification du raccordement réseau par root. Aucun succès revendiqué à ce stade.
