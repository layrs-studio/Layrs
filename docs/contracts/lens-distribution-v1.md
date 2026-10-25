# Distribution des lenses — contrat livré

État au 5 octobre 2026. Ce document décrit les routes et limites des sources
actuelles. Il complète le contrat SDK `layrs.lens.v1` ; il ne qualifie pas une
distribution publique ni tous les formats de fichiers.

## Trois niveaux

1. **Instance** : un administrateur installe une version immuable dans le
   catalogue de son déploiement. Le catalogue distingue `active`, `retired` et
   `revoked` et garde l'historique des changements d'état.
2. **Projet** : les responsables autorisés épinglent des versions exactes
   `{lensId, version, digest}` et associent éventuellement une extension à une
   lens. Une nouvelle version installée ne change aucun projet.
3. **Client** : Web ou Desktop lit la configuration de ce projet, récupère les
   paquets autorisés qui lui manquent, vérifie leurs empreintes et charge le
   composant d'affichage dans un worker hébergé par une iframe isolée. Le cache
   Desktop peut servir hors ligne les paquets déjà vérifiés pour ce projet.

L'absence de lens compatible, un chargement refusé ou un contenu non pris en
charge conduit à un affichage de secours explicite. Une version révoquée n'est
plus téléchargeable et le serveur refuse l'admission de nouveaux reçus qui
l'utilisent. Les viewers en ligne revérifient la configuration avant chargement ;
le moteur local utilise la dernière configuration obtenue pendant sa connexion
ou synchronisation. Un client hors ligne ne découvre une révocation qu'à
l'actualisation de cette configuration ; une révocation déjà connue reste
mémorisée. Les références historiques immuables sont
conservées et contrôlées séparément des nouvelles exécutions.

La compatibilité avec les anciens pins intégrés ne permet une nouvelle
publication que si le projet n’a jamais eu de configuration de lenses. Une
configuration explicite vide désactive aussi cette exception. Le serveur
refait ce contrôle sous verrou à l’admission ; les reçus déjà admis avec leur
identité exacte restent consultables.

## Paquet et identité

