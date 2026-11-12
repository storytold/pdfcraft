# Validation des tessellations PRC compressées — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Valider indépendamment la reconstruction `TESS_3D_Compressed` fusionnée dans la PR n° 7034, obtenir un cas minimal si un défaut géométrique subsiste, et ne modifier la production qu’après attribution démontrée de la cause.

**Architecture:** Un module de test `independent.rs` sérialise des recettes explicites de triangles sans appeler le parcours ni les fonctions géométriques de production. Les oracles géométriques sont externes à cet encodeur : positions littérales calculées manuellement pour les petits cas, ou topologie connue et borne d’erreur de quantification par rapport au maillage source pour les cas complexes. `encoded.positions` sert à diagnostiquer l’encodeur mais ne constitue pas une preuve géométrique à lui seul. Les tests nouveaux sont des tests de caractérisation dont le résultat attendu est `PASS` ; tout échec déclenche le portail de diagnostic de la tâche 6 avant toute correction.

**Tech Stack:** Rust 2024, Cargo, tests unitaires `pdfcraft-prc`, writer binaire de test existant, N570 ISO/TC 171/SC 2, GitHub CLI pour la coordination.

## Global Constraints

- Spécification approuvée : `docs/superpowers/specs/2026-11-12-prc-compressed-validation-design.md`.
- Branche : `test/prc-compressed-validation` ; ne jamais éditer directement `main`.
- Base contenant la PR n° 7034 : `490287fc9d04d1915c690a3d0f6954bde706e15e` ; la branche de spécification part de `120c62fb`.
- Périmètre : normales recalculées et variante de tableau `3T` acceptée par le lecteur actuel. N570 §7.8.9.2 définit **un drapeau de voisinage par triangle** ; la disposition `3T`, dont les `T` premiers drapeaux sont lus par le code actuel, est une convention de fixture et n’est pas présentée comme normative. Source publique : https://docs.techsoft3d.com/hoops/exchange/_downloads/a7028c5c324de43fc7d5083bfa100c2a/SC2N570-PRC-WD.pdf
- Exclusions : normales stockées, variante `T`, meilleur ajustement, planéité, récupération partielle, interface graphique, nouvelle crate, nouvelle dépendance et modification de CI.
- L’encodeur de test n’appelle ni `Walk`, ni `apex`, ni `unitize`, ni `wd_length`, ni les aides arithmétiques de production. Il utilise uniquement les types `Arrays`, `Rebuilt` et la fonction `rebuild` comme API sous test.
- Les positions attendues ne doivent pas provenir uniquement d’`encoded.positions`. Chaque test compare le résultat à des littéraux dérivés manuellement ou à `assert_within_tolerance(actual, source, tolerance)`, avec une borne euclidienne de `sqrt(3) / 2 * tolerance` par sommet. `encoded.positions` peut être vérifié de la même façon pour diagnostiquer l’encodeur.
- Les fixtures sont originales. Les modèles externes restent locaux tant que leur licence de redistribution n’est pas établie ; aucun fichier externe n’est commité.
- Aucune modification de production sans échec reproductible, oracle vérifié et cause attribuée. Si ce cas survient, appliquer le portail de la tâche 6 au lieu d’essayer un correctif spéculatif.
- Chaque commit suit les règles PDFCraft : formatage, Clippy `-D warnings`, tests `pdfcraft-prc`, couches, assets et WASM. Les avertissements WASM déjà présents dans d’autres crates ne sont pas corrigés dans ce chantier.
- Les tests utilisent les conventions du module voisin : `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]` et assertions détaillées.
- Le plan est inclus dans le premier commit afin que la branche reste propre ; chaque tâche suivante est commitée séparément.

---

## Structure des fichiers

- **Créer `crates/prc/src/tess/compressed/independent.rs`** : encodeur géométrique indépendant, recettes de triangles, oracles et tous les nouveaux tests.
- **Modifier `crates/prc/src/tess/compressed.rs:352-353`** : déclarer uniquement le module de test `independent` sous `#[cfg(test)]`.
- **Modifier `crates/prc/README.md:32-41`** : aligner la description de l’API sur la reconstruction partielle déjà fusionnée, sans élargir le périmètre.
- **Annoter `docs/superpowers/specs/2026-11-12-prc-compressed-validation-design.md`** : distinguer le drapeau par triangle décrit par N570 de la variante `3T` acceptée par le lecteur.
- **Créer `docs/superpowers/plans/2026-11-12-prc-compressed-validation.md`** : ce plan, commité avec la première tâche.
- **Ne pas modifier `crates/prc/src/tess.rs`** pendant la validation. Seule une cause démontrée au portail de la tâche 6 pourrait justifier un plan correctif ultérieur touchant ce fichier ou `compressed.rs` hors déclaration de test.

### Task 1: Encodeur indépendant et cas manuel graine + apex

