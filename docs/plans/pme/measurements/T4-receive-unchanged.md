# Réception inchangée vérifiée — 20 septembre 2026

Le client évite FetchPlan et GET de contenu uniquement après une réception
précédente ayant effectivement créé une Operation `ReceiveRemoteState`.
L'Operation immuable contient la métadonnée `layrs.native-receive.v1` : origine,
Store, Worktree, Operation, View et BLAKE3 du Bootstrap canonique entier. Ce
Bootstrap inclut compte authentifié, epoch, watermark, PolicySet, View/génération,
Layers, Markers, Releases et toutes les racines de graphe et de Products.

Le fichier mutable `native-remote.json` ne suffit jamais à autoriser ce raccourci.
Les anciennes Operations et les réceptions sans changement réel ne sont pas
réécrites pour fabriquer un témoin : elles continuent le parcours complet. Un
changement de politique ou un Product sans changement de View invalide également
le raccourci. Une nouvelle réception qui ne crée aucune Operation ne réensemence
pas le témoin ; le parcours complet peut donc rester nécessaire jusqu'au prochain
changement local accepté. C'est une limitation volontaire de cette première version.

Avant le second Bootstrap authentifié, le client relit la fermeture canonique
locale actuelle, vérifie les hashes, reconstruit les fichiers, valide les
renommages, vérifie les projections de Products et l'unicité de la tête Operation.
Tous les verrous Store sont relâchés pendant les requêtes HTTP. Après cette
authentification fraîche, l'état local et les fichiers de travail sont revérifiés.
L'annulation reste possible jusqu'à la frontière de commit ; une capture concurrente
provoque `StaleCapture`. Une corruption locale provoque un refus, sans réparation
silencieuse par téléchargement. Aucun cache d'autorisation n'est introduit.

Cette garantie suppose l'intégrité des admissions immuables du Store, comme les
autres opérations natives. La métadonnée n'est pas une signature contre un attaquant
capable de réécrire arbitrairement la base SQLite et son historique entier.

La recette réelle TLS/PostgreSQL est
[`support/unchanged_drill.rs`](../../../../crates/layrs-server/tests/support/unchanged_drill.rs),
intégrée à `native_clients_v3`. Le mode `LAYRS_TEST_UNCHANGED_ONLY=1` conserve
l'initialisation réelle et le nettoyage de la fixture mais sélectionne ce lot.
Elle compte les Bootstrap/FetchPlan/GET et couvre les chemins normal et annulable,
corruption d'un Tree CAS, annulation, capture concurrente, squash local, Product
admis sans View, fichier de liaison falsifié, changement d'epoch et de principal.
Le cas final révoque la session entre les deux Bootstrap et exige un refus sans
nouvelle Operation ni modification du fichier de liaison.

Première recette complète : **PASS 27,02 s**, Clippy strict client tous targets et
serveur test **PASS 22,73 s**. Recette finale **PASS 28,22 s**, avec révocation entre
les deux authentifications et cas principal isolé de tout changement d'epoch. Aucune nouvelle
durée 10k/99k n'est déduite de cette petite fixture. Le coût CPU/disque de la
fermeture et du checkout reste proportionnel aux données relues ; seules les
requêtes de contenu sont supprimées sur le chemin inchangé.

Clippy final du test serveur **PASS17,05s** ; garde de taille des fichiers PASS.
Une passe workspace intermédiaire a signalé un `needless_borrow` dans le nouveau
lot Observations d'un autre agent, transmis à son propriétaire ; ce résultat n'est
pas présenté comme un échec de la recette unchanged ni comme un PASS workspace.
