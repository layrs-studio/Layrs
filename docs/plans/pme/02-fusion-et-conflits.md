# T2 — Fusionner les projets de code et résoudre les conflits

Suivi d'exécution : [journal T2](journal/T2.md). Coordination et reprise : [REPRISE.md](REPRISE.md).

Statut au 20 septembre 2026 : **T2.C et T2.U livrés pour le sous-ensemble natif qualifié ; T2.M global partiel**. Fusion/résolution natives, blocs texte persistants, renommages couplés, surfaces CLI/Desktop, transport et revue sémantique T5 qualifiés. Le contrat code natif1.1 est livré ; le manifeste historique1.0 reste immuable. Restent les plateformes non qualifiées et les formes explicitement hors sous-ensemble. Priorité : P0. Les cases correspondent aux preuves du journal, sans prétendre qualifier toutes les plateformes ni toutes les formes de conflit.

| Sous-plan | Acquis vérifiés | Travail restant |
|---|---|---|
| T2.C | Plan pur, DAG/arbres, bundles/receipts, gardes ; delta replay/revert ; blocs mono-placement et termes complets multi-placements | Toute extension future hors formes publiées exige validation de fermeture et contrats |
| T2.M | Fusion structurelle/texte/code1.1, variantes/remplacement exact ; blocs partiels ; renommages exacts/ambiguïtés couplés ; WASM Windows/Linux ; matrice12cas identique entre OS | macOS/autres architectures, formes virtuelles/re-fusion riche ; mouvements hors identité exacte conservateurs |
| T2.U | Step/View/Operation/receipt atomiques ; CLI/Desktop blocs/renommages ; TLS partiel/réouverture/résolution distante ; revue T5 versions conflictuelles refusées puis finale réapprouvée | Sous-ensemble livré ; étendre les surfaces seulement avec les futurs contrats pris en charge |

## Résultat attendu

Deux modifications indépendantes d'un projet de code fusionnent automatiquement. Une incompatibilité produit un Step conservant toutes les variantes et reste résoluble, même sans Lens spécialisée. La résolution partielle est enregistrable et synchronisable.

## Point de départ de l'audit initial et références

- Réconciliation texte/raw et commandes de résolution présentes dans `crates/layrs-client-core/src/access_registry/weaves/` et `crates/layrs-lens-text/`.
- Les Lenses code/image exposent preview/diff mais pas de réconciliation ; le dispatch actuel ne fournit pas le comportement de repli cible.
- Le planner `crates/layrs-weave/` et les sessions actuelles ne représentent pas encore le graphe cible.
- Autorités : [ADR 0009](../../decisions/0009-conflicts-as-tree-objects.md), [ADR 0010](../../decisions/0010-weaves-and-lens-reconciliation.md), [scénarios de conflits](../../contracts/scm-v1/conflict-scenarios.md), contrats Lens dans `schemas/lenses/v1/`.

## Périmètre et décisions

Inclus : fusion à trois voies de texte/code, conflits structurels et binaires, choix de variante, édition manuelle, résolution partielle et UI commune.

La fusion de code initiale traite les octets et lignes selon un profil déterministe explicite. Une fusion sémantique AST n'est pas nécessaire à la V1 PME. Pour une image ou un format inconnu, conserver les variantes et proposer les choix applicables ; ne pas transformer arbitrairement les octets en texte.

Une fusion réussie avec conflits crée un Step. Aucune session `continue/abort` ne doit devenir une source de vérité. Un abandon d'édition UI ne supprime pas un résultat déjà accepté ; revenir en arrière passe par T1.

## Plan d'exécution

### T2.C — Définir le moteur partagé

- [x] Définir une entrée pure : têtes exactes, bases, arbres, profil de résolution, Lens pins et limites. Résoudre les sélecteurs avant cette entrée.
- [x] Définir sortie proposée, objets à installer, conflits, receipt, état sans changement et erreurs bloquantes, en réutilisant les contrats canoniques.
- [x] Séparer plan pur, admission et matérialisation T1.C ; preview ne modifie ni références, ni fichiers, ni file de sync.
- [x] Fixer l'API d'application d'un delta sur une autre base, réutilisable par revert T1 et replay T3, sans confondre ces opérations avec Weave.
- [x] Fixer les méthodes structurelles applicables au sous-ensemble courant et leur contrat CLI/SDK/UI : choix de terme, suppression et remplacement exact ; autres formes refusées.

**Sortie T2.C acquise pour le sous-ensemble courant :** contrat de réconciliation et fixtures utilisables sans serveur ni checkout (`layrs-weave/src/{reconciliation,graph_merge}.rs`, `graph_merge/replay.rs`). Les formes non prises en charge sont refusées explicitement.

### T2.M — Implémenter fusion et résolution pures