**Files:**
- Create: `crates/prc/src/tess/compressed/independent.rs`
- Modify: `crates/prc/src/tess/compressed.rs:352-353`
- Modify: `crates/prc/README.md:32-41`
- Modify: `docs/superpowers/specs/2026-11-12-prc-compressed-validation-design.md`
- Create: `docs/superpowers/plans/2026-11-12-prc-compressed-validation.md`

**Interfaces:**
- Consumes: `compressed::Arrays<'a>`, `compressed::Rebuilt`, `compressed::rebuild(&Arrays<'_>) -> Result<Rebuilt, &'static str>`, et `V = [f64; 3]` du module parent.
- Produces: `Slot`, `Recipe`, `Encoded`, `encode(origin: V, tolerance: f64, recipes: &[Recipe]) -> Encoded`, `Encoded::arrays(limit: usize) -> Arrays<'_>`, `Encoded::faces() -> usize`, `assert_positions(actual: &[V], expected: &[V])`, `assert_within_tolerance(actual: &[V], source: &[V], tolerance: f64)`. Les tâches 2 à 5 utilisent ces noms exacts.

- [ ] **Step 1: Ajouter le module et un test qui ne compile pas encore**

Dans `crates/prc/src/tess/compressed.rs`, conserver `mod tests;` et ajouter :

```rust
#[cfg(test)]
mod tests;
#[cfg(test)]
mod independent;
```

Créer `crates/prc/src/tess/compressed/independent.rs` avec :

```rust
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;

const S: f64 = std::f64::consts::SQRT_2;

#[test]
fn independent_seed_and_apex_match_a_hand_computed_stream() {
    let encoded = encode(
        [10.0, 0.0, 0.0],
        0.5,
        &[
            Recipe::Seed {
                slots: [
                    Slot::Point([10.0, 0.0, 0.0]),
                    Slot::Point([12.0, 0.0, 0.0]),
                    Slot::Point([10.0, 2.0, 0.0]),
                ],
                status: 2,
                face: 0,
            },
            Recipe::Apex {
                edge: (2, 1),
                opposite: 0,
                slot: Slot::Point([11.0 + 2.0 * S, 1.0 + S, -1.0]),
                status: 0,
                face: 0,
            },
        ],
    );

    assert_eq!(encoded.points, [0, 0, 0, 4, 0, 0, -2, 4, 0, 2, -6, 2]);
    let rebuilt = rebuild(&encoded.arrays(usize::MAX)).unwrap();
    assert_eq!(rebuilt.triangles, [[0, 1, 2], [2, 1, 3]]);
    let expected = [
        [10.0, 0.0, 0.0],
        [12.0, 0.0, 0.0],
        [10.0, 2.0, 0.0],
        [11.0 + 2.0 * S, 1.0 + S, -1.0],
    ];
    assert_positions(&rebuilt.positions, &expected);
}
```

- [ ] **Step 2: Vérifier le RED dû à l’absence de l’encodeur**

Run: `cargo test -p pdfcraft-prc tess::compressed::independent::independent_seed_and_apex_match_a_hand_computed_stream -- --exact`

Expected: échec de compilation indiquant que `encode` et `assert_positions` ne sont pas définis. Ce RED prouve seulement l’absence du nouveau harnais, pas un défaut de production.

- [ ] **Step 3: Implémenter l’encodeur sans réutiliser la géométrie de production**

Insérer le code suivant avant le test dans `independent.rs`, en conservant le test de l’étape 1 :

