# Worktree partiel : politique et autorisation serveur

## 2026-09-21 — politique et reconstructeur livrés ; admission en intégration

Décision produit : noms et chemins visibles (ADR0012), namespace réservé contre
création/déplacement non autorisés, aucune écriture aveugle. Les règles v1/v2
restent immuables. Aucun commit réalisé par cet agent.

- Fait : règles v3 lecture/écriture distinctes, écriture exigeant lecture.
- Fait : actions de projection et patch borné ; publication/upload génériques
  toujours interdits aux membres restreints.
- Tests : Sync règles 8 PASS ; autorisation serveur 7 PASS ; DTO patch 2 PASS ;
  reconstructeur Tree 5 PASS. `cargo check -p layrs-store-server --lib` PASS.
- Fait : `PathPatchRequest`, digest domaine `layrs-path-patch-request-v1`, attentes
  et remplacements obligatoirement présents (null explicite), chemins triés,
  1024 changements maximum, profondeur 128, longueur de chemin 4096 octets.
- Fait : reconstructeur pur `rebuild_path_patch`, revalidation IDs chargés,
  préservation siblings opaques/modes, refus collisions/attentes périmées et
  remplacements de Tree implicites ; bornes 16 384 Trees input/output,
  200 000 entrées visitées input/output, estimation conservatrice 64 MiB metadata.
  Les Trees vides résultants sont conservés canoniquement. Aucun contenu admis
  ni aucune référence déplacée par cette fonction.
- Fait : admission d'une nouvelle version Policy liée à un prédécesseur exact
  déjà admis ; PostgreSQL + sauvegarde/restauration PASS (coordination selectors).
- Hors de ce lot : DTO de snapshot/patch, reconstruction et admission atomique,
  projection locale éditable, HTTP/CLI/UI et recette HTTPS (coordination root).

Contrat : `layrs.space-policy-rules.v3`, `rules` et `pathPolicies`, chaque politique
nommant `principalIds`, `read` et `write` (`default`, `rules` exact/subtree).
Actions serveur distinctes `ReadProjection` et `ScopedPathWrite`, utilisant les
grants existants `space.bootstrap` et `transaction.apply` pour préserver les
rôles gérés v1/v2. Aucun nouveau nom d'action JSON.
Une cible serveur `ScopedPath` ancre le chemin à `base_step_id` et `root_tree_id` ;
l'appelant doit vérifier cette base avant autorisation. Une projection de contenu
requiert aussi les grants `object.plan_fetch` et `object.fetch` ; l'écriture
requiert ces mêmes lectures plus `transaction.apply`. Tout deny est prioritaire.
L'autorisation n'est pas
une preuve de fermeture CAS ni une autorisation de modifier un sous-arbre entier.

## Versions Policy — contrat et checkpoint

`previousProductObjectId` nomme le Product canonique exact déjà admis (un upload
seul ne suffit pas, pas de prédécesseur nouveau dans le même batch). Il doit
appartenir au même Space et au même PolicyId, avec un autre PolicyVersionId.
`dependsOnProductIds` contient le **PolicyId logique**, pas un ObjectId. L'entrée
PolicySet contient les dépendances directes triées `[rulesEvidence, previousProduct]`.
Le serveur conserve l'ancien Product, installe les arêtes vers les règles et le
prédécesseur, puis recontrôle admission et disponibilité dans la transaction.
L'admission reste inerte ; seule la mutation explicite de PolicyRoot active la version.

Le validateur Sync accepte uniquement l'auto-référence **logique** d'une Policy
munie d'un prédécesseur physique différent ; les cycles entre Products et les
identités dupliquées restent refusés. Le serveur vérifie la provenance réelle.

