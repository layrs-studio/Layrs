# Pages, parcours et logique de regroupement

Toutes les routes ci-dessous sont **des routes UI proposées**, pas des endpoints
serveur existants. Desktop utilise les mêmes identifiants de vue dans son routeur
et rattache chaque projet à son dossier local. `:project` désigne la clé stable
interne ; le nom lisible est toujours affiché. Aucun secret ne passe dans une URL.

## Navigation persistante

Sidebar : logo original, organisation/projet courant, projets récents, vues du
projet ; réglages et compte en bas. Zone supérieure : fil d'Ariane, Layer lorsque
pertinent, recherche/palette, activité, boîte de réception. La sidebar se replie
en rail avec libellés accessibles ; elle ne disparaît pas pendant un chargement.
L'organisation sert au regroupement visuel, mais les équipes et permissions
restent attachées au projet conformément au modèle actuel.

Desktop ouvre Changements pour un projet local existant ; le Web ouvre Fichiers
pour un projet serveur. Les liens directs ouvrent toujours leur cible. Sans
projet connu, les deux ouvrent Projets. Pas de page intermédiaire obligatoire.

Les groupes ci-dessous sont exhaustifs pour ce chantier. Les onglets explicités
sont des sous-vues navigables, les panneaux temporaires sont identifiés comme tels.

## P01 — Connexion · `/login`

**Public et objectif :** rejoindre l'instance choisie sans ambiguïté.

**Composition :** logo, titre « Retrouvez votre équipe », instance visible,
identifiant, mot de passe avec révélation accessible, bouton Se connecter.
Lien vers l'administrateur en cas de compte absent ; ne pas afficher « Mot de
passe oublié » ou inscription libre sans flux serveur effectif.

**Pourquoi ensemble :** identité, destination et erreurs de connexion forment
une seule décision. Ni réglages de stockage ni catalogue de projets ici.

**Comportement :** soumission clavier ; attente sur le bouton ; mot de passe
effacé après succès ; retour vers la destination demandée. Erreur de session
sans divulguer l'existence d'un autre compte. Si connexion impossible, expliquer
le prochain geste. Desktop permet de travailler localement sans connexion.

**Terminé quand :** un nouvel utilisateur peut se connecter et comprendre à
quelle instance, sans jeton ni identifiant technique à saisir.

## P02 — Connecter Desktop · assistant et `/auth/device`

**Composition :** dans Desktop, adresse de l'instance puis ouverture du navigateur,
code lisible, état d'attente et bouton de reprise. Dans le Web, page dédiée
« Autoriser cet appareil » avec destination et détails disponibles, confirmation
explicite et succès « Vous pouvez revenir dans Layrs ». Certificat d'entreprise
dans une étape avancée descriptive, sans champs PEM sur l'accueil quotidien.

**Pourquoi ensemble :** le navigateur autorise l'appareil ; Desktop suit la même
opération. Les deux montrent le même code et le même état sans exiger de copier
un jeton. Ne jamais inventer le nom d'un appareil absent des données.

**États :** attente, code expiré, refus, connexion confirmée, certificat invalide,
serveur indisponible. Relance bornée selon le protocole ; ne pas ouvrir plusieurs
fenêtres à chaque vérification. Annuler ferme le parcours, sans prétendre révoquer
un jeton déjà délivré ; la révocation dispose d'une action distincte.

**Terminé quand :** retour Web/Desktop sans perte de contexte et expiration
compréhensible ; confiance TLS maintenue.

## P03 — Projets · `/projects`

**Composition :** titre « Vos projets », recherche par nom dans le catalogue
accessible, organisation, favoris locaux et liste des projets récents. Chaque
ligne ou carte contient nom, contexte, rôle et dernière activité seulement si
la source le fournit. Un projet seul est directement accessible, sans faux
indicateurs ni illustrations obligatoires. CTA Nouveau projet ; Desktop ajoute
Ouvrir un dossier et la distinction « Sur cet ordinateur » / « Dans l'équipe ».

**Pourquoi ensemble :** retrouver, créer et reprendre sont les gestes d'entrée.
Pas de compteur de validations ou de métriques techniques pour remplir l'écran.

**Interactions :** ouvrir au clic ; menu contextuel Favori/Retirer des récents
(aucune suppression du projet) ; destination récente retrouvable. Nouveau projet
mène à P04. Un nom homonyme est désambiguïsé par organisation/serveur/dossier.

