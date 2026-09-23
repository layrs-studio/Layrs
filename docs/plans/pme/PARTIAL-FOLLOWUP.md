# Suites fonctionnelles du Store3 — 22 septembre 2026

Le périmètre livré et qualifié est décrit dans
[PARTIAL-CHECKOUTS.md](../../../deploy/selfhost/PARTIAL-CHECKOUTS.md).
Les sections détaillées plus bas sont le constat initial du22septembre.
Mise à jour23septembre :

| Lot | Livré dans les sources | Encore à faire |
| --- | --- | --- |
| Collaboration | Candidats sans avance, revues limitées aux chemins, approbation/preuve, intégration et rejouabilité HTTPS ; Desktop53,3sPASS | Qualification des nouveaux paquets |
| Grandes publications | Captures et transport en blocs1Mio, plafonds1Gio/fichier et4Gio/capture, reprise100Mio et1Gio validée | Qualification4Gio et mesure mémoire dédiée |
| Divergences | Fusion régulière base/local/distant, Lens borné, brouillon persistant, choix par fichier, revalidation et reprise | Plusieurs têtes/bases et conflits structurels ; persistance des choix de formulaire |
| Namespace/historique | Dossiers ordinaires déplacés atomiquement ; restauration d'un Step publié sous droits actuels | Transformations dossier/fichier, liens et résolution structurelle ; création de chemins historiques inconnus |

Commits et preuves : [REPRISE](REPRISE.md) et [journal courant](journal/FINISH-20260923.md).
La recette HTTPS fusion/restauration complète passe121,58s ; les paquets
qualified5 ne contiennent pas ces changements. Les qualifications externes
restent distinctes du développement accessible.

## Constat initial — 22 septembre, historique

## 1. Collaboration et Layers protégées

État : `collaboration/mod.rs` refuse une autorité `content_restricted`.
L'historique publié complet est également refusé au compte restreint ; les noms
et chemins restent consultables via la projection autorisée. L'historique local
des captures partielles est disponible.
`path_patch_service.rs` construit une publication `Advance` immédiate. Store3
n'a pas d'admission de candidats permettant revue/CI avant déplacement de tête.
Le test de Gate refusée prouve la protection, pas un parcours accepté complet.

À faire : définir une portée de revue liée aux chemins lisibles, filtrer toutes
les lectures et écritures de commentaires, preuves et journaux pouvant contenir
du contenu interdit ; conserver la garde globale tant que ce contrat manque.
Ajouter admission de patch candidat sans avance, reçu durable et reprise,
puis rattacher revue/preuve/intégration aux identités exactes du candidat.
Raccorder CLI/Desktop et tester approbation, refus, révocation entre phases,
rejeu et absence de fuite par ID connu ou logs CI. Ne pas réutiliser aveuglément
le parcours du Store complet : il exige une fermeture intégrale.

## 2. Grandes publications partielles

État : 16Mio cumulés, 1024 chemins, payload base64 complet. Clone/réception
utilisent le transfert vérifié, mais la publication partielle n'est pas en flux.

À faire : transport chunké lié à identité/chemin/capture/digest, quotas durables,
reprise et renouvellement d'autorité ; preuve effective des octets avant admission
(aucune autorisation par simple FileId connu). Conserver atomicité, reçu et CAS
du patch complet ; tests100Mio/1Gio, coupures, révoquer/reprendre et mémoire bornée.

## 3. Divergences et fusion partielles

État : tête distante unique exigée ; réception sur modifications locales
divergentes refusée et octets préservés. Pas de fusion sémantique Store3.

À faire : modèle de bases multiples/complétude autorisée sans créer de faux
Steps incomplets, réutilisation des plans de fusion sur seuls contenus autorisés,
conservation des branches protégées et registre de conflit récupérable.
Tester changements de droits pendant la résolution, publication concurrente,
rejeu et intégration après revue. Dépend du parcours candidat ci-dessus.

## 4. Namespace et restauration étendus

État : fichiers ordinaires seulement ; déplacement de dossier, dossier→fichier,
édition de symlinks/conflits structurels non disponibles. Restauration de chemins
explicitement capturés et inverse disponibles ; pas de restauration intégrale
d'un ancien checkout partiel.

À faire : ownership durable des répertoires, plans atomiques multi-chemins,
prévalidation de toutes les réservations et collisions, reprise sans écrasement.
Définir la restauration historique avec droits actuels et entrées devenues
invisibles au contenu. Tester interruptions à chaque frontière, alias Windows,
permissions et reparse/symlink. Ne pas promettre le mode exécutable sous Windows.

## Ordre et parallélisme

Les lots 1 (contrat/admission), 2 (transport) et 4 (namespace) peuvent être
préparés séparément. Leurs intégrations partagent les journaux Store3 et le
service de patch : figer les interfaces avant modifications simultanées.
Le lot3 dépend du candidat du lot1. Toute modification de production après
`6436fb0` exige tests ciblés, nouvelle recette intégrée et nouveaux paquets ;
les artefacts qualified5 restent liés à leur source exacte.
