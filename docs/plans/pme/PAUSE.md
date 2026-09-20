# Pause — 20 septembre 2026, 20 h 02 (Europe/Paris)

Instruction utilisateur : « Okay faisons une pause ! Note ou tu en est, et arrête toi ».
Ne reprendre que sur nouvelle instruction. Racine du projet : `D:\Layrs\layrs`.
Les modifications préexistantes et celles de cette passe sont conservées, sans
commit, déploiement ni installation. Aucun des artefacts qualified2 ne contient
les derniers changements streaming.

## Dernier travail intégré par root

- Réception distante raccordée à `apply_checkout_with_sources` : projections
  FileId/longueur, métadonnées seules dans le batch, chunks dans spool privé,
  admission atomique et matérialisation avec journal. Callbacks exclusivement
  locaux ; tout téléchargement précède l’écriture du Store.
- `remote/graph.rs` accepte un writer dynamique et ne reconstitue plus le batch
  complet en mémoire. `read_step_projection` fournit le parcours local borné.
- Budgets distincts : fichier1GiB, flux4GiB, métadonnées256MiB, objet64MiB.
  Les anciennes API retournant des buffers complets restent limitées.
- **15 tests remote PASS0,09s** après ce raccord.
- **TLS courte PASS46,37s**, avec expiration réelle de capacité, clients tués
  avant dispatch/après commit, rejeu exact et réception inchangée zéro download.
  Commande : `cargo test --locked --offline -p layrs-server --test native_clients_v3 -- --ignored --nocapture`
  avec `LAYRS_TEST_UNCHANGED_ONLY=1`, `LAYRS_TEST_DATABASE_ISOLATED=1` et URL
  PostgreSQL lue depuis le fichier privé existant, jamais copiée dans les journaux.
- TypeScript UI/Desktop/Web, tailles des sources,16 checksums de migrations,
  contrat SCM et garde WASM **PASS**. La garde WASM exécute son test réel.
- CLI74 commandes et Desktop équipe/règles JSON ont passé avant le dernier
  raccord streaming ; ne pas les présenter comme validation globale du code final.

## Travail des trois agents au moment de la pause

- **operations — A/B/C** : core incrémental qualifié1GiB ; capture et
  matérialisation100MiB PASS10,59s, heap Rust additionnel8 537 465 octets.
  Premier scénario1GiB échoué sur l’ancienne limite256MiB de fsck, puis correctif
  et **recette1GiB PASS124,95s** terminée au moment de la pause, fsck propre.
  Pic heap Rust additionnel9 791 294 octets (pas RSS) ;26 tests fsck PASS2,96s.
  Aucun processus restant, fixtures de cette recette nettoyées normalement.
  Linux/HTTPS1GiB et durcissement DACL explicite du spool restent à faire.
- **fusion_native — opérations locales** : restore/undo/redo/switch/preview et
  résumé diff utilisent les projections. Recette locale100MiB Windows
  PASS39,02s ;27 régressions PASS ; Clippy PASS22,76s. Linux non lancé.
  Les gros diffs omettent texte/classification ; petits contenus combinés≤8MiB
  conservent le rendu détaillé. Aucun test de redémarrage serveur ajouté à ce stade.
- **selectors — TLS gros fichiers** : recette100MiB avec restore déjà terminée
  **en échec45,90s** au contrôle d’arrêt, aucun processus restant. Initialisation/
  capture5,519s, publication16,148s, clone et vérification exacte octets/FileId
  passent. Échec lors de `receive-unchanged` : `Local InvalidState: materialization
  file limit exceeded`, helper de recette ligne238. Aucun correctif tenté après
  la pause ; c’est le premier défaut à traiter à la reprise. Binaire copié
  `tmp/native-streaming-20260920-180116.exe`, rapport
  `tmp/native-streaming-100mib-20260920-180116.jsonl`, session71794.
  Commande : `node scripts/test-native-streaming-volume.mjs --binary tmp/native-streaming-20260920-180116.exe --report tmp/native-streaming-100mib-20260920-180116.jsonl --sizes100 --restore`.
  Lire le journalT4 pour le SHA et les détails. Restore et1GiB non exécutés.
  Utiliser un nouveau rapport au prochain lancement.

## Test99 000 fichiers interrompu volontairement

Session22308 arrêtée par Ctrl+C, code1, à la demande utilisateur. Aucun processus
`native-volume` encore observé ensuite. Rapport conservé et marqué paused :
`measurements/T8-tls-volumes-leases-2026-09-20.json`.

Phases terminées (millisecondes) : génération38 723 ; initialisation105 145 ;
publication193 140 ; clone1 468 999 ; vérification exhaustive112 600 ; réception
inchangée208 992 ; capture/publication d’un fichier320 586. Réception incrémentale
et dernière vérification **non qualifiées**. Profil debug, données fortement
dédupliquées, compilations concurrentes : aucune promesse de performance.

Binaire historique conservé `tmp/native-volume-leases-20260920.exe`, SHA256
`257af0ae3d7b80bd665f3adbef1cc80bf9c1d8d6e74c8b6dda63aeac8a7022a8`.
Il précède les derniers changements streaming. L’arrêt forcé peut avoir laissé
sa base/son rôle PostgreSQL UUID et sa fixture temporaire ; ne rien supprimer
sans identifier précisément les ressources créées par la recette.

## Ordre de reprise

1. Lire ce fichier, STREAMING et les dernières entrées des journauxT1/T4/T8.
   Inspecter le diff sans écraser le travail déjà présent.
2. Corriger le refus64MiB résiduel rencontré par receive-unchanged100MiB ;
   qualifier Linux. Le fsck local1GiB est désormais vert.
3. Relancer100MiB TLS dans un rapport neuf, puis1GiB si100MiB passe ; mesurer
   la mémoire OS, contrôler tous les octets et restore, conserver les échecs.
4. Terminer la recette99k avec un binaire correspondant aux sources retenues.
5. Geler les sources, qualification Rust/Clippy/TLS/CLI/Desktop complète,
   nouveaux packages Windows/Linux/MSI dans des dossiers neufs, puis recette
   d’upgrade/rollback déjà préparée. Aucun package nouveau créé dans cette pause.
6. Les limites restantes restent explicites : Worktree partiel absent, navigateur
   Web indisponible pour qualification, macOS non qualifié, pilote humain et
   signature non réalisés ; ne pas cocher artificiellement les plans composés.

Le pilote Git public PyPA a déjà réussi ; aucun chemin de dépôt utilisateur
n’est requis pour reprendre.