Le fichier importé est un JSON `.layrslens.json` contenant `manifest` et
`files: [{path, bytesBase64}]`. Le manifeste annonce l'identifiant, la version,
les formats, les capacités et les points d'entrée. Chaque composant déclaré a
son empreinte `sha256:`. Le serveur refuse les chemins/fichiers manquants,
doublés ou non déclarés, les encodages et empreintes incorrects, et les versions
existantes dont le contenu diffère. La limite est de **8 Mio décodés**, **8
fichiers** et **12 Mio de JSON** pour la requête d'installation.
Un [paquet Markdown d'exemple](../../deploy/lenses/example-markdown.layrslens.json)
illustre le format importable.

L'identité du paquet est `sha256:` du manifeste canonique JCS après retrait de
`digest` et de `bundle.signature`. La signature Ed25519, si présente, porte sur
les octets ASCII de cette valeur `sha256:<hex>`. Le serveur vérifie la signature
avec la clé publique désignée par `keyId` dans `LAYRS_LENS_TRUSTED_KEYS`, une
table JSON `{"id-de-cle":"cle-publique-brute-base64"}` fournie par
l'exploitant. Une clé annoncée par le paquet ne devient jamais une ancre de
confiance. Sans signature valable, l'administrateur doit cocher explicitement
l'autorisation d'un paquet non signé (`approveUnsigned: true`). Une signature
présente mais inconnue ou invalide est refusée.

Le point d'entrée d'affichage livré est un module ESM
`layrs.viewer.worker.v1` exportant `render`. Son résultat est déclaratif :
blocs `text`, `code` ou `heading`, validés avant rendu par l'application. Il
n'obtient pas directement le jeton de session, les commandes Tauri ou un accès
libre au disque. Le point d'entrée de réconciliation livré est un module WASM
sans imports avec export `layrs_reconcile`, validé puis exécuté par le moteur
isolé. Le point d'entrée `analyzer` n'est **pas** exécutable dans cette version
et un paquet qui le déclare est refusé. Les budgets d'entrée/sortie et délais
du viewer sont contrôlés ; il n'existe pas encore de quota dur de mémoire du
worker JavaScript.

Le module d'affichage autonome et le module WASM sont chacun limités à
2 Mio par leur runtime. Le viewer reçoit au plus 2 Mio de JSON, produit au
plus 1 Mio et dispose de 5 secondes ; une limite plus basse déclarée dans le
manifeste s'applique. Son résultat contient au plus 1 000 blocs. Desktop garde
un cache d'affichage borné à 128 entrées et 32 Mio, isolé par serveur, compte,
dossier et projet. Le Web conserve ces paquets uniquement en mémoire.

La réconciliation dynamique est raccordée aux dossiers natifs complets. Avec
une configuration de lenses sur un dossier à accès limité, une fusion de
contenus concurrents conserve les variantes pour un choix structurel explicite ;
elle n'exécute pas implicitement une ancienne lens statique.

Les paquets intégrés Code et Texte version `1.1.0` possèdent un module de
réconciliation WASM ; Image et Raw sont aussi au catalogue. L'aperçu distant
actuel traite le **texte UTF-8 au plus 1 Mio**. Les images binaires et les
formats plus grands utilisent un état de secours, même si une lens Image est
installée.

## Routes et autorisation

Toutes les routes ci-dessous, hors hôte de runtime, exigent un Bearer valide.
Les réponses du catalogue et de la configuration portent `Cache-Control:
no-store`.

| Route | Fonction |
| --- | --- |
| `GET /v3/lenses/catalog` | Versions actives visibles par un compte connecté : `{items, errors}`. |
| `GET /v3/lenses` | Catalogue d'administration, y compris versions retirées/révoquées et nombre de projets utilisateurs. |
| `POST /v3/lenses` | Import administrateur ; JSON du paquet et `approveUnsigned`. |
| `POST /v3/lenses/:id/:version/:digest/retire` | Retire une version du choix des nouveaux projets ; les projets déjà épinglés la gardent. |
| `POST /v3/lenses/:id/:version/:digest/revoke` | Refuse nouveaux téléchargements et exécutions ; garde les preuves historiques. |
| `GET /v3/spaces/:space/lenses` | Configuration et catalogue autorisés de ce projet, `revision`, `pins`, `associations`, `canManage`. |
| `PUT /v3/spaces/:space/lenses` | Remplace la configuration avec `expectedRevision`, `pins`, `associations` ; conflit si la révision a changé. |
| `GET /v3/spaces/:space/lenses/:id/:version/:digest` | Paquet exact si ce projet l'épingle encore et si la version n'est pas révoquée. |
| `GET /v3/lens-runtime/host` | Hôte isolé du worker d'affichage. |

La configuration accepte au plus **32 pins** et **64 associations**. Une seule
version par identifiant de lens et une seule association par extension sont
admises. Une association a la forme `{extension: ".md", lensId:
"example.markdown"}` et doit référencer une lens épinglée déclarant cette
extension. Le serveur vérifie les droits à chaque lecture ou mutation ; un
catalogue affiché côté client n'est jamais une autorisation durable.

`GET /v3/contexts` expose l'entreprise, les organisations accessibles et le
contexte personnel selon les deux options indépendantes du serveur. L'ancien
`GET /v3/workspaces` retourne les contextes où la création est autorisée, par
pages de 100. `POST /v3/projects` accepte `contextId` (ou l'ancien `workspaceId`
pour un contexte existant), plus `spaceSlug`, `name` et éventuellement
`lensConfiguration: {pins, associations}`. La création du projet, du Layer
initial, des droits du créateur et des pins initiaux est transactionnelle. Un
réessai exact rend les mêmes identifiants ; un nom ou une configuration différente
pour le même identifiant est refusé. La route historique
`POST /v3/workspaces/provision` reste disponible pour CLI et Desktop avec un
`contextId` optionnel ; son absence sélectionne l'entreprise par défaut.
La création d'organisation passe séparément par `POST /v3/organisations`,
réservée à l'administration quand le mode multi-organisation est activé.
Voir le [contrat des contextes](project-contexts.md).

L'aperçu publié `POST /v3/spaces/:space_id/preview` prend le `stepId`, le
`path`, le `expectedPolicySetId` et le `expectedAuthorizationEpoch`. Il vérifie
le droit sur ce chemin exact et rend du texte UTF-8 borné ; un fichier binaire
ou trop grand est signalé comme omis sans fournir ses octets. Les droits et la
politique sont revérifiés avant la réponse.
