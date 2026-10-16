use math_iw4::{angle_normalize_360, angle_subtract, yaw_vectors_2d};
use movement_iw4::ANGLE2SHORT;
use playerstate_iw4::{UserCmd, buttons};

use crate::intent::{BotIntent, MotorReport, MoveMode, PathOutcome};
use crate::observation::BotObservation;

const MAX_YAW_DEG_PER_MS: f32 = 0.36;
const MAX_PITCH_DEG_PER_MS: f32 = 0.24;
const MAX_YAW_ACCEL_DEG_PER_MS2: f32 = 0.015;
const MAX_PITCH_ACCEL_DEG_PER_MS2: f32 = 0.01;
// Unhurried look-around, as a player checks angles while walking.
const CALM_YAW_DEG_PER_MS: f32 = 0.14;
const CALM_YAW_ACCEL_DEG_PER_MS2: f32 = 0.0012;
const FIRE_CONE_DEG: f32 = 8.0;
const PRECISE_CONE_DEG: f32 = 1.0;
const QUIET_SCALE: f32 = 0.55;
// Aim error on picking up a target (degrees, before the bot's own scale and
// a random 0.5-1.5 factor), its settling time constant, and the wobble left.
const AIM_ACQUIRE_DEG: f32 = 4.725;
const AIM_SETTLE_MS: f32 = 510.0;
const AIM_DRIFT_DEG: f32 = 1.0;
// Crosshair placement: a target that turns up near where the bot was
// already aiming needs only a small correction. The miss on picking it up
// scales with the turn, full beyond PREAIM_FULL_DEG, down to PREAIM_MIN_SHARE.
const PREAIM_FULL_DEG: f32 = 20.0;
const PREAIM_MIN_SHARE: f32 = 0.3;
// Sights knocked off target per shot fired (degrees, before the bot's scale).
const SHOT_KICK_DEG: f32 = 1.2;
// Shots on one target after which kicks are halved and the error cap closes;
// the cap on aim error (degrees, before the bot's own scale) and its floor.
const DIAL_IN_SHOTS: f32 = 6.0;
const AIM_CAP_DEG: f32 = 3.15;
const AIM_CAP_FLOOR_DEG: f32 = 0.6;
// Past this long since the first shot at a target the bot locks on harder:
// kick, error cap and wander shrink by up to LATE_DIAL_MAX times, reached
// LATE_DIAL_RAMP_MS later. The cap floor goes down with it.
const LATE_DIAL_AFTER_MS: i32 = 1500;
const LATE_DIAL_RAMP_MS: f32 = 1000.0;
const LATE_DIAL_MAX: f32 = 2.5;
const LATE_CAP_FLOOR_DEG: f32 = 0.35;
// This long without a shot at the target and the dial-in partly fades: the
// next time the bot opens fire (the enemy back from cover) its sights land
// off again, by half a fresh acquisition, and it keeps DIAL_KEEP of the
// shots and lock-on time it had built up.
const DIAL_RESET_MS: i32 = 2000;
const DIAL_KEEP: f32 = 0.5;
// Casual bots' hands: this much shakier than the same bot's in Competitive
// (bigger misses on picking up a target, slower to settle, more wander).
const CASUAL_AIM_SCALE: f32 = 1.125;
const ARRIVE_IN: f32 = 24.0;
const SPRINT_RETRY_TICKS: u8 = 6;
const PITCH_LIMIT: f32 = 70.0;

#[derive(Clone, Debug)]
pub struct Motor {
    yaw: f32,
    pitch: f32,
    yaw_vel: f32,
    pitch_vel: f32,
    /// How shaky this bot's hands are: scales the miss on picking up a
    /// target, how long it takes to settle, and the wobble left after.
    skill: f32,
    /// The current aim error in degrees (yaw, pitch); set on picking up a new
    /// target and shrinking as the bot settles onto it.
    aim_off: [f32; 2],
    rng: u64,
    /// The held weapon and its magazine last command: a drop is a shot.
    last_clip: (u16, i32),
    /// Shots fired at the current target: the longer a bot stays on one
    /// target, the tighter its aim gets.
    shots_on_target: u32,
    /// When the first shot at the current target went off.
    first_shot_ms: Option<i32>,
    /// When the last shot went off, and whether the next burst starts cold.
    last_shot_ms: Option<i32>,
    reacquire: bool,
    /// Lock-on time kept through a pause, credited when firing resumes.
    kept_lock_ms: i32,
    was_firing: bool,
    last_look: Option<[f32; 3]>,
    armed: Option<sim::LifeSequence>,
    trigger_down: bool,
    sprint_held: u8,
}