- [x] Calculer les bases depuis `StepDag`, jamais depuis les relations parent/enfant des Layers.
- [x] Traiter absence de base et bases multiples : base virtuelle déterministe ou conservation explicite des termes selon ADR 0010 ; ne jamais choisir par date ou ordre SQL.
- [x] Implémenter les cas triviaux, puis fusion texte à trois voies ; garder les conventions de fin de ligne et signaler les cas non représentables.
- [x] Fournir à la Lens code un reconcile et un manifeste versionné propres : `reconcile_code`, `reconciliation_manifest`1.1, capability/méthodes et pin effectif via `native_manifest`, exécutés dans le guest WASM. L'ancien manifeste1.0 conserve son comportement analyse/diff et son pin immuable. Les statuts de son viewer historique ne décrivent pas le protocole natif de résultat/conflit ; ils ne sont pas élargis artificiellement. Contrat1.1/refus pin1.0/variantes binaires/UTF-8 : **3 tests intégration PASS**, plus **2 tests legacy PASS** ; qualification WASM inter-OS existante inchangée. Aucune migration automatique des anciens receipts/pins.
- [x] Construire les ConflictObjects edit/edit, add/add, edit/delete, file/directory, symlink et modes ; distinguer absence et fichier vide, conserver toutes les variantes du sous-ensemble à un placement.
- [x] Étendre ces objets aux déplacements ambigus et conflits riches/multi-placements du sous-ensemble natif ; aucune variante non prise en charge ne doit être tronquée. Canon multi-placements, fermeture et validation des vrais originaux/DAG, occurrences couplées et sélection atomique d'un terme complet ; Store/CLI/Desktop et Sync qualifiés. TLS dossier déplacé/modifié → clone/réouverture → résolution B → A et quatre falsifications refusées, suite **PASS 297,84 s**. Formes virtuelles/autres politiques et re-fusion d'un Conflict riche/multi-placement refusées explicitement.
- [x] Définir une politique de renommage déterministe. Sous-ensemble livré : une base/deux côtés, fichier/symlink, identité exacte type/objet/mode unique, source disparue/destination nouvelle. Ambiguïtés explicites et chemins concurrents regroupés dans un Conflict ; aucune similarité ni choix arbitraire. Un choix natif de terme complet applique tous les placements ensemble ; lecture/comparaison/remplacement mono-fichier refusés sur cette forme.
- [x] Résoudre une variante, une suppression ou des octets manuels exacts en produisant un arbre/Step descendant ; résolution partielle par fichier conservant les autres conflits.
- [x] Résoudre des blocs dans le modèle canonique natif pour un placement texte/code, une base et deux variantes de même mode. Fragments CAS, IDs/ancres immuables, sélection/manuelle, résolution partielle persistante et finalisation en File. Toute sortie Lens ne reconstruisant pas exactement les trois originaux conserve le conflit structurel entier ; multi-placements/virtuels et re-fusion d'un conflit riche restent explicitement refusés.
- [x] Enregistrer implémentation exacte, digest, profil et limites des Lenses exécutées ; receipt validé et exécution non attestée refusée. Qualification entre plateformes suivie séparément ci-dessous.
- [ ] Qualifier ces bornes sur toutes les plateformes visées. **Acquis Windows et Linux x86_64 :** WASM sans imports, fuel/mémoire linéaire, Job Windows ou RLIMIT_AS Linux pour le seul worker, deadline/sortie bornées et intégrité bloquante. Linux : seccomp avant exec interdit les processus enfants et le relèvement des limites, image memfd scellée, mort du parent vérifiée. macOS/autres architectures restent refusés ; absence de facilités kernel requises refuse l'exécution. [Contrat détaillé](../../../crates/layrs-weave/README.md).

**Sortie T2.M :** moteur déterministe et testable en mémoire ; aucune dépendance à une session Web/Desktop.

### T2.U — Intégrer les parcours utilisateur

- [x] Installer objets, résultat Step, receipt, View et Operation via T1.S ; vérifier les têtes attendues au moment de l'acceptation.
- [x] Unir les parents réconciliés ; ne plus copier les Steps d'une Layer vers l'autre ni utiliser leur provenance comme graphe.
- [x] Afficher les variantes avec leurs identifiants stables et un diff lisible dans CLI/Desktop ; distinguer aperçu et résultat déjà enregistré. CLI native et Desktop `.ts` vérifiés ; diff textuel borné, absence/binaire/structure indiqués explicitement.
- [x] Rendre les conflits binaires résolubles par variante ou remplacement exact (CLI), et les conflits texte par variante/édition complète (Desktop).
- [x] Ajouter le choix de blocs dans l'interface et son modèle canonique ; conserver les brouillons lors des transitions associées. Blocs persistants, lecture exacte bornée, aperçu/apply gardés et commande `resolve-block` réelle vérifiée ; recette Desktop réelle **PASS 24,3 s** : manuel partiel, brouillon conservé, complétion puis undo retrouvant le conflit riche. Qualification du transport traitée séparément ci-dessous.
- [x] Préserver les brouillons UI séparément des ConflictObjects ; revalider les entrées d'un aperçu devenu ancien.
- [x] Adapter les commandes historiques `continue/abort` ou les remplacer avec une explication claire de la nouvelle sémantique ; aucun ancien stockage de session actif dans le format natif. Refus explicites orientant vers `resolve-term`/`resolve-file` et `restore-view`/`undo`, quatre parcours CLI sans effet vérifiés.
- [x] Terminer cette intégration commune avec T5 et la qualification distante sémantique du sous-ensemble natif. Vraie boucle TLS structurelle/blocs/renommages, pins exacts, fermeture et originaux validés. Revue T5 : versions conflictuelle puis partielle refusées malgré approbation, version finale exige sa nouvelle approbation exacte avant intégration ; trois versions conservées. Suite commune **PASS 297,84 s**, sans inventer une preuve CI ; limites de formes/pins conservées.

