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
- **Par défaut une Room est physique** (conseil s10, 2026-11-09 ; avant :
  virtuelle) : un dossier réel complet, lu comme n'importe quel dossier.
  `--virtual` crée une Room virtuelle (VFS) : ses fichiers apparaissent à la
  lecture, remplis depuis le Store. Chaque ouverture d'un placeholder in-sync
  coûte ~0,04 ms de plus qu'un fichier ordinaire (~0,4 s par lecture de
  10 000 fichiers, à chaque lecture), ce que la règle « plus rapide que git
  sur chaque commande » ne permet pas par défaut. Sur une plateforme sans VFS
  qualifié, `--virtual` donne une Room physique et la commande le dit. Les
  Rooms existantes gardent leur type.
- **Emplacement par défaut hors de l'arbre du projet** : sous Windows
  `%LOCALAPPDATA%\Layrs\rooms\<Space>\<Room>` (hors OneDrive, chemin court, pas
  d'imbrication de racines de synchronisation) ; Linux
  `$XDG_DATA_HOME/layrs/rooms/…` ; macOS `~/Library/Application Support/Layrs/rooms/…`.
  Le registre des Rooms vit dans `.layrs/rooms/`. Les outils qui parcourent le
  projet principal ne voient donc jamais les Rooms.
- **Commandes** : `layrs room new NOM [--virtual]` (affiche le chemin), `room list`,
  `room path NOM`, `room merge NOM [--into LAYER] [--preview] [--capture]`,
  `room drop NOM [--force] [--full]`.
  Une Room ne change jamais de Layer : on en ouvre une autre.

## Invariants

- Les garanties de la ADR 0024 et des contrats de matérialisation restent
  entières : aucun octet servi ou admis sans correspondre à son identifiant de
  contenu ; aucune écriture utilisateur perdue ou déplacée sans preuve.
- Durabilité « comme git » pour les fichiers projetés depuis le Store (décision
  du 4 novembre 2026) : un fichier abîmé par une coupure apparaît comme une
  modification et se restaure depuis l'historique.
- `room drop` refuse une Room dont des changements ne sont ni capturés ni
  fusionnés, sauf `--force` qui capture d'abord : rien de ce que la capture
  voit n'est effacé sans avoir été conservé dans le Store. Une réécriture qui
  garde exactement taille et dates n'est vue, comme dans git, que par
  `room drop --full` (voir « Garde-fous » et « Limites assumées »).
- `room merge` fusionne la dernière Step capturée de la Room, lue dans
  l'Opération active de son Worktree, sans lire ses fichiers (conseil s13 Q4,
  2026-11-12, comme git fusionne le dernier commit d'une branche) : une
  modification de la Room qui n'a pas été capturée reste dans la Room et la
  sortie le rappelle ; `room merge --capture` capture d'abord la Room (le
  comportement d'avant). `room drop` continue de protéger ce qui n'est ni
  capturé ni fusionné.
- `room merge` ne capture jamais le dossier du projet (conseil s10,
  2026-11-09, comme la fusion de git) : une modification de travail sur un
  chemin que la fusion n'écrit pas est conservée telle quelle, sans être
  enregistrée ; un chemin que la fusion écrit doit encore avoir son contenu de
  base, prouvé juste avant son remplacement, sinon la fusion refuse avant
  toute écriture (de même pour un dossier remplacé par un fichier qui contient
  autre chose que du contenu suivi supprimé par la fusion). `room merge
  --preview` n'exige plus un dossier propre. `layrs weave` garde sa capture
  préalable.
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

- il est rangé, avec le cache de projections, dans une base SQLite de caches
  dérivés (`metadata/derived.sqlite3`). Une commande relit ce que la
  précédente vient d'écrire, et sur une machine dont l'antivirus analyse en
  temps réel, ouvrir un fichier de données fraîchement écrit coûte ~5 ms de
  plus qu'une page de cette base (mesuré en novembre 2026). Contrat de cette
  base (conseil s12) :
  - chaque valeur garde son encodage, sa liaison et sa somme de contrôle ; la
    base n'est qu'un conteneur ;
  - écritures sans synchronisation, journal en mémoire : ce cache ne crée
    jamais d'autre fichier que la base. Toute connexion passe par un VFS qui
    n'ouvre que le fichier de base (conseil s13 Q7) : un journal, un `-wal`
    ou un `-shm` posé à côté (planté, ou d'un autre programme) n'est jamais
    vu, rejoué, ouvert ni supprimé. Les lecteurs ouvrent en lecture seule une
    base en mode retour arrière, vérifient le schéma puis lisent dans une
    même transaction ;
  - un écrivain n'écrit jamais dans un fichier de base à plusieurs liens : le
    VFS refuse toute ouverture en écriture d'un fichier lié ailleurs ou point
    de réanalyse, vérifié sur le handle que SQLite a ouvert (Windows), et
    l'écrivain reprend le nom (le retire puis crée une base neuve) à un
    fichier lié ailleurs (sauvegarde par liens physiques, ou lien planté),
    étranger ou abîmé ; une base vide est initialisée sur place ; une base
    occupée, ou d'une version plus récente de ce cache, est un défaut de
    cache et reste en place ;
  - tout accès vérifie le schéma exact (marqueur et version d'abord, puis au
    plus les quatre objets attendus, de longueur bornée) puis n'exécute que
    des requêtes fixes (`trusted_schema=OFF`, mode défensif) ; une valeur est
    lue par la requête qui vérifie son type et sa taille ;
  - une base ou une valeur abîmée (lue par un lecteur ou un écrivain) est lue
    vide et remplacée à la prochaine écriture ; un index illisible parce que
    la base est occupée ou plus récente est lu vide et n'est jamais réécrit
    par-dessus ;
  - la base du Store suit la même règle (conseil s13 Q7) : ses écrivains
    passent par le même VFS, qui n'ouvre que la base et son `-wal` (chacun à
    un seul lien), ne voit jamais de journal de retour arrière, et prennent
    `locking_mode=EXCLUSIVE` avant leur première lecture (index du WAL en
    mémoire, aucun `-shm`) ; ses lecteurs l'ouvrent `immutable=1` et ne
    regardent aucun fichier annexe. Ils ouvrent toujours avec
    `SQLITE_OPEN_NOFOLLOW` (SQLite refuse un chemin dont la résolution suit
    un lien), et le VFS refuse un nom qui est un lien *avant* que SQLite ne
    l'ouvre (sous Unix, SQLite change le mode d'un `-wal` vide qu'il ouvre ;
    sous Windows, il crée la cible absente d'un `-wal` lié) (conseil s15
    r1). Une base ou un `-wal` vide qui partage son fichier avec un autre
    lien (sauvegarde par liens physiques, `cp -al`) reçoit d'abord un
    fichier à lui : la base est copiée, synchronisée et renommée sur son nom,
    le nom du `-wal` vide est retiré ; l'autre lien garde ses octets, comme
    avec git. Un `-wal` lié qui contient des trames est refusé. Risques
    acceptés : sous Unix, SQLite ne donne pas son descripteur, le nom est
    revérifié après l'ouverture ; un nom remplacé entre les contrôles et
    l'ouverture reste possible. Un `-wal` planté *avec des trames valides*
    est replié dans la base par le prochain écrivain, comme le ferait
    SQLite : un processus qui peut écrire dans `metadata/` peut aussi
    remplacer la base elle-même, ce n'est donc pas une élévation. La base
    candidate d'un compactage (`VACUUM INTO`, maintenance) reste hors de ce
    VFS ;
