# Recette locale jetable

Cette recette qualifie l'assemblage Linux PostgreSQL + serveur natif + Studio Web
sur une seule origine HTTPS. Elle ne publie aucun service externe et ne remplace
pas les essais de charge, une installation chez une PME ou une distribution signée.

## Préparation et lancement

Prérequis : Docker Compose prenant en charge `!override`, Node 22 ou supérieur,
port local 19443 libre et images du code à qualifier déjà construites :

```sh
docker build -f deploy/selfhost/Dockerfile -t layrs-server:validation .
docker build -f deploy/selfhost/Web.Dockerfile -t layrs-studio:validation .
node scripts/test-selfhost.mjs
```

Le script crée un projet Compose `layrs-pilot-<aléatoire>`, quatre volumes dédiés
et des secrets neufs dans `tmp/<projet>` (ignoré par Git et Docker). Il remplace
uniquement les publications de ports de ce projet : `127.0.0.1:19443`, HTTPS.
Il utilise le Compose d'installation, son script PostgreSQL runtime et son routage
Caddy réel ; seule la PKI publique est remplacée par `tls internal`.

La CA privée est copiée depuis le volume Caddy et approuvée explicitement par
le client du test. Le premier appel sans cette CA doit échouer. Aucun contournement
de TLS, aucune installation de CA dans le système, aucun secret en arguments ou
dans les sorties. Sous Windows, utiliser un espace de travail dont les ACL limitent
l'accès aux utilisateurs autorisés ; les fichiers de secrets héritent de ces ACL.

Options d'environnement : `LAYRS_IMAGE_TAG` (défaut `validation`),
`LAYRS_TEST_HTTPS_PORT`, `LAYRS_TEST_DOCKER_CONFIG`, `LAYRS_TEST_DOCKER_HOST`.
Les deux dernières permettent de choisir explicitement le daemon de test local.

Le script détruit **uniquement** ses conteneurs, son réseau et ses volumes à la fin,
y compris après erreur. `--keep` conserve expressément ce projet pour diagnostic.
Les fichiers privés restent dans `tmp/<projet>` : ne pas les partager ; supprimer
uniquement ce dossier identifié après les investigations. `report.json` ne contient
que les contrôles réussis, date, origine locale et digests des images. Une erreur
Docker produit un diagnostic local avec les secrets connus masqués.

## Contrôles et preuve du 20 septembre 2026

Recette passée sur Docker Desktop Linux, PostgreSQL 17.6 bookworm :

- Démarrage PostgreSQL, migrations, serveur puis proxy ; readiness DB/CAS.
- Refus de la CA privée inconnue, succès avec confiance explicite.
- HTML Studio, bundle JavaScript, CSP et page `/auth/device` sur la même origine.
- Bootstrap à usage unique, identité authentifiée, refus anonyme HTTP 401.
- Création réelle Workspace/Space/Layer avec le compte runtime PostgreSQL.
- Serveur UID Linux 10001 ; runtime sans superuser, CREATEDB ni CREATEROLE.
- Redémarrage serveur et nouvel `initialize` : bootstrap toujours consommé,
  compte conservé, connexion puis déconnexion effectivement révoquée.

Images testées : serveur `sha256:9faa0a9a11cea0920700be20a0625a4dc0364e3e815a97482ec69b2f4e6b7880`,
Studio `sha256:65e2c16cf4f91705899189c23500255adf6c8423492e2f22eb3ed2689a7168ba`.
Ces digests figent la preuve ; reconstruire puis relancer après les changements
ultérieurs. Rapport local : `tmp/layrs-pilot-237b71392aa7/report.json`.

La recette a découvert une entrée tmpfs YAML scindée par une virgule ; la chaîne
est désormais quotée dans le Compose. Le test ne navigue pas dans un navigateur
et ne remplace pas le harnais Desktop/Web de revue et administration.

## Tests natifs Linux séparés

`scripts/test-linux-native.Dockerfile` fournit un environnement Rust 1.90 Linux.
Monter uniquement `crates`, `Cargo.toml` et `Cargo.lock` en lecture seule dans `/src`,
un volume dédié dans `/build` et un cache dans `/usr/local/cargo/registry`.
La commande par défaut exécute six suites client natives : capture, workspace,
histoire, réécriture, acceptation des fusions et matérialisation des changements
fichier/répertoire avec interruptions. Aucun artefact n'est écrit dans le target
Windows. La validation effective et ses limites sont consignées dans le journal T8.

Exemple depuis un shell Linux à la racine du dépôt :

```sh
docker build -f scripts/test-linux-native.Dockerfile -t layrs-linux-tests:validation .
docker run --rm \
  --mount "type=bind,source=$PWD/crates,target=/src/crates,readonly" \
  --mount "type=bind,source=$PWD/Cargo.toml,target=/src/Cargo.toml,readonly" \
  --mount "type=bind,source=$PWD/Cargo.lock,target=/src/Cargo.lock,readonly" \
  --mount type=volume,source=layrs-linux-test-build,target=/build \
  --mount type=volume,source=layrs-linux-test-cargo,target=/usr/local/cargo/registry \
  layrs-linux-tests:validation
```

Résultat du 20 septembre : **44 tests réussis, aucun ignoré**, dont les symlinks,
le bit exécutable Unix et les reprises des changements de types. Le conteneur de
compilation utilise root ; le serveur de la recette Compose, lui, a été vérifié
sous UID10001. Ces suites ne valident pas le gestionnaire de secrets du bureau Linux.
