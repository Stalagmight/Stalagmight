//! The grounded-Laplacian solve (`lsq::Grounded`) against a dense solve of
//! the same system, on every path: exact elimination (spurs, chains and
//! nodes up to the degree cap), the conjugate gradient on a core of nodes
//! over the cap, an axis nothing holds, and the factoring reused for a
//! second right-hand side. Synthetic systems only.

use super::lsq::Grounded;

/// A deterministic xorshift stream in `[0, 1)`.
struct Stream(u64);

impl Stream {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// One axis of a system: ground weights, edges `(i, j, w)`, right-hand side.
struct Axis {
    ground: Vec<f64>,
    edges: Vec<(usize, usize, f64)>,
    rhs: Vec<f64>,
}

/// The dense solve: Gaussian elimination with partial pivoting of
/// `L + diag(ground)`.
fn dense(axis: &Axis) -> Vec<f64> {
    let n = axis.rhs.len();
    let mut a = vec![vec![0.0; n + 1]; n];
    for (i, row) in a.iter_mut().enumerate() {
        row[i] = axis.ground[i];
        row[n] = axis.rhs[i];
    }
    for &(i, j, w) in &axis.edges {
        a[i][i] += w;
        a[j][j] += w;
        a[i][j] -= w;
        a[j][i] -= w;
    }
    for col in 0..n {
        let pivot = (col..n)
            .max_by(|&p, &q| a[p][col].abs().total_cmp(&a[q][col].abs()))
            .unwrap_or(col);
        a.swap(col, pivot);
        let pivot_row = a[col].clone();
        for row in a.iter_mut().skip(col + 1) {
            let f = row[col] / pivot_row[col];
            for (target, &p) in row.iter_mut().zip(&pivot_row).skip(col) {
                *target -= f * p;
            }
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let s: f64 = (row + 1..n).map(|k| a[row][k] * x[k]).sum();
        x[row] = (a[row][n] - s) / a[row][row];
    }
    x
}

/// Factor and solve both axes; `adj` built from each axis's edges (the two
/// share the sparsity, a missing edge is a zero weight).
fn grounded(x: &Axis, y: &Axis) -> (Vec<f64>, Vec<f64>) {
    let n = x.rhs.len();
    let mut adj: Vec<Vec<(usize, f64, f64)>> = vec![Vec::new(); n];
    for (k, (&(i, j, wx), &(_, _, wy))) in x.edges.iter().zip(&y.edges).enumerate() {
        assert_eq!((i, j), (y.edges[k].0, y.edges[k].1));
        adj[i].push((j, wx, wy));
        adj[j].push((i, wx, wy));
    }
    Grounded::factor(&x.ground, &y.ground, &adj).solve(&x.rhs, &y.rhs)
}

/// A random system on `n` nodes: a spanning tree, then `extra` random
/// edges, node 0 grounded, weights in `[0.5, 2]`, currents in `[-5, 5]`.
fn random_system(stream: &mut Stream, n: usize, extra: usize) -> (Axis, Axis) {
    let mut edges = Vec::new();
    for v in 1..n {
        edges.push(((stream.next() * v as f64) as usize, v));
    }
    for _ in 0..extra {
        let (i, j) = (
            (stream.next() * n as f64) as usize,
            (stream.next() * n as f64) as usize,
        );
        if i != j {
            edges.push((i, j));
        }
    }
    let axis = |stream: &mut Stream| {
        let mut ground = vec![0.0; n];
        ground[0] = 0.5 + 1.5 * stream.next();
        Axis {
            ground,
            edges: edges
                .iter()
                .map(|&(i, j)| (i, j, 0.5 + 1.5 * stream.next()))
                .collect(),
            rhs: (0..n).map(|_| 10.0 * stream.next() - 5.0).collect(),
        }
    };
    let x = axis(stream);
    let y = axis(stream);
    (x, y)
}

fn assert_close(found: &[f64], expected: &[f64], what: &str) {
    for (k, (f, e)) in found.iter().zip(expected).enumerate() {
        assert!(
            (f - e).abs() <= 1e-9 * (1.0 + e.abs()),
            "{what}: node {k} is {f}, the dense solve says {e}"
        );
    }
}

#[test]
fn trees_loops_and_dense_cores_match_a_dense_solve() {
    let mut stream = Stream(0x1234_5678_9ABC_DEF1);
    // Trees and chains (all eliminated), sparse loops (fill-in under the
    // cap), and dense webs whose nodes stay over the cap (the core's
    // conjugate gradient).
    for (n, extra) in [
        (1, 0),
        (2, 0),
        (12, 0),
        (30, 3),
        (40, 12),
        (24, 120),
        (16, 200),
    ] {
        for round in 0..6 {
            let (x, y) = random_system(&mut stream, n, extra);
            let (dx, dy) = grounded(&x, &y);
            assert_close(
                &dx,
                &dense(&x),
                &format!("n {n} extra {extra} round {round} x"),
            );
            assert_close(
                &dy,
                &dense(&y),
                &format!("n {n} extra {extra} round {round} y"),
            );
        }
    }
}

#[test]
fn a_complete_core_over_the_degree_cap_is_solved() {
    // K12: every node has degree 11, over the cap of 8, so nothing is
    // eliminated and the whole system goes to the conjugate gradient.
    let n = 12;
    let mut stream = Stream(0xC0FF_EE00_1234_5678);
    let mut edges = Vec::new();
    for i in 0..n {
        for j in i + 1..n {
            edges.push((i, j));
        }
    }
    let axis = |stream: &mut Stream| {
        let mut ground = vec![0.0; n];
        ground[3] = 1.0;
        Axis {
            ground,
            edges: edges
                .iter()
                .map(|&(i, j)| (i, j, 0.5 + stream.next()))
                .collect(),
            rhs: (0..n).map(|_| stream.next() - 0.5).collect(),
        }
    };
    let (x, y) = (axis(&mut stream), axis(&mut stream));
    let (dx, dy) = grounded(&x, &y);
    assert_close(&dx, &dense(&x), "x");
    assert_close(&dy, &dense(&y), "y");
}

#[test]
fn an_axis_nothing_holds_keeps_its_reference() {
    // A chain 0 - 1 - 2 grounded at 0 on y only: on x nothing holds it, so
    // its corrections stay zero there (no NaN), and y is solved.
    let x = Axis {
        ground: vec![0.0; 3],
        edges: vec![(0, 1, 0.0), (1, 2, 0.0)],
        rhs: vec![0.0, 0.0, 0.0],
    };
    let y = Axis {
        ground: vec![2.0, 0.0, 0.0],
        edges: vec![(0, 1, 1.0), (1, 2, 1.0)],
        rhs: vec![0.0, 0.0, 1.0],
    };
    let (dx, dy) = grounded(&x, &y);
    assert!(dx.iter().all(|&d| d == 0.0), "{dx:?}");
    assert_close(&dy, &dense(&y), "y");
}

#[test]
fn one_factoring_solves_any_right_hand_side() {
    let mut stream = Stream(0x0DDB_A11C_AFE0_0001);
    let (mut x, y) = random_system(&mut stream, 30, 20);
    let adj = {
        let mut adj: Vec<Vec<(usize, f64, f64)>> = vec![Vec::new(); 30];
        for (&(i, j, wx), &(_, _, wy)) in x.edges.iter().zip(&y.edges) {
            adj[i].push((j, wx, wy));
            adj[j].push((i, wx, wy));
        }
        adj
    };
    let system = Grounded::factor(&x.ground, &y.ground, &adj);
    for round in 0..3 {
        x.rhs = (0..30).map(|_| stream.next() - 0.5).collect();
        let (dx, _) = system.solve(&x.rhs, &y.rhs);
        assert_close(&dx, &dense(&x), &format!("round {round}"));
    }
    // Zero currents, zero corrections, to the bit.
    let (dx, dy) = system.solve(&[0.0; 30], &[0.0; 30]);
    assert!(
        dx.iter().chain(&dy).all(|&d| d.to_bits() == 0),
        "{dx:?} {dy:?}"
    );
}
