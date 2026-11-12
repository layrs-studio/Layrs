# 0028 — Store compact : taille égale ou inférieure à git

Date : 6 novembre 2026. Statut : décidé, en cours d'implémentation par
phases. Phase 1 faite. Phases 2-3 faites en opt-in (`LAYRS_STORE_FORMAT=6`),
relues par le conseil (r2, r3) : 2 373 Kio après `init` à 10 000 fichiers,
contre 43 233 Kio pour le format 5 et 1 655 Kio pour git. Demandé par l'utilisateur (« évidemment que la taille compte, fais
moi tout »), conçu avec le conseil (tour 1 : Astra, Sol, Opus, accord sur
la direction ; deux désaccords tranchés ci-dessous).

## Problème

Même projet de 10 000 fichiers (294 Ko de contenu), mesuré le 6 novembre :

| Partie | Layrs | git |
| --- | --- | --- |
| total | 44 701 Kio | 1 655 Kio |
| SQLite | 33 152 Kio | aucune |
| pack CAS | 7 644 Kio | 805 Kio (pack et idx) |
| arbres (10 objets séparés) | 1 484 Kio | dans le pack |
| index de changements | 1 415 Kio | index 784 Kio |
| cache de projection | 985 Kio | aucun |

Causes :

- **Six lignes SQLite par fichier**, avec des identifiants texte de 71
  caractères, recopiés dans environ 13 index.
- **Deux objets CAS par fichier** : le chunk, et un manifeste JSON de 589 octets.
- **Aucune compression.**
- **Identifiants en texte** dans les caches.

Même avec des identifiants binaires et des tables repliées, des lignes par
fichier coûtent 4 168 Kio, soit 2,5 fois tout `.git`. **Le contenu doit donc
sortir de SQLite.**

## Décision

Un nouveau format de Store **6**, avec un schéma SQLite **v3** et des packs **v2**.

### 1. Le contenu sort de SQLite, l'admission reste exacte

- **Aucune ligne par fichier** dans `objects`, `space_object_admissions`,
  `file_availability`, `file_manifests`, `file_manifest_chunks` ni
  `object_edges`.
- **Installation committée.** Un pack est inscrit dans SQLite (`cas_packs` :
  nom, empreinte blake3 de son index, nombre d'entrées) dans la transaction
  du lot qui l'a publié.
  - Un pack non inscrit, orphelin d'un lot échoué, n'est jamais présent.
  - Un index dont l'empreinte diffère de celle inscrite est une corruption,
    jamais une absence.
- **Admission exacte par Space.** Elle est stockée en **bitmaps des ordinaux**
  d'un pack : une ligne par couple (Space, pack). L'admission n'est pas
  déduite des Trees. Le conseil est unanime, pour trois raisons :
  - des chunks et des manifestes sont admis sans Tree ;
  - un FileId peut avoir plusieurs manifestes ;
  - l'induction de fermeture repose sur des admissions jamais retirées
    (`admission.rs`, vers la ligne 665).
- Chaque requête par fichier est remplacée par une recherche dans le
  catalogue des packs inscrits. Les sémantiques, erreurs et budgets actuels
  sont conservés. La table de correspondance requête par requête figure dans
  les réponses du conseil, tour 1.

### 2. Un petit fichier est stocké une seule fois

- Un fichier d'un seul chunk (au plus 1 Mio) est **un seul enregistrement**,
  clé FileId, qui contient ses octets bruts.
- Son ChunkId et sa recette de manifeste (découpage `deterministic_fixed` ou
  `single`) sont dans l'index : la recherche par ChunkId ou par
  FileManifestId reste exacte, sans relire ni rehacher le contenu.
- Les manifestes de plusieurs chunks ou d'un autre découpage restent
  explicites.
- **Le format réseau ne change pas** : le manifeste et le chunk canoniques
  (JCS) sont reconstruits à la demande, octet pour octet.

### 3. Packs v2

- **Encodages binaires** des Trees et des manifestes. Un enregistrement doit
  se réencoder exactement en son JCS canonique, et son identité est toujours
  recalculée sur ce JCS, jamais sur l'encodage de stockage.
- **Numéro de codec** par enregistrement.
- **Les identifiants complets ne sont que dans l'index**, que protège son
  empreinte inscrite dans SQLite. *Désaccord, tranché pour Opus contre
  Astra et Sol*, qui voulaient une seconde copie des identifiants pour que le
  scrub nomme un objet sans l'index. Cette copie coûterait environ 600 Kio à
  10 000 fichiers, plus que tout le pack de git. Comme `git fsck`, un objet
  abîmé dont l'index est aussi abîmé est nommé par l'atteignabilité.
- **Compression zstd par blocs d'environ 16 Kio**, avec une somme de contrôle
  par bloc. *Désaccord, tranché pour Opus contre Astra et Sol*, qui
  préféraient des trames indépendantes par objet.
  - Taille du contenu : 37 Kio en blocs, contre 186 Kio objet par objet et
    204 Kio pour git.
  - Coût : un octet abîmé rend illisible un bloc entier, et non un seul objet.
  - git accepte ce même compromis avec ses chaînes de deltas, où une base
    abîmée rend illisibles tous les objets qui en dépendent.
  - La réparation vient du serveur ou du dossier de travail.
- Tout enregistrement d'au plus 1 Mio va dans le pack du lot. Le seuil de
  64 objets et la limite de 64 Kio de l'ADR 0026 disparaissent.

### 4. Caches

- **Index de changements v2.**
  - Identifiants binaires de 32 octets, chemins préfixés, entiers variables.
  - Le FileId est omis s'il est égal à celui de la base.
  - Il garde tous les champs de StatKey, la marge anti-« racy » et la somme
    de contrôle.
  - Un fichier v1 est ignoré : tous les fichiers sont alors relus une fois.
- **Cache de projection.** Il est compacté avec des identifiants binaires. Il
  ne sera supprimé que si les mesures de `status` et de `step` restent
  égales sans lui.

### 5. Compatibilité

- Le nouveau Store a `formatVersion = minReaderVersion = 6`.
- Les formats 2 et 5 sont refusés sans être modifiés, avec une erreur
  explicite (ADR 0013). Cela remplace la promesse de lecture de l'ADR 0026.
- Une commande temporaire `layrs store convert` rejouera l'historique en lots
  validés.
- Les formats 3 et 4 de projection ne sont pas concernés.

### 6. Consolidation

Les packs sont consolidés automatiquement au-delà d'un seuil, avant le
passage du format 6 par défaut. La consolidation :

- publie le nouveau pack, puis l'inscrit et retire les anciens dans une même
  transaction ;
- ne supprime les fichiers retirés que plus tard, sous le verrou d'écriture ;
- garantit que l'ensemble admis n'est jamais réduit.

### 7. Format 7 : un seul condensé par petit fichier (conseil s13 Q5, Q6)

Un fichier d'un seul chunk gardait deux condensés de 256 bits : son FileId
(domaine fichier) et son ChunkId (domaine chunk), soit 32 octets de plus par
fichier dans l'index du pack.

- **Recette `whole` (version 1).** Un fichier de 1 octet à 1 Mio a un seul
  chunk, nommé par le condensé de son FileId : son ChunkId *est* ce
  condensé, calculé une fois. Le manifeste `whole` le vérifie (un chunk, de
  la taille du fichier, nommé par le FileId). Les recettes existantes et
  leurs identités ne changent pas ; un fichier vide garde zéro chunk.
- **Vérification par domaine.** Un chunk `whole` est vérifié dans le
  domaine fichier, partout où des octets sont comparés à un ChunkId : la
  recette du manifeste, l'enregistrement du pack ou le chunk lui-même
  (`Chunk::whole`, `Chunk::verified`) le déclarent. On n'essaie jamais les
  deux domaines.
- **Packs v2, version 2.** Un enregistrement `whole` ne stocke pas de
  ChunkId et sort de l'ordre des ChunkId (l'ordre des FileId en tient lieu).
  Un ChunkId n'y nomme jamais à la fois un chunk et un enregistrement
  `whole`. Un pack de version 1 ne contient jamais de `whole`.