**États :** aucun projet → explication et actions permises ; invitation reçue →
lien vers P18 ; accès révoqué → entrée retirée du catalogue serveur, message si
le projet était ouvert ; dossier local absent → Retrouver le dossier. Les récents
locaux ne deviennent pas une preuve d'autorisation serveur.

**Dépendance :** catalogue autorisé à ajouter, absent du V3 actuel ; pas de simple
champ `space:...` comme parcours principal de la refonte.

**Terminé quand :** l'utilisateur ouvre un projet autorisé par son nom depuis
une session neuve, pas uniquement grâce à un cache prérempli.

## P04 — Créer, ouvrir ou récupérer · `/projects/new` et assistant Desktop

**Composition :** choix court adapté à la surface. Web : créer un projet serveur
si autorisé. Desktop : ouvrir un dossier, initialiser un projet dans un dossier,
ou récupérer un projet de l'équipe. Nom lisible, organisation éventuelle, dossier
choisi par dialogue système. Paramètres avancés repliés, sans jargon au premier pas.

**Pourquoi ensemble :** même intention « commencer à travailler », mais pas une
liste de commandes techniques. Les branches de l'assistant évitent un formulaire
où 80 % des champs sont sans rapport avec le choix.

**Accès limité :** le choix du projet/Layer/capture résout les références.
Télécharger seulement les contenus autorisés, expliquer les chemins réservés.
Pour migrer un dossier complet vers un accès limité, choisir un autre dossier
vide et annoncer que l'original sera conservé ; aucune migration silencieuse.

**États :** nom invalide, dossier non vide, collision, droits insuffisants,
vérification en cours, téléchargement interrompu/reprenable. Résumé avant création
ou migration ; progression réelle ; ne pas marquer fini avant vérification locale.

**Git :** entrée dédiée « Importer depuis Git » prévue avec P22, visible comme
action seulement après raccordement du pont. Ne pas afficher une commande factice.

**Terminé quand :** premier projet atteignable sans connaître slug, IDs de Layer
ou capture ; annulation n'efface aucun dossier existant.

## P05 — Changements · Desktop `/projects/:project/changes`

**Composition :** écran de référence approuvé. Liste des fichiers à gauche,
comparaison à droite, message et action Créer une capture en bas. En-tête avec
Layer, capture automatique et état du travail. Détail du chemin et statut de
chaque fichier ; sélection claire, liste virtualisée si nécessaire.

**Pourquoi ensemble :** comprendre les différences est le préalable direct à
la capture. La liste, le diff et le message restent visibles sans changer de page.

**Actions :** sélectionner, comparer côte à côte ou en ligne, capturer, ouvrir
dans l'éditeur si cette commande est disponible, aller à l'historique du fichier.
Capture complète du périmètre supporté ; pas de cases de « staging » mensongères
si le moteur ne permet pas une capture sélective. Publication via P11, séparée.

**États :** aucun changement → « Votre travail local est capturé » ; capture en
cours ; capture automatique effectuée ; binaire non prévisualisable ; diff trop
grand ; accès limité ; détection de collision ; opération interrompue. Ne pas
afficher « synchronisé » parce qu'une capture locale a réussi.

**Partiel :** même page et mêmes repères, action activée selon les capacités
réelles ; noms réservés visibles sans contenu ; liens vers reprise et rebase.

**Terminé quand :** créer une capture de vrais changements, voir le résultat
dans l'historique et savoir s'il reste à publier, y compris en mode limité.

## P06 — Fichiers · Web/Desktop `/projects/:project/files`

**Composition :** arbre ou liste paginée, chemin en fil d'Ariane, référence active
(Layer/capture), recherche de nom bornée au périmètre chargé ou fourni par API,
panneau d'aperçu. Nom, type, taille uniquement si autorisés/disponibles.

**Pourquoi ensemble :** explorer et consulter sont une tâche distincte du travail
non capturé. Le Web expose ici la version publiée ; Desktop identifie explicitement
fichiers locaux versus référence publiée, sans mélanger les deux sources.

**Actions :** ouvrir le dossier, aperçu, copier le chemin, historique du fichier.
Dans Desktop, créer/renommer/déplacer quand supporté, avec aperçu d'impact pour
les dossiers et collisions. Le Web ne propose aucune mutation locale.

