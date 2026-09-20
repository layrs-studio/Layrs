# Réception inchangée : audit du 20 septembre 2026

Audit historique avant modifications. Le [lot livré et sa qualification](T4-receive-unchanged.md)
décrivent désormais le témoin immuable et le raccourci vérifié ; les observations
ci-dessous restent la justification initiale, pas l'état actuel du code.

Mesure signalée par root, configuration debug HTTPS/PostgreSQL locale : clone
10 000 chemins, 200 contenus de 256 octets environ 181 s ; réception inchangée
environ 85 s. Ces durées ne sont pas ventilées par profiler. Les points ci-dessous
sont des constats de code et des hypothèses de coût, pas des pourcentages mesurés.

- [receive.rs](../../../../crates/layrs-client-core/src/native_scm/remote/receive.rs)
  appelle systématiquement `download_graph` après le bootstrap authentifié.
  Le graphe démarre vide et ne consulte pas le CAS local. Le chemin annulable dans
  `control.rs` fait de même. `binding.observed` enregistre la dernière vue distante
  mais n'évite aucun transfert. Le retour sans nouvelle Operation arrive dans
  `accept_remote`, après téléchargement et relecture du graphe local entier.
- [publish.rs::fetch](../../../../crates/layrs-client-core/src/native_scm/remote/publish.rs)
  envoie un seul claim puis télécharge les capacités successivement. Pour 200
  FileIds à un chunk, cela représente environ 200 FetchPlan et 400 GET, auxquels
  s'ajoutent Steps/Trees/Products. Les fichiers répétés sont dédupliqués seulement
  dans le graphe en mémoire de cette réception.
- [production.rs::download](../../../../crates/layrs-server/src/v3/production.rs)
  réautorise chaque GET et appelle le resolver du claim, qui retrouve racine,
  chemin, manifeste et chunks. Ce contrôle est nécessaire ; le supprimer ou le
  mémoriser au-delà d'un epoch serait une régression de droits.
- `download_tree` cherche un FileManifest par scan de tous les objets pour chaque
  chemin. `GraphObjects::file_bytes` fait aussi une recherche linéaire ;
  `GraphObjects::local` demande `file_availability` à chaque occurrence de FileId.
  Un index FileId et une déduplication des recherches réduiraient ce travail sans
  changer la portée des droits. Le coût exact reste à mesurer.

## Lot proposé, non implémenté par cet audit

1. Instrumenter séparément bootstrap, graph download, reconstruction locale,
   vérification checkout et admission, avec nombres de requêtes/objets/octets.
   Aucun token, URL signée ou chemin de travail privé dans les logs.
2. Priorité réception inchangée : conserver après réception réussie un témoin
   explicite de la View/Operation locales et du bootstrap distant. Après un nouveau
   bootstrap authentifié, ne prendre le chemin court que si ce témoin, toutes les
   racines/products/versions/policy/epoch et la liaison origine/Space correspondent.
   Réacquérir le verrou de journal, revérifier état local/dirty/annulation avant
   retour. Un simple bootstrap inchangé ne suffit pas : des rewrites ou changements
   locaux peuvent demander une normalisation différente des têtes distantes.
3. Pour clone/changements : index FileId en mémoire puis batch de claims borné,
   conserver binding/descriptor/closure checks et ordre d'annulation. Ne jamais
   transformer un cache de contenu en autorisation.

Tests requis : réception inchangée sans nouveau téléchargement ni Operation,
révocation et epoch modifié refusés/revalidés, modification locale pendant HTTP,
rewrite local, bootstrap avec nouveau Product/Marker/Release, clone interrompu,
CAS local manquant/corrompu, bindings différents et annulation. Une instrumentation
doit distinguer amélioration réelle et simple décalage de travail hors de la mesure.

Cet audit n'a modifié aucun fichier de transfert ; le lot de cache/reprise de
fusion_native reste indépendant.
