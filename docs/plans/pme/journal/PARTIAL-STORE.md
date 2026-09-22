# Worktree partiel — fondation locale, 21 septembre 2026

Choix acquis : noms/chemins visibles, contenu interdit non matérialisé et namespace
réservé. Une application externe peut écrire sur le disque ; Layrs doit refuser
capture/publication/collision, jamais supprimer ou écraser le fichier protégé.

Lot en cours (operations) : types Core distincts, ReservedPaths, registre local
format3 durable et génération conditionnelle. Aucun changement de l'admission
complète format2. Prochaines frontières distinctes : matérialisation sélective,
captures/undo, patch serveur sur base exacte, migration explicite des Stores2.

Le format3 n'annonce ni closure complète ni autorité réseau. Les observations
PolicySet/epoch restent provenance ; chaque opération distante doit revalider
l'identité et les droits. Le stockage d'une structure vérifiée ne l'admet pas dans
le catalogue natif complet. Pas de commit par cet agent.

Checkpoint 21 septembre — bibliothèque compilable, qualification Windows :

- Core `projection::{ReservedPaths, ProjectionSnapshot, ProjectionEntry}` :
  protections exactes, ancêtres/descendants, deux côtés d'un déplacement, NFC,
  casse Unicode majuscule et aliases Windows conservateurs (dont tout `~`
  refusé). États Materialized/Reserved/Unavailable/Absent distincts ; permission
  writable explicite, jamais déduite de l'absence. Noms/IDs distants conservés.
- Store `projection::ProjectionStore` : FORMAT3 séparé de l'admission native2,
  SQLite distinct, identité Store/Worktree/Space/principal/origine, racine et
  identités filesystem épinglées, snapshots canoniques et générations CAS.
  Anciens lecteurs refusent ce format ; aucune migration implicite de Store2.
- `install_projection(generation, target, callback)` installe uniquement les
  transitions Unavailable→Materialized de fichiers réguliers. Staging streamé
  (1GiB/fichier,4GiB total), FileId vérifié, journal SQLite durable avant rename
  sans écrasement, inode et contenu contrôlés avant Ready. Callback local préparé,
  aucun réseau sous verrou. `resume_installation(root)` reprend explicitement ;
  l'ouverture normale refuse une installation inachevée. Les fichiers réservés
  existants ne sont ni lus ni écrasés ; une capture ultérieure les refuse.
- `capture_patch(generation)` conserve un diff complet contre la projection
  distante exacte, sans changer ses IDs ni créer de Step incomplète. Deux scans
  stables, namespace complet sans liens/reparse, lecture/hash bornée à1GiB/4GiB,
  au plus1024 changements et16MiB d'octets modifiés (endpoint actuel). Inconnu,
  réservé, indisponible, alias et changement d'un fichier readonly refusés.
  `CapturedPatch` possède sequence/digest/baseGeneration/baseDigest/identity et
  changes ; expected utilise CapturedTarget(FileId,mode), replacement utilise
  CapturedFile(FileId,mode,longueur). Les bytes et versions sont atomiquement
  conservés en SQLite. Retry identique renvoie la version précédente ; historique
  et bytes anciens restent accessibles après modification du fichier courant.
  `captured_patch`, `latest_captured_patch`, `write_captured_file` alimentent une
  publication réseau préparée après relâchement du verrou local.
- Tests Windows `cargo test --offline --locked -p layrs-store-local --lib projection` :
  13 PASS (0,57s), dont5 frontières d'interruption hydration, corruption/collision,
  remplacement externe par inode différent de contenu identique, readonly,
  capture concurrente, rollback SQL et reprise après commit, borne16MiB.
  Core2 tests réservations PASS lors du premier lot ; ajout CLOCK$ à requalifier.
  Clippy core/store-local tous targets strict PASS (9,84s avant ajout CLOCK$).
  Root a qualifié clone partiel TLS réel PASS19,98s ; publication de capture non
  encore qualifiée à ce checkpoint.

Reprise / restant exact : tests Linux à lancer ; création/déplacement/undo local
journalisés pas encore implémentés ; admission d'une absence fraîche autorisée
et commit de publication doivent être raccordés. Hydration symlink/conflit non
supportée actuellement (refus explicite). Pas de migration Store2↔3 ni de fsck
format3 complet. Ne pas déclarer le Worktree partiel entièrement livré tant que
ces parcours et la publication réelle ne sont pas qualifiés. Les sources de ce
lot sont `core/src/projection*` et `store-local/src/projection*`, avec seuls
exports `lib.rs` ajoutés. Parent seul responsable des commits.