- s'il est illisible, tout est relu ;
- il n'est écrit qu'après acceptation de la capture ;
- une entrée ne naît que d'un contenu prouvé : soit haché par la capture, soit
  écrit par la matérialisation pendant qu'elle tient le fichier en écriture
  exclusive. Une création à neuf (Room physique) tient chaque fichier écrit
  jusqu'à ce que la marge de dates du volume soit passée, puis compare
  l'énumération aux métadonnées prises par le handle d'écriture : la preuve,
  datée de ce moment, n'est pas « racy » (conseil s11).

Limites assumées, comme git :

- un programme qui garde un fichier ouvert après l'avoir écrit, car NTFS ne met
  à jour l'entrée de répertoire qu'à la fermeture : `room step` exige déjà que
  les écrivains soient arrêtés ;
- un outil qui réécrit un fichier en conservant exactement sa taille et ses
  dates ;
- les liens physiques vers des fichiers ordinaires.

Garde-fous :

- `room drop` capture d'abord tout ce qu'il voit, avec les passes de la
  vérification complète (une passe qui retrouve exactement la base suffit,
  sinon deux passes égales). Une
  Room physique fait confiance à son index de changements comme git à son
  index (conseil s10, 2026-11-09) : une entrée prouvée dont les métadonnées
  sont identiques et qui n'est pas « racy » n'est pas relue ; les autres le
  sont. Une réécriture qui garde exactement taille et dates n'est donc vue,
  comme dans git, que par `room drop --full`, qui relit et hache tout le
  contenu local sans confiance dans l'index. Une Room virtuelle hache chaque
  placeholder modifié ;
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
