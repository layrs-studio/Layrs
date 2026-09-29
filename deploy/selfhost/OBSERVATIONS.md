# Déclarer une observation de capture

Une capture n’a pas d’auteur ou de date de création intrinsèque. Le serveur
conserve séparément sa première réception et les observations déclarées par les
comptes authentifiés. Une déclaration ne prouve pas qui a créé le contenu.

La commande native suivante ajoute une déclaration explicite ; elle ne capture
aucun fichier et ne déplace aucune référence :

```text
layrs remote observe observation.json --endpoint https://serveur.exemple
```

Exemple de fichier, à adapter avec l’identifiant complet d’une capture déjà
admise dans ce projet :

```json
{
  "step": "blake3:IDENTIFIANT_COMPLET",
  "observedAt": "2026-09-20T10:30:00+02:00",
  "reason": "manual_snapshot",
  "idempotencyKey": "observation-equipe-20260920-001"
}
```

Le compte et l’acteur proviennent de la session authentifiée. Le client ne peut
pas fournir une autre identité, ni exporter un identifiant d’Operation, de
Worktree ou d’appareil local dans cette déclaration. L’identifiant de
l’observation est dérivé du projet, du compte, de la capture, de la date déclarée
et du motif ; les enregistrements existants ne sont jamais remplacés.

Réutiliser exactement le même fichier et la même clé après une réponse perdue.
Le client journalise la demande avant envoi. Tant que son résultat est inconnu,
reprendre cette demande avant une publication ou une réception ordinaire.
Employer une nouvelle clé pour une nouvelle intention. Les motifs suivent
`CaptureReason` V1, par exemple `manual_snapshot`, `automation` ou `rewrite` ;
ce sont des déclarations du compte, pas une attestation de l’opération locale.

`layrs remote history CAPTURE --endpoint …` et l’historique publié de Studio
affichent la provenance. La **date déclarée** reste distincte de la **réception
serveur**. Les filtres de période portent sur cette réception : antidater une
déclaration ne l’insère pas dans un ancien parcours d’historique. Les curseurs
antérieurs au nouveau format doivent être recommencés explicitement.

L’admission vérifie les droits courants, le Space et la disponibilité de la
capture dans celui-ci. Les quotas et les bornes de pagination évitent qu’un
afflux de déclarations rende l’historique illisible. Les détails et les preuves
de qualification sont consignés dans le journal T4 et l’ADR0021.
