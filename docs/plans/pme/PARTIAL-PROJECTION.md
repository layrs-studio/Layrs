# Projection partielle : audit et options de livraison

État courant au 21 septembre 2026 : **choix acquis, implémentation en cours**.
L'utilisateur demande un Worktree partiel éditable, avec noms et chemins visibles
et interdiction de création/renommage/déplacement sur les emplacements réservés.
Le moteur doit également refuser les collisions créées par un éditeur externe.
Voir [ADR0024](../../decisions/0024-editable-partial-worktrees.md).
Les lots ci-dessous sont conservés comme audit historique du 20 septembre ;
ils ne décrivent plus un choix en attente et ne constituent pas des fonctions livrées.

## Audit historique

État au 20 septembre 2026 : **analyse seulement, aucun choix acquis à cette date**.
La question utilisateur distingue une première version avec projets complets et
gestion des accès fins dans l'interface, une projection locale en lecture seule,
et un Worktree partiel éditable nécessitant un changement de modèle. Les deux
lots proposés ci-dessous ne sont pas implémentés ; le choix de comportement et
de compatibilité locale reste à trancher.

## Ce qui existe déjà

Les règles de contenu v2 contrôlent les chemins normalisés exact/subtree, avec
refus prioritaire et Principal authentifié. Elles ne créent jamais un droit
général absent. Les Step/Tree et noms restent visibles, conformément à
[ADR0012](../../decisions/0012-space-events-object-authorization.md) ; les octets
des fichiers sont autorisés par chemin, racine et identité exacts. Une lecture
ponctuelle de fichier est disponible. Les capacités et Range revalident l'epoch.

Le Bootstrap signale `contentAccess:restricted`. Clone et receive complets
refusent ce mode avant installation ; la réception classique le vérifie avant
capture implicite. La réception annulable peut contrôler l'état local avant le
Bootstrap, mais n'y crée aucune capture ni mutation. Les canaux opaques,
collaboration riche, CI et écritures restent fermés aux comptes restreints.
Cette protection est qualifiée ; **elle ne constitue pas un checkout partiel**.
Voir [ADR0023](../../decisions/0023-conservative-content-path-acl.md) et le journalT4.

Une révocation ne peut pas effacer les copies déjà obtenues. Si des octets
identiques sont autorisés sous un autre chemin, ce chemin permet de les lire :
les règles protègent l'accès par chemin, pas le caractère secret global d'un FileId.

## Pourquoi une simple suppression de garde serait incorrecte

| Source actuelle | Invariant ou limite |
|---|---|
| [remote/receive.rs](../../../crates/layrs-client-core/src/native_scm/remote/receive.rs) et [control.rs](../../../crates/layrs-client-core/src/native_scm/remote/control.rs) | Le parcours complet exige des accès complets ; il prépare une installation entière et une Operation locale. |
| [repository.rs](../../../crates/layrs-core/src/scm/repository.rs) | WorktreeStatus contient Ready, Conflicted, NeedsRepair, Stale ; aucun état Partial ni engagement explicite sur une sélection de chemins. |
| [admission.rs](../../../crates/layrs-store-local/src/scm_v1/admission.rs) et [admission/resolver.rs](../../../crates/layrs-store-local/src/scm_v1/admission/resolver.rs) | Un FileId dans un Tree admis exige un manifeste vérifié et sa fermeture complète admise au même Space. Une structure distante connue n'est pas une fermeture locale disponible. |
| [materialize.rs](../../../crates/layrs-store-local/src/scm_v1/materialize.rs) | La projection entière doit correspondre au materializedTreeId de la View ; la différence before/after produit aussi des suppressions. |
| [checkout/projection.rs](../../../crates/layrs-store-local/src/scm_v1/checkout/projection.rs) | Projection physique complète avec limites et identités exactes ; elle ne distingue pas un chemin masqué d'un chemin absent. |
| [format.rs](../../../crates/layrs-store-local/src/scm_v1/format.rs) et [schema.rs](../../../crates/layrs-store-local/src/scm_v1/schema.rs) | Format et schéma locaux version2, contrôle de compatibilité explicite. Un ancien lecteur ne doit pas interpréter une projection partielle comme un Store complet. |
| [catalog/content_acl.rs](../../../crates/layrs-store-server/src/v3/catalog/content_acl.rs), [policy_authorizer.rs](../../../crates/layrs-store-server/src/v3/policy_authorizer.rs), [plans.rs](../../../crates/layrs-store-server/src/v3/plans.rs) | Autorisation ancrée et résultats de plans existent ; aucune autorité ne peut être déduite d'un cache ou d'une simple possession d'objet. |

