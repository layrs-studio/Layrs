# Webhooks signés et reprise durable

Aucune destination n'est autorisée par défaut. Sur le serveur, configurer
`LAYRS_WEBHOOK_ALLOWED_ORIGINS` (origines HTTPS séparées par des espaces).
Une adresse privée nécessite aussi son origine exacte dans
`LAYRS_WEBHOOK_PRIVATE_ORIGINS`. `LAYRS_WEBHOOK_CA_FILE` fournit une CA privée.
La résolution DNS est vérifiée puis épinglée pour la requête ; redirections,
credentials/query dans l'URL et proxy implicite sont refusés.

Un propriétaire autorisé crée un abonnement par
`POST /v3/spaces/:space/webhooks` avec
`{"url":"https://outils.example/webhooks/layrs","eventKinds":["review.proposed","proof.completed"]}`.
La réponse contient `webhookId`, `secretGeneration` et `secret` (hexadécimal),
montré seulement à cette création. Événements : `review.proposed`, `review.action`,
`proof.completed`, `protection.changed`, `membership.changed`.
Les événements SCM génériques ne sont pas encore inclus dans cette version.

Les notifications métier sont écrites dans la transaction acceptée. Le dispatcher
reprend ces lignes persistantes et les livraisons abandonnées après redémarrage.
L'identifiant de livraison reste identique entre tentatives. Huit tentatives
automatiques au maximum, délai exponentiel à partir de 10 s, lease 60 s,
requête 10 s max (résolution DNS 5 s max). Les droits du créateur sont réévalués
avant envoi. La sémantique est « au moins une fois » : dédupliquer côté récepteur.

Signature : `HMAC-SHA256(secret_bytes, timestamp + "." + deliveryId + "." + raw_body)`.
Headers : `X-Layrs-Delivery`, `X-Layrs-Timestamp` (secondes Unix),
`X-Layrs-Key-Generation`, `X-Layrs-Signature: v1=HEX`.
Vérifier les octets exacts, l'identifiant et une fenêtre temporelle courte.

- GET `.../webhooks` : configuration sans secrets.
- GET `.../webhooks/:id/deliveries` : 100 dernières livraisons/statuts/erreurs bornées.
- POST `.../webhooks/:id/rotate` : nouvelle génération et nouveau secret.
- POST `.../webhooks/:id/disable` : arrêt des prochaines tentatives.
- POST `.../webhooks/:id/replay` avec `{"deliveryId":"..."}` : relance d'une
  livraison terminée/épuisée, même identité, action auditée.

Le secret est dérivé de la clé d'instance et de l'abonnement/génération. Conserver
la clé d'instance pendant restauration ; la changer modifie aussi les signatures.
La rotation d'un abonnement ne nécessite aucune valeur secrète en base.

`receiver.py` est un exemple standard-library, derrière un proxy HTTPS. Fournir
`LAYRS_WEBHOOK_KEYS_FILE` contenant `{"1":"SECRET_HEX"}` (mettre à jour lors de
rotation), et `LAYRS_WEBHOOK_RECEIPTS_DB` pour son inbox SQLite durable.
Il vérifie la signature, refuse les replays de plus de 5 minutes, accepte les
doublons identiques et refuse les doublons contradictoires. Il conserve l'événement
pour un worker propre à l'outil : il n'exécute aucune commande reçue.

```sh
python -m unittest discover -s examples/webhooks
python examples/webhooks/receiver.py
```
