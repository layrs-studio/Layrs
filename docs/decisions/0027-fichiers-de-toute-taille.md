# 0027 — Fichiers de toute taille

Date : 5 novembre 2026. Statut : implémenté le 5 novembre 2026 (non commité).
Demandé par l'utilisateur le 5 novembre 2026 (« ajoute le support des fichiers
de n'importe quelle taille »).

## Problème

Le chemin natif refusait tout fichier de plus de 1 Gio et toute capture de plus
de 4 Gio : capture, spool, admission, checkout, VFS, distant et serveur. Les
octets passaient pourtant déjà en streaming, un morceau à la fois. Ces plafonds
étaient des budgets, pas des limites mémoire.

Mesure avant ce changement, sur un fichier de 1,5 Gio : `layrs init` refuse
(« streaming checkout byte limit exceeded »).

## Décision

### Taille de morceau adaptative

La découpe reste fixe et déterministe. La taille de morceau dépend de la
longueur du fichier (`layrs_core::scm::fixed_chunk_bytes`) :

| Fichier | Morceau |
| --- | --- |
| jusqu'à 1 Gio | 1 Mio (inchangé) |
| au-delà | puissance de 2 de longueur/1024, entre 1 et 16 Mio |

- **Compatibilité** : aucun contenu déjà admis ne change, puisque les fichiers
  de plus de 1 Gio étaient refusés.
- **Ordre de grandeur** : un fichier a environ 1 024 morceaux jusqu'à 16 Gio,
  puis le nombre de morceaux croît linéairement (64 Gio donne 4 096 morceaux de
  16 Mio). Le manifeste reste petit.
- **Coût** : le CAS ne crée plus 100 000 fichiers pour un fichier de 100 Gio.
- **Identité** : le FileId ne dépend jamais de la découpe ; seuls le stockage et
  la déduplication en dépendent.
- **Où elle s'applique** : capture ordinaire, capture VFS des Rooms et spool.

### Plafonds levés

Les tailles de fichier et de capture ne sont plus bornées : seul le disque les
borne.

- Les nombres d'objets restent bornés :
  - 100 000 entrées par checkout ;
  - 262 144 objets par lot ;
  - 2^24 morceaux par fichier.
- La taille maximale d'un objet canonique reste 64 Mio, ce qui donne environ
  500 000 morceaux par manifeste.
- Chemins concernés :
  - capture, spool, admission (`preflight`) et projection ;
  - checkout, matérialisation et lecture de disponibilité ;
  - fsck, diff et restore ;
  - VFS, distant natif et clôture serveur.
- **Distant** : un plan de transfert de plusieurs fichiers se découpe toujours
  au-delà de 4 Gio, mais un fichier seul n'est jamais découpé et passe quelle
  que soit sa taille.
- **Hors périmètre** : la projection partielle (formats 3/4, ACL) garde ses
  limites et sa découpe de 1 Mio. Elle relève du chantier « clone partiel ».

### Ne pas relire ce qu'on vient de hacher (comme git)

Avant ce changement, l'admission relisait et re-hachait tout le contenu admis :

- une fois pour revérifier le FileId de chaque manifeste ;
- une fois pour chaque morceau racine d'admission ;
- une fois pour vérifier que la liste de morceaux est admise.

À 1,5 Gio, `step` prenait 19 s pour un seul morceau modifié.

- **Manifeste prouvé** : un `FileManifest` construit par `FileManifestBuilder`
  dans ce processus porte une marque privée `proven`. Ce builder a haché le
  FileId et chaque ChunkId à partir des mêmes octets. La marque n'est jamais
  sérialisée et elle est ignorée par l'égalité. Un manifeste lu, reçu ou
  désérialisé n'est jamais prouvé.
- **Admission d'un manifeste prouvé** : son FileId n'est pas relu. La clôture
  exige toujours la présence et l'admission de chaque morceau.
- **Présence d'un morceau** (`ScmReader::chunk_present_metadata`) : elle est
  lue sur la ligne `objects` vérifiée. Les octets d'un morceau sont vérifiés à
  l'installation et à chaque lecture.
- **Spool** : les morceaux déjà admis à l'Espace en sont retirés avant le lot
  (les parties inchangées d'un fichier modifié). Ils ne sont ni relus, ni
  re-hachés, ni réinstallés.

### Sauvegardes de matérialisation purgées après succès

Une matérialisation déplace les originaux remplacés dans
`metadata/repair-backups/<lot>-attempt-N/`. Elle ne les supprimait jamais :
chaque fusion ou checkout d'un fichier de 5 Gio gardait donc 5 Gio de plus.

Chaque original est prouvé égal au contenu suivi, donc présent dans le CAS,
avant d'être déplacé. Une fois le journal terminé, le dossier de la tentative
est supprimé, au mieux et en parallèle. Une tentative non terminée garde ses
sauvegardes, comme avant.

### Hydratation VFS

Les requêtes CFAPI arrivent par plages de 256 Kio, alors que les morceaux font
1 à 16 Mio. Chaque plage relisait et re-hachait son morceau entier, soit 8 fois
trop pour des morceaux de 2 Mio.

- La source garde les 8 derniers morceaux vérifiés, au plus 64 Mio.
- Elle trouve le premier morceau d'une plage par recherche dichotomique.

## Résultats (même machine, 5 novembre 2026)

Fichier aléatoire de 1,5 Gio, édition de 64 Kio :

| Commande | git 2.45.1 | Layrs avant | Layrs après |
| --- | --- | --- | --- |
| import initial | 49,0 s | refusé | 11,0 s |
| status, rien de changé | 204 ms | refusé | 124-390 ms |
| status, 1 changement | 6,9 s | refusé | 3,1 s |
| commit d'une édition | 47,2 s | refusé | 3,4 s |
| Store après 2 versions | 3,0 Gio | — | 1,5 Gio |
| lecture complète dans une Room virtuelle | — | 36,6 s (1) | 11,5 s |

(1) Mesuré avec les plafonds levés, avant le cache de morceaux.

Le parcours complet passe à 1,5 Gio et à 5 Gio, avec un contenu vérifié par
SHA-1 à chaque étape : `init`, `status`, `step`, Room virtuelle (lecture,
step, fusion), Room physique, `drop` et `diff`. Le Store fait 5,0 Gio après la
fusion à 5 Gio. Script : `bigfile_e2e.sh`.

## Risques et suites

- **Serveur** : la vérification d'un fichier coûte sa taille logique. Les
  quotas d'envoi bornent les octets uniques, pas l'amplification d'un
  manifeste qui répète un morceau. Il faudra un budget d'amplification.
- **Mémoire de capture** : jusqu'à 16 Mio par morceau, multipliés par les
  4 workers et le canal, soit environ 200 Mio au pire.
- **Spool sur TEMP** : il est encore dans le dossier temporaire, et non sur le
  volume du Store. Un fichier plus grand que l'espace libre de TEMP échoue.