impl Motor {
    pub fn new(seed: u64) -> Self {
        let self_preserve = (((seed >> 8) & 0xff) as f32) / 255.0;
        let skill = 0.75 + 0.75 * self_preserve;
        Self {
            yaw: 0.0,
            pitch: 0.0,
            yaw_vel: 0.0,
            pitch_vel: 0.0,
            skill,
            aim_off: [0.0; 2],
            rng: seed | 1,
            last_clip: (0, 0),
            shots_on_target: 0,
            first_shot_ms: None,
            last_shot_ms: None,
            reacquire: false,
            kept_lock_ms: 0,
            was_firing: false,
            last_look: None,
            armed: None,
            trigger_down: false,
            sprint_held: 0,
        }
    }

    pub fn report(&self, obs: &BotObservation, intent: &BotIntent, cmd: &UserCmd) -> MotorReport {
        if matches!(intent.move_mode, MoveMode::Mantle | MoveMode::Ladder) {
            return MotorReport::Unsupported;
        }
        if intent.path == PathOutcome::BudgetExhausted || intent.path == PathOutcome::ProgressLost {
            return MotorReport::Executing;
        }
        if intent.path == PathOutcome::Blocked || intent.path == PathOutcome::Unreachable {
            return MotorReport::Blocked;
        }
        if intent.move_mode == MoveMode::Drop {
            let Some(goal) = intent.move_goal else {
                return MotorReport::Executing;
            };
            if obs.self_state.origin[2] - goal[2] > crate::nav::STEP_Z_IN {
                return MotorReport::Executing;
            }
            if dist2(obs.self_state.origin, goal) <= ARRIVE_IN * ARRIVE_IN {
                return MotorReport::Arrived;
            }
            return MotorReport::Executing;
        }
        if intent.move_mode == MoveMode::Hold || (cmd.forwardmove == 0 && cmd.rightmove == 0) {
            if let Some(goal) = intent.move_goal
                && dist2(obs.self_state.origin, goal) <= ARRIVE_IN * ARRIVE_IN
            {
                return MotorReport::Arrived;
            }
            return MotorReport::Executing;
        }
        MotorReport::Executing
    }

    /// How much harder the bot has locked on to its target, 1 at first: a
    /// fight that has gone on 1.5 s past the first shot tightens quickly.
    fn late_dial(&self, now_ms: i32) -> f32 {
        let Some(first) = self.first_shot_ms else {
            return 1.0;
        };
        let over = (now_ms - first - LATE_DIAL_AFTER_MS) as f32;
        1.0 + (LATE_DIAL_MAX - 1.0) * (over / LATE_DIAL_RAMP_MS).clamp(0.0, 1.0)
    }