Il serait incorrect de filtrer un Tree en conservant son ID, de déclarer admise
une Step dont la fermeture requise manque, ou de fabriquer une View « complète »
depuis les seuls fichiers autorisés. Une absence dans la sélection ne doit jamais
devenir implicitement une suppression locale.

## Lot A proposé : projection locale explicite en lecture seule

Objectif : consulter et rafraîchir les fichiers accessibles d'une Step distante,
avec une sélection durable explicite. Ce mode serait distinct du NativeWorkspace
éditable actuel ; il ne promettrait ni capture, ni publication, ni fusion locale.
Première création dans un répertoire vide uniquement. La conversion d'un Worktree
existant serait une opération ultérieure, explicite et séparément qualifiée.

Le registre local versionné lierait origine, Principal, Space, Step et Tree racine
exacts, PolicySet et epoch, identité du répertoire, génération locale et état par
chemin. DTO indicatif, **pas un contrat déjà stabilisé** :

```text
ReadOnlyProjection {
  schemaVersion, origin, principalId, spaceId,
  stepId, rootTreeId, policySetId, authorizationEpoch,
  filesystemIdentity, generation,
  entries: [{ path, targetIdentity, state, reason? }]
}
state = materialized | reserved | unavailable
```

Le cache conserverait les octets canoniques exacts des Step/Tree et leurs IDs.
Il serait clairement séparé de l'admission native complète : aucun nouveau
RepositoryView/Step synthétique ne représenterait la sélection. Le registre
décrirait un état physique local ; il n'accorderait jamais de droit réseau.

Un parcours `clone_projection` puis `preview_refresh`/`apply_refresh` devrait :

1. Fixer une Step et une autorité observées, parcourir ses vrais Trees et obtenir
   des décisions de FetchPlan bornées. Refus, objet indisponible, page incomplète
   et panne réseau sont des états distincts ; aucun n'autorise une suppression.
2. Télécharger et vérifier les seules valeurs autorisées. Même si leur FileId est
   en cache, demander une autorisation pour le chemin exact. Les règles clientes
   ne remplacent jamais les décisions du serveur.
3. Produire un plan physique versionné avec attentes locales, puis revalider
   Principal/autorité et génération avant commit. Un changement invalide le plan.
   Cela ne prétend pas annuler rétroactivement des téléchargements autorisés.
4. Appliquer seulement les chemins possédés et explicitement permis, au moyen
   d'un journal durable avec annulation avant commit et reprise idempotente.

### Conservation des chemins réservés

Un fichier devenu masqué reste intact, même s'il a été modifié localement. Les
fichiers non possédés par la projection restent également intacts. Réautoriser
un chemin ne donne pas le droit d'écraser une modification : conflit explicite.

Pour supprimer un ancien chemin absent de la nouvelle Step, son absence seule
ne suffit pas. Il faut revalider l'accès courant au chemin sous son ancienne
racine admise, constater que la projection le possède et que les octets n'ont
pas changé. Sans ces preuves, conserver le fichier comme réservé. Les répertoires
contenant des chemins masqués/non possédés ne peuvent pas être supprimés.

Un passage fichier↔répertoire, une collision de casse, un parent lien/reparse,
`.layrs`, `.git` ou un objet non pris en charge doit refuser l'installation
concernée sans toucher aux chemins protégés. Les Conflicts opaques et leurs
variantes ne doivent pas être interprétés comme de simples fichiers absents.
Une première version peut refuser toute projection contenant ces cas plutôt
que prétendre les gérer.

### Format et migrations du lot A

Le protocole serveur possède déjà structures, claims ancrés et capacités :
**aucune migration PostgreSQL n'est identifiée comme nécessaire pour ce périmètre**.
Un endpoint supplémentaire de planification pourrait simplifier le parcours,
mais ne dispense pas des mêmes décisions d'autorisation et bornes de pagination.

