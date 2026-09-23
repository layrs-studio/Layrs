# Lanceur local — 23 septembre 2026

Demande : une commande `npm run dev` pour tester le serveur actuel et le Web.
L'ancien script utilisait encore le serveur historique et PostgreSQL/MinIO.

## Réalisé

- Remplacement par un lancement V3 Docker : PostgreSQL, migrations, serveur,
  Web et proxy HTTPS ; deux images construites sur le moteur local vérifié.
- Administrateur généré, secrets privés persistants, autorité HTTPS locale,
  ports uniquement sur loopback ; aucune installation de certificat système.
- `dev:info`, `dev:down`, reprise sans build ; arrêt sans suppression des volumes.
- Manifeste lié au moteur Docker, exclusion des commandes simultanées,
  refus des configurations incohérentes et des liens de fichiers privés.
- Tests de régression et recette réelle facultative, guide de lancement.
- Revue indépendante critique du lanceur ; corrections TLS, ACL, sélection du
  builder local, changement de moteur, erreurs JSON sans fuite de secrets.
- Deux défauts révélés par les essais Windows corrigés : application des ACL
  sans privilège système ; construction Buildx directe pour éviter l'erreur
  du builder nommé via Compose.

## Validation

- Sept tests Node PASS, dont ACL Windows avec droits de lecture préexistants.
- Construction réelle et initialisation de l'instance isolée
  `launcher-final-20260923` PASS, URL locale 19444.
- Recette réelle PASS : HTML et JavaScript, disponibilité API, refus d'une CA
  invalide malgré une variable ambiante permissive, port publié uniquement sur
  loopback, création de projet puis arrêt/reprise avec quatre volumes, CA et
  identifiants inchangés, récupération d'un accusé d'initialisation perdu.
  Sorties du lanceur vérifiées sans valeur des secrets générés.
- Revue indépendante des dernières corrections ACL et Buildx : aucun blocage.
- Instances `launcher-final-20260923` et prototype `launcher-check-20260923`
  arrêtées après recette ; leurs volumes privés sont conservés. Le prototype
  précède la liaison du manifeste au moteur et n'est pas une instance utilisateur.
- [Rapport de recette](../measurements/local-dev-launcher-2026-09-23.json).
- Contrôle de taille des sources PASS (avertissements existants hors périmètre).

## Limites et reprise

Ce mode fournit des images construites, sans rechargement à chaud. Un nouveau
`npm run dev` reconstruit les sources. Le premier build nécessite Internet.
Qualification sur le poste Windows et Docker Linux disponibles ; aucun résultat
macOS/ARM revendiqué. Les données de qualification sont conservées ; arrêter
uniquement les projets isolés du lanceur, sans suppression de volumes.

Ce lot ne change pas les limites des huit plans listées dans `../REMAINING.md`.
