# Extensions Desktop — 23 septembre 2026

## Complément validé — persistance des choix

Les choix de résolution sont enregistrés dans le stockage du WebView sur ce
poste. L'empreinte SHA256 de l'aperçu complet et la paire dossier/serveur lient
la sauvegarde au membre, à la capture et aux versions exactes. Un autre aperçu
ne récupère pas les choix ; aucun contenu de fichier ni credential n'est copié
dans ce cache. L'application reste soumise aux contrôles natifs habituels.
Échec de stockage signalé, choix gardés en mémoire ; cache nettoyé après succès,
sans effacer celui d'un aperçu plus récent dans une autre fenêtre.

5 tests Node PASS : réouverture/ordre JSON, changement de contexte/versions,
cache corrompu, quota et nettoyage concurrent. TypeScript Desktop PASS.
Recette Desktop étendue **1PASS,43,9s** : vrai conflit, choix local, rechargement
de page, brouillon/choix retrouvés puis application et suite dossiers/historique.
Log `D:/Layrs/tmp/partial-choice-ui-final-20260923.log`. Le premier essai a révélé
un sélecteur sans nom accessible exact ; libellé explicite ajouté, assertion du
conflit natif avant sélection. Cache relu après rechargement complet du WebView.
Le test Node est raccordé à `scripts/test-all.mjs`. Aucun Rust ni contrat natif
modifié ; relecture directe de ce petit complément UI, distincte d'une revue
indépendante. Les paquets qualified6 précèdent ce complément ; nouveau MSI
nécessaire après commit, archives natives inchangées.

## Lot précédent qualifié

Livré : envoi de candidat, proposition et lecture des revues, commentaire,
approbation/demande de modifications, intégration après approbation et preuve,
préparation/application de déplacements de dossier, fusion de capture avec la
tête distante, choix explicite par conflit, recharge du brouillon et restauration
d'un Step publié sous les droits actuels. Les chemins interdits restent réservés.

Recette réelle Tauri/WebView2 + HTTPS/PostgreSQL : **1PASS,53,3s**, aucune route
simulée. `D:/Layrs/tmp/partial-ui-pass-20260923.log`. Deux comptes de test distincts
pour auteur/approbateur ; bouton désactivé avant approbation. Publication/réception,
collision externe conservée, restauration locale/inverse, candidat/revue/commentaire,
intégration, fusion/recharge, déplacement physique et restauration historique ciblée
avec préservation des autres modifications. Fsck final réussi.

Les essais précédents ont corrigé : bouton d'intégration accessible sans accord,
double lecture locale sous StrictMode, retour de détail tardif après changement
de projet, annonce de réussite avant fin de la relecture locale. Le test final
exécute tous les parcours ; les échecs intermédiaires ne sont pas comptés comme PASS.

TypeScript Desktop et rustfmt workspace PASS. Les commandes de revue en attente
gardent leur clé exacte en mémoire lors d'une réponse perdue ; cette conservation
n'est pas annoncée persistante après fermeture. Le brouillon de fusion, lui,
est enregistré dans le Store ; la persistance des choix de formulaire est
ajoutée par le complément décrit en tête de ce journal.

Qualification transport sur la même source : HTTPS complet **1PASS,284,71s** avec
`LAYRS_TEST_SCOPED_MIB=1024`, capture/envoi1Gio et rejeu après réponse perdue.
Log `D:/Layrs/tmp/scoped-1gib-20260923.log`. Cela ne mesure pas le pic mémoire ni
un lot complet4Gio. Windows actuel et Linux Docker seulement, conformément aux
moyens confirmés par l'utilisateur.