**Permissions :** ligne verrouillée « Contenu non accessible » ; nom/chemin
visibles selon le choix produit, pas de taille, extrait, hash ou miniature déduite
du contenu interdit. Le sélecteur de destination marque un nom réservé. Une
création externe via un autre logiciel reste possible : la détecter, conserver
les octets et bloquer l'opération Layrs ; ne pas promettre un verrou OS.

**États :** chargement par page, dossier vide, sélection disparue, référence
périmée, format sans aperçu, contenu trop volumineux et accès révoqué. Aucun rendu
actif de HTML/SVG arbitraire sans isolation ; les aperçus riches sont une dépendance.

**Terminé quand :** navigation fluide sur une liste réelle et aucun contenu
restreint révélé par aperçu, recherche ou changement de sélection.

## P07 — Historique · `/projects/:project/history`

**Composition :** historique des captures avec message et provenance quand
connus, filtre Layer/chemin, sélection ouvrant P08. Distinguer date/auteur déclarés
par le client et date de réception/compte authentifié observés par le serveur.
Une capture peut avoir plusieurs observations : ne pas les aplatir en un seul
avatar présenté comme preuve d'auteur. Les détails et filtres respectent cette
distinction. Desktop propose un onglet
« Opérations locales » pour undo/redo et diagnostic de ce qui a changé sur ce
poste ; les captures publiées restent identifiées. Pagination avec position stable.

**Pourquoi ensemble :** les deux vues répondent à « que s'est-il passé ? », mais
capture de contenu et opération du poste ne sont pas confondues. L'historique de
revue demeure dans la revue, relié à la capture exacte.

**Actions :** comparer deux captures compatibles, consulter, accéder à une version
nommée. Desktop ajoute aperçu de restauration et undo/redo des opérations locales
supportées ; le Web ne modifie pas un dossier local. Filtres gardés au retour du détail.

**États :** aucune capture, historique partiel, page obsolète nécessitant nouvelle
ancre, entrée non accessible. L'auteur inconnu reste inconnu, pas un avatar inventé.

**Terminé quand :** retrouver une modification et sa portée sans copier un hash.

## P08 — Capture et restauration · `/projects/:project/history/:capture`

**Composition :** résumé de capture, fichiers et comparaison, origine locale ou
publiée et observations de provenance décrites en P07 ; identifiant en détails.
Dans Desktop seulement, l'action « Restaurer… » ouvre un aperçu listant
exactement ce qui sera remplacé, créé ou conservé, puis confirmation.

**Pourquoi ensemble :** on restaure ce que l'on vient d'inspecter. Séparer aperçu
et application évite de transformer un clic d'exploration en écrasement.

**Actions Desktop :** restaurer le périmètre autorisé, annuler l'effet si le moteur
fournit l'inverse, nommer une capture/cible via P14. Réécriture avancée accessible
depuis le menu expert avec impact et préconditions ; jamais sans aperçu.
**Web :** consulter et comparer les références autorisées ; aucune restauration,
réécriture ou autre mutation d'un dossier local simulée dans le navigateur.

**États :** conflit avec modifications locales, contenu historique indisponible,
chemin inconnu dans une projection, aperçu périmé, restauration interrompue. Les
limites actuelles restent expliquées. Aucun faux fallback qui récupère du contenu
au-delà des permissions.

**Terminé quand :** appliquer uniquement l'aperçu validé ; un changement d'état
intermédiaire impose un nouvel aperçu ; le travail antérieur reste récupérable.

## P09 — Layers · `/projects/:project/layers`

**Accès :** sélecteur du bandeau ; action « Gérer les Layers » vers une page dédiée.
Liste avec nom, Layer courant, références publiées et état de divergence si connu.

**Pourquoi ensemble :** le changement fréquent de contexte reste à un clic ;
création, comparaison et fusion ne surchargent pas la sidebar quotidienne.

**Actions Desktop :** créer, changer de Layer, comparer et préparer une fusion.
**Web :** sélectionner une référence à consulter et comparer ; aucune création de
Layer ni fusion locale sans contrat serveur ajouté et explicitement qualifié.
Les protections ont un lien vers P17, leur édition
n'est pas mélangée au sélecteur. Plusieurs têtes déclenchent une explication claire.

