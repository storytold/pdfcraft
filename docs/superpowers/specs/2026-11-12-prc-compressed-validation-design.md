# Validation indépendante des tessellations PRC compressées

Date : 2026-11-12
Statut : conception validée par le demandeur

## 1. Contexte

Le ticket [storytold/pdfcraft#1297](https://github.com/storytold/pdfcraft/issues/1297) demande la prise en charge des PDF 3D. KenM76 a signalé que la décompression des maillages PRC compressés nécessitait une aide extérieure, en raison de triangles encore incorrects dans des exports SolidWorks.

La PR [#7034](https://github.com/storytold/pdfcraft/pull/7034), fusionnée dans `490287fc9d04d1915c690a3d0f6954bde706e15e`, reconstruit désormais les `TESS_3D_Compressed` dont les normales sont recalculées et dont le tableau d’états contient trois valeurs par triangle. Son parcours et sa gestion des plis sont partiellement inférés des données, plutôt que directement prescrits par la spécification. Sa revue ne comprend pas de comparaison géométrique indépendante sur des modèles externes.

Le point de départ est le `main` actuel, au commit `120c62fb`, qui contient cette PR. Les fichiers principaux sont :

- `crates/prc/src/tess/compressed.rs` pour le parcours et la géométrie ;
- `crates/prc/src/tess.rs` pour le décodage, les budgets et l’assemblage du maillage ;
- `crates/prc/src/tess/compressed/tests.rs` et `crates/prc/src/tess/tests.rs` pour les tests existants ;
- `crates/prc/src/testw.rs` pour l’écriture de flux binaires de test.

La coordination a été annoncée publiquement dans [ce commentaire du ticket](https://github.com/storytold/pdfcraft/issues/1297#issuecomment-6833439155).

## 2. Objectif et critère de réussite

Éprouver indépendamment la reconstruction déjà fusionnée, puis corriger une cause précise uniquement si un défaut est reproduit.

Le premier incrément est réussi dans l’un des deux cas suivants :

1. **Défaut reproduit** : un petit cas synthétique original échoue sur le code non corrigé, un correctif limité à cette cause le fait passer, et les vérifications du projet passent.
2. **Aucun défaut reproduit** : les scénarios exécutés et leurs limites sont publiés sur le ticket, sans modification du code de production et sans prétendre valider les fichiers réels non testés.

Une suite de tests qui passe sans avoir distingué les erreurs possibles de l’encodeur de test et du décodeur ne suffit pas à déclarer un défaut du décodeur.

## 3. Approches comparées

### Synthétique d’abord — retenue

Construire des maillages originaux et un encodeur de test indépendant, comparer le résultat décodé à un oracle calculé séparément, puis minimiser tout écart. Cette approche est déterministe, immédiatement exécutable et produit des fixtures légères et commitables.

### Modèles réels d’abord — différée

Attendre les deux exports SolidWorks mentionnés par KenM76. Cette approche est la plus représentative, mais elle dépend d’une réponse externe et de la licence des fichiers. Elle complétera l’approche retenue dès que des fixtures utilisables seront disponibles.

### Fuzzing d’abord — écartée pour cet incrément

Muter les flux compressés est utile pour rechercher des paniques, mais fournit rarement un oracle de géométrie. Cette méthode ne cible donc pas directement les triangles incorrects signalés.

## 4. Périmètre

Sont inclus :

- les tessellations compressées avec normales recalculées et trois statuts d’arête par triangle ;
- la comparaison des positions, de la topologie, du regroupement par face et de la consommation des tableaux ;
- les erreurs propres et les limites de budget sur les entrées invalides ;
- un correctif ciblé si, et seulement si, un défaut est reproduit et attribué au décodeur ;
- les tests de rendu ou d’automation nécessaires uniquement si le défaut corrigé touche ces couches.

Sont exclus :

- les normales stockées ;
- le format à un statut d’arête par triangle ;
- la recherche du meilleur ajustement, le contrôle de planéité et la récupération partielle ;
- toute refonte générale du lecteur, nouvelle dépendance, modification de CI ou travail d’interface graphique ;
- toute modification de production spéculative en l’absence d’échec reproductible.

## 5. Architecture du harnais de validation

Les tests rejoindront les modules de test existants de `pdfcraft-prc`, sans nouvelle crate ni dépendance. Le writer binaire `W` pourra être réutilisé pour sérialiser les champs, mais l’encodeur géométrique de test ne réutilisera ni le parcours ni la fonction de reconstruction du code de production.

Le flux de validation est le suivant :

1. définir un maillage source et sa forme quantifiée attendue ;
2. encoder indépendamment les points, références, statuts et faces ;
3. décoder avec l’API réelle de `pdfcraft-prc` ;
4. comparer le maillage reconstruit à l’oracle ;
5. en cas d’écart, vérifier d’abord l’encodeur et l’oracle du test ;
6. réduire le cas, démontrer l’échec sur le code initial, puis corriger une seule cause.

Les cas couvriront, dans la mesure où l’encodeur indépendant peut les exprimer sans reprendre les hypothèses du décodeur :

- une graine et une bande de triangles non plane ;
- plusieurs composantes connexes ;
- des références à des sommets déjà décodés ;
- des plis, y compris ceux qui sollicitent la reprise d’une composante ;
- une arête de continuation déjà fermée ;
- des repères dégénérés ;
- des tableaux incomplets, excédentaires ou incohérents ;
- les limites du budget de reconstruction.

L’encodeur de test sera lui-même validé sur de petits exemples calculables manuellement avant de servir à accuser le décodeur.

## 6. Oracles et gestion des erreurs

Les oracles sont, selon le scénario :

- les positions exactes du maillage de travail quantifié, ou une borne d’erreur explicitement justifiée par l’encodage ;
- les triplets d’indices des triangles et leur ordre de parcours ;
- les intervalles de triangles par face et les styles déjà pris en charge ;
- la consommation exacte des tableaux d’entrée ;
- le maintien en `Tessellation::Compressed`, ou une `PrcError` appropriée, pour une entrée non prise en charge ou incohérente ;
- l’absence de panique, de débordement et de dépassement du budget.

Une divergence ne sera considérée comme un défaut du décodeur qu’après vérification croisée du calcul attendu. Un échec de l’oracle sera corrigé dans le test, sans modifier le code de production.

## 7. Fixtures réelles et licences

Les fichiers SolidWorks ou autres modèles externes seront utilisés localement uniquement, tant que leur origine et leur licence de redistribution ne sont pas établies. Ils ne seront ni commités, ni intégrés à une documentation publique, ni utilisés pour produire des captures publiées.

Seule une fixture synthétique minimale, originale et nécessaire à un test de non-régression pourra être commitée. Aucun asset Adobe, code GPL/AGPL ou code provenant d’un autre moteur ne sera introduit. Aucun nouveau paquet ne sera ajouté.

## 8. Correctif éventuel

Si un défaut est confirmé :

- le test minimal est ajouté et vérifié en échec sur le code initial ;
- le correctif reste dans la fonction responsable du défaut ;
- le test passe avec le correctif et échoue de nouveau lorsqu’il est retiré temporairement ;
- les tests existants ne sont pas affaiblis pour accommoder le nouveau comportement ;
- les formes toujours non prises en charge continuent à rester compressées ou à produire une erreur propre ;
- la description de la PR distingue les preuves synthétiques des validations sur modèles réels.

Si aucun défaut n’est confirmé, le chantier s’arrête après la publication des résultats et des angles morts ; il ne se transforme pas implicitement en implémentation des normales stockées.

## 9. Vérifications finales

Les commandes seront adaptées aux fichiers effectivement modifiés, avec au minimum :

- `cargo fmt --all -- --check` ;
- `cargo clippy -p pdfcraft-prc --all-targets -- -D warnings` ;
- `cargo test -p pdfcraft-prc` ;
- `cargo xtask layers` ;
- `cargo xtask assets` ;
- `cargo xtask wasm`.

Si le correctif touche le rendu ou son intégration, les tests et Clippy de `pdfcraft-prc-render` et de `pdfcraft-automation`, ainsi que le scénario `model_render` pertinent, seront ajoutés. Toute vérification non exécutée ou en échec sera signalée explicitement.

## 10. Hypothèses retenues

- Le travail commence sans attendre les fichiers de KenM76.
- Aucun droit de redistribution n’est supposé pour les modèles externes.
- Les tests et les fixtures synthétiques sont originaux.
- La branche de travail est `test/prc-compressed-validation`.
- Une éventuelle PR de correctif référencera le ticket n° 1297 sans prétendre le résoudre entièrement.

## 11. Actualisation pendant la planification — 2026-11-12

La lecture de [N570](https://docs.techsoft3d.com/hoops/exchange/_downloads/a7028c5c324de43fc7d5083bfa100c2a/SC2N570-PRC-WD.pdf), §7.8.9.2, confirme qu’un drapeau de voisinage décrit chaque triangle. Les mentions de « trois statuts par triangle » dans cette spécification désignent la variante de tableau `3T` acceptée par le lecteur actuel, dont les `T` premiers drapeaux sont lus. Elles ne constituent pas une affirmation que cette disposition est normative. Le périmètre validé reste inchangé.
