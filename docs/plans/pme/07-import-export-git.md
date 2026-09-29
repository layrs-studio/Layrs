# T7 — Permettre une adoption réversible depuis Git

Suivi d'exécution : [journal T7](journal/T7.md). Coordination et reprise : [REPRISE.md](REPRISE.md).

Statut au 20 septembre 2026 : **pont local livré dans son périmètre documenté ; critères du plan vérifiés dans leur portée explicite**. T7.C acquis, T7.I/T7.E utilisables ; sélection de refs et cache persistant d'acquisition livrés. Un pilote technique public compatible et une recette représentative Windows/Linux sont qualifiés. Progression fine SQLite, volumes supplémentaires et pilote humain en entreprise restent des améliorations/qualifications distinctes. Priorité : P1 ; indépendant du serveur.

| Sous-plan | Acquis vérifiés | Travail restant |
|---|---|---|
| T7.C | Matrice/mapping/provenance, dry-run et refs exactes API/CLI, source en lecture seule, cache d'acquisition persistant, export neuf/reprise explicite, multi-head et conflits explicites | Qualification supplémentaire ; aucun filtre de sous-arbre ni histoire tronquée promis |
| T7.I | Import borné, SHA-1/SHA-256 vérifiés, WriteBatch atomique, provenance et idempotence, CLI/progression, 15 pannes installation, reprise depuis cache sans source/Git | Progression fine SQLite ; acquisition incomplète à recommencer, conversion recommencée depuis le cache |
| T7.E | Snapshot explicite, historique complet ou refs exactes API/CLI avec tous les ancêtres ; pack v2, refs atomiques/fsck, reprise et pilote public PyPA (475 commits/33 références) | Qualification volumétrique/inter-OS plus variée ; pilote humain PME non réalisé |

Mesure Windows debug, poste partagé : 2 000 commits, export/fsck **15,246 s et 10 processus Git** (contre 278,687 s et 8 014 avant pack), scan 0,711 s, installation 206,749 s. [Rapport pack](measurements/T7-pack-2026-09-20.json). Une mesure n'est pas une garantie de débit ni une qualification multi-OS.

## Résultat attendu

Une PME importe un dépôt Git avec ses fichiers, branches, tags et historique pris en charge. Elle peut ensuite exporter un état et un historique Layrs vers un dépôt Git valide, avec un rapport explicite sur les conversions et les pertes éventuelles.

## Point de départ de l'audit initial et références

- Aucun import/export Git identifié dans le code audité.
- Layrs dispose d'un graphe natif, de types canoniques et de futurs points nommés T3. Git ne doit pas devenir son format de stockage.
- Autorité : [ADR 0002](../../decisions/0002-no-git-core-v1.md). Les modèles cibles sont décrits par [SCM V1](../../architecture/scm-v1-target.md).

## Périmètre et règles de conversion

Créer un module/crate frontière dédié, par exemple `layrs-git-bridge`, avec une dépendance Git confinée à ce composant. Ne jamais ajouter Git au chemin d'acceptation du cœur Layrs.

| Git | Layrs prévu | Limite à documenter |
|---|---|---|
| Blob/tree | File/Tree natifs | Modes, symlinks et chemins doivent être représentables |
| Commit et parents | Step et parents | Message/auteur/date/signature restent provenance/observations hors identité du Step |
| Branche | Layer référençant la tête | Noms validés ; collisions rendues explicites |
| Tag | Marker ; Release seulement sur choix explicite | Ne pas convertir automatiquement chaque tag en publication produit |
| Identité Git | Table de correspondance import | Plusieurs commits peuvent correspondre au même Step canonique ; garder chaque provenance |

Version initiale : dépôt complet avec historique, branches, tags, merges, fichiers texte/binaires et modes pris en charge. Dépôts shallow/partiels, submodules, Git LFS et signatures font l'objet d'une détection et d'une politique explicite : résolution complète proposée ou refus/rapport, jamais promesse de conservation non tenue. La prise en charge avancée et la synchronisation bidirectionnelle continue sont reportées.

