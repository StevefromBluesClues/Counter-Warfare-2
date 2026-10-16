//! Grenade throws: picking a throw a player would make, solving the arc, and
//! the aim / press / release sequence that puts it in the air.

use playerstate_iw4::buttons;

use crate::observation::{BotObservation, Grenade};
use crate::query::{QuerySubsystem, SightSample, WorldQuery};

/// Launch speed and the extra lift a hand throw adds, matching thrown grenades
/// measured from recorded matches, and world gravity.
const THROW_SPEED: f32 = 950.0;
const THROW_LIFT: f32 = 120.0;
const GRAVITY: f32 = 800.0;
/// Grenades skip and roll on after they land; aim the first bounce short.
const ROLL_SHARE: f32 = 0.85;
pub const AIM_CONE_DEG: f32 = 2.5;
const AIM_PATIENCE: u32 = 30;
const PRESS_TICKS: u32 = 4;
const RELEASE_TICKS: u32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Aim,
    Press,
    Release,
}

#[derive(Clone, Copy, Debug)]
pub struct Throw {
    pub kind: Grenade,
    /// A point along the launch direction for the view to settle on.
    pub aim: [f32; 3],
    pub stage: Stage,
    pub since: u32,
}

impl Throw {
    pub fn button(&self) -> u32 {
        if self.kind.lethal() {
            buttons::FRAG
        } else {
            buttons::SMOKE
        }
    }

    /// Advances the sequence. `Some(true)` when the grenade has left the hand,
    /// `Some(false)` when the throw was given up, `None` while it goes on.
    pub fn step(&mut self, tick: u32, aimed: bool) -> Option<bool> {
        let elapsed = tick.saturating_sub(self.since);
        match self.stage {
            Stage::Aim if aimed => {
                self.stage = Stage::Press;
                self.since = tick;
            }
            Stage::Aim if elapsed > AIM_PATIENCE => return Some(false),
            Stage::Press if elapsed >= PRESS_TICKS => {
                self.stage = Stage::Release;
                self.since = tick;
            }
            Stage::Release if elapsed >= RELEASE_TICKS => return Some(true),
            _ => {}
        }
        None
    }

    pub fn holding_button(&self) -> bool {
        self.stage == Stage::Press
    }
}

/// The lowest upward launch pitch, in degrees, that carries a throw from the
/// eye to `target`. `None` when the target is out of reach.
pub fn solve_pitch(eye: [f32; 3], target: [f32; 3]) -> Option<f32> {
    let reach = (target[0] - eye[0]).hypot(target[1] - eye[1]) * ROLL_SHARE;
    let rise = target[2] - eye[2];
    (-10..=55).map(|deg| deg as f32).find(|deg| {
        let (s, c) = deg.to_radians().sin_cos();
        let (vh, vz) = (THROW_SPEED * c, THROW_SPEED * s + THROW_LIFT);
        // Later root of rise = vz t - g t^2 / 2: the descending pass.
        let disc = vz * vz - 2.0 * GRAVITY * rise;
        disc >= 0.0 && vh * (vz + disc.sqrt()) / GRAVITY >= reach
    })
}

/// Where to look to launch at `pitch` degrees up toward `target`, and the
/// top of the arc, for the clearance check.
pub fn aim_and_apex(eye: [f32; 3], target: [f32; 3], pitch: f32) -> ([f32; 3], [f32; 3]) {
    let yaw = (target[1] - eye[1]).atan2(target[0] - eye[0]);
    let (s, c) = pitch.to_radians().sin_cos();
    let aim = [
        eye[0] + yaw.cos() * c * 200.0,
        eye[1] + yaw.sin() * c * 200.0,
        eye[2] + s * 200.0,
    ];
    let vz = THROW_SPEED * s + THROW_LIFT;
    let vh = THROW_SPEED * c;
    let t_top = (vz / GRAVITY).max(0.0);
    let apex_h = vz * t_top - 0.5 * GRAVITY * t_top * t_top;
    let reach = (target[0] - eye[0]).hypot(target[1] - eye[1]);
    let run = (vh * t_top).min(reach);
    let apex = [
        eye[0] + yaw.cos() * run,
        eye[1] + yaw.sin() * run,
        eye[2] + apex_h,
    ];
    (aim, apex)
}

/// Both legs of the arc are open: up to its top, and down onto the target.
pub fn arc_clear(world: &mut impl WorldQuery, obs: &BotObservation, apex: [f32; 3], target: [f32; 3]) -> bool {
    world.enter(QuerySubsystem::Tactical);
    let id = obs.self_state.id;
    let open = |sample| matches!(sample, Ok(SightSample::Clear | SightSample::HitPlayer { .. }));
    let landing = [target[0], target[1], target[2] + 32.0];
    open(world.shot_ray(obs.eye(), apex, id)) && open(world.shot_ray(apex, landing, id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_close_throw_needs_little_lift() {
        let pitch = solve_pitch([0.0, 0.0, 60.0], [400.0, 0.0, 0.0]).expect("in reach");
        assert!(pitch < 10.0, "{pitch}");
    }

    #[test]
    fn far_throw_lofts_and_out_of_reach_is_refused() {
        let near = solve_pitch([0.0, 0.0, 60.0], [400.0, 0.0, 0.0]).unwrap();
        let far = solve_pitch([0.0, 0.0, 60.0], [1300.0, 0.0, 0.0]).unwrap();
        assert!(far > near);
        assert!(solve_pitch([0.0, 0.0, 60.0], [4000.0, 0.0, 0.0]).is_none());
    }

    #[test]
    fn throw_sequence_presses_then_releases() {
        let mut throw = Throw {
            kind: Grenade::Frag,
            aim: [0.0; 3],
            stage: Stage::Aim,
            since: 0,
        };
        assert_eq!(throw.step(1, true), None);
        assert!(throw.holding_button());
        assert_eq!(throw.step(5, true), None);
        assert!(!throw.holding_button());
        assert_eq!(throw.step(21, true), Some(true));
    }
}