```rust
#[derive(Clone, Copy)]
enum Slot {
    Point(V),
    Reference(u32),
}

#[derive(Clone, Copy)]
enum Recipe {
    Seed { slots: [Slot; 3], status: i32, face: u32 },
    Apex { edge: (u32, u32), opposite: u32, slot: Slot, status: i32, face: u32 },
}

impl Recipe {
    fn status(self) -> i32 {
        match self {
            Self::Seed { status, .. } | Self::Apex { status, .. } => status,
        }
    }

    fn face(self) -> u32 {
        match self {
            Self::Seed { face, .. } | Self::Apex { face, .. } => face,
        }
    }
}

#[derive(Clone)]
struct Encoded {
    tolerance: f64,
    origin: V,
    points: Vec<i64>,
    edge_status: Vec<i32>,
    face_of: Vec<u32>,
    is_reference: Vec<bool>,
    references: Vec<u32>,
    positions: Vec<V>,
    triangles: Vec<[u32; 3]>,
}

impl Encoded {
    fn arrays(&self, limit: usize) -> Arrays<'_> {
        Arrays {
            tolerance: self.tolerance,
            origin: self.origin,
            points: &self.points,
            edge_status: &self.edge_status,
            triangles: self.triangles.len(),
            is_reference: &self.is_reference,
            references: &self.references,
            limit,
        }
    }

    fn faces(&self) -> usize {
        self.face_of.iter().max().map_or(0, |&face| face as usize + 1)
    }

    fn push_reference(&mut self, vertex: u32) -> u32 {
        let index = usize::try_from(vertex).unwrap();
        assert!(index < self.positions.len(), "a recipe reference must name an earlier vertex");
        self.is_reference.push(true);
        self.references.push(vertex);
        vertex
    }

    fn push_position(&mut self, position: V, point: [i64; 3]) -> u32 {
        self.points.extend(point);
        self.is_reference.push(false);
        self.positions.push(position);
        u32::try_from(self.positions.len() - 1).unwrap()
    }

    fn push_seed_point(&mut self, index: usize, target: V, triangle: [u32; 3]) -> u32 {
        let base = match index {
            0 => self.origin,
            1 => self.positions[usize::try_from(triangle[0]).unwrap()],
            2 => {
                let first = self.positions[usize::try_from(triangle[0]).unwrap()];
                let second = self.positions[usize::try_from(triangle[1]).unwrap()];
                p_scale(p_add(first, second), 0.5)
            }
            _ => panic!("a seed has exactly three slots"),
        };
        let point = quantize(p_sub(target, base), self.tolerance);
        let position = p_add(base, scaled(point, self.tolerance));
        self.push_position(position, point)
    }

    fn push_apex_point(&mut self, edge: (u32, u32), opposite: u32, target: V) -> u32 {
        let (p, q) = edge;
        let (lo, hi) = if p < q { (p, q) } else { (q, p) };
        let lo = self.positions[usize::try_from(lo).unwrap()];
        let hi = self.positions[usize::try_from(hi).unwrap()];
        let opposite = self.positions[usize::try_from(opposite).unwrap()];
        let (origin, x, y, z) = encoder_frame(lo, hi, opposite);
        let offset = p_sub(target, origin);
        let point = [
            quantize_component(p_dot(offset, x), self.tolerance),
            quantize_component(-p_dot(offset, y), self.tolerance),
            quantize_component(-p_dot(offset, z), self.tolerance),
        ];
        let delta = scaled(point, self.tolerance);
        let position = p_sub(
            p_sub(p_add(origin, p_scale(x, delta[0])), p_scale(y, delta[1])),
            p_scale(z, delta[2]),
        );
        self.push_position(position, point)
    }
}

fn encode(origin: V, tolerance: f64, recipes: &[Recipe]) -> Encoded {
    assert!(tolerance.is_finite() && tolerance > 0.0, "a positive finite tolerance");
    let mut edge_status: Vec<i32> = recipes.iter().map(|recipe| recipe.status()).collect();
    edge_status.resize(recipes.len().saturating_mul(3), 0);
    let mut encoded = Encoded {
        tolerance,
        origin,
        points: Vec::new(),
        edge_status,
        face_of: Vec::new(),
        is_reference: Vec::new(),
        references: Vec::new(),
        positions: Vec::new(),
        triangles: Vec::new(),
    };

    for &recipe in recipes {
        encoded.face_of.push(recipe.face());
        match recipe {
            Recipe::Seed { slots, .. } => {
                let mut triangle = [0; 3];
                for (index, slot) in slots.into_iter().enumerate() {
                    triangle[index] = match slot {
                        Slot::Point(target) => encoded.push_seed_point(index, target, triangle),
                        Slot::Reference(vertex) => encoded.push_reference(vertex),
                    };
                }
                encoded.triangles.push(triangle);
            }
            Recipe::Apex { edge: (p, q), opposite, slot, .. } => {
                let vertex = match slot {
                    Slot::Point(target) => encoded.push_apex_point((p, q), opposite, target),
                    Slot::Reference(vertex) => encoded.push_reference(vertex),
                };
                encoded.triangles.push([p, q, vertex]);
            }
        }
    }
    encoded
}

fn p_add(a: V, b: V) -> V {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn p_sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn p_scale(a: V, factor: f64) -> V {
    a.map(|component| component * factor)
}

fn p_dot(a: V, b: V) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn p_cross(a: V, b: V) -> V {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn quantize_component(value: f64, tolerance: f64) -> i64 {
    let quantized = (value / tolerance).round();
    assert!(quantized.is_finite() && quantized >= i64::MIN as f64 && quantized <= i64::MAX as f64);
    quantized as i64
}

fn quantize(value: V, tolerance: f64) -> [i64; 3] {
    value.map(|component| quantize_component(component, tolerance))
}

fn scaled(point: [i64; 3], tolerance: f64) -> V {
    point.map(|component| component as f64 * tolerance)
}

fn encoder_length(value: V) -> Option<f64> {
    let squared = (value[0] * value[0] + value[1] * value[1] + value[2] * value[2]) as f32;
    let bits = squared.to_bits();
    let exponent = (bits >> 23) & 0xff;
    let seed = if exponent > 127 {
        f32::from_bits((bits & !(0xff << 23)) | (((exponent - 127) / 2 + 127) << 23))
    } else {
        squared
    };
    let squared = f64::from(squared);
    let mut current = f64::from(seed);
    for _ in 0..100 {
        let next = 0.5 * (current + squared / current);
        if next == current {
            return Some(next);
        }
        current = next;
    }
    None
}

fn encoder_unit(value: V) -> Option<V> {
    let length = encoder_length(value).filter(|&length| length >= f64::from(f32::EPSILON))?;
    Some([value[0] / length, value[1] / length, value[2] / length])
}

fn encoder_ortho_z(x: V) -> Option<V> {
    let x = encoder_unit(x)?;
    encoder_unit(p_cross(x, [0.0, 1.0, 0.0])).or_else(|| encoder_unit(p_cross(x, [1.0, 0.0, 0.0])))
}

fn encoder_frame(lo: V, hi: V, opposite: V) -> (V, V, V, V) {
    let origin = p_scale(p_add(lo, hi), 0.5);
    let difference = p_sub(lo, hi);
    let x = encoder_unit(difference).unwrap_or(difference);
    let residue = p_cross(p_sub(opposite, origin), x);
    let (y, z) = encoder_unit(residue)
        .and_then(|z| Some((encoder_unit(p_cross(z, x))?, z)))
        .or_else(|| {
            let z = encoder_ortho_z(x)?;
            let y = encoder_unit(p_cross(z, x))?;
            Some(if p_dot(residue, z) < 0.0 { (p_scale(y, -1.0), p_scale(z, -1.0)) } else { (y, z) })
        })
        .unwrap_or_default();
    (origin, x, y, z)
}

fn assert_positions(actual: &[V], expected: &[V]) {
    assert_eq!(actual.len(), expected.len(), "vertex counts differ");
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        for component in 0..3 {
            assert!(
                (actual[component] - expected[component]).abs() < 1e-9,
                "vertex {index}, component {component}: actual {actual:?}, expected {expected:?}",
            );
        }
    }
}

fn assert_within_tolerance(actual: &[V], source: &[V], tolerance: f64) {
    assert_eq!(actual.len(), source.len(), "vertex counts differ");
    let limit = tolerance * 3.0_f64.sqrt() / 2.0;
    for (index, (actual, source)) in actual.iter().zip(source).enumerate() {
        let difference = p_sub(*actual, *source);
        let distance = p_dot(difference, difference).sqrt();
        assert!(
            distance <= limit + 1e-12,
            "vertex {index}: distance {distance} exceeds {limit}; actual {actual:?}, source {source:?}",
        );
    }
}
```

