# 0025 — Rooms : espaces de travail légers, virtuels par défaut

Date : 4 novembre 2026. Statut : décision produit acquise, implémentation par
phases (voir plus bas). Remplace le terme « Worktree » dans le produit.

## Nom

**Room** est le nom produit d'un espace de travail sur une Layer. `loom` est un
alias facultatif de la commande, `worktree` reste accepté pour compatibilité. Le
code interne garde `Worktree` ; les clés JSON existantes (`worktree`,
`worktreeId`…) ne changent pas : ce sont des interfaces machine.

## Comportement retenu

Une Room doit être assez légère pour qu'on en ouvre une sans y penser, en
particulier une par agent :

- **Créer ou supprimer une Room est quasi instantané** et ne copie pas le projet.
- **Une Room a sa propre Layer**, dérivée de la Layer source à la Step courante.
  Ses changements y restent (Steps) jusqu'à leur **Weave** vers la Layer cible
  (par défaut la Layer source).
- **Une Room se présente comme un dossier complet** : l'agent lit, écrit, compile
  et teste normalement. Les outils ne voient jamais un dossier partiel.
- **Par défaut une Room est virtuelle (VFS)** : ses fichiers apparaissent à la
  lecture et sont copiés depuis le Store à la première écriture (copy-on-write
  par fichier). `--physical` crée à la place un dossier réel complet. Sur une
  plateforme sans VFS qualifié, la Room est physique et la commande le dit.
- **Emplacement par défaut hors de l'arbre du projet** : sous Windows
  `%LOCALAPPDATA%\Layrs\rooms\<Space>\<Room>` (hors OneDrive, chemin court, pas
  d'imbrication de racines de synchronisation) ; Linux
  `$XDG_DATA_HOME/layrs/rooms/…` ; macOS `~/Library/Application Support/Layrs/rooms/…`.
  Le registre des Rooms vit dans `.layrs/rooms/`. Les outils qui parcourent le
  projet principal ne voient donc jamais les Rooms.
- **Commandes** : `layrs room new NOM` (affiche le chemin), `room list`,
  `room path NOM`, `room merge NOM [--into LAYER]`, `room drop NOM [--force]`.
  Une Room ne change jamais de Layer : on en ouvre une autre.

## Invariants

- Les garanties de la ADR 0024 et des contrats de matérialisation restent
  entières : aucun octet servi ou admis sans correspondre à son identifiant de
  contenu ; aucune écriture utilisateur perdue ou déplacée sans preuve.
- Durabilité « comme git » pour les fichiers projetés depuis le Store (décision
  du 4 novembre 2026) : un fichier abîmé par une coupure apparaît comme une
  modification et se restaure depuis l'historique.
- `room drop` refuse une Room dont des changements ne sont ni capturés ni
  fusionnés, sauf `--force` qui capture d'abord (rien n'est jamais effacé sans
  avoir été conservé dans le Store).
- Le statut et la capture d'une Room virtuelle sont en O(changements) : un
  fichier jamais modifié reste identique à sa base sans être relu.

## Détection des changements (décision du 4 novembre 2026)

Une Room reconnaît un fichier inchangé **sans le relire**, avec un contrat
« comme git ». La preuve dépend du type d'entrée :

- **Placeholder virtuel** : l'état *in-sync* de CFAPI (politique `TRACK_ALL` :
  toute écriture de données ou de métadonnées l'efface), lu dans l'énumération
  du répertoire (tag de reparse, sans ouvrir le fichier), plus la taille du
  catalogue et le numéro de fichier NTFS du placeholder créé pour ce chemin. Le
  contenu est alors celui de la base, que le fichier soit hydraté ou non : un
  `grep` sur toute la Room ne rend pas le `step` suivant coûteux.
- **Fichier ordinaire ou placeholder modifié** : l'**index de Room** (un par
  Worktree, dans les métadonnées du Store). Chaque entrée associe un chemin à
  son FileId, sa taille, son bit exécutable, sa clé stat (taille, date de
  modification, date de création ; inode et ctime sous Unix) et l'instant où
  ce contenu a été prouvé. Le fichier est inchangé si la clé stat énumérée est
  identique et si sa date de modification précède cet instant d'au moins une
  marge. Sinon il est « racy » comme dans git et il est relu. La marge vaut
  100 ms sur NTFS/ReFS et 2 s ailleurs.
