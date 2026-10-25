# Entreprise, organisations et projets personnels

Décision produit du 5 octobre 2026. Une installation Layrs représente une
entreprise. Le compte utilisateur appartient à cette installation et peut
accéder à plusieurs contextes selon les options du serveur et ses droits.

## Options indépendantes

| Configuration serveur | Valeur par défaut | Effet |
| --- | --- | --- |
| `LAYRS_MULTI_ORGANISATION_ENABLED` | `false` | Autorise plusieurs organisations dans l'installation. |
| `LAYRS_PERSONAL_PROJECTS_ENABLED` | `false` | Autorise un contexte personnel privé par compte humain. |

Les quatre combinaisons sont prévues. Activer les organisations n'active pas
les projets personnels, et les projets personnels ne nécessitent pas les
organisations. Les booléens acceptent uniquement `true` et `false`.

Le mode multi-organisation se règle aussi dans **Administration → Organisations**.
Après un changement depuis cette page, le choix enregistré en base prévaut sur
`LAYRS_MULTI_ORGANISATION_ENABLED`, y compris après redémarrage. La variable
reste la configuration initiale des installations qui n'ont pas encore utilisé
ce réglage. L'option des projets personnels reste indépendante.

Le contexte entreprise est créé pour l'installation neuve. Sans option activée,
l'utilisateur crée son projet directement dans l'entreprise : aucun formulaire
de création d'organisation n'est nécessaire. Le projet conserve ses permissions
propres ; rejoindre l'entreprise ne donne pas accès à tous ses projets.

Un projet personnel appartient à son compte. Le contexte personnel ne peut pas
recevoir d'autres membres ou d'équipes. Le catalogue des lenses reste commun
à l'installation ; les versions et associations restent configurées par projet.

## Passage réversible en mono-organisation

L'administrateur choisit explicitement l'organisation principale et confirme
une fenêtre de danger en recopiant son nom (son identifiant unique si plusieurs
organisations portent le même nom). Le récapitulatif nomme les autres
organisations et compte leurs projets et membres. Ces organisations restent
stockées, mais leurs projets sont inaccessibles jusqu'à la réactivation du mode
multi-organisation. Aucun projet, groupe, membre ou historique n'est supprimé
ou transféré. La principale garde ses adhésions et ses permissions : ce choix
n'ajoute aucun compte à une ancienne organisation, même celui d'un administrateur.
L'administration de l'installation reste accessible indépendamment de l'adhésion.

Les accès directs, publications et nouvelles demandes de transfert sont soumis
au même contrôle serveur. Une lecture déjà autorisée peut finir ; les copies
déjà téléchargées sur un ordinateur ne sont pas effacées. Les projets personnels
autorisés restent accessibles à leur propriétaire dans les deux modes.

`GET /v3/admin/organisations/settings` expose le mode, la principale, les comptes
de projets/membres, une révision et un jeton d'état. `PUT` exige `expectedRevision`,
`expectedStateToken`, `multiOrganisationEnabled`, `primaryOrganisationId` et
`confirmDanger`. Un état périmé est refusé avant mutation ; le client recharge
le récapitulatif. Les deux routes sont réservées aux administrateurs d'instance.

## API et sélection

- `GET /v3/contexts` expose les deux options, `defaultContextId`, les contextes
  accessibles et `canCreateOrganisation`. Chaque contexte fournit `id`,
  `workspaceId`, `kind`, `name`, `slug`, `role` et `canCreateProject`.
- `kind` vaut `company`, `organisation` ou `personal`. `id` est le `workspaceId`.
- `POST /v3/organisations` reçoit `{slug,name}`. La création nécessite le mode
  multi-organisation et les droits d'administration de l'installation.
- `POST /v3/projects` reçoit `{contextId,spaceSlug,name,lensConfiguration?}`.
  Le serveur vérifie le contexte et les droits dans la transaction de création.
- `GET /v3/spaces?contextId=...&limit=...&cursor=...` filtre le catalogue côté
  serveur. Un curseur ne peut pas être réutilisé dans un autre contexte.
- `POST /v3/workspaces/provision` accepte `contextId` pour publier un dossier
  natif existant tout en conservant les identifiants de son projet et du Layer.

Web et Desktop mémorisent la sélection par serveur et par compte. Un changement
de sélection réinitialise la pagination et les résultats précédents. Cette
sélection n'accorde aucun droit : les API vérifient toujours l'identité, les
adhésions et les règles du projet. Les dossiers locaux Desktop sans contexte
serveur connu restent distingués des projets du contexte sélectionné.

La navigation affiche le sélecteur **Organisation**, puis **Tous les projets**,
puis la liste des projets. La dernière entrée du menu crée une organisation
si le compte le permet. En mode mono, le sélecteur disparaît. Les projets
personnels ont leur propre accès, indépendant du sélecteur d'organisation.

## Équipes

Les nouvelles équipes appartiennent à une organisation (ou à l'entreprise en
mode simple). Leurs membres doivent appartenir à ce même contexte. Chaque
projet attribue séparément un rôle à l'équipe ; créer une équipe n'ouvre aucun
projet automatiquement. Une équipe d'une autre organisation ne peut pas être
affectée au projet.

L’ajout d’un compte à une organisation passe actuellement par une invitation ou
un accès direct à un premier projet. Il n’existe pas encore d’écran autonome
d’adhésion à l’organisation sans accès à un projet. Les équipes ne proposent
que les membres déjà actifs de ce contexte.

Les anciennes équipes propres à un projet conservent leurs identifiants, leurs
permissions et leur historique. Elles ne sont pas transformées automatiquement
en groupes donnant accès à d'autres projets.

## Installations existantes

La migration ajoute une entreprise distincte sans déplacer les anciens projets,
réécrire leurs permissions ni ajouter des membres aux anciennes organisations.
Cela évite d'activer des droits auparavant inaccessibles à un compte extérieur.
Lors de la première migration, l'opérateur conserve le mode multi-organisation
si des organisations existent. Il peut ensuite utiliser la bascule explicite
dans Administration pour choisir l'organisation principale et suspendre les autres.
La suppression et le transfert automatique des organisations ne font pas partie
de cette migration.

## CLI

`layrs remote contexts` expose les contextes disponibles.
`layrs remote projects --context workspace:IDENTIFIANT` filtre les projets ;
`layrs remote provision --context workspace:IDENTIFIANT` publie le dossier dans
ce contexte. Sans contexte explicite, le provisionnement utilise le contexte
principal disponible pour le compte.
