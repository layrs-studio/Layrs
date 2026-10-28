# Union ordinaire : preuve disponible pour l'admission autonome

Investigation de sources du 28 octobre 2026, sans exécution. Ce point concerne
l'admission autonome après publication B ; il ne bloque pas les deux commits B
du lot en cours, qui disposent encore de la racine familiale complète.

La racine familiale ExactProtection conserve toutes les représentations et
leurs manifests canoniques. Ready et son reçu engagent le rowset exact trié
`(FileId, ManifestId, bytes)`. En revanche, C `projection_content` ne conserve
qu'une représentation par FileId, et la closure copiée peut omettre les bases
sélectionnées qui ne figurent dans aucune capture/restauration.

Contre-exemple : B sélectionne F sous M0, sans mapping historique copié pour F.
Le checkout courant représente les mêmes octets F sous M1 (chunking différent).
L'union protégée contient M0 et M1. Si M1 est premier dans le tri, C content et
le journal checkout nomment tous deux M1 ; aucune ligne privée C ne donne M0.
Les digests de l'union et de la racine ne permettent pas de retrouver cet ID.

La validation avec famille dispose donc de suffisamment de données : relire
la racine complète, recalculer l'engagement exact, imposer les mappings historiques
et checkout exacts, la couverture des FileIds sélectionnés, les longueurs
cohérentes et les autres engagements. Cela prouve toute l'union engagée, sans
prétendre retrouver le mapping historique original de B parmi plusieurs
représentations valides du même FileId.

Le lecteur privé seul `read_admitted_import(c, marker)` n'a pas ces données.
Il faut résoudre explicitement cette frontière avant l'admission ordinaire :

- soit conserver un témoin immuable borné des triples complets lors de l'admission,
  issu de la vraie racine C/famille, et contrôler son count/digest à chaque lecture ;
- soit refondre l'interface de validation pour lui fournir la vraie union familiale
  authentifiée, en traitant les dépendances, snapshots et verrous correspondants.

Un témoin de 100 000 représentations ne tient pas dans le payload d'admission
actuel borné à 64 KiB. Une table normalisée est une option, mais impose un choix
explicite de version de schéma et de compatibilité ainsi qu'un budget de métadonnées.
Ne pas ajouter silencieusement une table à une empreinte de schéma existante,
réduire arbitrairement la capacité contractuelle ou utiliser un mapping FileId
quelconque comme preuve. Cette note ne tranche pas encore la migration.

Sources centrales : `managed_export.rs::include_selected_bases`,
`destination_ready/data.rs::prepare_suffix`, `exact_protection.rs::Protection`,
`destination_ready/autonomous/family_read.rs`, `autonomous/inventory.rs::require_union`,
`import_validation/destination_prefix.rs` et `destination_ready/historical.rs`.

## Décision d'implémentation du 28 octobre 2026

Retenir un schéma privé **10 distinct** pour l'admission d'origine Ordinary.
Le DDL et le chemin Provider8→9 restent inchangés ; aucune migration automatique
9→10. Le témoin local immuable contient les triples exacts
`(file_id, manifest_id, bytes)`, au plus 100 000 lignes et 15 000 000 octets
normalisés. Il provient de la vraie union C/famille et doit retrouver exactement
le count/digest Ready. Les mappings copiés et checkout, la couverture des bases
sélectionnées et la cohérence des longueurs restent contrôlés séparément.

La transition C8→10 doit être atomique : admission et archive existantes,
témoin complet, SourcePublished réel, retrait du journal opérationnel et version
10 dans une même transaction. La relecture privée doit retrouver toute la preuve.
Un permit fermé distingue exactement Provider9 et Ordinary10 et revalide version,
empreinte et FORMAT. Refuser l'origine Ordinary sous9 et le témoin sous9.

Le record d'admission v1 et son `metadata_layout_version=1` conservent leur
sémantique admission/archive ; Ready engage déjà les triples du témoin. FORMAT
ne porte pas de version privée et ne change pas. Le préfixe entrant décrit la
source B ; un export ultérieur C→D doit déclarer la vraie version10 de C.
Le témoin n'est pas un rowset hérité de la closure : D construit son propre témoin.

Le budget Ordinary doit charger explicitement le coût borné et réel du témoin.
Les quotas des plans persistés restent immuables : refuser l'enveloppe insuffisante,
sans augmentation ni consommation implicite d'une marge réservée séparément.
Cette décision décrit le prochain lot ; aucune qualification C10 n'est acquise.
