# Projection partielle — intégration

Décision : ADR0024. Le checkpoint le plus récent prévaut sur les étapes historiques.

## Checkpoint du 22 septembre 2026

- Serveur `82c0254`, opérations locales `0638dc2`, client/CLI `deb5937`.
- HTTPS/PostgreSQL réel **PASS 61,17 s** : migration distincte Store2→Store3,
  clone, capture immuable, publication, reprise après réponse perdue/réouverture,
  refus sous un autre compte, concurrence CAS, règles de chemins et Gates.
  Création puis renommage avant publication, collisions externes préservées,
  réception, révocation/réautorisation, restauration/annulation/rétablissement
  et refus d'un aperçu après révocation sont inclus.
  Log : `tmp/partial-full-20260922-combined.log`.
- Desktop réel WebView2/Tauri **PASS 29,8 s** (recette 30,3 s) : migration,
  chemins réservés/alias, création/déplacement, capture/publication/réception,
  collision externe, restauration depuis capture et annulation exacte,
  vérification finale du Store. `tmp/desktop-partial-restore-20260922.log`.
- Web réel Edge **PASS 4,6 s** : identité, projet, historique exact de deux Steps,
  chemin publié, droits affichés, commentaire de revue, preuve/journal CI,
  comptes et déconnexion sans credentials persistés dans le navigateur.
  `tmp/native-web-partial-20260922-final.log`. Cette recette seule ne qualifie
  pas toutes les clauses Web composées des huit plans.
- Store3 Windows **42 tests PASS**, Clippy strict ; Linux réception **11 PASS**,
  restauration **6 PASS**, namespace **24 PASS**. Client namespace/droits **10 PASS**.
  UI **38 tests Node PASS**, TypeScript UI/Desktop/Web PASS.
- Clippy strict workspace/all-targets **PASS 35,63 s** ; format Rust, taille des
  fichiers, immutabilité des migrations et `git diff --check` vérifiés.
- Limites assumées : publication partielle de fichiers ordinaires, 16 Mio et
  1024 chemins par capture ; pas de déplacement de dossier ni de remplacement
  d'un dossier physique par un fichier. Les Gates restent appliquées.
- Reste à ce checkpoint : tests CLI dédiés, paquets actualisés Windows/Linux/MSI,
  vérification des archives et journaux finaux. Les qualifications humaines,
  machine vierge, macOS/ARM et CI distante demeurent externes.

Les paragraphes ci-dessous sont historiques ; les tâches qu'ils annoncent
« en cours » sont remplacées par ce checkpoint.

## Dernier checkpoint — 21 septembre, publication réelle qualifiée

- Commits : `768e1ec` fondation Store3/capture ; `6d76172` journal de publication.
  Le remplacement versionné des politiques est aussi enregistré dans un commit dédié.
- HTTPS combiné **PASS 35,62 s**, sauvegarde/restauration comprise : vrai clone
  partiel, collision externe préservée et capture refusée, capture immuable,
  perte de réponse après commit, réouverture du Store et rejeu exact, acceptation
  de la nouvelle base sans écraser les modifications locales ultérieures.
- Serveur : batch atomique, concurrence CAS, révocation/réautorisation, Gates et
  lecture des chemins présents/absents vérifiés. Reçu lié au digest original
  dans le snapshot d'autorisation v2 ; rejeu indépendant des anciens Trees CAS.
- Store : 16 tests de publication/capture et Clippy strict PASS. Fondation aussi
  testée Linux (2 Core + 15 Store à cette étape). Client/CLI compilent.
- CLI : `remote clone-partial`, `partial status`, `partial capture GENERATION`,
  `remote publish-partial SEQUENCE LAYER_ID`, lecture et remplacement de politique.
- En cours : création/déplacement locaux journalisés, raccord UI, réception et
  changements de droits, vérification intégrale de l'historique Store3.
- Restent ensuite : recette des nouveaux parcours, migration explicite,
  restauration/annulation, transport des gros patchs et nouveaux paquets.
  Les sections suivantes décrivent les étapes historiques ; leur ancien statut
  « en cours » ne remplace pas ce checkpoint. Aucun résultat non terminé n'est clos.

## Réalisé et vérifié

- Pages structurales serveur `/v3/spaces/:space/projection`, bornées, ancrées sur
  membre/Space/Step/Tree/PolicySet/epoch, droits effectifs lecture/écriture.
  Curseurs inter-membres et pages périmées refusés. Pas de contenu téléchargé.
- Helper client typé, commandes de collaboration `projectionContext` et
  `projectionPage`, explorateur Web/Desktop raccordé par le lot UI.
- Première recette HTTPS ACL v2 + pagination : PASS 16,20 s. Elle prouve noms
  visibles, droits distincts pour des FileId identiques, refus des écritures v2,
  curseur d'un autre membre et changement d'epoch. Cette mesure précède les
  extensions suivantes ; ne pas lui attribuer leur qualification.

## En cours

Checkpoint : **recette HTTPS patchs + ACL PASS 18,24 s**, après correction d'une
clé de fixture trop courte. Modification/création autorisées, sibling interdit
conservé, écriture interdite rejetée avec reçu rejouable, identifiant connu sans
octets corrects refusé, idempotence et base périmée vérifiées. Le premier run
échoué 18,51 s n'est pas présenté comme PASS. Les scénarios multi-chemins,
révocation concurrente et réponse perdue restent à ajouter.

- Publication de patchs par fichier : Trees reconstruits sur la base canonique,
  attentes de chemins exactes, préservation des entrées non modifiées. Requête
  Sync interne déterministe dont le MutationId engage le digest du patch ;
  même moteur de reçus, idempotence, Gates et commit atomique, identité réelle.
- Le serveur exige les octets de chaque fichier remplacé, même si le CAS connaît
  son identifiant. Réservations de quota durables avant CAS ; aucune capacité
  d'upload générique n'est exposée au membre restreint.
- Premier transport de patch borné à 16 Mio de contenu cumulé, fichiers ordinaires
  uniquement. Étendre le transport sans réintroduire de preuve par simple ID
  avant de revendiquer la parité avec les transferts complets de 1 Gio.
- Clone partiel distinct : collecte de pages, vérification des fichiers autorisés
  dans un spool privé partagé, relecture d'autorité, registre format3 et
  matérialisation journalisée. Le materializer est encore en développement.
- Édition remplaçable des politiques : lots client/UI et serveur coordonnés,
  tests PostgreSQL, fermeture des versions et rejeu en cours.

## À terminer avant clôture

1. Recette HTTPS du service patch et tests multi-chemins, révocation, concurrence,
   réponse perdue, quota et absence de contournement par les autres endpoints.
2. Clone réel format3, collisions externes préservées, capture, publication,
   réception/réautorisation et annulation/reprise. Pas de capture locale Step
   incomplète ; registre et historique partiels distincts.
3. Raccord CLI/Desktop au vrai registre : création/renommage/déplacement contrôlés
   par le moteur, pas seulement par les composants d'interface.
4. Migration explicite d'un Store2, récupération et maintenance du Store3.
5. Qualification et nouveaux paquets. Les qualified4 restent historiques pour
   ce lot ; installation vierge, pilote humain et navigateur restent des preuves
   externes distinctes, aucune clôture artificielle.

Les journaux PARTIAL-STORE/SERVER/UI détaillent leurs sous-lots. Les erreurs de
fixtures initiales sont conservées dans `tmp` ; seuls les runs terminés avec code
zéro constituent un PASS. Aucun push ni installation effectué.
