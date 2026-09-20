# Critères restants — audit du 20 septembre 2026

Lecture croisée des huit plans et de leurs derniers journaux, après HTTPS100Mio/1Gio, recette intégrée603,59s et trois frontières de redémarrage serveur20,87s. **18 cases étaient ouvertes ; 17 restent ouvertes après actualisation de la seule génération des jeux T8.** Aucune autre case n'est cochée par cet audit. Les historiques de journaux restent conservés ; le [point de reprise courant](REPRISE.md) prévaut sur leurs anciens checkpoints.

| Plan | Cases ouvertes | Nature du reste |
| --- | ---: | --- |
| [T1](01-restauration-et-annulation.md) | 0 | Périmètre Windows/Linux qualifié ; limites annoncées maintenues |
| [T2](02-fusion-et-conflits.md) | 2 | Plateformes/profils supplémentaires |
| [T3](03-historique-markers-et-releases.md) | 2 | Qualification navigateur Web |
| [T4](04-synchronisation-et-acces.md) | 3 | Projection partielle à concevoir et qualification Web |
| [T5](05-revue-equipe-et-protections.md) | 1 | Qualification navigateur Web |
| [T6](06-ci-preuves-et-integrations.md) | 1 | Qualification navigateur Web |
| [T7](07-import-export-git.md) | 0 | Périmètre Git compatible qualifié ; extensions distinctes |
| [T8](08-installation-exploitation-et-fiabilite.md) | 8 | Mesures, incidents, installation et qualification externe |

## Liste exacte des 17 critères ouverts

1. **T2 : qualifier les bornes sur toutes les plateformes visées.** Windows/Linux x86_64 acquis ; macOS/autres architectures non qualifiés et exécution refusée. Une autre plateforme peut nécessiter un hôte supplémentaire, pas seulement un lancement de tests.
2. **T2 : mêmes octets/IDs entre plateformes pour tous les profils visés.** Matrice WASM12cas Text/Code Windows/Linux acquise ; macOS/autres architectures et combinaisons hors matrice restent ouverts.
3. **T3.H7 : CLI/Desktop puis Web, accès métadonnées/contenus.** Code partagé, API/CLI/Desktop et droits qualifiés ; parcours navigateur Web non exécuté.
4. **T3.V7 : références/cibles exactes sur le second client et rendu Web identique.** Sync réelle acquise ; rendu Web à qualifier.
5. **T4.L : fichiers masqués et projection partielle.** Fonctionnalité manquante : modèle de Worktree partiel, complétude explicite, mutations et reprise sûres. Les lectures fines PolicyRoot v2 existent ; elles ne permettent pas de retirer la garde d'un Store complet. Choix produit encore nécessaire, voir REPRISE.
6. **T4.P : deux Stores, CLI puis Desktop/Web.** Deux Stores réels et CLI/Desktop acquis ; navigateur Web et recette combinée des surfaces manquent.
7. **T4 validation : tous les scénarios T4.P avec deux clients réels.** Case agrégée retenue pour le point Web précédent ; grands transferts et redémarrages ne sont plus des manques.
8. **T5.V8 : Web réel et CLI contre V3.** CLI/Desktop et protections acquises ; navigateur Web manquant.
9. **T6.V6 : Gate serveur bloque API/CLI, affichage cohérent Web.** Refus serveur/API/CLI et runner réel acquis ; affichage navigateur Web à qualifier.
10. **T8.D : mesurer scan/capture/switch/diff/sync/mémoire/disque, fixer des budgets de régression et corriger les blocages PME.** Mesures locales et HTTPS100Mio/1Gio disponibles ; recette99k complète PASS3827,27s et mémoire complémentaire conservée. Comparateur de régression livré et testé, mais environnement stable, répétitions et seuils restent à calibrer ; aucune gate de performance statistiquement qualifiée.
11. **T8.D : incidents sur chaque plateforme supportée.** ENOSPC réel CAS/capture/matérialisation Linux, processus/réseau, casse/liens/permissions et reprise Windows/Linux ont des preuves distinctes. [PGDATA plein désormais qualifié](PG-DISK-FULL.md) : vraie admission Layrs refusée sans état métier partiel, reprise/rejeu exact après libération du seul filler,112,456s. Disque réellement plein Windows encore non qualifié ; aucune panne matérielle ni WAL indépendant revendiqués. Conserver une matrice précise, sans extrapoler un scénario aux autres.
12. **T8.D : documentation testée par une personne n'ayant pas écrit le code.** Runbooks disponibles ; exercice avec opérateur indépendant non réalisé.
13. **T8.D : pilote encadré de 3–5 utilisateurs.** Qualification humaine externe manquante. Le pilote Git public technique ne la remplace pas.
14. **T8 validation : installation sur machine vierge sans code ni SQL manuel.** Compose jetable qualifié et MSI construit ; aucune installation MSI réelle sur machine vierge, aucun test d'installation/désinstallation promis comme acquis.
15. **T8 validation : deux clients HTTPS, Web/CLI/Desktop sur la même instance.** API/CLI/Desktop acquis ; navigateur Web indisponible lors des vérifications.
16. **T8 validation : installer, mettre à jour et récupérer la version annoncée.** Archives qualified4 Windows/Linux : smoke et upgrade/rollback CLI isolés réussis, même source Rust/Cargo.lock ; Linux qualifié sur volume natif, bind Windows Docker non qualifié. Reste l'installation réelle annoncée sur machine vierge, notamment MSI. Packages construits ne signifient pas installés ; voir les manifestes courants.
17. **T8 validation : Gates CI correspondant à la matrice supportée.** Qualifications locales Windows/Linux, PG/HTTPS et Desktop réelles disponibles ; ne pas présenter les premières commandes globales échouées comme vertes après corrections ciblées. Vérification finale des gates/artefacts courants et exécution CI distante non observée restent distinctes.

