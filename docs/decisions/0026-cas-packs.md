# 0026 — Packs CAS pour les petits objets

Date : 4-5 novembre 2026. Statut : implémenté le 5 novembre 2026 (non
commité). Décidé par l'utilisateur le 4 novembre 2026, révisé après relecture
du conseil (tour 2 : Astra, Sol et Opus, tous « go avec changements »).

Résultat mesuré (mêmes jour et machine) : `layrs init` à 10 000 fichiers passe
de 51-52 s à 15 s ; la publication CAS de 34 s à 2 s ; `.layrs/objects` de
20 013 fichiers (54 Mo) à 10 fichiers et 1 pack (9 Mo). Code :
`crates/layrs-store-local/src/scm_v1/cas/packs.rs` ; tests :
`crates/layrs-store-local/tests/scm_cas_packs.rs`.

## Problème

Chaque objet CAS est un fichier : `objects/<kind>/<aa>/<bb>/<id>`. Publier un
objet coûte une création, une écriture, une relecture, un renommage et un scan
antivirus à la fermeture.

- Mesure à 10 000 fichiers (r14) : `layrs init` prend 54 à 65 s, dont 34 s de
  publication pour environ 20 000 petits objets, soit environ 1,7 ms par objet.
- Effets secondaires : 20 000 fichiers dans `.layrs`, et environ 4 Kio de
  cluster par objet de quelques dizaines d'octets.

## Décision

Un lot `put_ordered` qui publie au moins `PACK_MIN_OBJECTS` (64) **nouveaux et
distincts** petits objets range ces objets dans des **packs** immuables. Un
petit objet a des octets canoniques d'au plus 64 Kio. Les autres objets, et les
lots plus petits, restent des fichiers séparés, comme aujourd'hui.

### Un fichier par pack, autodescriptif

`objects/packs/pack-<uuid>.lpk` contient, dans l'ordre :

1. **En-tête** : magic et version.
2. **Enregistrements**, chacun autodescriptif : genre, identifiant, longueur,
   octets canoniques.
3. **Index** trié par (genre, identifiant), sans doublon : genre, identifiant,
   offset et longueur de chaque enregistrement.
4. **Pied** : offset et taille de l'index, nombre d'entrées, empreinte blake3
   de l'en-tête et de l'index, magic de fin.

Il se publie par **un seul renommage** sans écrasement, sans couple
`.pack`/`.idx` dont la cohérence serait à garantir après un crash. Les
enregistrements autodescriptifs permettent au scrub de nommer l'objet abîmé
même sans index.

Taille bornée :

- un pack s'arrête à 32 Mio : au-delà, un pack est fermé et un autre commence ;
- l'index au plus à 64 Mio et un million d'entrées ;
- chaque objet au plus à 64 Kio.

### Métadonnées : rien ne change

`objects.storage_path` est `UNIQUE` (`tests/fixtures/scm-v2.sql:74`). Il reste
le **chemin canonique dérivé de l'identifiant**, pour tout objet, qu'il soit
dans un pack ou non. C'est le CAS qui trouve l'emplacement physique : fichier
séparé ou membre d'un pack.

Les contrôles de `read.rs`, `read/availability.rs` et `fsck` qui vérifiaient
la présence et la longueur du fichier passent par `NativeCas::locate`.

### Publication (ordre exact, dans `put_ordered`)

1. Les workers ajoutent les petits objets nouveaux à un **constructeur de
   pack unique** par lot.
   - Les 63 premiers sont gardés en mémoire (au plus 4 Mio) ; dès le 64e, ils
     sont écrits dans un fichier de `staging` (préfixe `scm-v1-`, bail de
     staging partagé) et seuls les index restent en mémoire.
   - Doublons : la clé est l'identifiant ; un objet déjà présent dans le pack
     en cours est réutilisé.
2. Après la fin de tous les workers :
   - **s'il y a moins de 64 objets**, ils sont publiés en fichiers séparés,
     par le chemin habituel ;
   - **sinon**, le pack est finalisé :
     1. écriture de l'index et du pied ;
     2. vidage des données (écriture traversante en B, `sync` en A) ;
     3. **relecture et vérification de chaque enregistrement** depuis le
        fichier ;
     4. renommage sans écrasement dans `objects/packs/` ;
     5. vérification de l'identité du fichier publié ;
     6. inscription de `objects/packs` dans les répertoires à synchroniser
        en fin de lot (A).
3. **Ensuite seulement**, l'étape de durabilité du lot (A ou B) : le marqueur
   NTFS est créé après le dernier renommage de pack.
4. Les résultats sont rendus, un par entrée et dans l'ordre de soumission. Un
   échec de finalisation est rendu comme erreur du lot.

Les packs sont désactivés si le lot ne peut pas démarrer, ou avec
`LAYRS_CAS_BATCH_DURABILITY=per-object`.

### Déduplication et copies douteuses

- Un objet déjà présent est réutilisé. Avant d'être référencé, il est relu et
  ses octets comparés à ceux attendus.
- S'il est dans un pack, sa durabilité est rétablie avant le commit :
  - en A : synchronisation du fichier pack et de `objects/packs` ;
  - en B : le vidage final du lot couvre le journal et le cache disque.
  Un pack orphelin d'un lot échoué n'a peut-être jamais été vidé.
- Une copie en pack qui ne se vérifie pas est **ignorée** (traitée comme
  absente) et l'objet est réécrit. Un pack abîmé ne bloque jamais les
  écritures suivantes.