**États :** changements locaux empêchant le basculement, conflits, cible supprimée
ou inaccessible, Layer protégé. Pas de suppression/renommage offert sans commande
métier vérifiée. Le changement de Layer ne publie rien implicitement.

**Terminé quand :** on sait toujours sur quelle ligne de travail on agit, sans
perdre les changements en cours.

## P10 — Résoudre une fusion · Desktop `/projects/:project/merge/:operation`

**Composition :** espace de travail dédié, liste des conflits, comparaison base/
local/distant selon les données, résultat proposé et progression « 2 sur 4 résolus ».
Panneaux repliables pour garder le fichier au centre ; sources nommées explicitement.

**Pourquoi ensemble :** comparer, choisir et vérifier le résultat forment une tâche
de concentration. Ni dialogue minuscule ni formulaire perdu dans la synchronisation.

**Actions :** résolution disponible par bloc/fichier, aperçu final, appliquer,
abandonner seulement avec les garanties du moteur. Conserver les choix attachés
au même aperçu ; ne pas les réutiliser après changement de référence.

**États :** conflits texte, binaire, structurels, tête distante modifiée, reprise
après fermeture ; cas non supporté expliqué avec sortie sûre. Pour une projection,
rebase limité aux cas supportés, aucune promesse de fusion structurelle générale.

**Terminé quand :** aucune résolution appliquée silencieusement ; le résultat
vérifié correspond aux versions montrées et les conflits restants sont explicites.

## P11 — Synchronisation et activité · panneau Desktop + route de détail

**Composition :** accès permanent dans le bandeau. État du serveur, captures à
publier/réception attendue uniquement si calculables, actions distinctes Publier
et Recevoir. Liste des transferts avec étapes, volume réel et erreurs. Une route
de détail permet reprise après redémarrage.

**Pourquoi ensemble :** transfert, disponibilité réseau et reprise partagent le
même contexte. Un simple bouton « Sync » cacherait des effets différents.

**Actions :** publier, recevoir, proposer les captures candidates, annuler ou
reprendre lorsque le protocole le permet. Divergence → P10/P12 selon le parcours.
Projection en retard → rebase expliqué, sans remplacement automatique du travail.

**États :** hors ligne, certificat invalide, session expirée, limite de stockage,
transfert partiel, serveur ayant accepté mais confirmation locale en attente.
Ce dernier état n'est ni un échec total ni un succès local complet. Les captures
restent protégées ; relancer réutilise l'identité d'opération appropriée.

**Terminé quand :** on peut quitter le panneau sans interrompre le travail et
retrouver précisément l'opération encore active, sans double publication.

## P12 — Revues · `/projects/:project/reviews`

**Composition :** liste par état, recherche de titre, filtres disponibles ;
titre, auteur si connu, Layer cible, version de proposition et état des validations.
CTA « Proposer une revue ». Les compteurs dérivent de données réelles et de leur
périmètre ; « À examiner » n'implique pas une assignation qui n'existe pas.

**Pourquoi ensemble :** c'est la file de décisions collectives, pas un historique
général ni un écran d'administration des exécuteurs.

**Création :** choisir la capture/candidate et la cible par noms, donner titre et
contexte, voir ce qui est proposé. Desktop guide la publication préalable si
nécessaire. Afficher les préconditions des revues à portée limitée.

**États :** aucune revue, brouillon local, proposition en cours, résultat incertain,
version obsolète. Brouillons cloisonnés par serveur/compte/projet et préservés
après erreur ; relance idempotente.

**Terminé quand :** une revue réelle est créée sans saisie manuelle de hashes,
et retrouvable depuis la liste ou une notification.

## P13 — Détail d'une revue · `/projects/:project/reviews/:review`

**Composition :** titre, cible et version exacte ; onglets Discussion, Fichiers,
Validations. Résumé compact de ce qui bloque l'intégration. Fil des commentaires
et décisions, comparaison, brouillon de réponse. Les onglets appartiennent à
la même revue et conservent leur état lors du passage de l'un à l'autre.

**Pourquoi ensemble :** la discussion doit porter sur les changements visibles
et les preuves qui permettent de décider. Les validations sont accessibles ici
sans noyer la conversation dans des logs.

