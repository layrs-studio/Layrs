# Comparer les mesures natives sans masquer leurs limites

Le comparateur `scripts/compare-native-benchmarks.mjs` contrôle une régression
entre deux rapports **existants** et ne lance aucun benchmark. Il ne modifie ni
les sources ni les rapports. Son premier périmètre est strictement le JSON
`layrs.tls-volume-measurement.v1` produit par `sync_volume_drill.rs`.

Les formats locaux ont été inspectés : `local-volume-measurement.v1` ne déclare
pas le profil et contient aussi des refus attendus ; `local-memory-measurement.v1`
ne déclare pas platform/profile et résultat final. Ils sont donc **refusés**, sans
inventer ces dimensions depuis leur nom de fichier. Le JSONL100Mio/1Gio et ses
compteurs mémoire ne sont pas pris en charge par ce lot. Aucun schéma de mesure
n'a été modifié pour contourner ces limites.

## Entrées et compatibilité

```text
node scripts/compare-native-benchmarks.mjs --baseline baseline.json --candidate candidate.json --thresholds seuils.json
node --test scripts/compare-native-benchmarks.test.mjs
```

Chaque fichier doit être un JSON UTF-8 de1Mio maximum. Baseline et candidate
doivent avoir le même schéma, platform, profile, liste de limitations décrivant
la charge, et le même ensemble de cas. Aucun alias win32/windows ou debug/release
n'est appliqué. Les valeurs de profil sont comparées telles que déclarées : le
comparateur ne peut pas prouver les options de compilation d'un binaire.

Un résultat `passed` est requis pour le rapport et chacun de ses cas. Les neuf
phases du scénario doivent être présentes une seule fois dans leur ordre normal,
avec des durées entières finies et positives ou nulles. Un rapport incomplet
marqué artificiellement passed reste refusé. Les cas10k/99k doivent déclarer
leurs nombres cohérents de fichiers, répertoires et octets256par fichier. Les
dimensions inconnues ou manquantes, doublons et nouvelles phases sont refusés.
L'ordre des cas peut différer. Les notes et la durée totale facultatives ne sont
pas des métriques comparées.

Le schéma TLS actuel ne décrit ni la machine, ni sa charge concurrente, ni les
versions PostgreSQL/TLS, ni une identité structurée exhaustive du générateur.
**Ces conditions restent à vérifier extérieurement.** Égalité des champs ne
prouve pas identité de l'environnement ou signification statistique.

## Seuils explicites

Exemple **purement illustratif, non calibré et non recommandé comme SLA** :

```json
{
  "schemaVersion": "layrs.benchmark-thresholds.v1",
  "reportSchema": "layrs.tls-volume-measurement.v1",
  "relativePercent": 20,
  "absoluteFloorMs": 500,
  "phaseOverrides": {
    "receive-unchanged": { "relativePercent": 25, "absoluteFloorMs": 250 }
  }
}
```

Aucun seuil n'est implicite. Une surcharge de phase doit fournir ses deux valeurs.
Une régression existe si :

```text
candidateMs - baselineMs > max(ceil(baselineMs × relativePercent / 100), absoluteFloorMs)
```

Les deux seuils doivent donc être dépassés ; une variation relative importante
sur une opération très courte peut rester sous le plancher de bruit absolu.
L'égalité à la limite passe. Une baseline0 utilise le plancher absolu, sans
division par zéro. Les améliorations passent. Pourcentages finis0–1000 et
planchers entiers non négatifs sont acceptés ; débordements numériques refusés.

Sortie JSON sur stdout, incluant chaque cas/phase, temps, écart, seuil appliqué,
décision et SHA256 des trois fichiers lus. Codes : **0** comparaison conforme,
**1** régression détectée, **2** entrées invalides/incompatibles ou erreur de
lecture. Un code2 ne doit jamais être converti en résultat vert ou en métrique0.

## Calibration avant un contrôle CI bloquant

1. Choisir un environnement stable, même machine et profil, même fixture,
   versions de services et politique de cache. Isoler les compilations et autres
   qualifications ; conserver les hashes des binaires et les rapports bruts.
2. Répéter les scénarios complets plusieurs fois sur la référence et la candidate,
   vérifier tous les résultats fonctionnels, puis observer médiane, dispersion
   et valeurs extrêmes par phase. Ne pas sélectionner seulement le meilleur run.
3. Déterminer les seuils relatifs et absolus depuis cette variabilité et le coût
   opérationnel accepté. Le comparateur ne calcule pas de baseline statistique
   ni de p-value et ne transforme pas un run unique en engagement de performance.
4. Commencer en contrôle informatif, enquêter sur les faux positifs, puis versionner
   une référence et des seuils approuvés avant de bloquer la CI. Documenter chaque
   remplacement de référence, notamment après changement de charge ou de matériel.

Les mesures debug uniques existantes servent de preuves de parcours et de
diagnostic, pas de budget de performance contractuel. Ce lot ajoute un outil de
régression testé ; il ne coche pas le critère globalT8 de performance/observabilité.