- [ ] **Step 4: Vérifier le GREEN du cas manuel**

Run: `cargo test -p pdfcraft-prc tess::compressed::independent::independent_seed_and_apex_match_a_hand_computed_stream -- --exact`

Expected: PASS. Les douze entiers du tableau de points et les quatre positions littérales sont deux oracles manuels indépendants de `encoded.positions` ; si l’un diffère, vérifier d’abord l’encodeur de test.

- [ ] **Step 5: Aligner la documentation sur le comportement déjà fusionné**

Dans `crates/prc/README.md`, remplacer la puce `Tessellation::Compressed` par :

```markdown
- `Tessellation::Compressed` is read to its end so later entities decode. Recalculated-normal meshes in the accepted 3T form are rebuilt when their arrays fit; stored-normal meshes and unsupported traversals stay `Compressed`.
```

Remplacer la puce « Not yet in the crate » par :

```markdown
- Not yet in the crate: stored-normal or one-status compressed mesh reconstruction, textures.
```

Ajouter à la fin de `docs/superpowers/specs/2026-11-12-prc-compressed-validation-design.md` :

```markdown
## 11. Actualisation pendant la planification — 2026-11-12

La lecture de [N570](https://docs.techsoft3d.com/hoops/exchange/_downloads/a7028c5c324de43fc7d5083bfa100c2a/SC2N570-PRC-WD.pdf), §7.8.9.2, confirme qu’un drapeau de voisinage décrit chaque triangle. Les mentions de « trois statuts par triangle » dans cette spécification désignent la variante de tableau `3T` acceptée par le lecteur actuel, dont les `T` premiers drapeaux sont lus. Elles ne constituent pas une affirmation que cette disposition est normative. Le périmètre validé reste inchangé.
```

- [ ] **Step 6: Exécuter les portes avant commit**

Run:

```bash
cargo fmt --all -- --check && \
cargo clippy -p pdfcraft-prc --all-targets -- -D warnings && \
cargo test -p pdfcraft-prc && \
cargo xtask layers && \
cargo xtask assets && \
cargo xtask wasm
```

Expected: toutes les commandes passent. Les avertissements WASM déjà présents hors `pdfcraft-prc` peuvent rester affichés, mais la commande doit se terminer avec succès.

- [ ] **Step 7: Committer le harnais, la documentation et le plan**

```bash
git add crates/prc/src/tess/compressed.rs crates/prc/src/tess/compressed/independent.rs crates/prc/README.md docs/superpowers/specs/2026-11-12-prc-compressed-validation-design.md docs/superpowers/plans/2026-11-12-prc-compressed-validation.md
git commit -m "test(prc): add independent compressed mesh encoder"
```

### Task 2: Composantes multiples et références

**Files:**
- Modify: `crates/prc/src/tess/compressed/independent.rs`

