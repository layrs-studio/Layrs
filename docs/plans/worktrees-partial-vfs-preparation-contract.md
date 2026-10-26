# Contrat de préparation post-bootstrap — 26 octobre 2026

## Périmètre de cet incrément

Après le [catalogue bootstrap](worktrees-partial-vfs-catalog-checkpoint.md), cette
étape définit et implémente le format durable du futur enregistrement provider.
Elle livre un codec de données et ses comparaisons, ainsi que la comparaison
pure d'un ticket VFS avec l'identité de son contrôle. Elle n'écrit pas ce format
dans un Store de production et ne lance aucune population native.

Le format R4 `mount.v1`, les reçus A/P/T, le schéma privé 8 et le validateur
bootstrap restent inchangés. Le futur writer aura son propre validateur complet
et son propre détenteur ; un champ de données ne peut pas remplacer ces preuves.

## Représentation et liaisons

Le nouveau format canonique `layrs.projection-provider-mount.v2` utilise le
domaine d'empreinte `layrs-projection-provider-mount-v2`. Sa borne de **128 Kio**
est inférieure au plafond de 1 Mio de la colonne existante
`projection_provider_mounts.payload` : aucune migration ni nouvelle table
n'est nécessaire pour stocker ce contrat.

Les données communes comprennent :

- le prédécesseur exact `MountRecord v1` en phase `control_published`, dont
  l'empreinte v1 est recalculée pour la future écriture conditionnelle ;
- le binding bootstrap exact avec A et P, sans reçu terminal T ;
- le `SourceCommitment` complet, avec ses identifiants Store et VFS distincts ;
- un root ID explicite non nul et le ticket V2 `LYRTICK2` en hexadécimal canonique.

La taille du ticket brut est bornée à `186 + 4096` octets avant allocation du
buffer de décodage. Le ticket doit être identique après réencodage. Il correspond
au root ID, à l'identité de contrôle retenue dans P et au **catalogue VFS** de
la source ; le prédécesseur SQL correspond au **catalogue Store**. Les références
sont aussi comparées à l'allocation, au reçu préparé et à la source reconstruite
fournis au validateur de données.

| État v2 | Phase SQL | Sens des données |
| --- | --- | --- |
| `registrationPending` | `control_published` | Ticket destiné à être persisté avant le premier enregistrement natif ; le résultat natif n'est pas établi. |
| `providerPrepared` | `provider_prepared` | Déclaration du futur writer après preuve native complète ; contient l'empreinte de l'enregistrement pending exact. |

L'empreinte pending est recalculée à partir des mêmes données communes et du
tag `registrationPending`. Une empreinte simplement bien formée ne suffit pas.
Les comparaisons de ligne vérifient clé, catalogue, phase, payload et empreinte.
Elles ne prouvent ni que la ligne est la seule de la base, ni son schéma,
ni les clés étrangères, ni la fraîcheur des fichiers et des permissions.

Les champs inconnus, dupliqués ou manquants, les encodages non canoniques et les
liaisons contradictoires sont refusés. L'ancien décodeur et le validateur
bootstrap doivent refuser les deux états v2, même lorsque leur stockage respecte
le schéma 8. Cette incompatibilité fermée empêche les anciennes voies de reprise
et d'abandon d'interpréter une opération native qu'elles ne connaissent pas.

## Ordre obligatoire de la future intégration

1. Consommer le `ProviderControlPublished` et transférer ses véritables gardes
   et pins vers un détenteur de préparation unique. Revalider famille, membre,
   A/P/T, source et absence de claims sous les verrous appropriés.
2. Appeler la factory VFS existante avec le contrôle retenu et le catalogue.
   Son callback reçoit le ticket après validation des chemins et avant le
   premier enregistrement CFAPI. Le callback persiste pending par comparaison
   exacte du prédécesseur v1 : clé, catalogue, phase, empreinte **et payload**.
3. Le callback ne rend succès qu'après commit, readback et fermeture observable.
   S'il échoue, conserver séparément la cause Store typée avant de renvoyer une
   erreur au VFS ; le type d'erreur VFS ne remplace pas le rapport Store original.
