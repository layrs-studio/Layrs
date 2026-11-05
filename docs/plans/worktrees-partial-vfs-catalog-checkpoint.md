# Catalogue bootstrap — reprise du 26 octobre 2026

La reprise demandée après la fusion locale implémente le prochain incrément du
[plan après R7](worktrees-partial-vfs-next-slice-proposal.md) : les métadonnées
du bootstrap publié. Elle ne termine pas le programme Worktrees/VFS.

## Base intégrée

- `a4c3551` conserve les travaux locaux Studio avec la branche Worktrees/VFS.
- `10f93d7`, intégré par `b54d18c`, apporte le paquet de continuation distant.
- Les 20 268 fichiers du paquet sont restaurés sous `.layrs-local/validation`.
  Taille et SHA-256 de chaque fichier ont été vérifiés, ainsi que les empreintes
  des manifestes R7 et de sa disposition acceptée. Les preuves historiques sont
  conservées sans rejeu ni modification.
- La sauvegarde Git des modifications locales antérieures reste conservée.

## Contrat livré

`ProviderControlPublished::bootstrap_catalog()` retourne un
`ProviderBootstrapCatalog` qui emprunte le détenteur publié. La vue fournit
snapshot, sélection, génération et engagements exacts, catalogue VFS et
descripteurs ordonnés avec recherche par chemin. Chaque descripteur expose
chemin, identités du fichier et du manifeste, manifeste, longueur et bit exécutable.

La lecture acquiert une fois les verrous partagés lifecycle, owner et catalog.
Elle conserve l'inspection d'éligibilité existante, puis refait une validation
privée complète qui retourne la source reconstruite. Cette seconde lecture
exige aussi `ControlPublished` et le commitment de l'allocation retenue. La
connexion est fermée de façon observable ; guard, artefacts et binding sont
revérifiés avant de libérer les verrous et de retourner la vue.

La vue ne contient ni connexion, ni verrou d'opération, ni lease de contenu.
Les données ne renouvellent pas l'autorisation distante et n'attestent pas la
présence de blocs. `revalidate()` refait la lecture et compare l'engagement de
la source conservée. Les erreurs gardent l'opération courante, leur portée,
leurs causes typées et la même `RetainedIssuance` ; aucun nouvel effet d'écriture
n'est ajouté. Le wrapper historique `validation::private()` garde son résultat
et l'ordre complet de ses contrôles.

## Qualification locale

Binaire final de test conservé dans
`.layrs-local/validation/bootstrap-catalog-20261026-r3` :

```text
SHA-256 EXE: 0c6959f9233e74b03c10ec199fd60ba0f943af678c61ad2b71652e1d3af0aa38
```

La décision d'hôte lie cet EXE, son PDB et les sources sous les hypothèses
explicites de composants Rust/MSVC et chargeur Windows standards. Elle distingue
le compilateur natif de ce poste de celui de l'ancien R7. Elle ne qualifie pas
le démarrage CLI/Desktop, les DLL effectivement résolues ni une absence
universelle d'extensions automatiques SQLite.

Les **10 recettes natives ciblées passent** sur ce même binaire, dans des
processus enfants bornés et de nouvelles racines temporaires NTFS sur `C:` :

| Cas | Assertion vérifiée |
| --- | --- |
| Catalogue exact et froid | Fichier non vide, manifeste contenant un chunk, sept tables partagées de contenu/acquisition vides avant et après lectures répétées et revalidation. |
| Reprise | Même catalogue avec un nouveau détenteur obtenu par reprise, sans réinitialisation ni modification des témoins. |
| Deux membres | Catalogues, transactions et descripteurs distincts dans la même famille ; absence de croisement. |
| Contradictions SQL | Descripteur absent ou supplémentaire, manifeste et sélection incohérents refusés sans modification. |
| WAL/SHM | Sidecars privés refusés ; témoins conservés. |
| Reçus | Apparition terminale et altération de reçu refusées. |
| Phase entre lectures | Passage de published à prepared après la première inspection refusé par la seconde lecture. |
| Verrous | Concurrent de catalogue bloqué aux deux lectures et admis après retour. |
| Journal actif | Vrai `SQLITE_READONLY_ROLLBACK` privé conservé, sans ouverture writable ni récupération. |
| Fermeture BUSY | Statement réellement retenue, cause SQLite et portée préservées ; aucune ouverture VFS ultérieure observée. |

Les **6 recettes existantes du validateur privé** passent séparément pour
vérifier le wrapper historique. Les **73 tests purs du provider** passent,
avec 213 recettes spécialisées ignorées par cette commande générale ; les
16 sélections explicites ci-dessus ont leurs propres journaux. La compilation
`cargo check --locked --offline --workspace --all-targets`, le formatage ciblé
des fichiers Rust ajoutés/refactorisés et `git diff --check` passent.

Les résultats détaillés sont dans `runtime-c-volume/runtime-results.json` et
`validator-regressions/results.json` du dossier R3. Les premières tentatives
restent conservées : échec du précontrôle CFAPI sur `D:` avec
`ERROR_INVALID_FUNCTION`, puis deux défauts des recettes corrigés (décodage du
reçu volontairement altéré par le témoin ; installation de l'observateur VFS
après la déclaration du profil). Aucun dossier échoué n'a été réutilisé.

La revue indépendante de conception, source et résultats ne relève pas de défaut
bloquant dans ce périmètre. Elle confirme les 10 + 6 résultats natifs et les 73
tests purs, ainsi que les 367 empreintes de l'EXE, du PDB et des sources.
Le remplacement tardif d'un pin/FORMAT est couvert
par les contrôles existants relus, sans nouvelle recette dédiée dans cette
passe. Les résultats ne constituent pas une qualification power-loss, une
atomicité face à un writer hostile ou une qualification de distribution.

## Prochaine étape

1. Le [contrat durable post-bootstrap](worktrees-partial-vfs-preparation-contract.md)
   est désormais défini et son codec pur est implémenté. La suite est son
   validateur SQL complet, le transfert du détenteur et le writer du ticket,
   avant les états de population native sans source de contenu.
2. Implémenter ensuite les leases locales de lecture et leur mapping Active
   exact, avec les refus de contenu manquant et la gestion de l'annulation.
3. Intégrer capture, transitions managed/companion et expansion de sélection.
4. Qualifier les hôtes et l'activation CLI/Desktop, puis l'ensemble du parcours.

Le membre reste `initializing` après publication. L'activation produit et la
lecture de contenu ne sont pas introduites par ce catalogue. Le HOLD de
l'investigation race/performance reste inchangé.