- **Écriture.** Un chunk `whole` n'est stocké que comme l'enregistrement de
  son fichier ; un objet `whole` est refusé par tout Store qui n'est pas au
  format 7, et un pack de version 2 y est lu comme une corruption.
- **Captures.** Les captures d'un Store au format 7 gardent les petits
  fichiers en `whole` (`step`, `room step`, `init`, captures avant une
  opération). Les fichiers reçus d'un serveur, les fusions et les captures
  du VFS restent en v1 : un Store au format 7 lit et écrit les deux.
- **Réseau inchangé.** Un fichier `whole` part comme le fichier v1 de même
  FileId (son chunk et son manifeste `deterministic_fixed`) : les serveurs
  vérifient les chunks dans le domaine chunk. Arbres et Steps ne nomment
  que des FileId : ils ne changent pas.
- **Schéma SQLite v4.** Les tables de v3 sans les tables `pending_*` que le
  format 6 n'écrit jamais, `WITHOUT ROWID` pour les clés textuelles ou
  composées (plus d'index de clé séparé), une instruction par ligne
  (SQLite garde le texte de chaque instruction ; les commentaires sont
  entre les instructions). Mêmes colonnes et contraintes : chaque requête
  de v3 s'exécute telle quelle. Gain mesuré modeste (~8 Ko après le
  scénario) : avec des pages de 1 Kio, une ligne `WITHOUT ROWID` de plus de
  ~230 octets déborde, et les identifiants TEXT de 71 caractères y
  arrivent vite ; les identifiants binaires de 32 octets lèveraient cette
  limite.
- **Mesures** (10 000 fichiers, scénario du banc) : pack d'init 1 249 →
  909 Ko ; Store 1 761 → 1 385 Kio après `init`, 2 246 → 1 906 Kio après
  le scénario (git : 1 868 / 1 908).
- **Compatibilité.** `formatVersion = minReaderVersion = 7`, en opt-in
  (`LAYRS_STORE_FORMAT=7`) ; un lecteur du format 6 refuse le Store. Un
  Store au format 6 n'est jamais réinterprété ; sa conversion sera
  explicite.
- **Reporté** : identifiants binaires de 32 octets dans SQLite, purge des
  opérations de récupération terminées (leur clé d'idempotence), Trees et
  manifestes binaires (phase 4).

## Phases

Chaque phase ne garde que ce qui entre dans la conception finale. Le
critère pour toutes : taille au plus égale à celle de git et aucune
commande plus lente, sur 10 000 et 100 000 fichiers, avec un historique et
le fichier de 1,5 Gio de l'ADR 0027.

1. Index de changements v2, puis cache de projection compact (caches seuls).
2. Squelette du format 6 (`LAYRS_STORE_FORMAT=6`) : schéma v3, catalogue des
   packs inscrits, admissions en bitmaps, packs v2.
3. Enregistrements « fichier » et alias.
4. Trees et manifestes binaires.
5. Consolidation des packs.
6. Format 6 par défaut, et conversion.
7. Deltas entre versions, pour la taille d'un long historique.

## Estimation à 10 000 fichiers

Environ 1,1 à 1,6 Mo selon le conseil, contre 1,66 Mo pour git. Ce n'est pas
démontré : seule la mesure du critère fait foi.