Les sept clauses comportant Web partagent principalement un même blocage de qualification : aucun navigateur disponible dans cette session. Ce n'est pas la preuve de sept fonctions de production absentes. macOS, pilote humain, identité de signature et machine vierge sont également des qualifications ou moyens externes non disponibles ; aucune promesse n'est ajoutée pour les contourner.

## Limites fonctionnelles explicites hors de ces cases

- Révisions de définition de Layer : synchronisation toujours refusée explicitement (`remote_products.rs`), contrairement aux LayerHeads/Choose déjà livrés.
- Reprise persistante de représentation zstd : hors du parcours Identity qualifié ; ne pas confondre téléchargement zstd ponctuel et checkpoint réutilisable.
- Windows : symlink/mode exécutable non matérialisables refusés ; UI multi-Worktrees reportée. Autres formes de conflits que les sous-ensembles natifs documentés refusées.
- Git : acquisition incomplète à recommencer, conversion relancée depuis cache ; progression fine SQLite et volumes supplémentaires distincts. LFS/submodules/histoires non représentables refusés selon le contrat, pas filtrés silencieusement.
- S3, GC en ligne/Windows/macOS, suppression d'histoire, ordonnanceur CI hébergé et signature de publication ne deviennent pas des capacités livrées par ces qualifications.

## Corrections documentaires ciblées

Appliquées dans cet audit : introduction T4 et distinction lectures ACL/Worktree partiel ; résumé courant de STREAMING ; état courant du journal T7 (pack et pilote PyPA) ; case T8 de génération des jeux. La synthèse T4.P a déjà été corrigée par root pendant l'audit.

À conserver comme historique ou à précéder d'un résumé courant : blocs « Audit des cases » de T5/T6 (brouillons, corrections auditées, pending/cancel et deux tentatives distinctes désormais livrés), premiers checkpoints du journal T5. Le journal T6 décrit explicitement la clôture V3 plus bas ; sa phrase intermédiaire « V3 reste ouverte » ne décrit plus l'état courant. Aucun historique ni ancienne mesure n'est supprimé. REPRISE reste géré par root et le journal T8 par son propriétaire.

Preuves principales : [qualification intégrée et PG](measurements/T4-T8-resumed-qualification-2026-09-20.md), [HTTPS1Gio](measurements/T4-streaming-1gib-20260920.md), [journal T2](journal/T2.md), [journal T4](journal/T4.md), [pilote Git public](measurements/T7-public-pilot-2026-09-20.md), [journal T8](journal/T8.md). Les recettes99k, packaging/upgrade qualified4 et PGDATA plein ont depuis terminé avec succès ; leurs preuves sont conservées séparément, sans fermer leurs critères composés encore incomplets.
