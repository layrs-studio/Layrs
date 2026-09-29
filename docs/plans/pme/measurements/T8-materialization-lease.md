# Lease de matérialisation longue

Le 20 septembre 2026, la recette HTTPS de99k chemins de root a échoué après
environ1646s avec un journal de matérialisation pending. Les données temporaires
ayant été nettoyées, l'expiration particulière de cette fixture n'est pas relue
dans SQLite. Le défaut de code est confirmé : lease300000ms acquise une seule
fois, puis backup/install/scan complet sans heartbeat ; la garde d'admission
refuse une lease expirée. La reprise possédait aussi un unique heartbeat initial.

Correction dans `scm_v1/materialize.rs`, `materialize/lease.rs` et le hook privé
de progression `CheckoutReader::capture_tracked_with_progress` : renouvellement
coopératif toutes30000ms, durée300000ms conservée, contrôle exact owner et toutes
les générations, transaction heartbeat atomique existante. Avant les mutations
de chaque chemin, les retraits de répertoires et chaque entrée des scans finaux,
le temps est vérifié. Renouvellement forcé aux frontières metadata/completion.
La même logique accompagne quarantaine, restauration et vérification de reprise.
La staging initiale précède le claim et reste sérialisée par le verrou checkout.

Le temps injectable appartient à la seam de tests `MaterializationFaults` ; aucun
paramètre CLI, configuration de TTL ni contournement des générations n'est ajouté.
Les opérations de production utilisent toujours l'heure système. Un saut de temps
ou un blocage individuel plus long que la lease provoque un refus et conserve les
backups/journaux ; il ne ressuscite pas une autorité expirée. La transaction
metadata exclusive n'est pas interrompue par un thread de renouvellement.

`native_materialize_lease` : deux tests avec six scénarios,12fichiers réels chacun.
L'horloge avance31s par consultation et les heartbeats SQLite montrent une durée
supérieure au TTL initial. Application, reprise avant et après commit gardent les
octets/Operations attendus et passent fsck. Trois pertes d'autorité (expiry sans
reclaim, vrai claim autre owner, vrai claim même owner/nouvelle génération) arrêtent
avant le deuxième backup, puis une reprise explicite restaure exactement l'ancien
checkout. Windows **PASS14,35s**. Régressions Windows : capture11 **PASS1,63s**,
materialize_types5 **PASS27,08s**, workspace12 **PASS5,91s** ; Clippy strict
store-local/lib et client/test **PASS15,78s**. Rerun Windows après prévention de
renouvellements répétés en cas de recul d'horloge : **2PASS16,25s**. Linux transmis
par fusion_native : leases2 **PASS11,39s**, symlinks4 **PASS68,45s**, types5
**PASS22,50s** (compilation avant ce dernier ajustement du cas de recul d'horloge).
Clippy strict Linux final sur libs et les trois cibles **PASS21,87s**, après cet
ajustement. Sources stables pour la troisième recette99k de root.

Cette preuve ne remplace pas la mesure99k complète et ne promet pas qu'une I/O
bloquée arbitrairement longtemps aboutit sans intervention.
