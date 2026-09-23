# Reste à faire — audit du 23 septembre 2026

Les huit plans ne sont pas entièrement clos. Le [point de reprise](REPRISE.md)
et les [preuves courantes](measurements/partial-qualification-2026-09-23.md)
prévalent sur les anciens checkpoints. La projection partielle est livrée dans
son périmètre documenté ; elle n'est plus un choix produit en attente.

## Cases encore ouvertes

Après la recette Web/CLI/Desktop commune : **9 cases de qualification ouvertes**.

| Plan | Cases | Reste |
| --- | ---: | --- |
| T1 — restauration | 0 | Périmètre Windows/Linux qualifié |
| T2 — fusion | 2 | Plateformes/profils supplémentaires |
| T3 — historique | 0 | Références et accès qualifiés dans Web réel |
| T4 — synchronisation | 0 | Périmètre documenté qualifié ; extensions Store3 ci-dessous |
| T5 — revue | 0 | Parcours complet et revue limitée aux chemins autorisés livrés ; détails ci-dessous |
| T6 — CI | 0 | Preuve exacte et refus de Gate rendus dans Web |
| T7 — Git | 0 | Périmètre compatible et pilote Git public qualifiés |
| T8 — exploitation | 7 | Mesures et qualification externe |

Les sept clauses Web (T3.H7/V7, T4.P et sa validation agrégée, T5.V8, T6.V6,
T8 « même instance ») sont désormais qualifiées : recette commune réelle
PASS33,4s le22septembre. Deux Stores distincts, publications/réceptions dans les
deux sens CLI/Desktop, références exactes dans Web, commentaire Web relu CLI,
preuve exacte, Gate refusée sans événement et contrôles d'accès réels.
L'historique complet du compte restreint reste refusé ; la projection des noms
et chemins demeure accessible avec cadenas. Aucun contenu interdit téléchargé.

Les neuf critères restants sont :

1. **T2 : bornes sur toutes les plateformes visées.** Windows/Linux x86_64
   qualifiés ; macOS et autres architectures non qualifiés, exécution refusée.
2. **T2 : octets/IDs identiques pour tous les profils visés.** Matrice WASM12cas
   Text/Code Windows/Linux acquise ; autres plateformes et combinaisons ouvertes.
3. **T8 : budgets de performance calibrés.** Mesures locales, HTTPS100Mio/1Gio
   et99k fichiers disponibles ; répétitions sur environnement stable et seuils
   statistiques manquent. Le comparateur livré ne remplace pas cette calibration.
4. **T8 : incidents sur chaque plateforme.** ENOSPC réel Linux et PGDATA plein
   qualifiés, ainsi que processus/réseau/casse/permissions et reprise selon leur
   matrice. Disque réellement plein Windows encore non qualifié. Aucune panne
   matérielle ni WAL indépendant revendiqués.
5. **T8 : runbook testé par un opérateur indépendant.** Documentation livrée ;
   exercice avec une personne n'ayant pas écrit le code non réalisé.
6. **T8 : pilote encadré de 3–5 utilisateurs.** Qualification humaine manquante.
   Le pilote technique Git PyPA ne la remplace pas.
7. **T8 : installation sur machine vierge.** Compose jetable et construction
   MSI acquis ; installation/désinstallation réelle MSI non effectuée.
8. **T8 : installer, mettre à jour et récupérer.** Archives qualified6
   Windows/Linux : smoke8 et upgrade/rollback5→6 PASS. Linux sur volume natif,
   pas sur bind Windows. Reste installation réelle annoncée, notamment MSI.
9. **T8 : gates CI sur la matrice supportée.** Vérifications locales courantes
   réussies et artefacts vérifiés ; CI distante non exécutée/observée. Les anciens
   essais globaux échoués ne deviennent pas des commandes globales réussies.

## Développement restant, distinct des qualifications

[PARTIAL-FOLLOWUP.md](PARTIAL-FOLLOWUP.md) détaille le travail et ses dépendances :

Livré depuis le précédent audit : capture et publication en blocs1Mio,
1Gio/fichier, plafond de capture4Gio ; recette HTTPS1Gio PASS. Candidats sans
avance, revues/commentaires limités aux chemins autorisés, approbation/preuve
et intégration sur Layer protégée sont disponibles dans CLI/Desktop. Le test
réel vérifie désormais le parcours accepté après approbation, en plus des refus.
Fusion base/local/distant de fichiers ordinaires, brouillon persistant,
déplacements de dossiers et restauration de chemins historiques connus sont
livrés, avec revalidation des droits et reprise durable.

Extensions fonctionnelles encore ouvertes :

- Fusion partielle avec plusieurs têtes/bases et conflits structurels.
  Une divergence régulière contre une tête distante unique est désormais traitée.
- Transformations dossier/fichier et édition de liens dans une projection
  partielle. Les déplacements de dossiers ordinaires sont livrés.
- Restauration de chemins historiques inconnus de la projection actuelle :
  nécessite observation/autorisation actuelle et réservation avant création.
- Persistance des choix de formulaire Desktop avant application : le brouillon
  et ses variantes sont durables, mais les sélections du formulaire restent en
  mémoire jusqu'à validation.

Qualifications locales supplémentaires : lot complet4Gio et profil mémoire du
transport partiel non mesurés. Le test1Gio utilise des octets répétés. Le plafond
4Gio porte sur le scan de capture, incluant les fichiers matérialisés inchangés.
La relecture indépendante finale n'a pas pu être relancée après épuisement des
threads d'agents ; relecture racine et régressions finales documentées séparément.

L'historique complet, les commentaires non filtrables et les journaux CI bruts
restent refusés aux comptes restreints pour protéger les contenus. Les revues
limitées aux chemins disposent de leur contrat distinct ; ne pas enlever la
garde générale pour étendre ces lectures.

Autres limites du périmètre annoncé : synchronisation des révisions de définition
de Layer refusée, reprise durable zstd distincte du parcours Identity, symlinks et
mode exécutable non matérialisables sous Windows, UI multi-Worktrees reportée.
Git LFS/submodules/histoires non représentables sont refusés explicitement.
S3, GC en ligne/Windows/macOS, suppression d'histoire, ordonnanceur CI hébergé
et signature de publication ne sont pas devenus des capacités livrées.

## Preuves et reprise

[Journal T4](journal/T4.md), [intégration partielle](journal/PARTIAL-INTEGRATION.md),
[journal T8](journal/T8.md), [manifestes qualified6](measurements/partial-artifacts-2026-09-23.json).
Les archives qualified4/5 sont historiques. Les nouveaux paquets sont non signés ;
MSI vérifié mais non installé. Aucun push ou déploiement n'a été effectué.
