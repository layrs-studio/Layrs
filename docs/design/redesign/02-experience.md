# Système visuel et comportements

Cette spécification décline la maquette approuvée dans toutes les pages. Les
valeurs sont des cibles de conception à vérifier sur les rendus, pas une preuve
d'accessibilité ou de performance déjà obtenue.

## 1. Identité et lisibilité

| Élément | Décision | Raison |
| --- | --- | --- |
| Marque | SVG original non redessiné ; mot `layrs` en Audiowide | Garder l'identité fournie plutôt qu'une imitation générée |
| Police d'interface | Une sans-serif lisible, corps 14–16 px, titres 24–30 px | L'esthétique du logo ne doit pas rendre les formulaires difficiles à lire |
| Code | Monospace, 13–14 px initial, zoom possible | Comparaison alignée, chemins et identifiants distinguables |
| Base sombre | Graphite `#121215`, surfaces `#17171B` / `#1B1B20` | Éviter l'aplat noir sans profondeur et les contours omniprésents |
| Texte sombre | Principal `#F2EFF0`, secondaire proposé `#A6A1A9` | Hiérarchie sans texte gris presque illisible |
| Accents | Corail `#F43F5E`, ambre `#F59E0B` issus du SVG | Relier navigation, sélection et action à la marque |
| Action principale | Dégradé corail/ambre réservé à une seule action de la tâche ; texte sombre | Reconnaissable sans transformer toute la page en affichage lumineux |
| États | Vert succès/ajouts, rouge erreur/retraits, ambre attention, neutre information | Ne pas confondre couleur de marque et résultat métier |
| Espacement | Échelle 4/8/12/16/24/32 ; zones de travail structurées | Rythme cohérent sans formulaires étirés sur 2000 px |
| Formes | Rayons 8–12 px pour contrôles/panneaux ; ombres sobres | Écho discret aux couches du logo, pas de pilules partout |

Audiowide et la police de lecture seront livrées localement avec leurs licences
vérifiées ; aucun appel réseau tiers pour afficher l'interface. Réutiliser les
icônes de bibliothèque disponibles ou choisir une famille unique pour les icônes
fonctionnelles ; ne pas dessiner une nouvelle marque. Les avatars absents sont
des initiales, jamais des portraits inventés dans le produit.

Le corail de marque ne sera pas appliqué aveuglément au petit texte sur fond clair.
Créer des tokens sémantiques distincts pour texte, focus, bordure et remplissage,
avec variantes claires réellement mesurées. Cibles internes : contraste 4,5:1
pour texte courant et 3:1 pour grands textes/repères interactifs. Les états
sélectionné, interdit, erreur, ajouté et supprimé utilisent aussi mots et icônes.

## 2. Thème et préférences

- Première visite : sombre, même si le système est clair. Le choix Système est
  volontaire ; ensuite il suit les changements de l'OS en temps réel.
- Choix sombre/clair/système accessible depuis le compte et les préférences ;
  même composant Web/Desktop, valeurs stockées localement dans une version stable.
- Appliquer le thème avant le premier rendu pour éviter l'éclair blanc. Gérer
  un stockage indisponible sans bloquer l'application.
- Préférence d'animations réduites de l'OS prioritaire. Une option Layrs peut
  encore réduire le mouvement ; elle ne le réactive pas contre cette préférence.
- La maquette est sombre ; une planche claire et les états focus/erreur seront
  vérifiés au lot de fondations avant généralisation.

## 3. Shell, densité et fenêtres

Le shell demeure stable pendant les chargements. Sidebar 224–248 px, rail réduit
à la demande, bandeau 56–64 px. La zone de travail occupe le reste ; les formulaires
de lecture/saisie ont une largeur confortable, les diffs peuvent utiliser l'espace.
Les panneaux latéraux sont redimensionnables au clavier et à la souris si ajoutés ;
leurs bornes préservent un contenu utilisable. Éviter le scroll imbriqué inutile.

| Espace disponible | Adaptation attendue |
| --- | --- |
| Large, autour de 1440 px | Sidebar, liste de fichiers et comparaison côte à côte |
| Fenêtre 1024–1279 px | Rail facultatif, colonne fichiers réduite, commentaires en panneau |
| Fenêtre autour de 800 px | Une vue principale, ouverture des listes en volet, diff en ligne |
| Web étroit, autour de 390 px | Navigation en tiroir, pages à une colonne, actions lisibles ; aucune édition Desktop simulée |

Les seuils exacts seront déterminés par les composants, pas par des noms d'appareils.
Une comparaison de code peut avoir un défilement horizontal local ; la page entière
ne doit pas déborder. Vérifier 200 % de zoom, noms longs, chemins profonds et langue
française complète. Aucune action essentielle uniquement au survol.

