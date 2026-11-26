use super::lp::{Outcome, Row, Sense};
use super::model::{EvoOptions, Goal};
use super::nlp::{project, shape, violation};
use super::problem::Evaluator;
use super::{MAX_POPULATION, MIN_POPULATION, PRECISION};

// ---------------------------------------------------------------- evolutionary

/// A small deterministic generator (SplitMix64), so a seed reproduces a run on every platform.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x2545_F491_4F6C_DD1D)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    fn uniform(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn between(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.uniform()
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { usize::try_from(self.next() % n as u64).unwrap_or(0) }
    }
    /// Standard normal (Box–Muller).
    fn normal(&mut self) -> f64 {
        let u = self.uniform().max(f64::MIN_POSITIVE);
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * self.uniform()).cos()
    }
}

/// A candidate: its point, the objective to minimise and its constraint violation.
#[derive(Clone, Debug)]
struct Member {
    x: Vec<f64>,
    phi: f64,
    viol: f64,
}

impl Member {
    /// Feasible beats infeasible; then the lower objective, or the smaller violation.
    fn better(&self, o: &Member, tol: f64) -> bool {
        match (self.viol <= tol, o.viol <= tol) {
            (true, false) => true,
            (false, true) => false,
            (true, true) => self.phi < o.phi,
            (false, false) => self.viol < o.viol,
        }
    }
}

/// Why the Evolutionary search stopped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum EvoEnd {
    /// Almost all of the population is within Convergence of the best member.
    Converged,
    /// The best member didn't improve for the set number of generations.
    Stalled,
    /// The evaluation budget ran out.
    Budget,
}

/// The search box, integer flags and scoring shared by the genetic and pattern phases.
struct EvoSearch<'a> {
    ev: &'a mut Evaluator,
    rows: Vec<Row>,
    senses: Vec<Sense>,
    goal: Goal,
    lo: Vec<f64>,
    hi: Vec<f64>,
    integer: &'a [bool],
    tol: f64,
}

impl EvoSearch<'_> {
    /// Clamps into the box and rounds integer variables.
    fn repair(&self, x: &mut [f64]) {
        for (j, v) in x.iter_mut().enumerate() {
            let (l, h) = (self.lo.get(j).copied().unwrap_or(0.0), self.hi.get(j).copied().unwrap_or(0.0));
            *v = if v.is_finite() { v.clamp(l, h) } else { l };
            if self.integer.get(j).copied().unwrap_or(false) {
                *v = v.round().clamp(l.ceil(), h.floor());
            }
        }
    }

    /// Scores `x`; `None` once the budget is spent.
    fn score(&mut self, x: Vec<f64>) -> Option<Member> {
        let p = self.ev.eval(&x)?;
        let (phi, cons) = shape(&p, &self.rows, self.goal);
        let viol = violation(&cons, &self.senses);
        let phi = if phi.is_finite() { phi } else { f64::INFINITY };
        let viol = if viol.is_finite() && cons.iter().all(|c| c.is_finite()) { viol } else { f64::INFINITY };
        Some(Member { x, phi, viol })
    }
}

