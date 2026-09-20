# Mettre à jour et revenir en arrière

La [recette d'upgrade d'archives CLI](ARCHIVE-UPGRADE-TEST.md) prépare un essai
automatisé entre deux snapshots distincts de même format, avec sauvegarde locale,
incident détecté et restauration séparée. Son script et ses validations d'entrée
ne constituent pas encore une mise à jour réelle passée ; consulter le rapport
de la paire précise avant de revendiquer cette compatibilité.

Le protocole réseau est V3 ; le store local natif est au format 2. Les anciennes bases serveur V1/V2 et les stores locaux inconnus sont refusés. Ne pas supprimer le marqueur de format et ne pas attendre une conversion automatique. Une archive comprend CLI/serveur/runner du même build ; utiliser d'abord cette combinaison et les migrations qui y sont embarquées. Une compatibilité entre versions différentes doit être démontrée par des tests, pas déduite du numéro `0.0.0`.

## Avant modification

1. Conserver les archives et digests de la version actuellement installée, la configuration privée, les identités stables Deployment/Store et le point de restauration. Vérifier l'espace disponible sur DB, CAS et sauvegarde ; aucun quota global automatique n'est encore promis.
2. Terminer les transferts et arrêter les runners/webhooks puis l'admission de nouvelles écritures. Laisser la base accessible à la maintenance. Exécuter `layrs-server backup DOSSIER_NEUF`, puis `verify-backup` avec l'ancien binaire. Le runbook self-host précise les credentials et outils PostgreSQL nécessaires.
3. Restaurer cette sauvegarde sur une base et un CAS neufs, hors trafic. Vérifier l'historique et quelques fichiers réels, puis chronométrer l'exercice. Les petits fixtures de développement ne déterminent pas votre RPO/RTO.
4. Fermer les clients avant de sauvegarder leurs dossiers de travail complets, `.layrs` compris. Une copie faite pendant une écriture n'est pas une sauvegarde locale qualifiée.

## Essai puis bascule

1. Vérifier la somme de l'archive depuis votre canal de confiance et le manifeste après extraction dans un dossier neuf. Conserver les anciens exécutables dans leur dossier ; ne pas mélanger les fichiers de versions différentes.
2. Sur la restauration isolée, exécuter le nouvel `initialize` avec les credentials de maintenance. Les migrations sont append-only et checksummées ; une différence de checksum ou de baseline doit interrompre l'opération. Ne pas modifier la table de versions pour contourner un refus.
3. Démarrer le serveur avec le compte runtime non privilégié et tester readiness, connexion, deux clients, capture/publication, fetch, revue et protections utilisées par votre équipe. Rejouer la recette de sauvegarde/restauration sur cette version.
4. Après succès, maintenir la fenêtre sans écritures, arrêter les anciens processus, appliquer la même procédure et configuration sur la cible réelle puis rouvrir le trafic. Pour Compose, figer les tags/digests vérifiés des images serveur et Web, recréer le job d'initialisation, puis le serveur/proxy. Ne pas employer un tag mouvant comme preuve de version.
5. Mettre à jour CLI/Desktop avec le même build validé. Mettre à jour ensemble le worker Windows et son digest ; un ancien digest doit refuser le nouveau worker. Surveiller readiness, erreurs CAS/DB et place disque. Conserver le point de restauration jusqu'à acceptation.

## Retour arrière

Sans modification du schéma ni écritures après bascule, un retour aux anciens fichiers ne doit être envisagé qu'après vérification de leur compatibilité. Après migration ou écritures, **ne pas lancer l'ancien serveur sur la base modifiée**. Arrêter le trafic et préserver séparément l'état en échec pour diagnostic ; restaurer la sauvegarde d'avant changement dans une autre base vide et un autre CAS, avec le binaire correspondant. Vérifier entièrement avant de repointer le trafic. Les écritures réalisées après la sauvegarde ne sont pas conservées par ce retour : les inventorier et prévoir leur reprise explicite. Ne jamais restaurer par-dessus des données actives.

Un marqueur de restauration en échec doit rester en place. Corriger la cause et recommencer sur une cible neuve. Réutiliser les identités Deployment/Store et leurs secrets conservés ; recréer les rôles runtime selon le runbook. Après réouverture, révoquer les sessions/appareils compromis et vérifier les accès, files de webhook et proofs en attente.

Cette procédure dispose de recettes automatisées de restauration et d'installation/restart en environnement local isolé. L'exercice d'une mise à jour entre deux versions publiées et le pilote de 3–5 utilisateurs indépendants restent à réaliser avant l'engagement de dépôt principal d'entreprise.