## 4. Bibliothèque d'interactions communes

| Composant | Contrat d'usage |
| --- | --- |
| Bouton | Principal/secondaire/texte/danger ; état occupé avec libellé ; raison visible si indisponible |
| Champ | Label permanent, aide courte, erreur associée, état valide si utile ; placeholder jamais seul label |
| Sélecteur | Recherche par nom, contexte pour homonymes, aucune référence technique exigée par défaut |
| Dialogue | Une décision courte ; focus initial, piège de focus, retour au déclencheur, fermeture explicite |
| Panneau latéral | Information ou tâche liée à l'écran ; fermeture sans perte silencieuse d'une saisie |
| Menu | Actions secondaires ; comportement clavier et actions destructrices séparées |
| Onglets | Sous-vues d'un même objet ; navigation/restauration de sélection ; pas un menu de toute l'application |
| Liste/table | Colonnes utiles, sélection explicite, pagination réelle, états vide/chargement/erreur |
| Notification fugace | Confirmation secondaire et lien vers résultat ; jamais seule trace d'un échec critique |
| Bannière | Situation durable qui impacte le travail : hors ligne, accès révoqué, reprise nécessaire |
| État vide | Explique pourquoi et propose une action possible ; pas un simple « aucune donnée » |
| Aperçu d'impact | Cible, effets, collisions et préconditions avant appliquer une action sensible |

Ne pas utiliser le même `busy` global pour rendre toute l'application muette.
Conserver l'exclusion métier requise par projet/opération, mais laisser lire une
autre vue ou consulter un journal pendant une opération longue. Les mutations
incompatibles sont désactivées avec raison ; la navigation n'est bloquée que si
elle ferait perdre un brouillon ou compromettrait réellement l'opération.

## 5. Feedback : chaque action a une suite lisible

| Action | Pendant | Succès réel | Échec/reprise |
| --- | --- | --- | --- |
| Ouvrir un projet | Structure de page stable, indicateur dans le contenu | Nom et référence visibles | Message local, réessayer ou revenir aux projets |
| Capturer | Bouton occupé « Capture… », garde contre double clic | « Capture locale créée » et accès à l'historique | Changements conservés, cause et diagnostic |
| Capture automatique | Petite activité dans le bandeau | Heure/état mis à jour, sans toast à répétition | Alerte durable si protection du travail interrompue |
| Publier | Étape et octets réels quand connus | « Publication confirmée » après reçu | Distinguer refus, réseau et acceptation locale encore en attente |
| Recevoir | Cible et progression | État local vérifié | Reprise ou fusion, jamais écrasement silencieux |
| Commenter | Brouillon maintenu, envoi idempotent | Commentaire inséré et bref retour | Texte conservé, réessayer sans doublon |
| Inviter | Bouton en cours | Nom, expiration et invitation créée | Compte inéligible ou conflit expliqué |
| Changer un accès | Aperçu puis application | Droits relus/confirmés | Ne pas afficher l'état optimiste comme vérité |
| Copier un chemin | Retour discret du contrôle | « Chemin copié » | Erreur du presse-papiers, sélection manuelle |
| Changer le thème | Changement immédiat | Préférence visible | Usage de session si persistance impossible |

Les contrôles répondent visuellement immédiatement ; aucun minimum d'attente
artificiel. Afficher un squelette après un léger délai pour éviter les flashs
sur les réponses rapides ; conserver l'ancien contenu seulement s'il est toujours
autorisé et clairement identifié comme en cours d'actualisation. Une déconnexion
ou révocation impose la purge appropriée avant toute réutilisation.

Au-delà de quelques secondes, indiquer l'étape réelle et permettre de consulter
l'activité. Pas de pourcentage inventé quand le dénominateur est inconnu. Les
toasts d'information disparaissent après environ 4–6 s, restent au survol/focus ;
les erreurs persistantes restent consultables jusqu'à résolution ou fermeture.
Une alerte assertive est réservée aux situations exigeant l'attention immédiate.

## 6. Animations utiles, pas décor permanent