/// The Evolutionary method within `lo..hi` from `x0`.
pub(super) fn evolutionary(
    ev: &mut Evaluator,
    x0: &[f64],
    lo: &[f64],
    hi: &[f64],
    integer: &[bool],
    goal: Goal,
    o: &EvoOptions,
) -> (Outcome, EvoEnd) {
    let rows = ev.rows.clone();
    let senses: Vec<Sense> = rows.iter().map(|r| r.sense).collect();
    let scale = rows.iter().map(|r| r.rhs_const.abs()).fold(1.0, f64::max);
    let mut start = x0.to_vec();
    project(&mut start, lo, hi);
    // Without a bound, search within 100 of the start.
    let blo: Vec<f64> = lo.iter().zip(&start).map(|(l, s)| if l.is_finite() { *l } else { s - 100.0 }).collect();
    let bhi: Vec<f64> = hi.iter().zip(&start).map(|(h, s)| if h.is_finite() { *h } else { s + 100.0 }).collect();
    let no_integer = blo.iter().zip(&bhi).zip(integer).any(|((l, h), int)| *int && l.ceil() > h.floor());
    if blo.iter().zip(&bhi).any(|(l, h)| l > h) || no_integer {
        return (Outcome::Infeasible, EvoEnd::Converged);
    }
    let span: Vec<f64> = blo.iter().zip(&bhi).map(|(l, h)| (h - l).max(1e-9)).collect();
    let mut se = EvoSearch { ev, rows, senses, goal, lo: blo, hi: bhi, integer, tol: PRECISION * scale };
    let mut rng = Rng::new(o.seed);
    let size = o.population.clamp(MIN_POPULATION, MAX_POPULATION);
    let tol = se.tol;
    let mut end = EvoEnd::Budget;
    // The first generation: the start and random points in the box.
    let mut pop: Vec<Member> = Vec::with_capacity(size);
    se.repair(&mut start);
    let mut next_x = Some(start);
    while pop.len() < size {
        let x = next_x.take().unwrap_or_else(|| {
            let mut x: Vec<f64> = se.lo.iter().zip(&se.hi).map(|(l, h)| rng.between(*l, *h)).collect();
            se.repair(&mut x);
            x
        });
        match se.score(x) {
            Some(m) => pop.push(m),
            None => break,
        }
    }
    let mut best: Option<Member> = None;
    for m in &pop {
        if best.as_ref().is_none_or(|b| m.better(b, tol)) {
            best = Some(m.clone());
        }
    }
    let mut last_improved = 0u64;
    let mut generation = 0u64;
    'search: while pop.len() == size {
        let Some(b) = best.clone() else { break };
        // Converged: 99% of the population is feasible and within Convergence of the best.
        let close = pop.iter().filter(|m| m.viol <= tol && (m.phi - b.phi).abs() <= o.convergence * b.phi.abs().max(1.0)).count();
        if b.viol <= tol && generation >= 5 && close * 100 >= size * 99 {
            end = EvoEnd::Converged;
            break;
        }
        if generation.saturating_sub(last_improved) >= o.stall {
            end = EvoEnd::Stalled;
            break;
        }
        // The next generation: the best survives, the rest are children of two tournaments.
        let mut next = Vec::with_capacity(size);
        next.push(b.clone());
        while next.len() < size {
            let mut parent = || {
                let (i, k) = (rng.below(size), rng.below(size));
                match (pop.get(i), pop.get(k)) {
                    (Some(p), Some(q)) => Some(if q.better(p, tol) { q.x.clone() } else { p.x.clone() }),
                    _ => None,
                }
            };
            let (Some(pa), Some(pb)) = (parent(), parent()) else { break 'search };
            let mut x: Vec<f64> = pa
                .iter()
                .zip(&pb)
                .zip(&span)
                .map(|((a, c), w)| {
                    let mut v = a + rng.between(-0.25, 1.25) * (c - a);
                    if rng.uniform() < o.mutation {
                        v += 0.1 * w * rng.normal();
                    }
                    v
                })
                .collect();
            se.repair(&mut x);
            match se.score(x) {
                Some(m) => next.push(m),
                None => break 'search,
            }
        }
        generation += 1;
        for m in &next {
            let Some(cur) = &best else { continue };
            if m.better(cur, tol) {
                let improved = cur.viol > tol || cur.phi - m.phi > 1e-9 * cur.phi.abs().max(1.0);
                best = Some(m.clone());
                if improved {
                    last_improved = generation;
                }
            }
        }
        pop = next;
    }
    // Polish: a compass search on the continuous variables around the best member.
    if let Some(mut b) = best.clone() {
        let mut step: Vec<f64> = span.iter().map(|w| 0.01 * w).collect();
        let n = b.x.len();
        'polish: for _ in 0..10_000 {
            if step.iter().zip(&span).zip(integer).all(|((s, w), int)| *int || *s <= 1e-9 * w.max(1.0)) {
                break;
            }
            let mut moved = false;
            for j in 0..n {
                if integer.get(j).copied().unwrap_or(false) {
                    continue;
                }
                for dir in [1.0, -1.0] {
                    let mut x = b.x.clone();
                    if let (Some(v), Some(s)) = (x.get_mut(j), step.get(j)) {
                        *v += dir * s;
                    }
                    se.repair(&mut x);
                    if x == b.x {
                        continue;
                    }
                    let Some(m) = se.score(x) else { break 'polish };
                    if m.better(&b, tol) {
                        b = m;
                        moved = true;
                        break;
                    }
                }
            }
            if !moved {
                for s in &mut step {
                    *s *= 0.5;
                }
            }
        }
        best = Some(b);
    }
    match best {
        Some(b) if b.viol <= tol => (Outcome::Solved(b.x, b.phi, end != EvoEnd::Budget), end),
        _ if end == EvoEnd::Budget => (Outcome::Limit, end),
        _ => (Outcome::Infeasible, end),
    }
}