**Actions :** commenter, consulter les corrections, approuver, demander des
modifications selon actions réellement disponibles, nouvelle proposition,
intégrer si autorisé. « Approuver » et « Intégrer » restent distincts.
Une proposition renouvelée invalide visuellement les décisions de l'ancienne version.

**États :** contenu interdit masqué, validation expirée, conflits, preuves absentes,
commentaire envoyé avec retour perdu, revue déjà intégrée. Une notification
ancienne ouvre la version concernée avec avertissement, pas une autre à son insu.

**Terminé quand :** la décision est rattachée à la version affichée ; refus de
Gate expliqué et conservation du commentaire saisi.

## P13b — Validation et journal · sous-page de revue `/runs/:run`

**Composition :** capture exacte, résultat, exécuteur, recette, date et journal
consultable ; provenance en détails. Résultat et disponibilité des logs distincts.

**Pourquoi ensemble :** diagnostiquer une validation justifie une vue technique
dédiée, accessible depuis la revue et Réglages > Automatisations.

**Actions et états :** annuler l'acceptation de résultat lorsque permis, consulter
preuves et motifs. Dire explicitement que cette annulation ne tue pas un conteneur
distant. Journal absent/trop grand/tronqué identifié ; pas d'affichage de secrets.

**Terminé quand :** un résultat peut être expliqué sans ambiguïté sur sa capture,
sa validité ou les effets de son annulation.

## P14 — Versions et repères · `/projects/:project/releases`

**Composition :** onglets Versions publiables et Repères ; nom, cible, notes,
publication active ou révoquée. Détail avec historique des publications.

**Pourquoi ensemble :** les deux nomment un état important du projet. Ils restent
distincts : un repère local n'est pas une version rendue disponible à l'équipe.

**Actions :** choisir une capture, créer le type supporté depuis Desktop,
publier/révoquer une version serveur, consulter le résultat. Pas de téléchargement
d'archive universel tant que l'export correspondant n'est pas disponible.

**États :** cible non publiée, droit insuffisant, version déjà publiée, publication
révoquée. Confirmer une révocation par son nom et sa portée, pas un « Êtes-vous sûr ? ».

**Terminé quand :** on distingue clairement repère, version et publication active.

## P15 — Équipe et accès : personnes · `/projects/:project/team`

**Composition :** onglets Membres, Invitations, Équipes. Noms lisibles et rôles,
invitation depuis un sélecteur de comptes éligibles, état/expiration des invitations,
composition des équipes et historique de leurs changements.

**Pourquoi ensemble :** « qui travaille ici ? » regroupe personnes, arrivées et
groupes. Les règles détaillées sur les fichiers restent dans P16 pour éviter
une matrice gigantesque et difficile à interpréter.

**Actions :** inviter un compte existant, accepter/refuser via P18, révoquer une
invitation, créer/éditer/archiver une équipe, attribuer les rôles autorisés.
Expliquer qu'aucun courriel n'est envoyé par le flux actuel. La création d'un
compte d'instance appartient à P21, pas au formulaire d'invitation au projet.

**États :** absence de membre éligible, invitation expirée, rôle modifié entretemps,
archive d'équipe ; permissions effectives du compte courant avant édition.

**Terminé quand :** un responsable invite par nom et sait quand la personne peut
effectivement ouvrir le projet, sans partager un `principal:...`.

## P16 — Équipe et accès : fichiers · `/projects/:project/access`

**Composition :** choix membre/équipe, arbre de chemins et aperçu des droits
effectifs ; héritage, règle explicite et refus prioritaire identifiés par texte
et icône. Sélection de fichiers/dossiers avec règle proposée et résumé d'impact.

**Pourquoi ensemble :** répondre à « qui peut lire ou modifier quoi ? » demande
de voir sujet, chemin et effet simultanément. L'édition est séparée de l'exploration
des fichiers pour éviter une modification de sécurité par erreur.

**Actions :** consulter les règles, préparer une modification, comparer avant/
après, confirmer. Expliquer le conflit entre groupe et exception individuelle.
Les chemins interdits gardent nom et emplacement visibles, contenu protégé.

**États :** projet sans règles particulières, règle périmée, tentative de retrait
du dernier accès administratif si le serveur la refuse, aperçu incomplet.
La révocation interdit l'accès futur ; ne pas promettre l'effacement d'une copie
déjà obtenue. Aucun « aperçu en tant que » qui télécharge des contenus interdits.

