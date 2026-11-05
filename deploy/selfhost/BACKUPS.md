# Sauvegardes planifiées et alertes

Cette recette concerne le serveur natif avec **CAS filesystem**. Le démarrage normal ne propose pas de backend S3 ; les composants S3 de la bibliothèque ne constituent pas une installation ni une restauration S3 supportée. Ne pas appliquer ce script à un bucket en supposant sa cohérence garantie.

Le profil Compose `backup` utilise le même binaire que le serveur, le CAS en lecture seule et les credentials de maintenance injectés par fichiers secrets. `scheduled-backup.sh` crée une destination neuve, lance `backup` puis `verify-backup`, garde les logs privés et publie un statut seulement après vérification. Un verrou `flock` refuse les exécutions simultanées. Un échec conserve les éléments partiels pour diagnostic et retourne un code non nul ; il ne met pas à jour la dernière réussite.

## Préparer et essayer

Sur l'hôte Linux, créer un dossier de sauvegarde privé possédé par UID/GID 10001, mode 0700, avec assez d'espace. Définir `LAYRS_BACKUP_PATH` avec son chemin absolu dans le `.env` administrateur de Compose. Ce dossier contient données et empreintes d'identité ; restreindre son accès et utiliser une copie chiffrée hors de la machine. Ne pas publier le répertoire par le proxy Web.

```text
docker compose --profile backup run --rm --no-deps backup
docker compose --profile backup run --rm --no-deps --entrypoint cat backup /backups/last-backup.json
```

Prérequis : instance déjà initialisée, PostgreSQL accessible, transferts en cours terminés ou arrêtés. Un upload loué incomplet fait refuser le snapshot ; résoudre la cause et relancer dans une nouvelle destination. Aucun fichier préexistant n'est supprimé. Le script conserve **toutes les sauvegardes**, y compris les tentatives en échec : suivre l'espace disque et appliquer une politique de rétention administrée uniquement après restauration vérifiée. Il n'installe aucun scheduler et ne désactive aucun accès utilisateur.

Pour un serveur hors Compose, les mêmes commandes fonctionnent en exécutant le script avec `LAYRS_BACKUP_ROOT`, `LAYRS_SERVER_EXECUTABLE` absolus et la configuration privée habituelle. Les utilitaires PostgreSQL compatibles doivent être dans PATH ; le dossier CAS ne doit jamais être placé sous la destination de sauvegarde.

## Planifier explicitement

Les exemples `layrs-backup.service` et `.timer` ciblent `/opt/layrs/deploy/selfhost` et Docker système. Adapter les chemins, vérifier le premier exercice manuel, puis installer ces fichiers dans `/etc/systemd/system` avec l'administrateur de l'hôte. `systemctl daemon-reload` puis `systemctl enable --now layrs-backup.timer` active l'exemple quotidien à 02 h locale avec un décalage de 15 minutes. `Persistent=true` rattrape une échéance manquée au démarrage. Ces commandes sont à exécuter par l'exploitant : aucune programmation n'est effectuée par le dépôt ou ses tests.

Contrôler `systemctl list-timers layrs-backup.timer` et `journalctl -u layrs-backup.service`. Une erreur doit déclencher l'outil d'alerte déjà utilisé par l'équipe. `backup.prom` expose uniquement réussite de la dernière tentative, date Unix de la dernière réussite et durée ; le collecteur peut lire ce fichier via un accès dédié. Ne jamais exposer les dumps, manifests ou logs privés pour collecter trois métriques.

Pour une fréquence quotidienne, alerter sur `layrs_backup_last_attempt_success == 0` et sur une dernière réussite datant de plus de 26 h, ainsi que sur l'absence du fichier de métriques. Cette marge est un exemple de surveillance ; elle ne garantit pas un RPO. Le diagnostic d'espace et les quotas sont détaillés dans [STORAGE-BUDGET.md](STORAGE-BUDGET.md).

## Prouver la restauration

Une sauvegarde vérifiée n'est pas à elle seule un exercice de reprise. Restaurer périodiquement dans une base et un CAS neufs hors trafic selon [README.md](README.md), contrôler comptes/références/fichiers/preuves puis mesurer le temps de remise en service. Conserver date, versions, taille, durée et résultats. Après chaque modification du stockage ou du schéma, refaire cet exercice avant d'annoncer un RPO/RTO. Le pilote local automatisé valide la mécanique sur un petit projet ; le volume réel de l'entreprise et le pilote utilisateur restent à qualifier.

## Mesure locale du 20 septembre 2026

La recette HTTPS/PostgreSQL avec conflits texte/code par blocs, clients concurrents,
Choose et vrai runner Docker a passé en **263,44 s**. Son exercice de sauvegarde
porte sur **563 objets, 16 679 896 octets canoniques** (16,68 Mo / 15,91 Mio),
33 Steps et 6 événements Release. Les Views/générations, Steps et événements
restaurés correspondent à la source au repos.

| Phase instrumentée | Durée observée |
| --- | ---: |
| `create_backup`, dump et copie CAS vérifiée inclus | 8 479 ms |
| `restore_backup`, vérification, installation CAS et pg_restore inclus | 8 924 ms |
| Exercice entier avec essais négatifs et contrôles | 20 517 ms |
| Du watermark PostgreSQL à la fin de copie CAS, timestamps à la seconde | 8 s |

Le **point de reprise est le snapshot PostgreSQL exporté**, identifié par
`database_watermark_unix`. La copie CAS suffisamment récente rend sa fermeture
disponible ; les 8 secondes de copie **ne sont pas un RPO**. Au moment d'un
incident, la perte potentielle couvre les écritures postérieures au snapshot de
la dernière sauvegarde entièrement vérifiée. Elle dépend donc de l'âge de ce
snapshot, de la fréquence de sauvegarde et des éventuels échecs successifs.
L'exemple quotidien ne promet pas un délai maximal de perte.

Les 8,924 secondes mesurent la restauration mécanique sur cette configuration
locale. Elles excluent détection de panne, intervention, préparation d'un nouvel
hôte, récupération d'une copie externe, démarrage/readiness et bascule du trafic :
elles ne constituent pas un RTO de service ni un SLA d'entreprise. Aucun objectif
RPO/RTO de production n'est annoncé. Refaire la mesure avec le volume, le matériel,
la copie externe et le scénario d'incident réellement prévus avant d'en fixer un.

[Mesure structurée et périmètre exact](../../docs/plans/pme/measurements/T8-backup-restore-2026-09-20.json).
