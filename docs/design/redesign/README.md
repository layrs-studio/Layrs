# Refonte Layrs — plan détaillé Web et Desktop

Statut : **plan prêt pour implémentation, produit non modifié**.
Direction validée par l'utilisateur : première proposition révisée avec le logo
original, Audiowide pour la marque, graphite et accents corail/ambre.

## Résultat recherché

Un membre d'une petite entreprise doit pouvoir retrouver son projet, comprendre
ce qui a changé, protéger son travail et collaborer sans connaître les identifiants
internes ni parcourir un panneau d'administration. L'interface doit expliquer ce
qui se passe, ce qui est enregistré localement et ce qui est partagé.

Le Web est le lieu pour consulter et collaborer. Desktop ajoute le travail sur
les fichiers locaux. Ils partagent vocabulaire, navigation de projet, composants,
thèmes et comportements ; le Web ne simule pas un accès au disque local.

## Documents de référence

| Document | Contenu |
| --- | --- |
| [Pages et architecture](01-pages.md) | Chaque page, son public, ce qu'elle regroupe et pourquoi, actions, états et critères de réussite |
| [Design et interactions](02-experience.md) | Identité, composants, mouvement, feedback, clavier, accessibilité et responsive |
| [Exécution et qualification](03-delivery.md) | Contrats à compléter, lots, dépendances, parallélisation, tests, critères de livraison et journal |
| [Maquette approuvée](../layrs-brand/atelier-revised.png) | Référence de composition et d'ambiance ; pas une spécification de données métier |
| [Icône originale](../layrs-brand/icon.svg) | Source utilisateur à réutiliser telle quelle |
| [Journal historique](../../plans/pme/journal/UI-REDESIGN.md) | Exploration, inventaire et décisions précédentes |

## Décisions de produit

1. **Une seule interface cohérente.** Réutiliser les contrats métier actuels,
   remplacer progressivement les deux présentations Desktop et la page Web
   monolithique. Retirer les anciens accès après couverture des fonctions utiles.
2. **Le nom avant l'identifiant.** Projets, Layers, personnes et versions sont
   sélectionnés par noms et contexte. Les IDs restent copiables en détails.
3. **Un contexte de projet explicite.** Organisation, projet, Layer, serveur et
   état local/distant sont visibles au bon endroit. Aucun changement de cible
   implicite pendant une action.
4. **Une action principale par tâche.** Capturer dans Changements, décider dans
   une revue, publier dans Versions. Les outils rares restent accessibles dans
   un menu ou une page dédiée, sans masquer les avertissements nécessaires.
5. **Pas de simplification trompeuse.** Capturer n'est pas publier ; approuver
   n'est pas intégrer ; connecté n'est pas synchronisé. Les libellés le disent.
6. **Des états vrais.** Pas de chiffres décoratifs, de succès anticipé, de
   progression artificielle ni de bouton sans effet. Une capacité absente du
   moteur est signalée et planifiée, pas maquillée.

## Architecture à deux niveaux

**Global :** Projets, boîte de réception, compte/préférences. Administration
d'instance uniquement pour les personnes autorisées. Le choix d'organisation
filtre le contexte ; il ne crée pas un nouveau niveau de navigation complexe.

**Dans un projet :** Changements (Desktop), Fichiers, Historique, Revues,
Versions ; Équipe et accès, Réglages en zone secondaire. Le sélecteur de Layer
donne accès à sa gestion. Synchronisation est un panneau persistant de Desktop,
pas une section de formulaires affichée sur chaque page.

Les pages contextuelles — capture, revue, validation, restauration, conflit —
ont une URL ou une route stable mais ne deviennent pas autant d'entrées de menu.
Les filtres et la sélection survivent aux allers-retours. Chaque page détaillée
possède un lien de retour qui restaure la liste précédente.

## Propositions retenues dans le plan

- Palette `Ctrl/Cmd+K` pour les projets accessibles, pages et actions disponibles.
  Le premier périmètre n'est pas une recherche plein texte de tout le serveur.
- Centre d'activité pour les transferts et opérations longues, avec reprise
  proposée à partir du journal réel après interruption.
- Aide intégrée au premier usage : états vides utiles et explications courtes,
  sans visite guidée bloquante ni avalanche de bulles.
- Comparaison et revue au centre ; configuration progressive à proximité de
  l'objet concerné. Pas de tableau de bord de statistiques arbitraires.
- Même présentation pour projets complets et accès limités, avec capacités
  explicites et chemins réservés clairement identifiés.

## Périmètre et limites

Ce chantier couvre connexion, première utilisation, tous les parcours actifs
Web/Desktop, administration et préférences, états incomplets et reprise.
Il inclut les petits compléments de contrat indispensables à une navigation
moderne, détaillés dans le registre technique. Il ne transforme pas à lui seul
les limites actuelles du moteur de fusion, du pont Git ou des aperçus d'assets.

L'identité retenue et ce plan n'exigent pas une nouvelle validation esthétique
pour chaque écran. Les choix courants pourront être résolus pendant le travail ;
les changements de capacité métier devront être rendus explicites dans le journal.
La demande présente est un plan : aucune implémentation produit n'est lancée ici.
