# Exécuteur externe natif

`cargo build --locked -p layrs-runner` construit un exécuteur indépendant du serveur.
Docker doit être disponible sur la machine d'exécution. L'image doit être préchargée
et identifiée par son digest, jamais par une étiquette mutable.

La recette est un objet JSON :

```json
{
  "image": "registry.example/team/test@sha256:REMPLACER_PAR_64_CHIFFRES_HEXA",
  "command": ["/bin/sh", "-c", "./ci/test.sh"],
  "timeoutSeconds": 300,
  "maximumLogBytes": 65536
}
```

L'administrateur crée un compte de service, lui accorde le rôle `automation` dans
le Space, puis configure cette recette et ce Principal dans la protection du
Layer. Le fichier JSON passé au runner doit correspondre exactement à la recette
canonique du serveur. Le jeton est enregistré dans un fichier privé **hors du dépôt**.

```sh
export LAYRS_RUNNER_TOKEN_FILE=/private/layrs-ci-token
layrs-runner https://layrs.example space:ID blake3:STEP layer:ID \
  /private/recipe.json /runs/nouvelle-execution
```

Le répertoire d'exécution doit être neuf et absolu. Le runner récupère le Step
exact avec vérification des objets et de la reconstruction des fichiers. Il refuse
les conflits et, pour cette première recette d'exécution, les liens symboliques.
Sous Windows, il refuse aussi les fichiers portant le bit exécutable et les noms
non représentables sans ambiguïté (alias de casse, noms réservés, flux ADS), pour
éviter de tester un checkout différent du Step demandé. Utiliser un hôte Unix si
le dépôt exige ces attributs.
Le conteneur n'accède qu'au checkout : aucun secret/env hôte, aucun réseau, aucune
capability ; racine du conteneur en lecture seule, mémoire/CPU/processus limités.
Sous Windows, le conteneur utilise l'UID 10001. Sous Unix, il reprend l'UID/GID
du runner : lancer celui-ci avec un compte dédié non root. Une recette de compilation doit
donc inclure ses dépendances dans l'image préchargée ; aucun téléchargement implicite.

La durée maximale est 30 minutes. Le journal est borné et la coupure est indiquée.
Le timeout supprime le conteneur et produit un verdict `failed`. Un fichier
`completion.json` durable est écrit avant le callback. Si la réponse réseau est
perdue, le même résultat peut être rejoué :

```sh
layrs-runner retry /runs/nouvelle-execution/completion.json
```

Un résultat contradictoire est refusé. Une autre tentative reçoit une autre identité.
`LAYRS_CA_FILE` permet une CA privée explicite. `LAYRS_RUNNER_DOCKER_CONFIG` et
`LAYRS_RUNNER_DOCKER_HOST` servent uniquement à sélectionner le daemon local.
Le runner conserve le checkout et les traces ; l'opérateur gère leur rétention.
Les callbacks réussis impriment uniquement runId, Step et verdict, jamais le jeton.

Validation : test Docker réel d'un fichier, absence du secret dans l'environnement,
timeout et troncature de journal passé. Le test d'intégration HTTPS + PostgreSQL
`native_clients_v3` peut inclure l'exécution et la Gate en définissant
`LAYRS_TEST_RUNNER_IMAGE` avec l'ID d'une image locale contenant `/bin/sh`.