**Interfaces:**
- Consumes: `Slot`, `Recipe`, `Encoded`, `encode`, `Encoded::arrays`, `assert_positions` de la tâche 1.
- Produces: `triangle_seed(x: f64, status: i32, face: u32) -> Recipe`, réutilisé par les tâches 3 et 5.

- [ ] **Step 1: Ajouter le test de caractérisation**

Ajouter à `independent.rs` :

```rust
fn triangle_seed(x: f64, status: i32, face: u32) -> Recipe {
    Recipe::Seed {
        slots: [
            Slot::Point([x, 0.0, 0.0]),
            Slot::Point([x + 2.0, 0.0, 0.0]),
            Slot::Point([x, 2.0, 0.0]),
        ],
        status,
        face,
    }
}

#[test]
fn independent_components_and_a_reference_are_rebuilt() {
    let encoded = encode(
        [0.0; 3],
        0.5,
        &[
            triangle_seed(0.0, 2, 0),
            Recipe::Apex {
                edge: (2, 1),
                opposite: 0,
                slot: Slot::Reference(0),
                status: 0,
                face: 0,
            },
            triangle_seed(5.0, 0, 1),
        ],
    );

    let rebuilt = rebuild(&encoded.arrays(usize::MAX)).unwrap();
    assert_eq!(rebuilt.triangles, [[0, 1, 2], [2, 1, 0], [3, 4, 5]]);
    assert_eq!(encoded.references, [0]);
    assert_eq!(encoded.face_of, [0, 0, 1]);
    assert_eq!(rebuilt.positions.len(), 6);
    let expected = [
        [0.0, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        [0.0, 2.0, 0.0],
        [5.0, 0.0, 0.0],
        [7.0, 0.0, 0.0],
        [5.0, 2.0, 0.0],
    ];
    assert_positions(&rebuilt.positions, &expected);
}
```

- [ ] **Step 2: Exécuter le test et appliquer le portail en cas d’échec**

Run: `cargo test -p pdfcraft-prc tess::compressed::independent::independent_components_and_a_reference_are_rebuilt -- --exact`

Expected: PASS. Si le test échoue, ne pas modifier la production ; vérifier la recette puis appliquer la tâche 6, section « Portail de défaut ».

- [ ] **Step 3: Exécuter les portes avant commit**

Run:

```bash
cargo fmt --all -- --check && \
cargo clippy -p pdfcraft-prc --all-targets -- -D warnings && \
cargo test -p pdfcraft-prc && \
cargo xtask layers && \
cargo xtask assets && \
cargo xtask wasm
```

Expected: succès complet.

- [ ] **Step 4: Committer**

```bash
git add crates/prc/src/tess/compressed/independent.rs
git commit -m "test(prc): cover compressed components and references"
```

### Task 3: Plis, reprise d’une composante et arête fermée

**Files:**
- Modify: `crates/prc/src/tess/compressed/independent.rs`

**Interfaces:**
- Consumes: `Slot`, `Recipe`, `encode`, `triangle_seed`, `Encoded::arrays`, `assert_within_tolerance`, et la constante `S`.
- Produces: `panel(target: V) -> Encoded`, local à ce module de test ; aucun autre module n’en dépend.

- [ ] **Step 1: Ajouter les tests de pli et de reprise**

Ajouter :

```rust
fn panel(target: V) -> Encoded {
    encode(
        [0.0; 3],
        0.5,
        &[
            triangle_seed(0.0, 3, 0),
            Recipe::Apex { edge: (2, 1), opposite: 0, slot: Slot::Point(target), status: 3, face: 0 },
            Recipe::Apex {
                edge: (2, 3),
                opposite: 1,
                slot: Slot::Reference(0),
                status: 2,
                face: 0,
            },
            Recipe::Apex {
                edge: (0, 3),
                opposite: 2,
                slot: Slot::Reference(1),
                status: 0,
                face: 0,
            },
        ],
    )
}

#[test]
fn independent_fold_and_component_retry_rebuild_the_same_panel() {
    let folded = [1.0 - 1.0 / S, 1.0 - S, 0.0];
    let unfolded = [1.0 + S, 1.0 + 1.0 / S, 0.0];
    for target in [folded, unfolded] {
        let source = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0], target];
        let encoded = panel(target);
        assert_within_tolerance(&encoded.positions, &source, 0.5);
        let rebuilt = rebuild(&encoded.arrays(usize::MAX)).unwrap();
        assert_eq!(rebuilt.triangles, [[0, 1, 2], [2, 1, 3], [2, 3, 0], [0, 3, 1]]);
        assert_eq!(rebuilt.positions.len(), 4);
        assert_within_tolerance(&rebuilt.positions, &source, 0.5);
    }
}
```

- [ ] **Step 2: Ajouter le test d’arête de continuation fermée**

Ajouter :