| Interaction | Animation proposée | Utilité / alternative réduite |
| --- | --- | --- |
| Survol/focus/sélection | Couleur et bordure 120–160 ms | Confirmer la cible ; immédiat si mouvements réduits |
| Changement de page/onglet | Fondu court 140–180 ms, contenu sans déplacement massif | Continuité ; remplacement direct en mode réduit |
| Ouverture de panneau | 180–220 ms, déplacement limité | Montrer l'origine de l'action ; fondu/direct en mode réduit |
| Dialogue | 140–180 ms, opacité et très légère échelle | Donner la priorité à la décision ; sans échelle en mode réduit |
| Toast | Apparition 160–200 ms, déplacement de quelques pixels | Signaler l'achèvement ; opacité/direct en mode réduit |
| Ligne créée/modifiée | Brève teinte de fond, moins d'une seconde | Retrouver le résultat dans une liste ; repère fixe alternatif |
| Progression | Barre liée aux mesures du moteur, interpolation courte | Comprendre l'avancement ; aucun mouvement continu superflu |

Éviter parallaxe, particules, halo permanent, logo qui tourne et glissement de tout
l'écran. Ne pas animer hauteur/largeur d'une immense liste à chaque élément. Les
animations ne retardent jamais une action, n'altèrent pas le focus et ne sont pas
nécessaires pour comprendre un résultat. Un diff n'apparaît pas ligne par ligne.

## 7. Palette, clavier et accessibilité

Palette `Ctrl/Cmd+K` : rechercher un projet accessible, une page ou une action
disponible dans le contexte. Résultats groupés et clairement nommés. Aucun appel
de mutation avant sélection explicite et, lorsque nécessaire, aperçu/confirmation.
Recherche de fichiers séparée et périmètre annoncé ; pas de recherche de contenu
interdit ou de catalogue serveur via devinette d'identifiants.

Raccourcis proposés : `Ctrl/Cmd+Entrée` soumet le formulaire actif seulement s'il
est valide ; `Échap` ferme le panneau non bloquant ; flèches parcourent une liste
active ; Entrée ouvre l'élément. Les raccourcis de frappe simple ne sont jamais
actifs dans les champs ni globalement sans contexte. Éviter ceux réservés à l'OS.

Parcours clavier complet, lien d'évitement vers le contenu, titres hiérarchisés,
labels et descriptions associés ; éléments natifs privilégiés. Focus visible
contrasté, retour logique après fermeture/suppression, annonces des changements
de page et résultats utiles. Indiquer les nombres de fichiers et conflits en
texte. Cibles d'action autour de 40–44 px ; commandes denses restent utilisables.

## 8. Vocabulaire et erreurs

| Terme utilisateur | Sens et précaution |
| --- | --- |
| Projet | Space ; le terme technique reste dans les détails de diagnostic |
| Layer | Ligne de travail ; courte explication au premier usage |
| Capture | État enregistré localement, ou capture publiée explicitement identifiée |
| Publier | Envoyer et faire accepter le travail au serveur ; ne veut pas dire rendre public sur Internet |
| Recevoir | Récupérer l'état autorisé, avec effets locaux expliqués |
| Revue | Discussion et décisions sur une version précise de proposition |
| Accès limité | Même projet, certains contenus non accessibles ; pas un produit parallèle nommé « Store3 » |

Erreurs en français : ce qui n'a pas abouti, ce qui est conservé, prochaine action.
Code technique copiable en détails si utile, jamais seul message. Exemples :
« La publication n'a pas été confirmée. Votre capture locale est conservée. » ;
« Ce nom est réservé dans ce dossier. Choisissez un autre nom. » ;
« Le projet a changé depuis l'aperçu. Actualisez avant d'appliquer. »

## 9. Continuité et confidentialité

Ne persister que les préférences et brouillons prévus par les politiques actuelles.
Jetons Web en mémoire ; secrets Desktop selon stockage natif existant. Les
brouillons sont cloisonnés par identité/serveur/projet/version. Pas de mise en
cache durable de contenu interdit, previews ou réponses d'administration.

Un changement de contexte annule/ignore les réponses anciennes, réinitialise la
sélection incompatible et préserve seulement le brouillon autorisé. Un formulaire
de permissions ne doit jamais appliquer la réponse d'un autre membre sélectionné.
Déconnexion, retour arrière, rechargement et liens profonds font partie des tests.

## 10. Performance perçue et vérification

Chargement par vue, pagination et fenêtres pour grandes listes ; pas de lecture
globale de tous les fichiers pour afficher la sidebar. Diffs volumineux bornés,
prévisualisation paresseuse et révocation des URL temporaires. Un identifiant
technique tronqué n'est copiable que par une action explicitement nommée.

Cibles : réaction visible d'un contrôle en moins de 100 ms sous charge normale,
transitions sans saccades perceptibles et absence de déplacement de mise en page
pendant chargement. Mesurer sur le poste réel avec jeux de données documentés ;
ne pas annoncer ces cibles atteintes avant mesure. Aucun benchmark universel déduit
d'une démonstration avec cinq fichiers.
