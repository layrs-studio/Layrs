# Projection partielle — intégration, 21 septembre 2026

Décision : ADR0024. Travail repris, **T4.L encore ouvert**.

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