## Plan d'exécution

### T7.C — Définir mapping et rapport de compatibilité

- [x] Choisir le lecteur/écrivain Git après un prototype borné ; Git CLI isolé, lectures batch, SHA locaux et pack v2. Budgets et chemins documentés ; format pack vérifié dans la documentation officielle Git.
- [x] Fixer mapping exact, politique de noms et stockage de la provenance sans polluer l'identité des Steps.
- [x] Définir le `dry-run` du périmètre courant : branches locales/tags de commits, volume, objets spéciaux, collisions, mapping et pertes prévues.
- [x] Sélectionner explicitement un sous-ensemble de branches/tags par API (`ImportSelection::References`) et CLI `--ref` répétable, avec tous les ancêtres. Refs absentes/doublons refusés ; aucun filtre de chemin ni coupure de parents. Reflogs, refs distantes et HEAD détaché seul exclus.
- [x] Définir import local en lecture seule de la source et export vers une destination neuve ou explicitement préparée ; ne pas modifier un dépôt utilisateur existant par défaut.
- [x] Définir un identifiant d'import stable conservé dans l'Operation ; répétition après réouverture sans doublons de Layer/Marker/provenance. Checkpoint exact de reprise d'export livré.
- [x] Persister l'acquisition complète avant conversion : `git scan-cache` puis `git import-cache --digest`. Reprise sans source ni processus Git, digest attendu séparé, objets et fermeture revérifiés. Acquisition incomplète à recommencer ; conversion redémarre au premier commit du cache. Ni plan natif désérialisé ni reprise au milieu d'une allocation.
- [x] Fixer export des Layers multi-head : conserver toutes les têtes dans des références distinctes nommées explicitement ou refuser la sélection ambiguë.
- [x] Définir export des conflits : un export sans perte ne peut pas les convertir silencieusement en fichiers propres. Refuser avec liste des Steps concernés ou demander une sélection propre explicite.

**Sortie T7.C :** matrice de compatibilité et fixtures de mapping indépendantes du serveur.

### T7.I — Implémenter l'import

- [x] Énumérer références et objets en ordre topologique avec bornes de ressources ; ne pas exécuter hooks, filtres ou scripts du dépôt importé.
- [x] Traduire fichiers, arbres et parents puis installer via les APIs T1.S, sans SQL direct ni second CAS.
- [x] Conserver messages, auteurs, dates, tags annotés et données de provenance nécessaires à un export explicable ; les signatures conservées comme données ne deviennent pas des signatures valides de nouveaux objets.
- [x] Créer Layers/Markers avec les types/références natifs via WriteBatch ; ne pas confondre un tag avec une Release publiée.
- [x] Vérifier fermeture, objets et octets avant publication atomique des références locales via le writer natif.
- [x] Vérifier les pannes spécifiques à l'installation : 15 frontières CAS/SQLite, avant/après reçu et commit ; reprise exacte via le reçu transactionnel du writer, réouverture/rescan publique, aucun doublon ni reset des opérations ultérieures. Cache d'acquisition indépendant disponible ; aucun second writer/journal natif.
- [x] Exposer la commande CLI, progression, dry-run et rapport final. Une UI Desktop peut appeler la même API sans réimplémenter la conversion.

### T7.E — Implémenter l'export

