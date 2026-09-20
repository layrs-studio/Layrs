# HTTPS 99 000 fichiers — recette complète réussie

20 septembre 2026 : **PASS3827,27s** (63min47s), un test effectivement exécuté,
zéro échec, zéro ignoré, trois helpers filtrés. Session26811 terminée avec code0,
nettoyage de la fixture terminé. [Rapport brut](T8-tls-volumes-streaming-2026-09-20.json)
et [compteurs mémoire complémentaires](T8-tls-volumes-streaming-memory-2026-09-20.json).

Vrai HTTPS vérifié, PostgreSQL isolé, un client source et un clone indépendant.
99 000 fichiers de256octets,99 répertoires,25 344 000octets logiques mais seulement
200 contenus distincts. Profil **debug Windows**, caches chauds et autres tests/
compilations en parallèle. Ce résultat prouve le parcours et ses reprises de
leases longues ; il ne mesure pas le débit des archives release ni une charge
de99 000 fichiers uniques. Le plafond compte également les répertoires : un jeu
de100 000 fichiers plus leurs répertoires dépasse le budget de100 000 entrées.

| Étape | Durée mesurée |
|---|---:|
| Génération | 69,189s |
| Initialisation native | 159,509s |
| Provision et publication | 308,774s |
| Clone indépendant | 1645,222s |
| Tous les octets clonés et diff propre | 119,406s |
| Réception sans changement | 261,615s |
| Capture/publication d'un fichier modifié | 416,055s |
| Réception de la modification | 678,209s |
| Tous les octets finaux et diff propre | 108,764s |

Somme des neuf phases :3766,743s ; le temps du test inclut aussi le montage et
le nettoyage. Les têtes initiales du clone, l'absence de changement inutile,
la publication appliquée, la matérialisation incrémentale et les octets de
chaque fichier sont vérifiés par `support/sync_volume_drill.rs`.

Binaire de test copié avant les longues recettes, pour éviter les reconstructions
concurrentes : `tmp/native-streaming-20260920-204610.exe`, SHA256
`0f847e0db3b1a7530c596e6671cf5737c809650283b3fdaf9fb9545ff9601c88`.
Log local : `tmp/tls-volume-streaming-20260920.log`. Les correctifs ultérieurs
de style Rust et les nouvelles recettes n'impliquent pas que ce binaire debug
soit l'archive release5ab6994. Les preuves de distribution restent séparées.

Le compteur OS de pic mémoire du processus a atteint **423 661 568octets
(404,04Mio)** ; pic de mémoire paginée416 231 424octets. Serveur de test et clients
partagent le processus ; PostgreSQL et cache filesystem OS sont exclus. Le
sondage a commencé après le clone mais lit les compteurs de pic sur la vie du
processus, qui incluent les phases antérieures ;1321 observations, fin du
processus constatée. Un dernier pic inférieur à l'intervalle d'une seconde
pourrait être manqué. Le rapport brut de recette ne mesure pas lui-même la
mémoire ; ce complément ne modifie pas ses limitations déclarées.

Les essais99k antérieurs restent conservés : dépassement de budget, expiration
de lease, puis arrêt volontaire pendant la pause utilisateur. Aucun résultat
précédent n'est réécrit comme réussi. Cette mesure finale et le comparateur de
benchmarks ne remplacent pas la calibration de budgets de régression sur une
machine stable ; le critère T8 global de performance reste ouvert.