    pub fn drive(&mut self, obs: &BotObservation, intent: &BotIntent, dt_ms: i32) -> UserCmd {
        if self.armed != Some(obs.self_state.life_sequence) {
            self.yaw = obs.self_state.viewangles[1];
            self.pitch = angle_subtract(obs.self_state.viewangles[0], 0.0);
            self.armed = Some(obs.self_state.life_sequence);
        }
        let dt = dt_ms.max(1) as f32;
        let skill = if obs.casual { self.skill * CASUAL_AIM_SCALE } else { self.skill };
        // Every shot knocks the sights off a little (mostly upward, as recoil
        // does) and the bot has to pull back on: a spray drifts, a careful
        // shooter stays closer, and a first hit is not the end of the fight.
        let clip = (obs.self_state.weapon, obs.self_state.ammo_clip);
        if let Some(last) = self
            .last_shot_ms
            .filter(|last| obs.time_ms - last > DIAL_RESET_MS)
        {
            let locked = self.first_shot_ms.map_or(0, |first| last - first);
            self.kept_lock_ms = (locked as f32 * DIAL_KEEP) as i32;
            self.shots_on_target = (self.shots_on_target as f32 * DIAL_KEEP) as u32;
            self.first_shot_ms = None;
            self.last_shot_ms = None;
            self.reacquire = true;
        }
        // Opening fire again after the pause: off target again, though not
        // as far as on a fresh one.
        if intent.fire && !self.was_firing && self.reacquire && !intent.precise {
            let size = AIM_ACQUIRE_DEG * skill * (0.5 + self.random()) * (1.0 - DIAL_KEEP);
            let dir = self.random() * core::f32::consts::TAU;
            self.aim_off = [size * dir.cos(), size * 0.6 * dir.sin()];
            self.reacquire = false;
        }
        self.was_firing = intent.fire;
        let late = self.late_dial(obs.time_ms);
        if clip.0 == self.last_clip.0 && clip.1 < self.last_clip.1 && !intent.precise {
            self.shots_on_target += 1;
            let kept = std::mem::take(&mut self.kept_lock_ms);
            self.first_shot_ms.get_or_insert(obs.time_ms - kept);
            self.last_shot_ms = Some(obs.time_ms);
            let dial = (1.0 + self.shots_on_target as f32 / DIAL_IN_SHOTS) * late;
            let size = SHOT_KICK_DEG * skill * (0.5 + self.random()) / dial;
            let side = (self.random() - 0.5) * 2.0;
            self.aim_off[0] += size * side;
            self.aim_off[1] -= size * (0.4 + 0.6 * self.random());
        }
        self.last_clip = clip;
        if let Some(at) = intent.look_at {
            // Swinging onto a new target lands off it in a random direction,
            // and the error closes as the bot settles: early shots can miss,
            // later ones land, but no bot misses the same way forever.
            if self
                .last_look
                .is_none_or(|old| dist2(old, at) > 48.0 * 48.0)
                && !intent.calm
                && !intent.precise
            {
                let desire = look_angles(obs.eye(), at);
                let turn = angle_subtract(desire[1], self.yaw)
                    .abs()
                    .hypot(angle_subtract(desire[0], self.pitch).abs());
                let placed = (turn / PREAIM_FULL_DEG).clamp(PREAIM_MIN_SHARE, 1.0);
                let size = AIM_ACQUIRE_DEG * skill * (0.5 + self.random()) * placed;
                let dir = self.random() * core::f32::consts::TAU;
                self.aim_off = [size * dir.cos(), size * 0.6 * dir.sin()];
                self.shots_on_target = 0;
                self.first_shot_ms = None;
                self.reacquire = false;
                self.kept_lock_ms = 0;
            }
            self.last_look = Some(at);
            let settle = (-dt / (AIM_SETTLE_MS * skill)).exp();
            self.aim_off = [self.aim_off[0] * settle, self.aim_off[1] * settle];
            // Never wildly off for long: the error is capped, and the cap
            // closes in as shots on the same target add up.
            let floor = AIM_CAP_FLOOR_DEG
                - (AIM_CAP_FLOOR_DEG - LATE_CAP_FLOOR_DEG) * (late - 1.0) / (LATE_DIAL_MAX - 1.0);
            let cap = (AIM_CAP_DEG * skill
                / (1.0 + self.shots_on_target as f32 / DIAL_IN_SHOTS)
                / late)
                .max(floor);
            let size = self.aim_off[0].hypot(self.aim_off[1]);
            if size > cap {
                let scale = cap / size;
                self.aim_off = [self.aim_off[0] * scale, self.aim_off[1] * scale];
            }
            let desire = look_angles(obs.eye(), at);
            let (rate, accel) = if intent.calm {
                (CALM_YAW_DEG_PER_MS, CALM_YAW_ACCEL_DEG_PER_MS2)
            } else {
                (MAX_YAW_DEG_PER_MS, MAX_YAW_ACCEL_DEG_PER_MS2)
            };
            // Stunned hands turn slowly and shake.
            let dazed = obs.stun.clamp(0.0, 1.0);
            let (rate, accel) = (rate * (1.0 - 0.7 * dazed), accel * (1.0 - 0.7 * dazed));
            let (err_yaw, err_pitch) = if intent.precise {
                self.aim_off = [0.0; 2];
                (0.0, 0.0)
            } else {
                // A hand is never quite still: a small wander stays after
                // settling, as a player's crosshair does.
                let t = obs.time_ms as f32 / 1000.0;
                let phase = skill * 7.0;
                let wobble = AIM_DRIFT_DEG * skill * (1.0 + 3.0 * obs.stun.clamp(0.0, 1.0))
                    / late.sqrt();
                (
                    self.aim_off[0] + wobble * (t * 1.7 + phase).sin(),
                    self.aim_off[1] + wobble * 0.6 * (t * 2.3 + phase).sin(),
                )
            };
            let (yaw, yaw_vel) = slew_yaw(
                self.yaw,
                desire[1] + err_yaw,
                self.yaw_vel,
                rate,
                accel,
                dt,
            );
            self.yaw = yaw;
            self.yaw_vel = yaw_vel;
            let (pitch, pitch_vel) =
                slew_pitch(self.pitch, desire[0] + err_pitch, self.pitch_vel, dt);
            self.pitch = pitch;
            self.pitch_vel = pitch_vel;
        } else {
            self.yaw_vel = 0.0;
            self.pitch_vel = 0.0;
            self.last_look = None;
        }
        let unsupported = matches!(intent.move_mode, MoveMode::Mantle | MoveMode::Ladder);
        // A walk intent under a starved plan or a denied check keeps the
        // committed step; only a hold stops the bot.
        let walking = matches!(intent.move_mode, MoveMode::Walk | MoveMode::Drop)
            && matches!(
                intent.path,
                PathOutcome::Clear | PathOutcome::ProgressLost | PathOutcome::BudgetExhausted
            );
        let (mut forwardmove, mut rightmove) = (0i8, 0i8);
        if !unsupported
            && walking
            && let Some(goal) = intent.move_goal
        {
            let wish = [
                goal[0] - obs.self_state.origin[0],
                goal[1] - obs.self_state.origin[1],
            ];
            (forwardmove, rightmove) = wish_to_move(self.yaw, wish);
        }
        // A requested weapon has to survive the whole change: the simulator
        // settles a dropping hand back to ready when the command asks for the
        // weapon it is already lowering.
        // A quiet walk keeps under the pace at which footsteps are heard.
        if intent.quiet {
            forwardmove = (f32::from(forwardmove) * QUIET_SCALE) as i8;
            rightmove = (f32::from(rightmove) * QUIET_SCALE) as i8;
        }
        let weapon = intent.weapon.unwrap_or(obs.self_state.weapon);
        let delta = obs.self_state.delta_angles;
        let mut cmd = UserCmd {
            server_time: obs.time_ms,
            angles: [
                ((self.pitch - delta[0]) * ANGLE2SHORT) as i32,
                ((self.yaw - delta[1]) * ANGLE2SHORT) as i32,
                0,
            ],
            forwardmove,
            rightmove,
            weapon,
            weapon_mapped: weapon,
            ..UserCmd::default()
        };
        if intent.fire && self.weapon_on_target(obs, intent) {
            // A semi-automatic or burst weapon only fires on a fresh press.
            let semi = obs.held().is_some_and(|slot| slot.facts.semi_auto);
            if !(semi && self.trigger_down) {
                cmd.buttons |= buttons::ATTACK;
            }
        }
        self.trigger_down = cmd.buttons & buttons::ATTACK != 0;
        if intent.use_button {
            cmd.buttons |= buttons::USE;
        }
        // Reload shares the key with use: never both at once.
        if intent.reload && !intent.use_button {
            cmd.buttons |= buttons::RELOAD;
        }
        cmd.buttons |= intent.throw_buttons;
        if intent.jump {
            cmd.buttons |= buttons::JUMP;
        }
        if intent.ads {
            cmd.buttons |= buttons::ADS;
        }
        if intent.sprint
            && walking
            && intent.move_mode == MoveMode::Walk
            && forwardmove > 110
            && rightmove.abs() < 45
            && !intent.fire
            && !intent.ads
            && !intent.use_button
            && !intent.reload
            && !intent.crouch
        {
            // Sprint starts only on a fresh press: a press that landed while
            // still crouched or turning is let go for a tick and tried again.
            let sprinting = obs.self_state.weapon_action == crate::observation::WeaponAction::Busy;
            if sprinting || self.sprint_held < SPRINT_RETRY_TICKS {
                cmd.buttons |= buttons::SPRINT;
                self.sprint_held = self.sprint_held.saturating_add(1);
            } else {
                self.sprint_held = 0;
            }
        } else {
            self.sprint_held = 0;
        }
        if intent.crouch {
            cmd.buttons |= buttons::CROUCH;
        }
        cmd
    }

