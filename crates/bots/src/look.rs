//! Where a player looks when nobody is in sight: not at the wall beside
//! them, but down the long lines and round the corners an enemy could come
//! from, moving on to whatever has gone longest unchecked.
//!
//! A bot keeps a coarse picture of how far it can see in each of a ring of
//! directions, refreshed a ray at a time, and picks its next look from it.

use crate::query::{QuerySubsystem, SightSample, WorldQuery};
use sim::ClientId;

const DIRS: usize = 16;
const RAY_IN: f32 = 2000.0;
const RAYS_PER_TICK: usize = 1;
/// Closer than this is a wall to stare at, not a view.
const WALL_IN: f32 = 170.0;
/// A sightline this much longer than its neighbour's opens past an edge: a
/// corner or doorway someone could step out of.
const CORNER_JUMP_IN: f32 = 300.0;
/// Seconds-worth of ticks after which an unchecked direction counts fully
/// stale.
const STALE_TICKS: f32 = 70.0;
/// Pull toward where enemies come from, and the cost of looking well away
/// from it (more than about 115 degrees off).
const THREAT_WEIGHT: f32 = 2.5;
const BEHIND_RAD: f32 = 2.0;
const BEHIND_COST: f32 = 2.5;
/// Score lost per radian of turning: small checks come before big swings.
const TURN_COST: f32 = 0.5;
/// About how far a calm look turns per tick.
const TURN_DEG_PER_TICK: f32 = 6.0;

#[derive(Clone, Debug, Default)]
pub struct LookScan {
    reach: [f32; DIRS],
    known: [bool; DIRS],
    checked: [u32; DIRS],
    cursor: usize,
    current: Option<(f32, u32)>,
}

/// What the bot wants out of its looking.
pub struct LookBias<'a> {
    /// A direction to favour (where it walks, or the angle it holds) and how
    /// strongly.
    pub forward: Option<(f32, f32)>,
    /// Where enemies come from.
    pub threat: Option<f32>,
    /// Keep the look within this many radians of `forward` (walking).
    pub cone: Option<f32>,
    /// Directions blocked by teammates standing close.
    pub mates: &'a [f32],
    /// How long to hold a look once there: minimum and spread, in ticks.
    pub dwell: (u32, u32),
    /// Where the view points now (radians): nearer looks come first.
    pub view: f32,
}

impl LookScan {
    /// Measure the next direction or two from the eye.
    pub fn refresh(&mut self, world: &mut impl WorldQuery, eye: [f32; 3], id: ClientId) {
        world.enter(QuerySubsystem::Tactical);
        // The direction being looked along is re-measured every tick, so a
        // wall walked up to is noticed at once; the rest go round in turn.
        let watching = self.current.map(|(yaw, _)| index_of(yaw));
        for step in 0..=RAYS_PER_TICK {
            let i = match (step, watching) {
                (0, Some(i)) => i,
                (0, None) => continue,
                _ => self.cursor,
            };
            let yaw = yaw_of(i);
            let end = [eye[0] + yaw.cos() * RAY_IN, eye[1] + yaw.sin() * RAY_IN, eye[2]];
            let Ok(sample) = world.sight_ray(eye, end, id) else {
                return;
            };
            self.reach[i] = match sample {
                SightSample::Clear => RAY_IN,
                // A player in the way is no wall; treat it as open.
                SightSample::HitPlayer { .. } | SightSample::Unknown => RAY_IN,
                SightSample::Blocked { fraction, .. } => fraction * RAY_IN,
            };
            self.known[i] = true;
            if step > 0 {
                self.cursor = (i + 1) % DIRS;
            }
        }
    }

    /// Whether looking along `yaw` (radians) shows more than a wall, if known.
    pub fn open_toward(&self, yaw: f32) -> Option<bool> {
        let i = index_of(yaw);
        self.known[i].then_some(self.reach[i] >= WALL_IN * 2.0)
    }

    /// Forget the current look (it is decided elsewhere for now).
    pub fn release(&mut self) {
        self.current = None;
    }

    /// The yaw (radians) to look along this tick.
    pub fn pick(&mut self, tick: u32, roll: u32, bias: &LookBias) -> Option<f32> {
        if let Some((yaw, until)) = self.current
            && tick < until
            && self.reach[index_of(yaw)] >= WALL_IN
            && bias.cone.is_none_or(|c| bias.forward.is_none_or(|(f, _)| angle(yaw, f) <= c))
        {
            return Some(yaw);
        }
        let mut best: Option<(f32, usize)> = None;
        for i in 0..DIRS {
            if !self.known[i] || self.reach[i] < WALL_IN {
                continue;
            }
            let yaw = yaw_of(i);
            if let (Some(cone), Some((f, _))) = (bias.cone, bias.forward)
                && angle(yaw, f) > cone
            {
                continue;
            }
            let d = self.reach[i];
            let mut score = (d / 200.0).ln().max(0.0);
            let prev = self.reach[(i + DIRS - 1) % DIRS];
            let next = self.reach[(i + 1) % DIRS];
            if d - prev.min(next) > CORNER_JUMP_IN {
                score += 1.5;
            }
            if let Some((f, weight)) = bias.forward {
                score += weight * angle(yaw, f).cos();
            }
            if let Some(t) = bias.threat {
                let off = angle(yaw, t);
                score += THREAT_WEIGHT * off.cos();
                // Well behind the way enemies come: nobody is there.
                if off > BEHIND_RAD {
                    score -= BEHIND_COST;
                }
            }
            score -= TURN_COST * angle(yaw, bias.view);
            if bias.mates.iter().any(|m| angle(yaw, *m) < 0.45) {
                score -= 1.5;
            }
            score += (tick.saturating_sub(self.checked[i]) as f32 / STALE_TICKS).min(2.0);
            // A little chance in it, different per direction.
            score += ((roll.wrapping_mul(2654435761).wrapping_add(i as u32 * 40503)) >> 16) as f32
                % 100.0
                / 100.0
                * 1.2;
            if best.is_none_or(|(s, _)| score > s) {
                best = Some((score, i));
            }
        }
        let (_, i) = best?;
        for j in [i + DIRS - 1, i, i + 1] {
            self.checked[j % DIRS] = tick;
        }
        let yaw = yaw_of(i);
        // Held once the view gets there: turning takes a moment.
        let turn = (angle(yaw, bias.view).to_degrees() / TURN_DEG_PER_TICK) as u32;
        let dwell = turn + bias.dwell.0 + roll % bias.dwell.1.max(1);
        self.current = Some((yaw, tick + dwell));
        Some(yaw)
    }
}

fn index_of(yaw: f32) -> usize {
    (yaw.rem_euclid(core::f32::consts::TAU) / (core::f32::consts::TAU / DIRS as f32)).round() as usize % DIRS
}

fn yaw_of(i: usize) -> f32 {
    i as f32 * core::f32::consts::TAU / DIRS as f32
}

/// Absolute difference between two yaws, 0..PI.
pub fn angle(a: f32, b: f32) -> f32 {
    ((a - b + core::f32::consts::PI).rem_euclid(core::f32::consts::TAU) - core::f32::consts::PI).abs()
}
