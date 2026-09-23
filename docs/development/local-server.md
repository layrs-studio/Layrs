# Tester Layrs en local

Prérequis : Node.js 22 ou plus récent, Docker avec un moteur Linux local,
Compose 2.24.4 ou plus récent et Buildx. Sur Windows, démarrer Docker Desktop
en mode conteneurs Linux. Exécuter les commandes depuis la racine du dépôt.

```powershell
cd D:\Layrs\layrs
npm run dev
```

La commande construit le serveur actuel et Studio Web, démarre PostgreSQL,
applique les migrations et crée un administrateur de test. Le premier démarrage
peut prendre plusieurs minutes. Rust, pnpm et les dépendances sont installés
dans les images : aucun `npm install` local n'est nécessaire pour ce lancement.
Docker doit pouvoir télécharger les images et dépendances.

Ouvrir l'adresse affichée, normalement **https://localhost:19443**. Si ce port
est occupé au premier lancement, le lanceur essaie 19444 à 19446, puis conserve
le port choisi. L'interface et l'API partagent la même adresse. Les services
tournent en arrière-plan après la fin de la commande.

## Se connecter

Identifiant : **admin@layrs.local**. Le mot de passe aléatoire est dans
`.layrs-local/native-dev-default/login.json`. Ouvrir ce fichier localement ;
ne pas le partager ni le committer. La commande affiche son chemin absolu.

Le navigateur signale le certificat local au premier accès. Vérifier que
l'adresse est bien celle du lanceur puis accepter l'exception locale si le
navigateur le permet, ou faire confiance explicitement à cette autorité locale.
Le certificat public de l'autorité est `root.crt` dans le même dossier.
Le lanceur ne modifie pas les certificats de confiance du système.

Pour Studio Desktop, utiliser cette même URL serveur et copier le contenu de
`root.crt` dans « Certificat privé de l'entreprise » → « Autorité de
certification PEM », puis se connecter. Le lancement du serveur n'ouvre pas
automatiquement Desktop.

## Arrêter, reprendre, reconstruire

```powershell
npm run dev:info
npm run dev:down
npm run dev
```

L'arrêt conserve les projets, l'administrateur, les clés et les volumes Docker.
Le redémarrage réutilise ces données. Ne pas supprimer le dossier privé ou les
volumes pour un simple redémarrage : ils forment ensemble l'instance persistante.
Les commandes `dev:server` et `dev:services` sont des alias du lancement complet.

Ce mode utilise des images construites et n'offre pas de rechargement à chaud.
Après modification du code, relancer `npm run dev` pour reconstruire avec le
cache Docker. Pour reprendre sans reconstruire :

```powershell
npm run dev -- --no-build
```

## Instance distincte et diagnostic

Pour une autre instance isolée, utiliser le même nom pour toutes ses commandes :

```powershell
$env:LAYRS_DEV_INSTANCE = 'essai'
$env:LAYRS_DEV_PORT = '19450' # uniquement au premier lancement
npm run dev
npm run dev:down
```

Le dossier devient `.layrs-local/native-dev-essai`. Retirer ensuite les variables
de cette session PowerShell pour revenir à l'instance par défaut. Les ports ne
sont publiés que sur `127.0.0.1`. Les contextes Docker distants sont refusés.
Un changement de moteur Docker est refusé pour une instance existante : revenir
au moteur initial ou choisir une nouvelle instance, sans réutiliser ses secrets.

En cas d'échec, consulter le `docker.log` privé indiqué. Après une interruption,
un fichier `command.lock` peut subsister : vérifier que son processus `pid` est
arrêté avant de retirer ce seul fichier. Une configuration privée incohérente
est refusée afin de préserver les données ; ne pas régénérer les identifiants
sur des volumes existants. Les anciens services PostgreSQL/MinIO du Compose
racine ne sont pas utilisés par ce lanceur.

## Vérifications du lanceur

`npm run test:dev` exécute les tests sans Docker. La recette réelle est volontaire :
créer une instance dédiée avec `LAYRS_DEV_INSTANCE=launcher-test-<nom>`, lancer
`npm run dev`, puis `node scripts/test-local-dev-live.mjs` dans la même session.
Elle crée un projet, teste arrêt/reprise et récupération de l'initialisation,
puis arrête les conteneurs en conservant les volumes pour inspection.
