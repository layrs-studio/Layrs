# Instance native mono-serveur — intégration en cours

Ce dossier compose PostgreSQL, le CAS filesystem persistant, le serveur V3 réel et Studio Web derrière Caddy HTTPS, sur la même origine. Le bundle Web fournit connexion, consultation des références publiées et revues ; le flux d'autorisation d'appareil possède sa page `/auth/device`. L'assemblage a passé une [recette locale HTTPS complète](LOCAL-PILOT.md), avec bootstrap, provisioning, restart et révocation. Cette preuve sur images identifiées ne constitue pas encore une distribution PME qualifiée ni un pilote client.

## Installation neuve

Prévoir un hôte Linux avec Docker Compose, un nom DNS public pointant vers lui et les ports 80/443 accessibles. Les identifiants `deployment:` et `store:` doivent rester stables après installation. La baseline refuse les anciennes bases Layrs V1/V2.

1. Depuis ce dossier, exécuter `python generate-secrets.py`. Six secrets aléatoires sont créés sans être affichés ; aucun fichier existant n'est remplacé. Sur Linux, le dossier privé `0700` protège les fichiers, lisibles par le UID 10001 dans les montages Compose. Sous Windows, restreindre aussi les ACL du dossier `secrets` aux administrateurs autorisés.
2. Créer `.env` avec `LAYRS_HOST=layrs.example.com`, `LAYRS_DEPLOYMENT_ID=deployment:entreprise`, `LAYRS_STORE_ID=store:principal` et `LAYRS_IMAGE_TAG=local`.
3. Exécuter `docker compose up --build -d`. Le job `initialize` utilise le compte de maintenance, installe les migrations checksummées et accorde le rôle writer au compte runtime non privilégié. Le serveur refuse les comptes PostgreSQL superutilisateurs.
4. Vérifier `docker compose ps`, puis `https://VOTRE_HOTE/readyz`. PostgreSQL et CAS doivent être disponibles. `/healthz` mesure seulement la vie du processus.
5. Envoyer une seule fois `POST /v3/auth/bootstrap` avec un JSON `{"secret":"SECRET_DU_FICHIER","login":"admin@example.com","password":"MOT_DE_PASSE_LONG_UNIQUE","display_name":"Administrateur"}`. Utiliser un fichier temporaire protégé ou un client HTTP qui ne conserve pas les secrets dans l'historique. La réponse contient un Bearer valable 8 h ; garder le secret hors des logs.

Le bootstrap consomme atomiquement son empreinte. Deux demandes concurrentes ne peuvent pas créer deux premiers administrateurs. Redémarrer `initialize` ne réarme pas le bootstrap. Le secret du fichier n'est utilisé que pour préparer la première installation ; l'empreinte consommée persiste en base.

Après bootstrap, ouvrir `https://VOTRE_HOTE/` et se connecter. Studio garde le Bearer uniquement en mémoire ; recharger la page demande une nouvelle connexion. Les identifiants de projet proviennent de `layrs init` / `layrs remote provision` ou du Desktop. Le proxy réserve `/v3/*` et `/auth/device` au backend ; les fichiers Web sont servis sur la même origine et les corps privés ne sont pas mis en cache. `Web.Dockerfile` compile le monorepo avec le lockfile pnpm, puis copie seulement les assets dans l'image Caddy.

## Authentification et récupération

- `POST /v3/auth/login` : `login` et `password`, réponse Bearer. Les API natives consomment `Authorization: Bearer TOKEN` ; les cookies ne sont pas acceptés.
- `POST /v3/auth/logout` : révoque le Bearer envoyé. Expiration normale après 8 h.
- `POST /v3/auth/password` : Bearer, `current_password` et `new_password` ; révoque toutes les générations de tokens précédentes.
- Récupération hors ligne : arrêter le trafic, utiliser les credentials PostgreSQL de maintenance et lancer `layrs-server recover-password LOGIN PASSWORD_FILE`. Le fichier contient le nouveau mot de passe ; il ne figure pas dans les arguments. Cette opération révoque les anciens tokens. Aucun endpoint anonyme de récupération n'est exposé.

L'administrateur peut créer des comptes (`POST /v3/auth/accounts` : `login`, `password`, `displayName`) et des comptes de service (`POST /v3/auth/service-accounts` : `displayName`, `label`, `expiresIn`, entre 1 h et 90 jours). Les services ne reçoivent aucun droit Workspace implicite. La rotation (`/v3/auth/service-accounts/rotate`) invalide leurs anciens tokens. Les tokens d'appareil durent 30 jours ; les autres sessions, 8 h. La création d'un Workspace/Space passe par `POST /v3/workspaces/provision` (`workspaceSlug`, `spaceSlug`, `name`). Les UI complètes d'administration et de revue restent à raccorder.

## Exploitation et mises à jour

Les volumes `postgres`, `cas` et `caddy_data` sont persistants. Ne pas utiliser `docker compose down -v` pour une mise à jour. Les secrets restent hors de l'image ; sauvegarder leur configuration sous chiffrement. La rotation de `signing_key` invalide les capacités de transfert et curseurs signés en cours, pas les sessions en base.

Le journal HTTP contient identifiant de requête généré côté serveur, statut et latence. Il exclut URL, query string, headers et corps. La readiness effectue une requête DB et une écriture synchronisée temporaire dans le CAS. Les limites actuelles CAS sont de 64 MiB par objet ; cela ne constitue pas un quota global.

## Sauvegarde et restauration