- **Tout le reste** est relu et haché comme aujourd'hui.

L'index est un cache :

- s'il est illisible, tout est relu ;
- il n'est écrit qu'après acceptation de la capture ;
- une entrée ne naît que d'un contenu prouvé : soit haché par la capture, soit
  écrit par la matérialisation pendant qu'elle tient le fichier en écriture
  exclusive.

Limites assumées, comme git :

- un programme qui garde un fichier ouvert après l'avoir écrit, car NTFS ne met
  à jour l'entrée de répertoire qu'à la fermeture : `room step` exige déjà que
  les écrivains soient arrêtés ;
- un outil qui réécrit un fichier en conservant exactement sa taille et ses
  dates ;
- les liens physiques vers des fichiers ordinaires.

Garde-fous :

- `room drop` relit d'abord tout le contenu local, sans confiance dans
  l'index : chaque fichier ordinaire ou placeholder modifié est haché ;
- chaque placeholder énuméré in-sync est réinspecté en direct par un handle
  d'attributs : état in-sync natif, octets modifiés nuls, identité et numéro
  de fichier. Ce handle n'accède pas aux données, donc rien n'est hydraté ni
  scanné. Un écrivain resté ouvert, invisible à l'énumération, est ainsi
  détecté ;
- s'il reste un changement, il est capturé et le `drop` refusé (sauf
  `--force`, qui le conserve sur la Layer de la Room) ;
- tout écrasement par une matérialisation revérifie le contenu des chemins
  touchés ;
- un placeholder jamais hydraté n'a pas d'octets locaux à perdre ;
- le dossier supprimé est d'abord renommé à côté (instantané), puis effacé par
  un processus détaché. Si un handle ouvert empêche ce renommage, il est
  supprimé sur place.

Capture virtuelle, comme la capture ordinaire :

- `.layrsignore` est appliqué, et un chemin ignoré n'est ni lu ni capturé,
  sauf s'il est déjà suivi ;
- les fichiers hachés passent en streaming par blocs de 1 Mio dans un spool
  borné : 1 Gio par fichier, 4 Gio au total, sans tout charger en mémoire ;
- une hydratation attend un écrivain exclusif du Store, comme une ouverture
  partagée ; elle ne prend que le verrou partagé et lit chaque manifest à la
  première hydratation du fichier.

Conséquences :

- **Renommer ou déplacer un fichier dans une Room virtuelle est accepté**,
  tant que la cible reste dans la Room ; sortir un placeholder de la Room
  reste refusé. La capture relit, en l'hydratant si besoin, le fichier à son
  nouveau chemin et marque l'ancien supprimé.
- **Le Weave ne lit que les fichiers modifiés des deux côtés**. L'admission
  d'une Step garantit déjà que toute sa clôture est présente.

Le dossier principal est un Worktree comme les autres et garde lui aussi un
index (décision du 4 novembre 2026). Un dossier existant sans index en reçoit
un vide, que sa capture suivante remplit. `layrs step --full` et
`layrs status --full` relisent tout sans confiance dans l'index, après un outil
qui réécrit des fichiers en conservant leur taille et leurs dates.

## Découpage

1. **Surface produit** sur l'existant : `room new/list/path/merge/drop`,
   emplacement par défaut, VFS par défaut (création VFS actuelle) ou
   `--physical`, retrait propre d'une Room (nouvelle mutation du Store).
2. **Room virtuelle instantanée** : population à la demande (aujourd'hui la
   création VFS crée tous les placeholders, ~12 s à 10 000 fichiers), cycle de
   vie de l'hôte VFS automatique (l'agent ne gère ni ticket ni processus),
   capture en O(changements) depuis l'état des placeholders.
3. **Autres plateformes** : Linux (FUSE ou overlay avec copie à la première
   écriture), macOS (File Provider ou FSKit). Rien n'est annoncé supporté avant
   qualification.

Mesures de référence (10 000 fichiers, Windows, Defender actif, r215) : Room
physique 11 s, création VFS actuelle 12 s ; 1 000 fichiers : Room physique 1,4 s.