Checkpoint publication (après commit fondation/capture768e1ec) :

- Linux isolation/source RO/réseau none : Core2 et Store15 PASS (0,61s Store),
  y compris liens symboliques DB et checkout. Pas de fichier externe suivi.
- `begin_publication(sequence,generation,requestJcs)` conserve les octets exacts
  avant HTTP ; requête et reçu JSON objets JCS bornés24MiB, digest et capture/base
  liés. `pending_publication` reste accessible après réouverture. Captures,
  installation et CAS refusés tant que publication en attente ; lectures permises.
- `record_publication_receipt` conserve le reçu exact même si une révocation ou
  une panne empêche ensuite de récupérer la nouvelle projection. Aucun succès
  d'acceptation locale implicite. Retry du reçu différent refusé.
- `finish_publication(sequence,receipt,Some(snapshot))` accepte seulement les
  feuilles capturées et nouveaux IDs Step/racine, mêmes permissions/epoch et
  namespace ; suppression représentée explicitement Absent, non oubliée. Reçu,
  snapshot et retrait du pending forment une transaction SQLite. Avec None,
  rejet terminal conservé sans changer la base. `completed_publication` permet
  de retrouver reçu et état après perte de réponse client, sans republier.
- Tests Windows projection16 PASS0,62s : début durable, rollback avant receipt,
  reprise après receipt durable, received-pending, changement protégé/epoch
  refusé, requête corrompue/noncanonique/tropgrande refusée. Publication HTTP
  réelle raccordée et qualifiée par parent séparément, pas revendiquée ici.
- Sources nouvelles `projection/publication.rs` et `publication/storage.rs`,
  nouvelles tables natives3 uniquement. Le format3 est encore en développement :
  fixtures locales créées avant ces tables doivent être recréées explicitement.
  Aucun Store2 ni donnée utilisateur n'est converti.

Checkpoint namespace (après commit publication6d76172) :

- `observe_absent_paths(generation,digest,paths)` enregistre uniquement des
  destinations absentes explicitement classifiées par le serveur sur la même
  identité/base/epoch ; le client est responsable de cette vérification fraîche.
  Namespace local inconnu ≠ permission. Toutes collisions physiques, réservations
  exactes/ancêtres/descendants et aliases sont refusés avant CAS SQLite.
- `preview_namespace` renvoie un NamespacePreview sérialisable : operationId,
  génération/digest projection, mutation Create ou Move (rename identique),
  descripteur source et empreinte complète des fichiers visibles. `apply_namespace`
  revalide ces témoins sous verrou, vérifie droits observés et destination encore
  absente, puis conserve un journal avant rename sans écrasement. Création≤16MiB,
  déplacement de fichier régulier, budget global de patch1024changements/16MiB
  prévalidé avant mutation. Réseau effectué avant ce verrou par le client.
- Le renommage utilise le même handle épinglé qui a été vérifié. Création stage
  durable ; reprise exige même inode/contenu, jamais adoption d'un fichier externe
  identique. Capture durable + historique namespace + retrait du pending forment
  une transaction SQLite. `resume_namespace(root)` reprend après interruption,
  l'ouverture normale refuse le pending. Les répertoires temporaires abandonnés
  sont conservés, aucune suppression automatique de données.
- Les IDs distants restent identiques après la mutation locale ; déplacement
  signifie suppression/création dans CapturedPatch contre la base distante,
  jamais Step artificiellement complète. Rejeu du même preview strict, rejet
  si bytes de création différents. Le nouvel ID d'opération évite de confondre
  une intention nouvelle avec un ancien retry après retour au même état.
- Tests Windows22 PASS1,54s :4 nouveaux tests namespace dont8 interruptions
  create/move, replay, capture ancienne conservée, source modifiée depuispreview,
  collision externe, réservé/alias/read-only, reprise refusée après remplacement
  inode même contenu. Fsck parent enrichi `namespace_operations` et validation
  canonique de chaque opération liée à sa capture/destination/source.
- Reste à cette frontière : undo/redo/restore de captures, déplacement de
  répertoire, symlink, réception/réautorisation et migration explicite. L'agent
  fusion prend le moteur refresh/quarantaine ; root prend réception client ;
  selectors prend contrôles serveur frais et UI namespace. Aucune revendication
  de complétude du Worktree partiel avant leurs recettes intégrées.