```rust
#[test]
fn independent_closed_continuation_uses_the_open_edge() {
    let encoded = encode(
        [0.0; 3],
        0.5,
        &[
            triangle_seed(0.0, 2, 0),
            Recipe::Apex {
                edge: (2, 1),
                opposite: 0,
                slot: Slot::Point([0.0, 0.0, 2.0]),
                status: 2,
                face: 0,
            },
            Recipe::Apex {
                edge: (3, 1),
                opposite: 2,
                slot: Slot::Reference(0),
                status: 2,
                face: 0,
            },
            Recipe::Apex {
                edge: (3, 0),
                opposite: 1,
                slot: Slot::Reference(2),
                status: 0,
                face: 0,
            },
        ],
    );

    let source = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 2.0]];
    assert_within_tolerance(&encoded.positions, &source, 0.5);
    let rebuilt = rebuild(&encoded.arrays(usize::MAX)).unwrap();
    assert_eq!(rebuilt.triangles, [[0, 1, 2], [2, 1, 3], [3, 1, 0], [3, 0, 2]]);
    assert_within_tolerance(&rebuilt.positions, &source, 0.5);
}
```

- [ ] **Step 3: Exécuter les deux tests**

Run: `cargo test -p pdfcraft-prc tess::compressed::independent::independent_fold_and_component_retry_rebuild_the_same_panel -- --exact`

Run: `cargo test -p pdfcraft-prc tess::compressed::independent::independent_closed_continuation_uses_the_open_edge -- --exact`

Expected: les deux tests passent. Tout écart de topologie déclenche le portail de la tâche 6.

- [ ] **Step 4: Exécuter les portes avant commit**

Run:

```bash
cargo fmt --all -- --check && \
cargo clippy -p pdfcraft-prc --all-targets -- -D warnings && \
cargo test -p pdfcraft-prc && \
cargo xtask layers && \
cargo xtask assets && \
cargo xtask wasm
```

Expected: succès complet.

- [ ] **Step 5: Committer**

```bash
git add crates/prc/src/tess/compressed/independent.rs
git commit -m "test(prc): probe compressed folds and closed edges"
```

### Task 4: Bande non plane et repère presque dégénéré

**Files:**
- Modify: `crates/prc/src/tess/compressed/independent.rs`

**Interfaces:**
- Consumes: toutes les interfaces des tâches 1 et 2.
- Produces: aucune interface nouvelle réutilisée hors de ces tests.

- [ ] **Step 1: Ajouter la bande non plane construite par recettes explicites**

Ajouter :

```rust
#[test]
fn independent_non_planar_strip_tracks_the_quantized_working_mesh() {
    let source: Vec<V> = (0..16)
        .map(|index| {
            [
                f64::from(index) * 0.05,
                f64::from(index % 2) * 2.0,
                (f64::from(index % 5) - 2.0) * 0.03,
            ]
        })
        .collect();
    let mut recipes = vec![Recipe::Seed {
        slots: [Slot::Point(source[0]), Slot::Point(source[1]), Slot::Point(source[2])],
        status: 2,
        face: 0,
    }];
    let (mut p, mut q, mut opposite) = (2_u32, 1_u32, 0_u32);

    for index in 1..source.len() - 2 {
        let vertex = u32::try_from(index + 2).unwrap();
        let status = if index % 2 == 1 { 1 } else { 2 };
        recipes.push(Recipe::Apex {
            edge: (p, q),
            opposite,
            slot: Slot::Point(source[index + 2]),
            status,
            face: 0,
        });
        if status == 1 {
            (p, q, opposite) = (p, vertex, q);
        } else {
            (p, q, opposite) = (vertex, q, p);
        }
    }

    let encoded = encode([0.0; 3], 1e-4, &recipes);
    assert_within_tolerance(&encoded.positions, &source, 1e-4);
    let rebuilt = rebuild(&encoded.arrays(usize::MAX)).unwrap();
    assert_eq!(rebuilt.triangles.len(), source.len() - 2);
    assert_eq!(rebuilt.triangles[1..4], [[2, 1, 3], [2, 3, 4], [4, 3, 5]]);
    assert_eq!(encoded.points.len(), source.len() * 3);
    assert_within_tolerance(&rebuilt.positions, &source, 1e-4);
}
```

- [ ] **Step 2: Ajouter le repère presque dégénéré**

Ajouter :

```rust
#[test]
fn independent_near_degenerate_parent_uses_an_orthogonal_frame() {
    let encoded = encode(
        [0.0; 3],
        1e-8,
        &[
            Recipe::Seed {
                slots: [
                    Slot::Point([1.0, 1.0 + 1e-8, 0.0]),
                    Slot::Point([2.0, 0.0, 0.0]),
                    Slot::Point([0.0, 2.0, 0.0]),
                ],
                status: 2,
                face: 0,
            },
            Recipe::Apex {
                edge: (2, 1),
                opposite: 0,
                slot: Slot::Point([0.3, 0.7, 0.2]),
                status: 0,
                face: 0,
            },
        ],
    );

    let source = [
        [1.0, 1.0 + 1e-8, 0.0],
        [2.0, 0.0, 0.0],
        [0.0, 2.0, 0.0],
        [0.3, 0.7, 0.2],
    ];
    assert_within_tolerance(&encoded.positions, &source, 1e-8);
    let rebuilt = rebuild(&encoded.arrays(usize::MAX)).unwrap();
    assert_eq!(rebuilt.triangles, [[0, 1, 2], [2, 1, 3]]);
    assert_within_tolerance(&rebuilt.positions, &source, 1e-8);
}
```

