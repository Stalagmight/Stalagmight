//! The per-axis sigma floor of the Closures Computed View (plan Deviations
//! G9, C24 finding F2): a short shot near a grid axis had a huge, infinite
//! or zero least-squares weight on the axis across it. Synthetic data only.

use crate::config::ClosureAdjustmentConfig;
use crate::{GpsCoordinates, StationId};

use super::test_helpers::{add, at, flat, project};
use super::{ClosureStrategy, Project, ShotSigmas, StartAt, ZeroDatum};

fn sigmas(length: f64, azimuth_deg: f64) -> ShotSigmas {
    ShotSigmas::of(
        &ClosureAdjustmentConfig::default(),
        1.0,
        length,
        azimuth_deg.to_radians(),
    )
}

#[test]
fn the_floor_is_sigma_len_times_sigma_az() {
    let config = ClosureAdjustmentConfig::default();
    let floor = config.base_sigma_length_m * config.base_sigma_azimuth_deg.to_radians();
    // A zero-length connector at azimuth 0: no first-order weight on x.
    let s = sigmas(0.0, 0.0);
    assert_eq!(s.first_order[0], 0.0);
    assert_eq!(s.floor, floor);
    assert_eq!(s.axes, [floor, config.base_sigma_length_m]);
    assert!(s.floored());
    // Due south: sin(pi) is 1.2e-16, a weight of 1e34 before the floor.
    let s = sigmas(0.0, 180.0);
    assert!(s.first_order[0] > 0.0 && s.first_order[0] < 1e-15);
    assert_eq!(s.axes[0], floor);
    // A 1 mm shot due east: 1.7e-5 m across, a weight of 3e9.
    let s = sigmas(1e-3, 90.0);
    assert!((s.first_order[1] - 1e-3 * config.base_sigma_azimuth_deg.to_radians()).abs() < 1e-18);
    assert_eq!(s.axes[1], floor);
    assert!(s.floored());
    // Quality scales both sigmas, so the floor by its square; a quality
    // that is not a positive number counts as 1, as in 0.6.0.
    let q = ShotSigmas::of(&config, 2.0, 0.0, 0.0);
    assert_eq!(
        q.floor,
        (config.base_sigma_length_m * 2.0) * (config.base_sigma_azimuth_deg.to_radians() * 2.0)
    );
    for quality in [f64::NAN, 0.0, -1.0, f64::INFINITY] {
        assert_eq!(ShotSigmas::of(&config, quality, 0.0, 0.0), sigmas(0.0, 0.0));
    }
}

#[test]
fn a_shot_at_least_sigma_len_long_keeps_the_first_order_sigmas_bit_for_bit() {
    // sin^2 * sl^2 + len^2 * cos^2 * sa^2 >= sl^2 * sa^2 once len >= sl (and
    // sa < 1 rad): the floor never binds there, whatever the azimuth.
    let config = ClosureAdjustmentConfig::default();
    let mut state = 0x5EED_F00D_u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    let axis_azimuths = [0.0, 90.0, 180.0, 270.0, 360.0, 359.999_999_999_999_94];
    for k in 0..20_000 {
        let length = config.base_sigma_length_m * (1.0 + next() * 2_000.0);
        let azimuth = if k % 4 == 0 {
            axis_azimuths[k / 4 % axis_azimuths.len()]
        } else {
            next() * 360.0
        };
        let quality = [1.0, 0.5, 2.0][k % 3];
        let s = ShotSigmas::of(&config, quality, length * quality, azimuth.to_radians());
        assert!(!s.floored(), "{length} m at {azimuth} deg: {s:?}");
        assert_eq!(s.axes.map(f64::to_bits), s.first_order.map(f64::to_bits));
    }
    // The floor binds only under sigma_len, near an axis.
    assert!(!sigmas(0.04, 45.0).floored());
    assert!(sigmas(0.04, 0.5).floored());
    assert!(!sigmas(0.04, 3.0).floored());
}

/// A loop from a datum: line A east 10 m then north 10 m; line B north
/// 10 m, a zero-length connector at `connector_azimuth`, then east 10.2 m;
/// a closure ties the two far ends (a 0.2 m misclosure on x). Returns the
/// project and the connector's two stations.
fn loop_with_a_connector(connector_azimuth: f64) -> (Project, StationId, StationId) {
    let mut p = project();
    let a = add(
        &mut p,
        StartAt::New,
        vec![flat(90.0, 10.0), flat(0.0, 10.0)],
    );
    let origin = at(&p, a, 0);
    let b = add(
        &mut p,
        StartAt::Station(origin),
        vec![
            flat(0.0, 10.0),
            flat(connector_azimuth, 0.0),
            flat(90.0, 10.2),
        ],
    );
    p.push_datum(ZeroDatum::new(
        origin,
        GpsCoordinates {
            latitude: 20.0,
            longitude: -87.0,
            depth_reference: 0.0,
        },
    ));
    let (a_end, b_end) = (at(&p, a, 2), at(&p, b, 3));
    p.push_closure(a_end, None, b_end, None, ClosureStrategy::Batch);
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    let (before, after) = (at(&p, b, 1), at(&p, b, 2));
    (p, before, after)
}

#[test]
fn a_zero_length_connector_on_a_grid_axis_keeps_its_two_ends_together() {
    // Before the floor the connector had no weight on x, so the loop was
    // open there: the solve left the misclosure in the connector, which
    // opened by the whole 0.2 m. Its ends are one point in the survey; the
    // floor keeps them within a few micrometres and spreads the misclosure
    // over the shots that can take it.
    for azimuth in [0.0, 180.0, 360.0] {
        let (p, before, after) = loop_with_a_connector(azimuth);
        let adjusted = p.adjusted_positions(&ClosureAdjustmentConfig::default());
        assert!(adjusted.failures().is_empty());
        let (Some(u), Some(v)) = (adjusted.xy(before), adjusted.xy(after)) else {
            panic!("the connector's stations are positioned");
        };
        let opened = (u[0] - v[0]).hypot(u[1] - v[1]);
        assert!(
            opened < 1e-5,
            "azimuth {azimuth}: the connector opened by {opened} m"
        );
        let raw = p.raw_positions();
        let moved = raw
            .iter()
            .filter_map(|(s, r)| adjusted.xy(s).map(|q| (q[0] - r[0]).hypot(q[1] - r[1])))
            .fold(0.0_f64, f64::max);
        assert!(
            moved > 0.01 && moved <= 0.2 + 1e-9,
            "azimuth {azimuth}: {moved}"
        );
    }
}
