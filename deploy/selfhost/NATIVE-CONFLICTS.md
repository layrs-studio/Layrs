# Synchronisation des conflits natifs

Le service admet les conflits structurels et le sous-ensemble texte/code natif
à un placement : une base réelle, deux côtés et des blocs de texte UTF-8 sans
NUL. Les octets CRLF, fragments vides et occurrences répétées sont conservés.
Le sous-ensemble renommage exact admet aussi les placements multiples couplés :
une base réelle, deux côtés, toutes les valeurs/absences, choix d'un terme entier.
Les formes virtuelles, les autres formes multi-placements et les pins inconnus
restent refusés. Cette admission ne remplace pas les droits du Space ni ses Gates.

## Ce que vérifie le serveur

- Identité canonique des objets, du Weave, du profil et des exécutions ; parents
  du résultat et bases réellement calculées dans le DAG admis.
- Algorithmes et politiques supportés, absence de capacités ambiantes,
  budgets bornés, host WASM connu et pins Text/Code du module embarqué exact.
- Rôles des termes recalculés à partir du contenu du Step d'origine au chemin
  annoncé ; correspondance entre exécution, pin, chemin et Conflict produit.
- Manifeste et chunks disponibles et vérifiés dans le même Space pour chaque
  original, fragment, segment commun et résultat manuel. Les ancres doivent
  reconstruire intégralement les trois originaux, sans recherche de sous-chaîne.
- Pour un nouveau conflit épinglé, présence d'un Weave valide qui le produit.
  Une résolution partielle doit prolonger au même chemin un conflit d'un
  parent direct. Elle conserve origines, blocs, ancres et pin, ainsi que toute
  décision déjà publiée. Une nouvelle décision ne réécrit pas les précédentes.
- Pour un renommage couplé, réexécution de l'analyse d'identité exacte depuis
  les vrais Trees autorisés et vérification de la base DAG. Les occurrences
  Conflict dans chaque résultat couvrent exactement les chemins d'ancrage ;
  un ancêtre masque ses placements descendants, sans les perdre dans les termes.
  Un nouveau conflit requiert un Weave dont les côtés/bases correspondent, ou
  l'héritage identique depuis un parent direct. Un ancrage manquant ou déplacé,
  une valeur falsifiée même entièrement rehashée et une origine inventée refusent
  l'admission. Les arbres historiques lus pour cette preuve n'ajoutent pas leurs
  anciens conflits à la liste des conflits actifs du Weave.

Le client vérifie également les fragments téléchargés avant de créer ou
d'admettre le Store du clone. Les arêtes de fragments font partie de la
fermeture du Conflict ; connaître un FileId ne donne aucun droit de lecture à
un autre chemin ou dans un autre Space.

## Limites de la preuve

Un receipt est une déclaration de l'auteur authentifié. Le serveur vérifie
son contrat et les objets qu'il désigne ; il **n'atteste pas l'exécution distante**
du worker et ne rejoue pas le Lens pour démontrer le résultat. Les profils et
exécutions ont des identités recalculées, mais le digest de politique engage
aussi des paramètres locaux et le binaire worker dont la préimage n'est pas
contenue dans le receipt. Son égalité entre HostPolicy et ResolutionPolicy est
contrôlée, sans prétendre le recalculer.

Seuls les pins du module actuellement embarqué sont supportés. Une mise à jour
du module exige de qualifier les anciens historiques et clients avant mise en
service ; il n'existe pas encore de registre de modules historiques ni de
conversion automatique des anciens pins. Un pin différent échoue explicitement.

Bornes actuelles : fichier reconstruit 64 Mio, payload de fragments cumulé
256 Mio avant allocation, fermeture canonique 256 Mio / 100 000 objets et
profondeur de chemin 128. Le téléchargement réseau utilise un spool borné,
mais la validation finale conserve les fichiers du payload en mémoire.
La validation des renommages limite en outre le DAG à 10 000 Steps, les chemins
parcourus à 100 000, leur profondeur à 64 et chaque conflit à 4096 placements.
La lecture/comparaison ou le remplacement manuel mono-fichier ne s'appliquent
pas à ces choix couplés ; la résolution utilise un terme entier pour tous les
placements. La re-fusion d'un conflit multi-placement non résolu reste refusée.

## Recette et sources

La recette `native_clients_v3` utilise PostgreSQL isolé, HTTPS avec CA privée
explicitement approuvée, le vrai worker épinglé et deux Stores indépendants.
Elle publie deux blocs, clone le conflit, publie une résolution partielle,
rouvre le Store, puis résout le dernier bloc et compare les octets exacts.
Elle exerce aussi les receipts falsifiés avec IDs recalculés, une ancre
altérée et re-hachée, le refus d'accès par mauvais chemin et la tentative
d'effacer une décision publiée. Consulter le journal pour le dernier résultat
effectivement exécuté ; écrire une recette ne signifie pas qu'elle a passé.

Dernière suite commune **PASS 297,84 s** : elle comprend aussi le déplacement
d'un dossier, les quatre placements couplés, clone/réouverture/résolution sur
le second Store et retour au premier, plus quatre falsifications de renommage.
La revue T5 y refuse les versions encore conflictuelle et partielle malgré
approbation ; la version finale exige une nouvelle approbation exacte.
Cette recette de revue n'invente ni exécution CI ni preuve : sa protection
isole les règles d'approbation et de refus des conflits.

- [Admission de fermeture](../../crates/layrs-store-server/src/v3/closure.rs)
- [Origines et payload](../../crates/layrs-store-server/src/v3/closure/conflicts.rs)
- [Lignée des résolutions](../../crates/layrs-store-server/src/v3/closure/lineage.rs)
- [Contrat Weave](../../crates/layrs-store-server/src/v3/transaction_products/weave.rs)
- [Validation des renommages](../../crates/layrs-store-server/src/v3/closure/renames.rs)
- [Recette TLS renommages](../../crates/layrs-server/tests/support/rename_conflict_drill.rs)
- [Recette TLS](../../crates/layrs-server/tests/support/semantic_conflict_drill.rs)
- [Journal T4](../../docs/plans/pme/journal/T4.md)
