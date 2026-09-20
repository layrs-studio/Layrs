# Croissance des captures automatiques

Recette Windows/debug du 20 septembre 2026 : un fichier de 64 Kio, 32 modifications à contenu distinct puis les mêmes 32 contenus revisités. L'appel `capture_with_reason(AutomaticIdle)` est celui du Desktop ; l'ordonnanceur et sa période de calme ne sont pas simulés. Avant et après les changements, 64 appels sans modification, soit 128 au total, n'ajoutent aucun objet, aucune observation, aucune Operation et aucun octet au Store.

| Mesure en octets | Initialisation | Après 32 contenus nouveaux | Après 32 contenus revisités |
| --- | ---: | ---: | ---: |
| CAS | 67 802 | 2 239 802 | 2 282 106 |
| Fichier SQLite principal | 618 496 | 1 028 096 | 1 400 832 |
| Tous fichiers du Store | 719 246 | 3 300 846 | 3 715 886 |
| Steps / Operations | 1 / 1 | 33 / 33 | 65 / 65 |
| Chunks / manifests / Trees | 1 / 1 / 1 | 33 / 33 / 33 | 33 / 33 / 33 |

Les 32 contenus nouveaux ajoutent 2 172 000 octets CAS. Les 32 reprises n'ajoutent aucun contenu/manifeste/arbre : les 42 304 octets CAS supplémentaires proviennent des Steps et Views historiques. Les Operations et observations conservées occupent aussi SQLite. La déduplication ne rend donc pas gratuite une capture réellement différente, même si elle revient à des octets connus.

Assertions : anciens contenus relus exactement, 64 observations `automatic_idle` et 65 Steps présents, fsck propre, no-op sans croissance et comptes typés de fragments identiques. Les nombres de pages SQLite sont mesurés, pas figés comme contrat. Les durées (16,685 s nouveaux contenus ; 25,327 s reprises ; 62,043 s exercice complet) incluent les coûts debug et vérifications. Aucun débit promis ni mesure de blocs physiques alloués.

Reproduction : `cargo test --offline -p layrs-client-core --test native_capture_growth -- --nocapture`. Nouveau répertoire temporaire UUID, nettoyage limité à cette fixture après succès. [JSON enregistré](T8-auto-capture-growth-2026-09-20.json), [test](../../../../crates/layrs-client-core/tests/native_capture_growth.rs).

Le stockage serveur a ses propres réservations et quotas logiques ; cette mesure locale ne les remplace pas. Les [seuils de capacité](../../../../deploy/selfhost/STORAGE-BUDGET.md) et la recette `v3_pg_quotas` qualifient séparément les refus d'admission et alertes. Toutes les captures restent retenues : aucun pruning implicite, âge limite ni promesse de volume constant.