En revanche, le mode local, son registre et son journal doivent avoir un format
explicite, des contrôles de compatibilité et un refus des anciens lecteurs. Il
faut décider comment l'isoler du format2 existant avant tout DDL ou migration.
Pas de modification rétroactive de la baseline2, ni de flag dans un simple
binding mutable qui permettrait aux anciennes commandes de capturer une vue
tronquée. Toutes les commandes natives de mutation doivent refuser ce mode
avant capture ou écriture. L'absence de migration PostgreSQL ne signifie donc
pas absence de travail sur le stockage local.

## Lot B distinct : véritable Worktree partiel éditable

Ce lot permettrait de capturer des modifications autorisées en conservant les
sous-arbres/fichiers inconnus dans leur identité canonique exacte. Il demande :

- Un état partiel natif versionné et un lien explicite à la projection locale,
  sans réinterpréter Ready ou Stale. Décider les évolutions Core/RepositoryView
  par ADR avant de choisir schéma et compatibilité.
- Une distinction durable entre structure distante vérifiée, objet complet
  disponible, absence réelle et contenu inaccessible. Ne pas faire répondre
  « disponible » aux lecteurs/admissions actuels quand seul l'ID est connu.
- Une capture par modification des chemins autorisés, qui réutilise les
  références canoniques intactes aux chemins non matérialisés ; contrôle serveur
  de tous les chemins affectés, y compris suppressions, renommages et conflits.
- Des règles cohérentes pour Operations, undo/redo, switch, restore, fusion,
  réparations, fsck, rétention/GC, sauvegarde et migration des Stores existants.
  Une ancienne Operation ne peut restaurer silencieusement une projection sous
  une autorité périmée.
- Un passage explicite entre projection partielle et complète, avec contrôle des
  modifications locales, identités de fichiers et collisions.

Une évolution additive du schéma local et du minimum de lecteur est à prévoir,
avec stratégie de migration ou refus explicite. Le besoin de migrations serveur
dépendrait du futur contrat d'écriture par chemin ; aucune n'est réservée ici.
Ce lot ne peut pas être présenté comme une extension mineure de l'interface.

## Preuves nécessaires avant livraison du lot A

- Vrais clients HTTPS/PostgreSQL et deux comptes : même FileId sous chemin public
  et privé, refus d'alias et d'accès opaque, aucune réutilisation de cache sans
  autorisation actuelle du chemin demandé.
- Projection complète en lecture seule puis restriction : octets masqués
  modifiés, fichiers non possédés et répertoires parents conservés. Réautorisation
  avec collision refusée ; suppression/renommage explicitement qualifiés.
- Droits ou Principal changés pendant pagination, téléchargement et préparation
  du commit : aucun plan périmé appliqué. Une page manquante ne produit jamais
  une projection annoncée complète.
- Crash à chaque frontière du journal physique, annulation avant commit,
  réouverture/reprise : mêmes fichiers et même registre que sans panne, ou état
  NeedsRepair explicite. Aucun succès fabriqué après résultat incertain.
- Registre/cache modifiés, fermeture manquante, casse, fichiers↔répertoires,
  liens/reparse et chemins réservés : refus avant mutation des fichiers protégés.
- Ancien client et commandes capture/publish/undo/restore classiques : refus
  explicite du mode partiel. Tests du lot B supplémentaires avant toute écriture.

## Ordre de grandeur et décision restante

Lot A : environ **12–20 jours-ingénieur**, avec six composants couplés : plan de
snapshot, registre/cache, matérialisation sélective, reprise, API/CLI/interface,
gardes et recettes adversariales. Lot B : **20–35 jours-ingénieur supplémentaires**
selon le contrat d'admission et la migration retenue. Ce sont des estimations
d'audit, hors revue d'architecture et pilote utilisateur, pas un engagement.

Le comportement conservateur actuel reste utilisable pour une première version
avec projets complets, gestion des accès et lecture ponctuelle autorisée. Les
cases de projection partielle ne doivent pas être cochées pour cette raison.
Attendre le choix utilisateur avant de transformer l'un de ces lots en plan
d'implémentation ou de modifier le code.
