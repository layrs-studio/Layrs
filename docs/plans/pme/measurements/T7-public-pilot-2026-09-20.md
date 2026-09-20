# Qualification Git sur deux dépôts publics

Recette Windows x64, binaire debug, poste partagé, 20 septembre 2026. Aucun code, hook, filtre ou sous-module de ces projets n'a été exécuté. Les clones bare complets et artefacts restent dans `tmp/t7-*-20260920` ; aucun dépôt externe n'a été modifié. Les empreintes de tous les fichiers de chaque miroir sont identiques avant/après.

## Projet compatible : PyPA pyproject-hooks

Source officielle [PyPA](https://github.com/pypa/pyproject-hooks), commit figé `184c9f56a87ede1276a5b33f5024204adb792cd2`. [README](https://github.com/pypa/pyproject-hooks/blob/184c9f56a87ede1276a5b33f5024204adb792cd2/README.rst) et [licence MIT](https://github.com/pypa/pyproject-hooks/blob/184c9f56a87ede1276a5b33f5024204adb792cd2/LICENSE) vérifiés avant la recette ; licence et provenance conservées dans l'import. [Mesures détaillées](T7-public-pyproject-hooks-2026-09-20.json).

| Vérification | Résultat |
|---|---|
| Historique des branches/tags, tous leurs ancêtres | 475 commits → 475 Steps |
| Références importées | 10 branches et 23 tags, dont 10 annotés |
| Octets des objets Git sélectionnés | 2 939 257 |
| Dry-run / installation | 1,005 s / 57,641 s |
| Second import | 1,367 s, même Operation, aucun doublon |
| Matérialisation de git/main | 9,784 s, 71 fichiers, identité Git de chaque blob vérifiée |
| Export | 7,083 s, 476 commits et 34 références |
| Validation Git | `git fsck --full --strict` PASS, environ 0,08 s |
| Store natif / export Git | 11 958 806 / 1 409 253 octets mesurés |

Le commit et la référence supplémentaires de l'export correspondent à l'initialisation native vide. Pour chacun des 475 commits importés, la recette compare la provenance brute au véritable objet Git source, l'identité de l'arbre exporté à celle de l'arbre source, ainsi que les parents reconstruits. Tous les ensembles de parents sont identiques, et aucun changement d'ordre n'a été observé. Chaque branche/tag sélectionné pointe sur la conversion du même Step ; les octets originaux des tags annotés sont conservés comme provenance.

Limites visibles : 141 références de pull requests GitHub sont hors du périmètre branches/tags, avec avertissement explicite ; aucun ancêtre des références sélectionnées n'a été tronqué. Les signatures de 216 commits restent des données de provenance et sont retirées des commits reconstruits : aucune signature du nouvel historique n'est revendiquée. Ce sont des mesures ponctuelles, pas une garantie de débit ou de mémoire globale.

## Projet explicitement refusé : Pallets ItsDangerous

Source officielle [Pallets](https://github.com/pallets/itsdangerous), commit figé `672971d66a2ef9f85151e53283113f33d642dabd`, [README](https://github.com/pallets/itsdangerous/blob/672971d66a2ef9f85151e53283113f33d642dabd/README.md) et [licence BSD-3-Clause](https://github.com/pallets/itsdangerous/blob/672971d66a2ef9f85151e53283113f33d642dabd/LICENSE.txt). [Rapport du refus](T7-public-itsdangerous-2026-09-20.json).

La sélection complète de ses 2 branches et 32 tags contient 678 commits. Le scan refuse `Git submodule/gitlink`, sans créer de Store natif. Le commit historique `f7d6c8a8bc5b202ec43ea8e9e70db906f14696ad` contient le gitlink `docs/_themes`, mode160000, objet `1cc44686f0f9dad27cce2c9d16cf42f97bc87dbd`. Son retrait ultérieur ne rend pas l'historique complet compatible. Aucun chemin ou commit n'a été filtré pour transformer ce refus en réussite.

## Reproduire

Cloner les dépôts officiels avec `git clone --mirror --no-recurse-submodules` dans leurs dossiers `tmp/t7-<projet>-20260920/source.git`. Le script refuse un HEAD différent du SHA fixé. Sur Windows, lancer sous le compte propriétaire du clone ; aucune exception globale `safe.directory` n'est ajoutée.

```text
node scripts/test-git-public-pilot.mjs --project=pyproject-hooks
node scripts/test-git-public-pilot.mjs --project=pyproject-hooks --resume
node scripts/test-git-public-pilot.mjs --project=itsdangerous
```

La dernière commande doit sortir avec code2 et rapport `refused`. Les commandes CLI intermédiaires et leurs réponses complètes sont conservées dans le dossier de recette ; `pilot-progress.json` indique la dernière étape. `--resume` réutilise seulement les étapes terminées et vérifie l'empreinte source, puis rejoue les contrôles de graphes, fichiers, références et fsck.

Cette recette qualifie techniquement un projet public réel compatible. Elle ne constitue ni l'import d'un dépôt privé utilisateur, ni un pilote humain dans une PME, ni la qualification de tous les projets Python ou de tous les systèmes d'exploitation.