Les quotas CAS logiques se configurent par `LAYRS_CAS_MAX_TOTAL_BYTES`,
`LAYRS_CAS_MAX_TOTAL_OBJECTS`, `LAYRS_CAS_MAX_SPACE_BYTES` et
`LAYRS_CAS_MAX_SPACE_OBJECTS`. Chaque valeur présente est un entier de 1 à
9 223 372 036 854 775 807 ; une variable absente désactive ce plafond. Configurer
les mêmes valeurs sur toutes les instances servant la même base.

Le serveur réserve le budget avant de remettre une capacité d'envoi, sous verrou
PostgreSQL commun aux admissions. Il compte chaque ObjectId une fois globalement,
et une fois par Space qui le réserve ou le possède. La charge d'un objet est le
maximum de sa longueur canonique et encodée. Les reprises exactes ne doublent pas
la charge ; elles restent permises après réduction d'un plafond. Les créations
d'objets serveur sont aussi contrôlées avant admission SQL.

`layrs-server storage-budget [SPACE_ID]` utilise les credentials PostgreSQL locaux
de maintenance et affiche compteurs, plafonds et taille actuelle de la base.
Aucune route HTTP publique de métriques globales n'est ajoutée. Les compteurs
séparent objets catalogués et réservations sans objet catalogué. Les réservations
expirées restent comptées : elles peuvent correspondre à des objets CAS durables
dont l'admission SQL a échoué. Ne pas supprimer des lignes pour récupérer du quota.

Ces quotas ne sont pas des limites physiques de disque ou de PostgreSQL : fichiers
temporaires concurrents, objets générés avant un rejet, index/WAL et sauvegardes
occupent aussi de l'espace. La mesure PostgreSQL est indicative et ne limite pas sa
croissance. Superviser les volumes au niveau système ; le GC serveur reste à livrer.
Les [journaux corrélés et métriques HTTP](OBSERVABILITY.md) sont disponibles sans infrastructure supplémentaire, avec lecture des compteurs réservée à l'administrateur.

Les rapports et seuils de [capacité](STORAGE-BUDGET.md) sont disponibles via CLI/API ;
l'exploitant choisit la périodicité et le routage des alertes. Les quotas sont désactivés par défaut et doivent
être choisis explicitement pour l'installation.

Les commandes suivantes utilisent les credentials PostgreSQL de maintenance. `pg_dump` et `pg_restore`, de version compatible avec le serveur PostgreSQL, sont requis ; ils sont présents dans l'image serveur. Le dossier de sauvegarde contient des données privées et les empreintes d'authentification : conserver son accès restreint et l'archiver sous chiffrement.

```text
layrs-server backup /sauvegardes/nouvelle-sauvegarde
layrs-server verify-backup /sauvegardes/nouvelle-sauvegarde
layrs-server restore /sauvegardes/nouvelle-sauvegarde
```

`backup` crée uniquement un dossier neuf. Il maintient un snapshot PostgreSQL exporté pendant `pg_dump`, fige les racines/catalogues concernés, conserve tous les objets CAS catalogués puis écrit le manifeste en dernier. Le snapshot CAS termine après le watermark DB. Un upload encore loué mais incomplet ou un objet en quarantaine fait refuser la sauvegarde ; arrêter/terminer les transferts puis réessayer dans un nouveau dossier. Le dump contient les leases existantes. Le serveur ne réalise actuellement aucun GC : toute l'histoire et tous les objets catalogués sont retenus.

`verify-backup` contrôle baseline, checksum du dump, identités/longueurs/digests CAS, fermeture Steps/Trees/Views/Policies et reconstruction des FileIds. Un fichier manquant, une corruption, un manifeste incomplet ou un watermark CAS antérieur sont refusés.

Avant `restore`, configurer **une base PostgreSQL vide et un nouveau chemin CAS** ; conserver les mêmes `LAYRS_DEPLOYMENT_ID` et `LAYRS_STORE_ID`. Cette cible doit rester hors trafic. La commande refuse de remplacer une base ou un CAS existant ; elle vérifie la sauvegarde, charge les objets puis le dump, compare le catalogue restauré au manifeste et vérifie la baseline. Un marqueur durable maintient le serveur non servant en cas d'échec. Ne pas enlever ce marqueur pour contourner une erreur. Après succès, accorder le rôle runtime via `LAYRS_RUNTIME_ROLE`, remettre les secrets adéquats puis lancer `serve`. Le chemin CAS peut changer lors d'une restauration ; sa configuration persistée est mise à jour après vérification.

Premier exercice local réel du 20 septembre 2026 : 24 objets, 3 Steps, sauvegarde/restauration en 2 111 ms, fichiers et pointeurs d'histoire identiques ; scénarios de corruption/refus vérifiés. Ce petit fixture ne constitue ni un benchmark PME, ni un engagement RPO/RTO. Les recettes plus récentes et leurs éventuels blocages figurent dans le [journal T8](../../docs/plans/pme/journal/T8.md).

Le [runbook de sauvegardes](BACKUPS.md) fournit le profil Compose, le script vérifié et des exemples de timer ; aucun timer n'est installé automatiquement. Les quotas globaux/par Space et les [alertes stockage](STORAGE-BUDGET.md) sont disponibles, leurs variables optionnelles sont transmises par Compose lorsqu'elles sont définies dans l'environnement. S3 reste non supporté dans le serveur normal.

## Vérification PostgreSQL explicite

Dans une instance de test jetable autorisant CREATE DATABASE et CREATE ROLE, configurer `LAYRS_TEST_DATABASE_URL` puis exécuter depuis la racine :

```text
cargo test -p layrs-server --test native_identity -- --ignored --nocapture
```

Le test crée une base au nom UUID, exerce bootstrap concurrent, révocation, expiration et récupération, puis supprime uniquement sa base et son rôle générés. Sans PostgreSQL, il est ignoré explicitement ; sa compilation seule ne valide pas le comportement SQL.