- [x] Exporter un snapshot sélectionné sans parents ou l'historique complet visible, comme deux modes explicites ; coût des captures automatiques documenté.
- [x] Sélectionner un sous-ensemble de références Layers/Markers via `SelectedHistory` et CLI `--ref` répétable, avec fermeture complète des parents. Les têtes divergentes se choisissent par noms Git explicites ; aucun tronquage d'ancêtre. Checkpoint exact normalisé, autre sélection refusée à la reprise.
- [x] Reconstruire arbres/parents, branches et points nommés selon le mapping ; vérifier les modes et les noms.
- [x] Garder toutes les têtes sélectionnées et ne supprimer aucun parent conflictuel pour fabriquer un historique propre. Signaler les cas non représentables avant écriture finale.
- [x] Choisir une attribution synthétique documentée lorsque Layrs ne possède pas les métadonnées Git correspondantes ; ne pas inventer une attribution humaine.
- [x] Générer un rapport des identifiants, pertes et transformations ; conserver l'export temporaire jusqu'à validation avant de l'annoncer utilisable.
- [x] Documenter les sorties exactes garanties : octets, topologie représentable, références et provenance. Ne pas promettre les mêmes hashes Git après conversion.

## Validation et critères de fin

- [x] Fixtures : historique linéaire, merge, commits vides, commits différents donnant un même Step, tags légers/annotés et branches multiples.
- [x] Recette ciblée de casse Windows/Linux : fichiers `A`/`a`, dossiers `D`/`d`, fichier `Kind`/dossier `kind`, Unicode NFC `É`/`é`. Cache et export bare gardent noms/octet/IDs ; checkout Windows refuse avant capture, Linux matérialise les variantes. Branches `Main`/`main` via packed-refs : conservées dans le cache, export complet refusé Windows avant destination (sélection univoque possible), export des deux réussi Linux. Aucun renommage silencieux. Recette représentative, pas qualification de tous les systèmes de fichiers ni de macOS.
- [x] Détection LFS/submodule/shallow et signatures ; aucun pointeur LFS présenté comme contenu binaire importé sans avertissement explicite.
- [x] Recette interruption/reprise import : 15 pannes injectées, source Git inchangée octet pour octet, checkout sale préservé, fsck natif propre après reprise, refus provenance incohérente et réutilisation du transaction ID avec un autre digest. Injection d'erreurs, pas qualification de coupure électrique. Reprise export et `git fsck --strict --full` également testés.
- [x] Aller-retour Git → Layrs → Git : comparer octets/arborescences/références/parents selon la matrice, pas uniquement le nombre de commits ni les hashes.
- [x] Export d'une Layer divergente et d'un historique avec conflit : résultat conforme à la politique, aucune variante omise silencieusement.
- [x] Réussir un projet pilote réel compatible. PyPA pyproject-hooks complet dans le périmètre branches/tags : **475 commits, 10 branches, 23 tags**, import/répétition/matérialisation de 71 fichiers/export/fsck ; chaque arbre, parent, référence et provenance brute comparés. [Rapport épinglé](measurements/T7-public-pilot-2026-09-20.md). Projet public réel, pas pilote humain PME ni dépôt privé utilisateur. Refus du dépôt Layrs sur `.layrs` et d'ItsDangerous sur gitlink historique conservés ; aucun sous-arbre/commit filtré pour simuler une réussite.

## Dépendances et parallélisme

| Phase | Prérequis bloquant | Parallèle possible |
|---|---|---|
| T7.C et parsing Git | T1.C + T3.C pour le mapping ; parsing isolé possible avant | T2.M, T4.E, T8.I |
| T7.I | T7.C + T1.S + T3.H local | T4.L, T5.R, T6.P |
| T7.E | T7.C + T1.S + T3.H local | Revue/CI/déploiement |
| Recette conflits/export | T7.E + T2.M | T8.B |

Aucune dépendance au serveur V3 pour livrer le pont local. T7 possède conversion/provenance et fixtures Git ; T1/T3 possèdent les admissions et références natives. Proposer les extensions de types à leurs propriétaires, sans modifier le cœur pour reproduire les objets Git.

Charge indicative : 4–7 semaines-personnes hors compatibilité avancée LFS/submodules et pont bidirectionnel. Voir [l'ordonnancement global](01-restauration-et-annulation.md#ordonnancement-global-des-huit-fichiers).