    /// A uniform number in [0, 1).
    fn random(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }

    /// The view is within `cone` degrees of looking at `at`.
    pub fn aimed_at(&self, obs: &BotObservation, at: [f32; 3], cone: f32) -> bool {
        let desire = look_angles(obs.eye(), at);
        angle_subtract(desire[1], self.yaw).abs() <= cone && (desire[0] - self.pitch).abs() <= cone
    }

    fn weapon_on_target(&self, obs: &BotObservation, intent: &BotIntent) -> bool {
        let Some(at) = intent.look_at else {
            return false;
        };
        let desire = look_angles(obs.eye(), at);
        let cone = if intent.precise {
            PRECISE_CONE_DEG
        } else {
            FIRE_CONE_DEG
        };
        angle_subtract(desire[1], self.yaw).abs() <= cone && (desire[0] - self.pitch).abs() <= cone
    }
}

fn look_angles(from: [f32; 3], to: [f32; 3]) -> [f32; 3] {
    let mut angles = math_iw4::vect_to_angles([to[0] - from[0], to[1] - from[1], to[2] - from[2]]);
    // vect_to_angles wraps upward pitch into [0, 360); the motor clamps
    // and compares pitch in the signed range.
    angles[0] = angle_subtract(angles[0], 0.0);
    angles
}