**Terminé quand :** une exception de fichier peut être posée, expliquée et vérifiée
avec un autre compte sans fuite dans liste, historique ou recherche.

## P17 — Réglages du projet · `/projects/:project/settings`

**Sous-pages :**

| Sous-page | Contenu et raison du regroupement |
| --- | --- |
| Général | Nom/contexte en lecture et options réellement modifiables ; renseignements du projet, pas du compte |
| Capture locale (Desktop) | Capture automatique, délai, comportement du dossier et identité déclarée pour les futures captures ; préférences locales distinctes des règles serveur |
| Protections | Approbations, auto-approbation, conflits, recette et exécuteurs de confiance par Layer ; expriment la politique d'intégration |
| Automatisations | Recettes, exécuteurs autorisés et accès aux résultats ; configuration récurrente distincte de l'exécution P13b |
| Webhooks | Destination, événements, secret, livraisons et relance ; toutes les étapes d'une intégration externe sont réunies |
| Avancé | Références techniques, migration vers accès limité, lien import/export ; fonctions rares sans perte d'accès |

**Interactions :** sous-navigation lisible, formulaires courts, sauvegarde explicite
des règles partagées, état Modifications non enregistrées et protection contre
sortie accidentelle. Secrets révélés uniquement lors de leur génération avec copie
contrôlée ; rotation et désactivation expliquent l'effet sur le destinataire.

L'identité locale actuellement éditable via `NativeIdentityPanel` est conservée
ici : nom/attribution des prochaines captures, sans réécrire les observations
anciennes ni accorder de droits serveur. Ce n'est pas l'édition du profil de
connexion, qui reste un contrat distinct en P20.

**États :** droit lecture seule, erreur de validation près du champ, règle obsolète,
livraison échouée et motif. Aucune action de renommage/suppression ou maintenance
n'est promise seulement parce qu'une sous-page Général existe.

**Terminé quand :** régler une protection ou un webhook sans parcourir les autres
réglages, avec état sauvegardé cohérent au retour sur la page.

## P18 — Boîte de réception · `/inbox`

**Composition :** onglets Invitations et Activité du projet sélectionné ; événements
avec contexte lisible et lien direct vers revue/version. Badge limité au périmètre
effectivement consulté ; une vue globale interprojets nécessite un agrégat dédié.

**Pourquoi ensemble :** ces éléments appellent l'attention de la personne. Ne pas
les confondre avec le centre d'activité des transferts machine (P11).

**Actions :** accepter/refuser une invitation, ouvrir le sujet, marquer comme lu
après confirmation serveur ; pas d'acceptation automatique à l'ouverture.

**États :** aucun événement, invitation devenue caduque, sujet inaccessible, page
de notifications suivante. Ne pas inventer e-mails, mentions ou messages directs.

**Terminé quand :** une invitation est trouvée sans avoir déjà ouvert le projet
et une activité mène au contexte exact concerné.

## P19 — Préférences personnelles · `/settings/appearance`

**Composition :** Sombre (initial), Clair, Système ; démonstration immédiate,
taille de texte via zoom natif, option Réduire les animations qui peut renforcer
la préférence OS mais jamais la contourner. Aide sur les raccourcis.

**Pourquoi ensemble :** personnaliser l'outil ne modifie ni l'équipe ni les règles
du projet. Ces réglages sont propres à ce navigateur/poste, clairement annoncés.

**Actions et états :** changement instantané mémorisé sans bouton Sauvegarder ;
stockage local indisponible → thème utilisable pour la session. Aucun flash clair
au démarrage, images et contrôles lisibles dans les deux modes.

**Terminé quand :** rechargement et changement OS respectent le choix explicite,
sans dépendre d'une connexion au serveur.

## P20 — Compte, connexions et appareils · `/settings/account`

**Composition :** identité connue, instance, sessions/jetons disponibles, connexions
Desktop et autorités privées. Sous-vues Compte, Appareils et Serveurs (Desktop).

**Pourquoi ensemble :** « qui suis-je et depuis où suis-je connecté ? » reste
personnel. Les comptes de service d'équipe et autres utilisateurs sont en P21.

**Actions :** copier l'instance, déconnecter, révoquer un jeton/appareil existant,
ajouter/changer une connexion Desktop par le flux P02. Le changement de mot de
passe existe : formulaire dédié avec vérification, confirmation et explication
de la révocation des sessions, puis reconnexion guidée. Ne pas fabriquer l'édition
du nom ou de l'avatar, absente du contrat actuel.

