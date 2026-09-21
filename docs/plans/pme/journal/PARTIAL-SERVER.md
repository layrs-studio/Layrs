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

## Audit indépendant de la publication partielle — en cours

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