- [ ] **Step 3: Exécuter les deux tests**

Run: `cargo test -p pdfcraft-prc tess::compressed::independent::independent_non_planar_strip_tracks_the_quantized_working_mesh -- --exact`

Run: `cargo test -p pdfcraft-prc tess::compressed::independent::independent_near_degenerate_parent_uses_an_orthogonal_frame -- --exact`

Expected: PASS pour les deux. Si la bande échoue, vérifier d’abord que chaque recette décrit l’arête réellement attendue et que le repère de l’encodeur est calculé sur le maillage quantifié, puis appliquer le portail de la tâche 6.

- [ ] **Step 4: Exécuter les portes avant commit**

Run:

```bash
cargo fmt --all -- --check && \
cargo clippy -p pdfcraft-prc --all-targets -- -D warnings && \
cargo test -p pdfcraft-prc && \
cargo xtask layers && \
cargo xtask assets && \
cargo xtask wasm
```

Expected: succès complet.

- [ ] **Step 5: Committer**

```bash
git add crates/prc/src/tess/compressed/independent.rs
git commit -m "test(prc): cover non-planar and degenerate compression"
```

### Task 5: Consommation exacte, budgets et regroupement par face

**Files:**
- Modify: `crates/prc/src/tess/compressed/independent.rs`

**Interfaces:**
- Consumes: `Encoded::clone`, `Encoded::arrays`, `Encoded::faces`, `triangle_seed`, `encode`, `rebuild`, et `super::super::by_face`.
- Produces: aucune interface nouvelle.

- [ ] **Step 1: Ajouter les tests d’entrées incohérentes et de budget**

Ajouter :

```rust
#[test]
fn independent_rebuild_requires_exact_arrays_and_enough_budget() {
    let encoded = encode([0.0; 3], 0.5, &[triangle_seed(0.0, 0, 0)]);
    assert!(rebuild(&encoded.arrays(usize::MAX)).is_ok());

    let mut extra_point = encoded.clone();
    extra_point.points.push(0);
    assert_eq!(
        rebuild(&extra_point.arrays(usize::MAX)).unwrap_err(),
        "the decoded triangles do not use up the stored arrays",
    );

    let mut extra_slot = encoded.clone();
    extra_slot.is_reference.push(false);
    assert_eq!(
        rebuild(&extra_slot.arrays(usize::MAX)).unwrap_err(),
        "the decoded triangles do not use up the stored arrays",
    );

    let mut extra_reference = encoded.clone();
    extra_reference.references.push(0);
    assert_eq!(
        rebuild(&extra_reference.arrays(usize::MAX)).unwrap_err(),
        "the decoded triangles do not use up the stored arrays",
    );

    assert_eq!(
        rebuild(&encoded.arrays(0)).unwrap_err(),
        "the rebuild needs more than the decoded-element allowance left",
    );
}
```

- [ ] **Step 2: Ajouter le test indépendant de regroupement par face**

Ajouter :

```rust
#[test]
fn independent_faces_group_walk_order_triangles() {
    let encoded = encode(
        [0.0; 3],
        0.5,
        &[triangle_seed(0.0, 0, 1), triangle_seed(5.0, 0, 0)],
    );
    let rebuilt = rebuild(&encoded.arrays(usize::MAX)).unwrap();
    let mesh = super::super::by_face(rebuilt, &encoded.face_of, encoded.faces(), &[]);

    assert_eq!(mesh.faces, [0..1, 1..2]);
    assert_eq!(mesh.triangles, [[3, 4, 5], [0, 1, 2]]);
    let expected = [
        [0.0, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        [0.0, 2.0, 0.0],
        [5.0, 0.0, 0.0],
        [7.0, 0.0, 0.0],
        [5.0, 2.0, 0.0],
    ];
    assert_positions(&mesh.positions, &expected);
}
```

- [ ] **Step 3: Exécuter les deux tests**

Run: `cargo test -p pdfcraft-prc tess::compressed::independent::independent_rebuild_requires_exact_arrays_and_enough_budget -- --exact`

Run: `cargo test -p pdfcraft-prc tess::compressed::independent::independent_faces_group_walk_order_triangles -- --exact`

Expected: PASS. Les trois tableaux incohérents doivent produire la même erreur de consommation exacte ; le budget nul doit produire l’erreur explicite indiquée.

- [ ] **Step 4: Exécuter les portes avant commit**

Run:

```bash
cargo fmt --all -- --check && \
cargo clippy -p pdfcraft-prc --all-targets -- -D warnings && \
cargo test -p pdfcraft-prc && \
cargo xtask layers && \
cargo xtask assets && \
cargo xtask wasm
```

Expected: succès complet.

- [ ] **Step 5: Committer**

```bash
git add crates/prc/src/tess/compressed/independent.rs
git commit -m "test(prc): validate compressed budgets and face grouping"
```