## Validation et critères de fin

- [x] Compléter la recette des longues lignes : moteur pur Unicode64Kio/CRLF/sans saut final et conflit128Kio gardant chaque variante. WASM réel Windows :1Kio accepté au défaut100M ;16/64Kio refusés au défaut mais exacts avec1B/30s explicites ;2Mio mémoire refusés par code typé. Optimisation JCS0..255 conserve les goldens et les plafonds. Ce sont fixtures bornées, pas garantie pour toute taille/contenu ni qualification inter-OS. [Mesures locales](measurements/T2-wasm-long-lines-2026-09-20.json).
- [x] Cas graphe : fast-forward, parents distincts avec arbre identique, aucune base, criss-cross, plusieurs têtes, entrée permutée.
- [x] Compléter la matrice avec les renommages concurrents dans le parcours intégré. Store réel modification/suppression/collision/destination divergente/dossier, réouverture, garde dirty, undo/redo : Windows **27,50 s**, Linux **8,81 s**. CLI **4,23 s**, Desktop couplé + undo **43,5 s**, boucle TLS comprise dans suite **297,84 s**. Graphe Windows/Linux inclut ambiguïtés/symlinks, mêmes IDs golden casse/Unicode et refus des falsifications. Dossiers vides devenus non suivis conservés sur disque.
- [ ] Qualifier les mêmes octets/IDs entre plateformes pour tous les profils visés. **Acquis Text/Code natifs Windows/Linux x86_64 :** matrice WASM12cas Unicode/CRLF/sans saut final, conflits, absence/vide, suppression, binaire et UTF-8 invalide ; même golden `73a211…f125`, **7,74 s / 10,20 s**, en plus des longues lignes et limites. Identités de contenu/pins module qualifiées ; digest de politique hôte spécifique au binaire OS. macOS/autres architectures et combinaisons hors matrice publiée restent ouverts.
- [x] Une Lens absente ou sans reconcile laisse un conflit résoluble si le profil le permet ; une sortie corrompue ne contourne pas la validation.
- [x] Preview n'a aucun effet ; application périmée ne perd aucune tête ; le vrai no-op ne crée aucun objet métier accepté.
- [x] Ajouter une recette unique combinant résolution partielle, redémarrage, sync puis résolution distante. TLS deux Stores **PASS 263,44 s** : clone riche, partiel publié/réouverture, final exact UTF-8/CRLF ; parents conservés. Receipts/payload falsifiés rehashés, régression de décision et mauvais chemin refusés. Backup563objets/33Steps restauré en8924ms. Sous-ensemble texte/code mono-placement/pins exacts ; aucune attestation d'exécution distante revendiquée.
- [x] Fermer la recette Desktop `.ts` convertie récemment. CLI réel `code.ts` avec WASM/pin incorrect PASS ; Desktop `.ts` variantes puis édition manuelle PASS 47,9 s. Comparateur de variantes ajouté ensuite : recette étendue à relancer.

## Dépendances et parallélisme

| Phase | Prérequis bloquant | Parallèle possible |
|---|---|---|
| T2.C | Aucun ; accord d'interface avec T1.C | T3.C, T4.C, T8.C |
| T2.M | T2.C | T1.S, T4.E, T8.I et mapping T7.C |
| T2.U local | T2.M + T1.S | Historique T3.H, serveur/clients T4 |
| Recette distante T2.U | T2.U local + T4.P | Revue T5 et livraison T8 |

Propriétaire : moteur Weave, réconciliation Lens et représentation des résultats. T1 possède l'acceptation/matérialisation ; T3 possède les règles de réécriture ; T4 possède les échanges. Les changements de `conflict.schema.json` ou du contrat Lens sont intégrés une seule fois, avant les consommateurs Rust/TypeScript.

Charge indicative : 5–8 semaines-personnes. Risques : bases multiples, conflits structurels, cohérence des pins et limite des gros fichiers. Se référer à [l'ordonnancement global](01-restauration-et-annulation.md#ordonnancement-global-des-huit-fichiers).
