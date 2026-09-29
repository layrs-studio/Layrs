# Compacter les métadonnées locales sans perdre l'historique

Depuis un Worktree natif, fermer les autres commandes utilisant le même Store,
puis exécuter `layrs compact`. Il s'agit d'une maintenance SQLite physique :
toutes les Operations, Views, observations, références, admissions et données de
reprise terminées sont conservées. Aucun objet CAS, fichier de travail, ancien
Step ou contenu utilisateur n'est supprimé. L'opération peut donc ne rien gagner
si la base contient peu de pages libres ; elle ne réduit pas la taille du CAS.

Le rapport JSON contient les tailles SQLite avant/après, le nombre de lignes
conservées, leur empreinte logique et `backupDirectory`. Ce dossier conserve la
base originale, le journal et le reçu de fin. Aucune suppression automatique de
ces copies n'est activée. Prévoir l'espace nécessaire à la base originale, sa
copie et le candidat compacté : la réduction du fichier actif ne signifie pas
une réduction immédiate de tout l'espace du Store, puisque le backup reste là.

La commande refuse un Store inconnu/ancien, un lecteur ou écrivain actif sur
n'importe quel Worktree du Store, une reprise incomplète, une lease de
matérialisation non libérée, une admission/étape/référence pendante ou un WAL non
checkpointé. Une reprise défaillante n'est pas effacée pour permettre la compaction.

## Interruption et reprise

`VACUUM INTO` prépare une base indépendante depuis l'original ouvert en mode
immutable. Avant toute publication, le moteur vérifie schéma, identité Store,
intégrité SQLite, clés étrangères et empreinte de toutes les lignes typées. Le
backup est synchronisé sur disque, puis le journal
`metadata/metadata-compaction.json` est publié durablement. Ce journal interdit
les ouvertures ordinaires du Store jusqu'à la fin ou la reprise explicite :

```text
layrs compact --resume
```

La reprise reconnaît l'original ou le candidat déjà publié par leur empreinte
physique, vérifie à nouveau backup et contenu logique, puis termine la publication
atomique et le reçu. Un journal, backup, candidat ou fichier actif altéré est
refusé ; les copies sont conservées. Ne jamais retirer le journal ou remplacer
manuellement le fichier verrou pour débloquer l'application. Une interruption
avant publication du journal laisse l'original intact et des copies de diagnostic,
sans empêcher une nouvelle tentative.

Le fichier verrou et les répertoires appartiennent au propriétaire du Store.
Toutes les versions/outils qui accèdent au SQLite doivent respecter ce verrou.
Un éditeur SQL externe ou une ancienne version ignorant le protocole ne doit pas
être utilisé pendant cette maintenance. Les composants symlink/reparse sont
refusés par les ouvertures sécurisées existantes ; aucun chemin issu d'un rapport
ne devient une commande de suppression.

## Limites et pagination

Cette première version borne SQLite à 1 Gio, le contenu à un million de lignes,
chaque cellule texte/blob à 16 Mio et le journal à 16 Kio. Toute incohérence ou
borne dépassée fait échouer l'opération, sans publication du candidat.
L'empreinte logique couvre les noms de tables et toutes les colonnes/valeurs
typées, indépendamment de l'ordre physique des lignes.

Les `rowid` SQLite ne sont pas des identifiants métier. Le rapport recommande de
recommencer les lectures paginées d'historique après maintenance. Les anciens
snapshots sont vérifiés : soit ils restent exactement valides, soit ils sont
refusés avec une demande de nouvelle lecture. Aucun changement de frontière
n'est accepté silencieusement. Le SQLite qualifié a conservé les rowids de la
fixture ; un scénario séparé simule leur renumérotation pour vérifier le refus.

## Qualification

Tests [store-local](../../crates/layrs-store-local/tests/scm_compaction.rs) : réduction
réelle de **2 715 648 à 614 400 octets** sur une fixture contenant des pages libres,
backup original identique, verrou interprocessus, quatre frontières d'interruption,
reprise et refus de copies/journal altérés. Le helper enfant ignoré par défaut est
effectivement exécuté par le test du verrou ; ce n'est pas une couverture sautée.

Test [natif](../../crates/layrs-client-core/tests/native_compaction.rs) : Operations et
View inchangées après réouverture, Marker masqué et Release toujours attachés à
leurs anciens Steps lisibles, modifications de travail non capturées conservées,
observations identiques, reprise pendante refusée et fsck propre.

Windows : cinq tests store PASS1,81s, deux tests client PASS1,60s, Clippy strict
PASS3,90s. Linux : cinq tests store PASS1,72s, deux tests client PASS1,07s, mêmes
tailles SQLite et vérifications d'intégrité après reprise. Le raccordement CLI est
qualifié séparément par root ; ces preuves couvrent l'API native de maintenance.
Ce lot est postérieur aux packages `qualified2`, qui restent des snapshots de leur
propre recette ; reconstruire les artefacts pour distribuer cette commande.
