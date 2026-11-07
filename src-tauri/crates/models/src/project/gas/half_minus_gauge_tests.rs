//! [#576 RV8c-3] Absolute pins for the Half-Minus back-gas gauge rule above the Z == 1 knee.
//!
//! Under Half Minus the back gas is turned at a quarter (OneStage) or a sixth (TwoStage) of its
//! GAUGE (#576 A18(e)). Above ~96 bar a share of the gauge holds less gas than the same share of
//! the gas, so the rule is only visible there - and there it was pinned by the agreement test
//! alone (budget, turn gauge and usable column describe the same gas), which a change moving all
//! three together passes. These pin the numbers themselves: the independent review's hand-computed
//! cases C6 and C7 (RV8c.json), to the hundredth of a litre and bar, and the budget again from the
//! documented formula on the Z-derated gas curve.

#![expect(
    clippy::unwrap_used,
    reason = "tests construct from known-valid literals; a failed unwrap should fail the test"
)]

use super::model::{CylRole, Cylinder, Diver, GasReachField, TankMaterial};
use super::physics::deliverable_gas;
use crate::config::{GasConfig, ReservePreset};
use crate::{Project, SectionData, ShotData, StationRef};

fn cyl(volume: f64, rated: f64, fill: f64, role: CylRole) -> Cylinder {
    Cylinder::new(volume, rated, fill, TankMaterial::Steel, role, None).unwrap()
}

/// The gas between `from` and `to` bar in a `volume` L tank, on the real (uncapped) Z curve.
fn window(volume: f64, from: f64, to: f64) -> f64 {
    deliverable_gas(from, volume, f64::INFINITY, false)
        - deliverable_gas(to, volume, f64::INFINITY, false)
}

fn plan(rig: &[Cylinder], preset: ReservePreset) -> GasReachField {
    let mut project = Project::new("Gas".to_string(), None);
    let sec = project
        .add_section(SectionData {
            shots: (0..3)
                .map(|_| ShotData {
                    length: 50.0,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })
        .id;
    let origin = StationRef::new(sec, 0);
    let cfg = GasConfig {
        default_reserve: preset,
        ..GasConfig::default()
    };
    assert!(
        cfg.conservative_tank_gas,
        "the review's cases are under the cap"
    );
    assert!((cfg.reserve_profile.stage_reserve_bar - 30.0).abs() < 1e-12);
    project.compute_gas_reach_field(
        Some(&origin),
        None,
        &Diver::new(20.0, None).unwrap(),
        rig,
        &cfg,
    )
}

fn close(what: &str, got: f64, want: f64, tol: f64) {
    assert!((got - want).abs() <= tol, "{what}: {got} against {want}");
}

#[test]
fn one_stage_back_gas_turns_at_a_quarter_of_its_gauge_above_the_knee() {
    // RV8c C6: 2 x 12 L at 232/232 back gas, an 11 L stage rated 207 filled to 230, OneStage,
    // 30-bar stage reserve. Back gas: 232 - 232/4 = 174 bar. Stage (capped basis 207): spends
    // (207 - 30)/2 = 88.5 bar off its real gauge, dropped at 230 - 88.5 = 141.5 bar.
    let f = plan(
        &[
            cyl(12.0, 232.0, 232.0, CylRole::BackGas),
            cyl(12.0, 232.0, 232.0, CylRole::BackGas),
            cyl(11.0, 207.0, 230.0, CylRole::Stage),
        ],
        ReservePreset::OneStage,
    );
    close("turn gauge", f.backgas_turn_bar.unwrap(), 174.0, 1e-9);
    assert_eq!(f.cylinder_usable_bars.len(), 3);
    for (got, want) in f.cylinder_usable_bars.iter().zip([58.0, 58.0, 88.5]) {
        close("usable column", *got, want, 1e-9);
    }
    assert_eq!(f.stage_drop_bars.len(), 1);
    close("stage drop", f.stage_drop_bars[0], 141.5, 1e-9);
    let formula = 2.0 * window(12.0, 232.0, 174.0) + window(11.0, 230.0, 141.5);
    close("budget against the formula", f.budget_litres, formula, 1e-9);
    close(
        "budget against the hand value",
        f.budget_litres,
        2025.34,
        0.005,
    );
}

#[test]
fn two_stage_back_gas_turns_at_a_sixth_of_its_gauge_above_the_knee() {
    // RV8c C7: 12 L at 207/207 back gas, an 11 L stage at 207/207 and a 7 L stage rated 207
    // filled to 180, TwoStage, 30-bar stage reserve. Back gas: 207 - 207/6 = 172.5 bar. Stages:
    // (207 - 30)/2 = 88.5 bar (dropped at 118.5) and (180 - 30)/2 = 75 bar (dropped at 105).
    let f = plan(
        &[
            cyl(12.0, 207.0, 207.0, CylRole::BackGas),
            cyl(11.0, 207.0, 207.0, CylRole::Stage),
            cyl(7.0, 207.0, 180.0, CylRole::Stage),
        ],
        ReservePreset::TwoStage,
    );
    close("turn gauge", f.backgas_turn_bar.unwrap(), 172.5, 1e-9);
    assert_eq!(f.cylinder_usable_bars.len(), 3);
    for (got, want) in f.cylinder_usable_bars.iter().zip([34.5, 88.5, 75.0]) {
        close("usable column", *got, want, 1e-9);
    }
    assert_eq!(f.stage_drop_bars.len(), 2);
    close("stage 1 drop", f.stage_drop_bars[0], 118.5, 1e-9);
    close("stage 2 drop", f.stage_drop_bars[1], 105.0, 1e-9);
    let formula =
        window(12.0, 207.0, 172.5) + window(11.0, 207.0, 118.5) + window(7.0, 180.0, 105.0);
    close("budget against the formula", f.budget_litres, formula, 1e-9);
    close(
        "budget against the hand value",
        f.budget_litres,
        1731.85,
        0.005,
    );
}
