# Extensions Desktop — 23 septembre 2026

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
est enregistré dans le Store, mais pas les choix non appliqués du formulaire.

Qualification transport sur la même source : HTTPS complet **1PASS,284,71s** avec
`LAYRS_TEST_SCOPED_MIB=1024`, capture/envoi1Gio et rejeu après réponse perdue.
Log `D:/Layrs/tmp/scoped-1gib-20260923.log`. Cela ne mesure pas le pic mémoire ni
un lot complet4Gio. Windows actuel et Linux Docker seulement, conformément aux
moyens confirmés par l'utilisateur.