### Task 6: Résultat final, portail de défaut et publication

**Files:**
- Review: `crates/prc/src/tess/compressed/independent.rs`
- Review: `crates/prc/src/tess/compressed.rs`
- Review: `docs/superpowers/specs/2026-11-12-prc-compressed-validation-design.md`

**Interfaces:**
- Consumes: tous les tests des tâches 1 à 5 et les résultats effectivement exécutés.
- Produces: soit une branche de tests sans changement de production et un compte rendu sur le ticket, soit un cas minimal transmis à `superpowers:systematic-debugging` avant tout plan correctif.

- [ ] **Step 1: Vérifier une seule fois si Ken a fourni des fixtures ou un symptôme précis**

Run:

```bash
gh issue view 1297 --repo storytold/pdfcraft --json comments --jq '.comments[-5:][] | {author: .author.login, createdAt, url, body}'
```

Expected: utiliser toute réponse nouvelle pour orienter l’interprétation, sans relancer de sondage répété. Si des fichiers sont fournis, exécuter uniquement les instructions de reproduction associées à ces fichiers, localement, enregistrer leurs empreintes SHA-256 avec `sha256sum`, et ne pas les commiter. Sans réponse, poursuivre avec les seuls résultats synthétiques et le dire explicitement.

- [ ] **Step 2: Exécuter toute la validation indépendante**

Run: `cargo test -p pdfcraft-prc independent`

Expected: tous les tests du module `independent` passent. Si un test échoue, passer immédiatement à la section « Portail de défaut » sans modifier la production.

- [ ] **Step 3: Portail de défaut — uniquement si un test indépendant échoue**

1. Conserver le test en échec et son commit ; ne pas changer son oracle pour obtenir un passage artificiel.
2. Vérifier manuellement le tableau encodé et les positions attendues sur le plus petit cas, en comparant à N570 §7.8.9.1-7.8.9.2 et §12.3.
3. Réduire la recette au plus petit nombre de triangles qui conserve l’échec et démontrer que l’encodeur produit encore les entiers calculés manuellement.
4. Exécuter `cargo test -p pdfcraft-prc independent -- --nocapture` et conserver la sortie complète comme preuve.
5. Invoquer `superpowers:systematic-debugging`. Ajouter au plan correctif le cas minimal, la cause racine confirmée, le fichier responsable, le test RED et la modification minimale avant toute édition de production. Ne pas éditer `tess.rs` ni la logique de `compressed.rs` dans la présente tâche.

Expected: un défaut n’est déclaré qu’après exclusion d’une erreur d’encodeur ou d’oracle. La correction devient alors une série de micro-tâches TDD distincte, toujours dans le périmètre de la spécification.

- [ ] **Step 4: Exécuter les portes finales**

Run:

```bash
cargo fmt --all -- --check && \
cargo clippy -p pdfcraft-prc --all-targets -- -D warnings && \
cargo test -p pdfcraft-prc && \
cargo xtask layers && \
cargo xtask assets && \
cargo xtask wasm && \
git diff --check
```

Expected: succès complet. Si la production n’a pas changé, `git diff 490287fc..HEAD -- crates/prc/src/tess.rs` reste vide et le seul changement dans `compressed.rs` est la déclaration du module de test.

- [ ] **Step 5: Vérifier la branche et le contenu livré**

Run:

```bash
git status --short --branch
git log --oneline 490287fc..HEAD
git diff --stat 490287fc..HEAD
```

Expected: aucun fichier non suivi, seulement les commits de validation et de documentation, aucun fichier externe ou binaire ajouté. Si le portail de défaut a été déclenché, la branche s’arrête avant tout correctif de production non planifié.

- [ ] **Step 6: Publier le résultat exact sur le ticket**

Si tous les tests passent sans fixture externe et sans défaut, publier ce compte rendu avec le commit réel interpolé par le shell :

```bash
commit=$(git rev-parse HEAD) && gh issue comment 1297 --repo storytold/pdfcraft --body "Independent synthetic validation at ${commit} is complete. The new encoder does not call the production traversal or geometry helpers; expectations are hand-computed literals or quantization bounds from source meshes, not encoder-generated positions alone. It covers a hand-computed seed/apex stream, disconnected components, vertex references, folds and component retry, a closed continuation edge, a non-planar strip, a near-degenerate frame, exact array consumption, budget limits, and face grouping. All independent tests pass, as do fmt, pdfcraft-prc Clippy with -D warnings, the full pdfcraft-prc test suite, layers, assets, and WASM. No production code was changed. Scope: recalculated-normal compressed meshes in the currently accepted 3T container form only; no SolidWorks files, stored normals, or one-status meshes were tested."
```

Si une réponse de Ken a fourni des fichiers ou si un défaut a été trouvé, ne pas utiliser ce texte : publier à la place les commandes réellement exécutées, les empreintes des fixtures, le résultat précis et les limites correspondantes. Ne jamais déclarer « all passed » pour une commande non exécutée ou en échec.