fn slew_yaw(current: f32, desire: f32, vel: f32, rate: f32, accel: f32, dt: f32) -> (f32, f32) {
    let err = angle_subtract(desire, current);
    let (delta, vel) = slew_vel(err, vel, rate, accel, dt);
    (angle_normalize_360(current + delta), vel)
}

fn slew_pitch(current: f32, desire: f32, vel: f32, dt: f32) -> (f32, f32) {
    let desire = desire.clamp(-PITCH_LIMIT, PITCH_LIMIT);
    let err = desire - current;
    let (delta, vel) = slew_vel(
        err,
        vel,
        MAX_PITCH_DEG_PER_MS,
        MAX_PITCH_ACCEL_DEG_PER_MS2,
        dt,
    );
    ((current + delta).clamp(-PITCH_LIMIT, PITCH_LIMIT), vel)
}

fn slew_vel(err: f32, vel: f32, max_rate: f32, max_accel: f32, dt: f32) -> (f32, f32) {
    let want = (err / dt).clamp(-max_rate, max_rate);
    let vel =
        (vel + (want - vel).clamp(-max_accel * dt, max_accel * dt)).clamp(-max_rate, max_rate);
    (vel * dt, vel)
}

fn wish_to_move(yaw_deg: f32, wish: [f32; 2]) -> (i8, i8) {
    let len = (wish[0] * wish[0] + wish[1] * wish[1]).sqrt();
    if len < 1.0 {
        return (0, 0);
    }
    let (forward, right) = yaw_vectors_2d(yaw_deg);
    let f = (wish[0] * forward[0] + wish[1] * forward[1]) / len;
    let r = (wish[0] * right[0] + wish[1] * right[1]) / len;
    // Full deflection on the larger axis, as movement keys give: the
    // simulator scales speed by the larger axis, so a unit diagonal would
    // only walk at about 70%.
    let peak = f.abs().max(r.abs()).max(1e-3);
    let (f, r) = (f / peak, r / peak);
    (
        (f * 127.0).round().clamp(-127.0, 127.0) as i8,
        (r * 127.0).round().clamp(-127.0, 127.0) as i8,
    )
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    dx * dx + dy * dy + dz * dz
}