Vérifications au 2026-09-21 : nouveau test serveur de prédécesseur 1 PASS,
contrat versions 2 PASS, régressions `sync_v3_contract` 26 PASS ; Clippy strict
`layrs-sync` + `layrs-store-server`, libs/tests, PASS (26,86 s).
Recette PostgreSQL `native_sync_v3` + backup **PASS 75,45 s**, session 32413 ; log
privé `tmp/policy-version-pg-20260921-recheck.log`. Compilation 11,58 s. Sont
qualifiés : refus mauvaise version/logical ID/dépendance et prédécesseur seulement
uploadé, admission inerte, activation avec fermeture exacte, relecture de l'ancien
Product inchangé, replay exact. Sauvegarde : 586 objets, 853 767 octets canoniques,
52 Steps, 9 runs annulés, 3 corrections de commentaires ; backup 5471 ms,
restauration 7014 ms, fenêtre CAS 5 s, recette backup totale 19045 ms.

Historique des incidents : premier lancement arrêté à compilation sur un appel
privé du helper. Le suivant a révélé un `FOR SHARE` exigeant UPDATE sur Products
immuables (preuve PostgreSQL `permission denied for table products`). Remplacé
par lecture simple sous verrou CAS déjà acquis, avec FK physique au commit ;
aucun élargissement de droits. La fixture échouée
`layrs_sync_test_ae3fb3ce668b4715b467ea940cfa26aa`, son rôle runtime et son CAS
temporaire ont été identifiés par les reçus propres à ce test puis supprimés.
Le test final a exécuté son nettoyage normal.

Pas de migration : `products.previous_product_object_id` reste réservé au modèle
Layer de la baseline ; le précédent Policy est attesté par l'enveloppe canonique
et les arêtes `product_dependencies`/`canonical_object_edges`, vérifiées et
conservées par le backup existant. L'intégration Worktree partiel complète reste
à qualifier par root ; ces tests ne remplacent pas la recette HTTP/CLI/UI.

Clippy final après correction des droits : sync/store-server libs/tests PASS
(2,44 s), cible serveur `native_sync_v3` PASS (17,07 s). Selectors rapporte une
recette client HTTPS Policy versionnée PASS 28,49 s, incluant lecture exacte des
règles, refus opaque du membre restreint, perte de réponse/reprise et restauration
de la politique gérée ; sa preuve détaillée reste dans son journal.

## Audit indépendant de la publication partielle — checkpoint initial

Sources auditées en lecture seule : `path_patch_service`, `path_patch_scope`,
`transaction_authorization`, `transaction_store`, `plan_store` et catalogue CAS.
Le quota est bien recontrôlé au catalogue et à la disponibilité, même si le plan
initial est ancien. Pas de contournement de droits identifié dans cette lecture.
Limite concrète remontée : le rejeu exigeait la relecture des Trees avant lookup
du reçu. Root ajoute une liaison durable du digest patch au snapshot pour lever
cette dépendance ; aucun correctif production effectué par cet agent dans ce lot.

Nouveau helper `support/path_patch_security_drill.rs` écrit, pas encore exécuté :
refus atomique d'un batch mixte, concurrence de deux View CAS, rename fichier
autorisé comme paire delete/create, révocation puis rétablissement de droits et
rejeu exact des reçus terminaux. Root raccorde le helper ; Gates/perte de réponse
attendent ses paramètres HTTP et la barrière du harnais. Ne pas compter ces tests
comme réussis avant leur résultat réel.

## Publication partielle et classification du namespace — qualification finale

2026-09-21 : recette HTTPS `native_clients_v3`, mode
`LAYRS_TEST_CONTENT_ACL_ONLY=1`, **PASS 35,62 s**, compilation 34,18 s,
session 21044 terminée avec code 0. Log privé :
`tmp/path-patch-security-20260921-combined.log`. La fixture isolée et son rôle
PostgreSQL ont été nettoyés par le harnais.

Le helper `support/path_patch_security_drill.rs` prouve désormais :

- Un batch comportant un chemin autorisé et un chemin interdit est rejeté
  atomiquement, sans Step admis, View déplacée ni nouvel événement.
- Deux requêtes concurrentes sur la même View ont un seul gagnant ; le perdant
  reçoit un rejet durable. Les deux reçus se rejouent exactement.