4. La factory enregistre, peuple sans source de contenu, audite puis établit
   et relit READY. Après son retour et sa revalidation, une nouvelle écriture
   conditionnelle peut remplacer pending par prepared, avec le même ticket.
5. Seule la combinaison de validation Store complète et du véritable
   `ProviderRootGuard` permet de livrer le détenteur préparé. La fermeture
   observable et les refus de profil/rollback restent des points d'arrêt.

Le catalogue complet, les descripteurs et les autres tables doivent être validés
par un nouvel interprète post-bootstrap. Le validateur historique ne doit pas
être assoupli. Les verrous SQL ne constituent pas une garantie d'atomicité contre
un writer hostile ou de quiescence de l'espace de noms natif.

## Résultats incertains et reprise

La persistance du ticket constitue l'intention durable. Après un échec de commit,
de fermeture ou de callback, sa durabilité peut être inconnue. Après le callback,
les effets natifs peuvent être absents, partiels ou complets. Dans tous ces cas,
conserver les preuves et refuser le rejeu automatique de l'enregistrement ou de
la population ; ne pas nettoyer, adopter ou fabriquer un résultat known-success.

Pour prepared, la future reprise vérifiera le ticket authentifié avec
`Registration::guard_with_control_directory`, sans repeupler. READY atteste
l'audit initial, pas l'égalité de l'espace de noms utilisateur actuel : les
modifications et suppressions ultérieures doivent être préservées.

Une ligne pending ne devient pas prepared parce qu'un ticket se décode ou qu'une
racine paraît présente. La récupération d'un pending dont le natif est déjà READY
nécessitera un contrat distinct avant toute promotion. Le présent codec ne
fournit aucune récupération, lease de contenu, activation ou autorisation distante.

## Vérifications

La revue indépendante de conception et de code accepte ce périmètre pur.
Elle a fait renforcer la comparaison avec le commitment complet de l'allocation
et le refus de passer directement de v1 à prepared. Les tests de contradiction
conservent des références A/P valides et des tickets V2 syntaxiquement valides
pour vérifier les liaisons, au-delà du simple refus d'un encodage malformé.

Résultats locaux : **77 tests du provider**, **45 tests VFS**, **1 recette
SQLite de compatibilité v1/v2** et **6 régressions existantes du validateur**
réussis. Les commandes générales ignorent respectivement 214 et 31 recettes
spécialisées ; les sept sélections SQLite ont été exécutées explicitement.
Le test de compatibilité valide d'abord le bootstrap v1, stocke chacun des deux
états v2 dans la ligne existante, puis vérifie son refus par l'ancien validateur
et l'absence de modification de la ligne lors de ce refus.

Les preuves et le binaire de test Store sont conservés dans
`.layrs-local/validation/preparation-record-20261026-r1` :

```text
SHA-256 EXE: 3e75e6003ed10605de6fea057724f6325c09df9bb24a2f7e7872e1dde5047c1a
```

La recette SQLite s'exécute en mémoire dans le processus enfant borné existant,
sur un binaire rattaché à son PDB et à ses sources. Elle ne crée aucune racine
CFAPI. Les hypothèses documentées de composants Rust/MSVC/Windows standards
restent celles de l'hôte de test ; aucune qualification produit n'en découle.
Les premiers échecs de compilation des nouveaux tests sont conservés dans les
journaux ; aucun échec runtime de cette passe n'a été observé.

La compilation `cargo check --locked --offline --workspace --all-targets`, le
formatage ciblé des deux nouveaux fichiers Rust et `git diff --check` passent.
La revue finale indépendante confirme les résultats et **369 empreintes**
concordantes (EXE, PDB et 367 sources), sans défaut bloquant restant dans ce
périmètre.

## Suite

Le prochain incrément est le validateur SQL post-bootstrap complet et le transfert
du détenteur, puis le writer de ticket et sa qualification de commit/fermeture.
La population native, les leases locales et le mapping Active, la capture,
les transitions managed/companion, l'expansion et l'activation CLI/Desktop restent
des étapes distinctes. Le HOLD race/performance demeure.
