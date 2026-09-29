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

### Correctif des droits Windows après retour utilisateur

Échec reproduit sur `native-dev-default` : son propriétaire était déjà le compte
courant, avec des droits hérités de modification. Le script tentait néanmoins
`SetOwner`, ce qui exige `WRITE_OWNER` et provoquait un refus Windows.
Le lanceur conserve maintenant le propriétaire, vérifie qu'il correspond au
compte courant sur tous les fichiers avant intervention, puis ne réécrit que
la DACL privée. Un dossier appartenant à un autre compte est refusé.

Huit tests Node PASS, dont un nouveau cas qui interdit explicitement la
réattribution de propriété sur le dossier et son fichier, puis vérifie deux
applications successives des protections. Le lancement complet de l'instance
par défaut a réussi : `https://localhost:19443`. Elle reste démarrée pour
l'utilisateur ; ses données et identifiants sont conservés.

Ce mode fournit des images construites, sans rechargement à chaud. Un nouveau
`npm run dev` reconstruit les sources. Le premier build nécessite Internet.
Qualification sur le poste Windows et Docker Linux disponibles ; aucun résultat
macOS/ARM revendiqué. Les données de qualification sont conservées ; arrêter
uniquement les projets isolés du lanceur, sans suppression de volumes.

Ce lot ne change pas les limites des huit plans listées dans `../REMAINING.md`.