- Le renommage autorisé d'un fichier est une paire delete/create atomique ;
  les identités des fichiers masqués restent inchangées.
- La révocation après préparation refuse la requête ; rétablir les droits ne
  transforme pas ce reçu rejeté en publication. Un reçu déjà appliqué reste
  rejouable par le membre actif après révocation des droits de contenu.
- Des octets de transport volontairement invalides sur ce rejeu prouvent que
  le reçu est retrouvé avant décodage ou relecture du contenu.
- Une Layer protégée bloque la publication. La perte déterministe d'une réponse
  HTTP après commit, puis les réessais exacts, ne déplacent la View qu'une fois.

Le lot client raccordé par root passe dans la même recette : collision locale
sur chemin protégé refusée sans perte d'octets, capture immuable, perte de réponse,
réouverture du Store format 3 avec publication pending, reprise exacte, acceptation
de la nouvelle base et conservation d'une édition locale postérieure à la capture.

Liaison d'audit : `AuthorizationSnapshot` v2 porte obligatoirement
`requestBinding.pathPatchRequestDigest` ; v1 conserve sa sérialisation antérieure.
La validation v2 impose un principal, 1..1024 décisions `transaction.apply`,
uniquement des chemins uniques issus du même Step. Au rejeu, les chemins et le
Step audités doivent correspondre exactement au patch ; un reçu Applied avec
une décision refusée est rejeté. Hashs/JCS, identité, client, clé et digest restent
vérifiés. Les snapshots clients restent interdits à l'upload.

`POST /v3/spaces/:space/projection/access` classe 1..200 chemins triés uniques,
avec Step admis, PolicySet et epoch exacts. Réponse `ProjectionAccess` v1 :
principal, Space, Step, root Tree, PolicySet, epoch et une entrée par chemin
(`target` nullable, executable, contentReadable, writable). Aucun contenu ni
capability n'est renvoyé. Le parcours prouve la présence ou l'absence ; un
ancêtre fichier/symlink/conflit/tombstone refuse la requête entière. Les règles
ACL s'appliquent au chemin exact, même absent ; `.git`/`.layrs` ne sont jamais
des emplacements d'écriture. L'autorité et l'identité sont recontrôlées après le
parcours. Bornes : corps HTTP 1 Mio, chemin 4096 octets/profondeur 128, métadonnées
64 Mio au total, objet Step/Tree 1 Mio. Les noms restent visibles selon le contrat.

Preuves endpoint HTTPS : présence et absence, y compris parent absent, droits
read/write exacts, refus sous ancêtre fichier, chemins dupliqués/201 chemins,
absence d'authentification et epoch périmé. Contrats Sync : 2 tests snapshots
(mutations réhachées incluses), 1 test borne/fermeture access, 26 régressions PASS.
Clippy strict sync/store-server libs/tests et cible serveur native_clients_v3
PASS 22,86 s. Backup de la recette : 140 objets, 94 253 octets canoniques,
12 Steps ; backup 1277 ms, restauration 2371 ms, exercice total 4927 ms.

Incident de recette : le premier scénario perte de réponse était placé après
activation d'une protection ; même minimumApprovals=0 conserve le contrôle
d'admission préalable du Step. Déplacé avant le scénario Gate, sans changement
production. Deux essais ont donc échoué puis nettoyé leur fixture ; un autre
essai s'est arrêté à compilation pendant les éditions parallèles du Store.

Limites restantes de ce lot : endpoint patch limité aux fichiers ordinaires et
16 Mio fournis ; pas d'écriture aveugle, remplacement opaque de dossier, publication
de conflit ou symlink. Un nouveau Step destiné à une Layer protégée nécessite le
parcours de revue/admission approprié ; le refus est qualifié, pas une approbation
de patch protégé. Le raccordement du namespace au Store/client est le lot selectors ;
le présent endpoint ne constitue ni une réservation durable ni une capability.
