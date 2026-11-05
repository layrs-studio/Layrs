# GC hors ligne du CAS serveur

Le nettoyage Linux conserve tous les objets catalogués, toutes les réservations
et leases, même expirées ou libérées, et tous les objets du manifeste de backup.
Il ne réduit ni l'historique, ni les quotas SQL, ni les Operations locales.
Les fichiers de staging et noms inconnus restent conservés. S3 et les mutations
GC Windows/macOS ne sont pas pris en charge.

## Procédure opérateur

Arrêter les serveurs et tous les outils partageant le CAS, y compris les anciennes
versions. Conserver les credentials de maintenance dans la configuration habituelle,
hors des arguments. Créer une sauvegarde vérifiée dans un répertoire extérieur au
CAS, selon [BACKUPS.md](BACKUPS.md), puis exécuter :

```text
layrs-server gc-plan /backups/snapshot
layrs-server gc-apply PLAN_ID /backups/snapshot
```

Examiner le JSON du plan et son `planId` avant `gc-apply`. Le plan seul ne déplace
aucun objet. Il est conservé sous `.layrs-retention/PLAN_ID/plan.json`, avec son
digest, l'installation, l'identité filesystem, l'inventaire SQL et le digest du
backup. Seuls les noms CAS connus, absents de toutes les racines, âgés d'au moins
24 heures deviennent candidats. L'âge seul ne suffit jamais : l'exclusion réelle,
la vérification des racines et les empreintes des fichiers sont aussi obligatoires.

`gc-apply` déplace les candidats en quarantaine, sans écraser une destination.
Pour reprendre une interruption ou restituer les candidats :

```text
layrs-server gc-resume PLAN_ID /backups/snapshot
layrs-server gc-restore PLAN_ID /backups/snapshot
```

`gc-resume` reprend la phase inscrite dans le journal. Le marqueur
`.layrs-gc-active` interdit toute ouverture normale du CAS jusqu'à la fin réussie.
Ne jamais supprimer ce marqueur ni le fichier permanent de verrou maintenance.
Une incohérence conserve le blocage et les copies disponibles pour diagnostic.
Application et restitution exigent le même catalogue, les mêmes réservations et
le même backup que le plan ; ne pas reprendre le service entre ces opérations si
une restitution immédiate est nécessaire.

Une suppression définitive demande une commande séparée et au moins 24 heures
supplémentaires après la fin de la mise en quarantaine :

```text
layrs-server gc-purge PLAN_ID /backups/snapshot-courant
```

La purge relit les racines sous verrou et accepte une sauvegarde vérifiée plus
récente couvrant le catalogue actuel. Un candidat devenu protégé est remis à son
chemin CAS original si celui-ci est absent. Si une copie identique existe déjà,
la copie de quarantaine est conservée ; aucune collision n'est écrasée. Un objet
protégé absent des deux emplacements fait échouer même une reprise de purge.
Les autres candidats peuvent être supprimés définitivement. Le backup contient
le catalogue : il ne garantit pas la récupération des candidats non catalogués
purgeables. Aucune purge automatique n'est activée.

## Frontière de confiance et bornes

Le verrou exclusif est incompatible avec les ouvertures `FsServerCas`, streams,
backup et restore coopératifs. Les répertoires appartiennent à l'opérateur : un
outil externe privilégié ou une ancienne version ignorant le verrou invalide
cette exclusivité. L'opérateur ne doit pas déplacer le CAS ou son fichier verrou.
Le collecteur épingle les répertoires, refuse les liens, hardlinks et changements
d'inode, vérifie SHA256 et longueurs, et utilise `renameat2(RENAME_NOREPLACE)` et
`fsync`. Les tables catalogue/réservations restent verrouillées pendant la mutation.

Filesystems Linux acceptés : ext4, XFS, Btrfs, tmpfs et overlay. NFS, SMB et FUSE
sont refusés. Les bornes sont 1 000 candidats, 64 Mio par objet/fichier, 1 Gio de
contenu à vérifier, 100 000 objets catalogués/réservés et 2 Mio par document de
plan/manifeste. Profondeur relative maximale 8, chemin 1 024 octets. Dépasser une
borne fait refuser l'opération entière ; aucun résultat partiel ne vaut succès.
Un catalogue corrompu/quarantiné, un fichier manquant ou un backup incomplet bloque
également la collecte. Aucun parcours de membres aux droits partiels n'est utilisé.

## Preuves et limites

Le 20 septembre 2026, [la recette Linux](../../scripts/test-linux-gc.mjs) a exécuté
[le test réel PostgreSQL](../../crates/layrs-server/tests/native_gc.rs) en **2,65 s** :
15 interruptions injectées aux frontières durables, reprise, restitution, délai de
purge, réserve acquise après quarantaine, objet protégé manquant, plan altéré et
substitution symlink refusés. Trois candidats sont restitués, puis deux purgés et
un remis au CAS après nouvelle réserve. Tous les octets catalogués sont relus ;
Marker, Release, Views, Steps et réservations restent identiques après un vrai
`pg_dump`/`pg_restore`. Les délais utilisent une horloge de test explicite ; la CLI
utilise l'heure système, sans option de contournement.

Le script n'utilise qu'un PostgreSQL et des fichiers jetables, sans port publié,
avec sources en lecture seule. La gate CI exige un test réellement exécuté ; son
exécution distante n'a pas été observée ici. Aucun GC n'a été appliqué à des
données utilisateur. Collecte en ligne, expiration des réservations, compaction
et suppression d'historique nécessitent des contrats distincts.