**États :** session actuelle clairement identifiée quand le contrat le permet,
révocation effective, serveur inaccessible. Avant déconnexion, signaler les
brouillons non envoyés ; purge des secrets selon la politique existante, pas
suppression des captures locales.

**Terminé quand :** on sait quelle session sera révoquée et l'interface cesse
d'utiliser immédiatement ses permissions et contenus serveur périmés.

## P21 — Administration de l'instance · `/admin`

**Public :** administrateur d'instance ; lien secondaire, jamais premier écran.

| Sous-page | Contenu et justification |
| --- | --- |
| Comptes | Créer/lister/révoquer les comptes humains ; prérequis aux invitations, portée instance clairement annoncée |
| Comptes de service | Créer et faire tourner les identifiants techniques ; cycle de vie différent d'un utilisateur humain |
| Stockage | Budgets logiques, réservations, usage disque et base distincts ; ne pas additionner des mesures incompatibles |
| État du serveur | Disponibilité, métriques et portée temporelle ; un compteur depuis redémarrage n'est pas un historique d'activité |

**Actions et états :** formulaires protégés, confirmations contextualisées,
révélation unique d'un nouveau secret, lecture impossible expliquée. Pas de bouton
« sauvegarde effectuée » ni de restauration serveur sans support réel et recette.
Les procédures d'exploitation existantes sont liées comme documentation.

**Terminé quand :** un membre normal ne voit ni données administratives ni actions
qui échoueraient systématiquement ; le serveur reste l'autorité de permission.

## P22 — Import/export Git · Desktop, entrée avancée

**Composition :** assistant source/destination locale, analyse préalable, branches
et limites compatibles, résumé puis exécution ; rapport consultable à la fin.

**Pourquoi ensemble :** import et export font le pont entre deux modèles et
doivent exposer les limites au même endroit, hors des gestes de capture courants.

**Dépendance :** moteur/CLI existants à raccorder au pont Tauri avant visibilité.
Présenter les chemins réservés incompatibles, notamment `.layrs`, dès l'analyse.
Ne jamais lancer une écriture dans une destination non vérifiée ni une commande
shell construite avec du texte utilisateur. Les dépôts originaux sont conservés.

**États :** format non supporté, historique incompatible, destination occupée,
traitement en cours, rapport partiel ; annulation seulement si arrêt sûr disponible.

**Terminé quand :** recette avec dépôt de test indépendant et comparaison de la
sortie ; aucune promesse de fidélité au-delà du pont actuel.

## P23 — Diagnostic et reprise · Desktop `/projects/:project/health`

**Composition :** résumé lisible de l'état local, dernier échec, opération en
attente, action de reprise déduite du journal, détails techniques repliés.

**Pourquoi ensemble :** les opérations de réparation doivent être guidées par
les faits, pas présentées comme cinq boutons de reprise au choix de l'utilisateur.

**Actions :** diagnostiquer, reprendre le type confirmé, copier un rapport expurgé
si cette exportation est ajoutée et vérifiée. Afficher que l'opération est bloquante
sans bloquer l'accès à l'aide. Lien depuis une erreur, menu secondaire sinon.

**États :** sain, réparation nécessaire, journal incohérent, espace insuffisant,
verrou actif. Aucune réparation automatique destructrice, aucun secret dans le rapport.

**Terminé quand :** après une interruption, l'utilisateur comprend quoi reprendre
et le moteur confirme le retour à un état utilisable.

## Parcours prioritaires à valider de bout en bout

1. Connexion neuve → invitation → projet par nom → fichiers autorisés.
2. Desktop sans serveur → ouvrir/initialiser → modifier → comparer → capturer.
3. Capture locale → connecter Desktop → publier → retrouver dans le Web.
4. Proposition → commentaire → validation → approbation → intégration autorisée.
5. Divergence → aperçu de fusion → résolution → application → historique exact.
6. Responsable → exception d'accès fichier → autre membre → chemin réservé et
   contenu absent → création interdite dans Layrs et collision externe préservée.
7. Arrêt réseau/processus → reprise guidée → absence de doublon ou perte.
8. Changement de thème, clavier seul et mouvements réduits sur chaque parcours.
