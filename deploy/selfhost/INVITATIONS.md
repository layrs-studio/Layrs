# Invitations nominatives par Space

Un propriétaire ou administrateur actuellement autorisé à administrer la politique invite un **compte humain existant** de cette installation. Le destinataire accepte ou refuse depuis sa session, même sans projet ouvert. Aucun email, lien public ou token d'invitation n'est nécessaire. Les comptes sont créés séparément par l'administrateur de l'installation.

Dans Desktop/Web : Administration → Invitations. Copier l'identifiant du compte destinataire, choisir `member` ou `auditor`, puis la durée (interface : 1 à 7 jours). Le destinataire ouvre sa boîte de réception. Les droits n'apparaissent qu'après acceptation ; une invitation seule ne crée aucune appartenance.

## API authentifiée

| Action | Route |
|---|---|
| Boîte du destinataire | `GET /v3/invitations?after=0` |
| Invitations du Space, administrateur | `GET /v3/spaces/:space/invitations?after=0` |
| Inviter | `POST /v3/spaces/:space/invitations` |
| Accepter, refuser, révoquer | `POST /v3/invitations/:invitation/actions` |

Corps de création :

```json
{"idempotencyKey":"invitation-example-unique","recipientPrincipalId":"principal:example","role":"auditor","expiresInSeconds":86400}
```

Le compte réel remplace l'identifiant d'exemple. Une même clé et un même corps rejouent le reçu ; un corps différent avec cette clé est refusé. Durée API : 60 à 604800 secondes. Maximum 256 invitations pendantes non expirées par Space. Les listes retournent jusqu'à 100 éléments et `nextAfter` pour la page suivante.

Décision : `{"action":"accept"}`, `{"action":"reject"}` ou `{"action":"revoke"}`. Seul le destinataire accepte/refuse ; seul l'auteur, encore autorisé, révoque une proposition pendante. Les autres comptes ne peuvent pas lire le reçu. Un compte suspendu ne peut pas agir. Le grant accorde le rôle standard sur tout le Space ; il ne confère pas un rôle administrateur ni de droits sur les autres Spaces.

L'acceptation exige la politique/epoch d'origine encore courante et une appartenance Workspace qui n'a pas été désactivée. Si l'invitation est périmée ou obsolète, la refuser/révoquer puis en créer une nouvelle. Une acceptation déjà effectuée est définitive ; retirer ensuite les droits via l'administration des membres. Rejouer le reçu d'acceptation ne rétablit pas ces droits.

La proposition et la décision sont des Evidence canoniques ; acceptation, PolicySet, epoch, notifications et reçu sont atomiques. La sauvegarde PostgreSQL/CAS conserve ces objets et projections. Il n'existe actuellement ni groupes Teams, ni invitation d'une adresse sans compte, ni envoi de courriel, ni ACL de chemin.

Validation : `native_invitations` utilise PostgreSQL réel et un rôle runtime limité, notamment une panne SQL après installation du grant pour vérifier son rollback complet. Les contrats de route UI sont testés par `node scripts/test-native-admin.mjs`. La recette UI finale reste distincte de ces tests API.