- En lecture, toutes les copies sont essayées : index en mémoire, fichier
  séparé, puis relecture de `objects/packs/`. La première copie qui se vérifie
  est rendue. Une corruption n'est signalée que si aucune ne se vérifie.

### Lecture

- **Cache des index par processus**, indexé par l'identité du Store. L'hôte
  VFS recrée sa vue CAS toutes les 2 s, et ne doit pas recharger les index à
  chaque fois.
- Écrivain : un inventaire au début du lot, puis ses propres packs y sont
  ajoutés. Jamais de relecture du répertoire à chaque absence pendant un lot.
- Lecteur : au plus une relecture du répertoire par absence ; aucun cache
  négatif.
- **L'index est une entrée non fiable.** Sont vérifiés :
  - les bornes, l'ordre et l'unicité ;
  - que les plages sont dans les enregistrements, sans chevauchement ;
  - la longueur de chaque objet, au plus 64 Kio ;
  - les genres.
  Chaque lecture est exacte : un lecteur borné à la longueur attendue, et une
  lecture courte compte comme une corruption. Le contenu est validé par le
  décodeur canonique avant d'être livré, y compris pour un chunk lu en flux.
- La présence dans l'index n'est jamais une preuve de contenu ni d'admission.
- Aucun handle de pack ne reste ouvert entre deux lectures. Exception
  (2026-11-09, conseil s9) : une lecture en masse explicite — le peuplement
  d'une Room virtuelle, le staging d'une matérialisation — tient les fichiers
  pack de son catalogue (`hold_pack_files`) jusqu'à sa fin : au plus 16 packs
  par catalogue tenu et 32 pour tout le processus, ouverts en lecture seule
  sans droit de suppression, avec tous les modes de partage. Un peuplement
  ne tient que la vue CAS courante de l'hôte (la précédente est relâchée
  quand une nouvelle est liée) ; un staging tient une session par fil de
  lecture (au plus 8). Rouvrir le pack à chaque bloc coûtait ~230 µs par
  fichier lors du peuplement d'une Room (git garde aussi ses packs ouverts,
  en nombre borné). Indépendamment, chaque catalogue garde ses 64 derniers
  blocs décompressés (au plus 64 Kio chacun), au lieu de 8.

### Format et compatibilité

- Les capacités du Store (`NativeStoreOpen`, `NativeStoreExclusive`,
  `NativeStoreAudit`) portent la **version de format validée**, transmise à
  `NativeCas`. Les packs ne sont écrits et lus que pour la version **5**.
- `init` crée des Stores en format **5** (`minReaderVersion` 5). Les numéros
  3 et 4 de la même magic `LAYRS_NATIVE_STORE` sont déjà pris par les Stores
  de projection (ADR 0024, `client-core/native_scm/partial.rs`), et restent
  refusés comme Store complet. Un ancien binaire voit un format 5 en
  `FutureFormat` et refuse. `objects/packs` est créé au premier pack.
- Les lecteurs acceptent 2 et 5. Reprendre une initialisation **conserve** la
  version du marqueur : un Store en format 2 n'est jamais promu
  implicitement. `LAYRS_STORE_FORMAT=2` crée encore un Store en format 2.
- Les Stores existants restent en format 2, sans packs ; une montée de
  version explicite viendra plus tard.

### Contrôles

- Le scrub ignore `objects/packs` dans son parcours des fichiers séparés, puis
  vérifie **chaque enregistrement de chaque pack à son emplacement**, sans
  passer par la recherche par identifiant. Une copie saine ailleurs ne masque
  donc pas un membre abîmé.
- `fsck` contrôle les lignes de la base, comme aujourd'hui, avec `locate` pour
  l'emplacement physique. Les orphelins, séparés ou en pack, ne sont pas
  signalés, comme aujourd'hui.

### Reprise après crash

- Un pack publié dont le lot n'a pas été commité n'est référencé par rien.
- Il peut être réutilisé plus tard, après vérification et durabilité, ou
  ignoré s'il est abîmé.
- Les fichiers de `staging` sont nettoyés comme aujourd'hui.
- Un pack n'est jamais modifié ni supprimé, ce qui garde valide la clôture
  par induction de l'admission (`admission.rs` vers la ligne 574).

## Tests requis

- Aller-retour des objets, lots mixtes séparés et en pack, doublons, seuil à
  63 et à 64 objets, changement de pack à la limite de taille.
- Pannes autour du renommage du pack, de l'étape de durabilité et du commit,
  puis réouverture et nouvel essai.
- Corruptions :
  - pack tronqué ;
  - octet modifié dans un enregistrement, où seul cet objet doit échouer et
    le scrub doit le nommer ;
  - index invalide ;
  - même objet présent séparé et en pack.
- Réutilisation d'un pack orphelin après un lot échoué.
- Lecteur long qui découvre un pack publié après lui ; hydratation VFS depuis
  un pack.
- Un Store en format 2 ne reçoit jamais de pack ; reprise d'une
  initialisation en format 2 ; refus d'un ancien binaire sur un format 5.

## Hors périmètre

- Reconditionnement et suppression de packs.
- Packs dans les Stores en format 2.
- `layrs-store-server`.
- Gros chunks : ils gardent un fichier chacun.

## Gain attendu (estimation du conseil)

- `init` à 10 000 fichiers : de 54-65 s à environ 18-22 s. Il resterait la
  validation (environ 14 s), l'insertion des lignes et la capture.
- Même effet sur les captures et les réceptions de nombreux petits fichiers.
- Quelques fichiers au lieu de 20 000 dans `.layrs`.
