# Mesures locales : 10 000 et 99 000 fichiers

Mesures du 20 septembre 2026, Windows 11, i9-13900K, 64 Gio de RAM.
Les rapports JSON contiennent le SHA-256 du binaire et les durées brutes.

| Commande | 10 000 fichiers | 99 000 fichiers |
|---|---:|---:|
| Initialiser | 6,39 s | 18,28 s |
| État du dossier | 1,56 s | 13,52 s |
| Capture sans changement | 1,62 s | 14,39 s |
| Diff après modification | 1,56 s | 14,17 s |
| Capturer la modification | 4,49 s | 17,50 s |
| Changer de Layer, aller | 6,28 s | 58,62 s |
| Changer de Layer, retour | 6,55 s | 59,22 s |

Chaque dossier contient des fichiers de 256 octets, avec seulement 200 contenus
distincts répétés. Ces résultats mesurent surtout le parcours des chemins et la
réutilisation des contenus ; ils ne représentent pas un projet aux contenus tous
différents. Une seule exécution, caches système chauds et compilations concurrentes.
Une recette séparée mesure la mémoire du processus CLI sous Windows, toutes les
50 ms. L'état de 10 000 fichiers utilise un pic observé de 27,2 Mio de mémoire
résidente ; celui de 99 000 fichiers atteint 185,1 Mio. Les pics privés échantillonnés
sont respectivement 19,8 et 178,5 Mio. Les durées de cette seconde recette sont
1,71 et 19,14 s : la variation sous charge interdit une promesse stricte de latence.
Ces compteurs excluent les autres processus et les charges aux contenus uniques.
Voir [le rapport mémoire](T8-memory-2026-09-20.json) et
`scripts/measure-native-memory.ps1` pour reproduction.

La recette de clients natifs mesure aussi le clonage TLS et une réception sans
changement : 2 clients simultanés en 2,35 s, 10 clients en 3,30 s. Chaque client
possède son propre Store et vérifie les octets reçus. La fixture est petite,
les binaires sont en profil debug et les caches serveur sont chauds. Ce résultat
ne mesure pas dix publications simultanées ni la synchronisation de 99 000 fichiers.
Voir [le rapport de concurrence](T8-concurrent-clones-2026-09-20.json).

Une recette distincte vérifie désormais [2 et 10 publications simultanées](T8-concurrent-publications.md), avec réponse perdue/rejeu exact et conservation de toutes les divergences. Elle utilise également de petites fixtures ; la synchronisation des 99 000 fichiers n'est toujours pas mesurée.

Le cache de vérification est limité à une lecture d'arbre : chaque commande relit
et vérifie les objets utilisés. La corruption entre deux lectures reste détectée.
Avant ce changement, l'état de 10 000 fichiers prenait 8,63 s et le changement de
Layer 39,00 s dans cette même fixture synthétique. Ce sont des observations locales,
pas une garantie de performance sur d'autres machines.

La limite de 100 000 entrées inclut les dossiers : 100 000 fichiers plus leurs
100 dossiers sont explicitement refusés ; 99 000 fichiers plus 99 dossiers passent.
Les fichiers de 100 Mio et 1 Gio dépassent la limite actuelle de 64 Mio par fichier :
refus vérifié avant création d'Operation ou modification du fichier. Une PME utilisant
des fichiers plus gros nécessite encore un chantier de streaming et de budget mémoire.

Rapports : [avant et limites](T8-volumes-2026-09-20.json),
[après cache par lecture](T8-volume-cache-2026-09-20.json).
Reproduction : `node scripts/measure-native-volumes.mjs BINAIRE RAPPORT DOSSIER_NEUF 10000,99000 --skip-binaries`.
Le script conserve ses fixtures et refuse un dossier de destination existant.
