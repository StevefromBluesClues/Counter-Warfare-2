use playerstate_iw4::UserCmd;
use sim::{ClientId, ClientLifecycle};

use crate::grenade;
use crate::intent::{BotIntent, MotorReport, MoveMode, PathOutcome};
use crate::memory::Memory;
use crate::motor::Motor;
use crate::nav::{self, NavGraph, PathError};
use crate::observation::{
    BotEvent, BotObservation, Contact, Grenade, KnowledgeSource, ModeObjective, ObjectiveAction,
    TeamRole, WeaponClass,
};
use crate::query::{QueryResult, QuerySubsystem, SightSample, WalkSample, WorldQuery};
use crate::task::{ActionStage, Decision, SwitchReason, Task, TaskKind};
use crate::weapon::WeaponSkill;

const THINK_PERIOD_TICKS: u32 = 2;
const SWITCH_MARGIN: f32 = 12.0;
const STEP_IN: f32 = 64.0;
const WAYPOINT_IN: f32 = 24.0;
const LAST_SEEN_FRESH_TICKS: u32 = 80;
/// How stale the positive observation behind a shot may be. Committing to a
/// target is a movement and aiming decision; firing needs the enemy to have
/// been seen, not merely remembered.
const FIRE_FRESH_TICKS: u32 = 4;
/// Free-space fallback ring, used only where there is no navigation graph.
const FIGHT_RING: u32 = 12;
/// How many baked supports one scan evaluates. Two queries each, so the whole
/// scan fits inside a few slices of the shared quota.
const FIGHT_CANDIDATES: usize = 6;
/// How far the threat may move before its firing position is re-derived. Far
/// wider than a step, so a moving target does not rebuild the goal every tick.
const FIGHT_COMMIT_MOVE: f32 = 128.0;
const FIGHT_SCAN_MAX_AGE_TICKS: u32 = 8;
/// A committed position is abandoned if the engagement has not resolved by
/// then, so a stale goal cannot outlive its fight.
const FIGHT_COMMIT_MAX_AGE_TICKS: u32 = 60;
/// Commanded movement without displacement for this long is a stuck bot.
const STUCK_TICKS: u32 = 40;
const ESCAPE_CROUCH_TICKS: u32 = 30;
const DETOUR_MIN_IN: f32 = 900.0;
const STALEMATE_TICKS: u32 = 60;
const STALEMATE_BREAK_TICKS: u32 = 80;
const DETOUR_STEP_IN: f32 = 320.0;
const DETOUR_SNAP_IN: f32 = 300.0;
const DETOUR_ROAM_SHARE: f32 = 0.6;
const DETOUR_ROAM_MAX_IN: f32 = 1600.0;
const DETOUR_REACHED_IN: f32 = 160.0;
// Share of rounds a bot comes into its goal from a side entry instead of a
// lane beside the straight route, and how far out that entry point sits.
const DETOUR_ENTRY_PCT: u32 = 40;
const DETOUR_ENTRY_MAX_IN: f32 = 1100.0;
const SPREAD_IN: f32 = 170.0;
// Room a bot likes between itself and a teammate while walking, and how far
// sideways it eases off over the next stretch of its route.
const SHOTGUN_FIRE_IN: f32 = 650.0;
// A new target only takes over from the one being fought when it is under
// 60% of the distance and at least 250 units nearer.
const TARGET_SWITCH_SHARE: f32 = 0.6;
const TARGET_SWITCH_IN: f32 = 250.0;
// The use key goes down once the sights are this close to the bomb, and an
// unanswered hold is pressed again after this many ticks.
const USE_AIM_CONE_DEG: f32 = 8.0;
const USE_REPRESS_TICKS: u32 = 20;
// The bomb carrier or defuser starts no throw this close to the bomb.
const NADE_ACTOR_CLEAR_IN: f32 = 700.0;
// An enemy the bot was fighting and lost sight of stays its business: it is
// hunted down ahead of the objective for this long, and for the first part
// the bot holds the angle it was last seen at, ready for the re-peek.
const FIGHT_MEMORY_TICKS: u32 = 120;
// Casual bots take this much longer to react to a new target.
const CASUAL_REACTION_TICKS: f32 = 2.0;
// An enemy appearing within this many degrees of where the bot is aiming is
// shot this much sooner: the holder's edge from good crosshair placement.
const PREAIM_REACTION_DEG: f32 = 12.0;
const PREAIM_REACTION_TICKS: f32 = 2.0;
// Before the plant a defender chases an enemy it lost or heard only this
// close; farther off it holds the angle instead.
const DEFENDER_CHASE_IN: f32 = 500.0;
const FIGHT_PREAIM_TICKS: u32 = 30;
const FIGHT_MEMORY_SCORE: f32 = 92.0;
// Enemies remembered this recently (and this close) are watched while
// walking or holding; older ones only tilt where the bot looks.
const MEMORY_WATCH_TICKS: u32 = 200;
const MEMORY_WATCH_IN: f32 = 2500.0;
// A carrier already planting keeps at it unless an enemy is this close.
const PLANT_FIGHT_IN: f32 = 600.0;
// Contacts below this confidence came from a teammate's callout.
const CALLOUT_CONFIDENCE_MAX: f32 = 0.45;
const CALLOUT_WATCH_IN: f32 = 2500.0;
// Cover fighting: search radius and leash, peek and hide rhythm (ticks),
// quick jiggle peeks, and magazine thresholds (percent of a full one).
const COVER_SEARCH_IN: f32 = 160.0;
const COVER_LEASH_IN: f32 = 260.0;
const COVER_SEARCH_TICKS: u32 = 20;
const COVER_CANDIDATES: usize = 8;
const COVER_PEEK_MIN_TICKS: u32 = 36;
const COVER_PEEK_SPREAD_TICKS: u32 = 28;
// Not every fight is taken from cover (percent of fights, by style), and an
// enemy this close is just fought.
const COVER_USE_PCT: u32 = 60;
const COVER_USE_RUSH_PCT: u32 = 30;
const COVER_CLOSE_IN: f32 = 450.0;
const COVER_HIDE_MIN_TICKS: u32 = 8;
const COVER_HIDE_SPREAD_TICKS: u32 = 10;
// Duck out of a peek only when it has gone badly; after a few rounds of
// peeking without a kill, stop trading from cover and take the fight on.
const COVER_HURT_HP: i32 = 35;
const COVER_HURT_LOSS: i32 = 50;
const COVER_MAX_PEEKS: u32 = 2;
const COVER_RETRY_TICKS: u32 = 200;
const COVER_JIGGLE_TICKS: u32 = 6;
const COVER_MAX_JIGGLES: u32 = 3;
const COVER_LOST_TICKS: u32 = 80;
const COVER_LOW_CLIP_PCT: i32 = 25;
const COVER_RELOAD_PCT: i32 = 70;
// Turning toward an unseen attacker, combat strafing and quiet walking.
const HIT_LOOK_TICKS: u32 = 30;
const STRAFE_MIN_TICKS: u32 = 6;
const STRAFE_SPREAD_TICKS: u32 = 10;
const STRAFE_STEP_IN: f32 = 64.0;
const QUIET_NEAR_IN: f32 = 1200.0;
const ROOM_IN: f32 = 150.0;
const ROOM_LOOKAHEAD_IN: f32 = 64.0;
const ROOM_SIDESTEP_IN: f32 = 56.0;
const HOLD_ROOM_IN: f32 = 90.0;
const HOLD_STEP_IN: f32 = 48.0;
const GUARD_SECTORS: usize = 16;
const GUARD_PER_THINK: usize = 1;
const COVER_RAYS: usize = 4;
const GUARD_PATIENCE: u32 = 60;
const COVER_REACH: f32 = 96.0;
const PACE_OPENING_MIN: u32 = 4;
const PACE_OPENING_SPREAD: u32 = 26;
// Measured from a recorded human match: the view leads the path only about
// 40% of the time, crouching 15-20%, a short sprint now and then, and holds
// that last a few seconds before shifting to the next angle.
// Looking around: how long a look is held walking and holding (ticks), how
// far either side of the path a walking bot looks, and how close a teammate
// has to be to block the view.
const LOOK_WALK_MIN_TICKS: u32 = 12;
const LOOK_WALK_SPREAD_TICKS: u32 = 22;
const LOOK_HOLD_MIN_TICKS: u32 = 20;
const LOOK_HOLD_SPREAD_TICKS: u32 = 40;
const LOOK_WALK_CONE: f32 = 2.4;
const LOOK_MATE_IN: f32 = 450.0;
const PACE_LOOK_IN: f32 = 600.0;
const PACE_CROUCH_WALK_TICKS: u32 = 70;
const PACE_CROUCH_WALK_PCT: u32 = 18;
const PACE_CROUCH_HOLD_TICKS: u32 = 80;
const PACE_CROUCH_HOLD_PCT: u32 = 30;
const PACE_SPRINT_TICKS: u32 = 60;
const PACE_SPRINT_PCT: u32 = 45;
const PACE_SPRINT_WINDOW: u32 = 300;
const GUARD_HOLD_MIN: u32 = 30;
const GUARD_HOLD_SPREAD: u32 = 50;
const GUARD_SHIFT_IN: f32 = 480.0;
// Grenades: considered a few times a second, thrown onto a site from this
// far out before going in, or at an enemy lost from sight this recently.
const NADE_CHECK_TICKS: u32 = 10;
const NADE_SITE_NEAR_IN: f32 = 600.0;
const NADE_SITE_FAR_IN: f32 = 1500.0;
const NADE_LOST_MIN_TICKS: u32 = 20;
const NADE_LOST_MAX_TICKS: u32 = 120;
const NADE_LOST_NEAR_IN: f32 = 350.0;
const NADE_LOST_FAR_IN: f32 = 1300.0;
// A sniper on the attack trails the bomb this far back and watches.
const OVERWATCH_TRAIL_IN: f32 = 800.0;
const ROUTE_DRIFT_SHARE: f32 = 0.3;
const ROUTE_DRIFT_MAX_IN: f32 = 600.0;
const ROUTE_STALE_IN: f32 = 800.0;
const ROUTE_START_PIN_IN: f32 = 128.0;
const GLASS_PATIENCE_TICKS: u32 = 40;
const FIGHT_GLASS_MAX_IN: f32 = 1200.0;
const GLASS_SHARE_IN: f32 = 400.0;
// An enemy seen this close, or heard or lost from sight this close, is dealt
// with before the bomb, unless the bomb cannot wait.
const THREAT_SEEN_IN: f32 = 2000.0;
const THREAT_HEARD_IN: f32 = 1500.0;
const THREAT_OBJECTIVE_CAP: f32 = 50.0;
const THREAT_ACTOR_SEEN_IN: f32 = 1200.0;
const URGENT_SLACK_MS: f32 = 5000.0;
// Pushing guns and bold players come at a lost enemy from the side.
const FLANK_MIN_IN: f32 = 400.0;
const FLANK_MAX_IN: f32 = 550.0;
const FLANK_ANGLE: f32 = 1.05;
const FLANK_REACHED_IN: f32 = 96.0;
// Aiming down the sights: past point-blank range, and ticks for the sights to
// come most of the way up before the first shot.
const ADS_MIN_IN: f32 = 250.0;
const ADS_READY_PISTOL: u32 = 5;
const ADS_READY_SMG: u32 = 5;
const ADS_READY_RIFLE: u32 = 8;
const ADS_READY_LMG: u32 = 10;
const ADS_READY_SNIPER: u32 = 12;
const BURST_NEAR_IN: f32 = 600.0;
// Semi-automatic trigger pulls by distance.
const SEMI_NEAR_IN: f32 = 500.0;
const SEMI_FAR_IN: f32 = 1200.0;
// Choices for the wait before the next pull, near / mid / far: shots land
// roughly 150-250 ms apart up close, 200-300 ms at mid range and 300-350 ms
// far away (pistols a little slower), never in a fixed rhythm.
// The wait counts from the tick a shot is seen, which is a tick after it left
// and a tick before the next press can land, so each entry is two short of
// the spacing it gives.
const SEMI_GUN_TICKS: [[u32; 4]; 3] = [[1, 2, 2, 2], [2, 2, 3, 3], [3, 4, 4, 5]];
const SEMI_PISTOL_TICKS: [[u32; 4]; 3] = [[2, 2, 2, 3], [2, 3, 3, 4], [4, 5, 5, 6]];
const BURST_FAR_IN: f32 = 1500.0;

#[derive(Clone, Debug)]
pub struct HostController {
    aggression: f32,
    self_preserve: f32,
    memory: Memory,
    task: Task,
    intent: BotIntent,
    motor: Motor,
    last_think_tick: Option<u32>,
    reaction_until: u32,
    last_target: Option<ClientId>,
    /// The client a live fire authorization rests on, and the tick of the
    /// positive observation that granted it.
    fire_target: Option<(ClientId, u32)>,
    committed: Vec<nav::RouteStep>,
    route_goal: Option<[f32; 3]>,
    /// The goal a search still in progress was asked for; kept while the live
    /// goal stays near it so the search is not restarted on every step.
    route_pending: Option<([f32; 3], [f32; 3])>,
    route_work: nav::RouteWork,
    fight_scan: Option<FightScan>,
    weapon: WeaponSkill,
    nav_key: Option<(u64, u64, u32, u32)>,
    roam: Option<[f32; 3]>,
    roam_gen: u32,
    decision: Decision,
    prev_seen: Vec<ClientId>,
    was_touching: bool,
    last_cmd: UserCmd,
    life_sequence: Option<sim::LifeSequence>,
    report: MotorReport,
    progress_origin: Option<[f32; 3]>,
    progress_tick: u32,
    escape: Option<([f32; 3], u32)>,
    /// The current escape is a crouched push on toward the blocked goal.
    escape_crouch: bool,
    escape_jump: bool,
    stuck_tries: u32,
    pace_seed: u32,
    pace_released: Option<u32>,
    guard: Option<GuardScan>,
    detour: Option<Detour>,
    objective_held: Option<[f32; 3]>,
    unfired_since: Option<u32>,
    stalemate_until: u32,
    spawn_at: Option<[f32; 3]>,
    throw: Option<grenade::Throw>,
    nades_used: u8,
    nade_check: u32,
    /// The pane being shot out and since when; a bot that has not cleared it
    /// in a couple of seconds stops shooting and pushes on.
    glass_attempt: Option<([f32; 3], u32)>,
    /// The side point picked to come at a lost enemy from, and whether it
    /// has been reached.
    flank: Option<(ClientId, [f32; 3], bool)>,
    /// Since when the sights have been held up.
    ads_since: Option<u32>,
    /// The tick of the last trigger press with a semi-automatic weapon.
    last_semi_shot: u32,
    /// Ticks to wait after it before the next press; drawn per shot.
    semi_gap: u32,
    /// The held weapon and its magazine last tick: a drop is a shot fired.
    semi_clip: (u16, i32),
    /// Hits counted so far, and where to look after an unseen one: yaw and
    /// until when.
    damage_seen: Option<u32>,
    hit_look: Option<(f32, u32)>,
    /// Combat strafe: direction (+1 / -1) and when to reconsider.
    strafe: (f32, u32),
    /// Fighting from cover: where to duck, where to peek from, and the
    /// rhythm between them.
    cover: Option<CoverFight>,
    cover_search_at: u32,
    /// Since when the use key has been held on the objective, the weapon in
    /// hand when it went down, and whether the interaction has begun.
    use_since: Option<(u32, u16, bool)>,
    /// The last tick the bot was fighting `last_target` with it in sight.
    fought_at: u32,
    /// Where to look when there is nothing to shoot.
    look: crate::look::LookScan,
}

#[derive(Clone, Copy, Debug)]
struct CoverFight {
    target: ClientId,
    cover: [f32; 3],
    peek: [f32; 3],
    peeking: bool,
    until: u32,
    /// Quick looks out at an enemy that went out of sight.
    jiggles: u32,
    peeks: u32,
    last_seen: u32,
    health_at_peek: i32,
    /// The enemy was hurt when last seen.
    target_hurt: bool,
}

/// How a bot plays its gun, as recorded players did: snipers hang back and
/// hold long angles, shotguns and SMGs push and keep moving, rifles and LMGs
/// hold the site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Style {
    Overwatch,
    Rush,
    Hold,
}

impl Style {
    fn of(obs: &BotObservation) -> Self {
        let primary = obs
            .inventory
            .iter()
            .find(|slot| slot.facts.selectable && !slot.facts.quick_select && slot.facts.grenade.is_none())
            .map(|slot| slot.class);
        match primary {
            Some(WeaponClass::Sniper) => Self::Overwatch,
            Some(WeaponClass::Smg | WeaponClass::Shotgun) => Self::Rush,
            _ => Self::Hold,
        }
    }

    fn guard_range(self) -> nav::GuardRange {
        match self {
            Self::Overwatch => nav::GuardRange {
                near: 700.0,
                far: 1500.0,
                ideal: 1050.0,
            },
            Self::Rush => nav::GuardRange {
                near: 600.0,
                far: 1300.0,
                ideal: 950.0,
            },
            Self::Hold => nav::GuardRange {
                near: 250.0,
                far: 850.0,
                ideal: 500.0,
            },
        }
    }
}

/// A bot's own way to a shared goal: a point off the straight line, on its own
/// side of it, visited first. Teammates heading the same way take different
/// lines without ever having to wait for one another.
#[derive(Clone, Copy, Debug)]
struct Detour {
    goal: [f32; 3],
    via: Option<[f32; 3]>,
}

/// The search for a covered place to hold an objective from. Candidates are
/// scored a few per think, since every one costs shot rays from the shared
/// quota; the pick then stays fixed for the life.
#[derive(Clone, Debug)]
struct GuardScan {
    key: (u32, u32, u32),
    candidates: Vec<(usize, [f32; 3])>,
    cursor: usize,
    best: Option<(f32, [f32; 3])>,
    chosen: Option<[f32; 3]>,
    started: u32,
    /// Where the scoring settled; later holds shift around it.
    home: Option<[f32; 3]>,
    held_since: Option<u32>,
    shifts: u32,
}

impl HostController {
    pub fn new(seed: u64) -> Self {
        let aggression = ((seed & 0xff) as f32) / 255.0;
        let self_preserve = (((seed >> 8) & 0xff) as f32) / 255.0;
        Self {
            aggression,
            self_preserve,
            memory: Memory::default(),
            task: Task::default(),
            intent: BotIntent::default(),
            motor: Motor::new(seed),
            last_think_tick: None,
            reaction_until: 0,
            last_target: None,
            fire_target: None,
            committed: Vec::new(),
            route_goal: None,
            route_pending: None,
            route_work: nav::RouteWork::default(),
            fight_scan: None,
            weapon: WeaponSkill::default(),
            nav_key: None,
            roam: None,
            roam_gen: 0,
            decision: Decision::default(),
            prev_seen: Vec::new(),
            was_touching: false,
            last_cmd: UserCmd::default(),
            life_sequence: None,
            report: MotorReport::Executing,
            progress_origin: None,
            progress_tick: 0,
            pace_seed: (seed >> 16) as u32,
            pace_released: None,
            guard: None,
            detour: None,
            objective_held: None,
            unfired_since: None,
            stalemate_until: 0,
            escape: None,
            escape_crouch: false,
            escape_jump: false,
            stuck_tries: 0,
            spawn_at: None,
            throw: None,
            nades_used: 0,
            nade_check: 0,
            glass_attempt: None,
            flank: None,
            ads_since: None,
            last_semi_shot: 0,
            semi_gap: 0,
            semi_clip: (0, 0),
            damage_seen: None,
            hit_look: None,
            strafe: (1.0, 0),
            cover: None,
            cover_search_at: 0,
            use_since: None,
            fought_at: 0,
            look: Default::default(),
        }
    }

    pub(crate) fn cancel_navigation(&mut self) {
        self.route_work.cancel();
        self.fight_scan = None;
        self.committed.clear();
        self.route_goal = None;
        self.route_pending = None;
    }

    pub fn decision(&self) -> Decision {
        self.decision
    }

    pub fn route_stats(&self) -> nav::RouteStats {
        self.route_work.stats()
    }

    /// Slices the live route request has waited, and its longest run without
    /// granted progress. `(0, 0)` when nothing is pending.
    pub fn route_age(&self) -> (u32, u32) {
        self.route_work.live_age()
    }

    /// A bot with a movement task that has not moved. Holding, interacting and
    /// waiting to respawn are not navigation failures.
    pub fn stuck(&self, tick: u32) -> bool {
        matches!(
            self.intent.move_mode,
            MoveMode::Walk | MoveMode::Drop | MoveMode::BreakGlass
        ) && tick.saturating_sub(self.progress_tick) >= STUCK_TICKS
    }

    /// The enemy this bot has committed to, for perception to refresh before it
    /// spends the same allowance looking for new ones. `None` when the bot is
    /// not fighting.
    pub fn focus_target(&self) -> Option<ClientId> {
        self.last_target
            .filter(|_| self.task.kind == TaskKind::Fight)
    }

    pub fn task(&self) -> Task {
        self.task
    }

    pub fn intent(&self) -> BotIntent {
        self.intent
    }

    pub fn report(&self) -> MotorReport {
        self.report
    }

    pub fn last_cmd(&self) -> UserCmd {
        self.last_cmd
    }

    pub fn drive(
        &mut self,
        obs: &BotObservation,
        world: &mut impl WorldQuery,
        dt_ms: i32,
    ) -> UserCmd {
        self.drive_nav(obs, world, None, &mut 512, dt_ms)
    }

    pub fn drive_nav(
        &mut self,
        obs: &BotObservation,
        world: &mut impl WorldQuery,
        nav: Option<&NavGraph>,
        astar_budget: &mut u32,
        dt_ms: i32,
    ) -> UserCmd {
        world.enter(QuerySubsystem::Execution);
        let key = nav.map(|g| (g.generation, g.digest, g.schema, g.hull));
        if self.nav_key != key {
            self.nav_key = key;
            self.cancel_navigation();
        }
        if self.life_sequence != Some(obs.self_state.life_sequence) {
            self.life_sequence = Some(obs.self_state.life_sequence);
            self.cancel_navigation();
            self.weapon.reset();
            self.roam = None;
            self.memory = Memory::default();
            self.prev_seen.clear();
            self.was_touching = false;
            self.last_think_tick = None;
            self.last_target = None;
            self.fire_target = None;
            self.progress_origin = None;
            self.escape = None;
            self.escape_crouch = false;
            self.escape_jump = false;
            self.stuck_tries = 0;
            self.flank = None;
            self.cover = None;
            self.decision = Decision::default();
            self.task = Task::default();
            self.pace_released = None;
            self.guard = None;
            self.detour = None;
            self.objective_held = None;
            self.unfired_since = None;
            self.stalemate_until = 0;
            self.spawn_at = None;
            self.throw = None;
            self.nades_used = 0;
        }
        if obs.self_state.lifecycle != ClientLifecycle::Alive {
            self.task = Task {
                kind: TaskKind::Hunt,
                reason: SwitchReason::Died,
            };
            self.intent = BotIntent::default();
            self.route_work.cancel();
            self.weapon.reset();
            self.fight_scan = None;
            self.fire_target = None;
            self.decision.stage = ActionStage::Failed;
            self.decision.objective = None;
            self.committed.clear();
            self.last_cmd = UserCmd {
                server_time: obs.time_ms,
                weapon: obs.self_state.weapon,
                weapon_mapped: obs.self_state.weapon,
                ..UserCmd::default()
            };
            return self.last_cmd;
        }
        if self.should_think(obs.tick, obs.self_state.id.0) || self.urgent_think(obs) {
            self.think(obs, world, nav, astar_budget);
            self.last_think_tick = Some(obs.tick);
        }
        if self.task.kind == TaskKind::Fight
            && self
                .last_target
                .is_some_and(|id| self.memory.currently_seen().any(|c| c.id == id))
        {
            self.fought_at = obs.tick;
        }
        // Intents are reused between scheduled thinks, so the authorization is
        // re-checked on the tick that actually emits the command.
        self.gate_fire(obs);
        // A fight the bot cannot shoot in, seen through a gap with no reachable
        // firing position, would otherwise hold both sides forever: after a
        // while the bot goes back to its job and finds a new angle on the way.
        let busy = self.cover.is_some_and(|plan| !plan.peeking)
            || obs.self_state.weapon_action == crate::observation::WeaponAction::Reloading;
        if self.task.kind == TaskKind::Fight && !self.intent.fire && !busy {
            let since = *self.unfired_since.get_or_insert(obs.tick);
            if obs.tick.saturating_sub(since) >= STALEMATE_TICKS {
                self.stalemate_until = obs.tick + STALEMATE_BREAK_TICKS;
                self.unfired_since = None;
            }
        } else {
            self.unfired_since = None;
        }
        // Walking on through a starved plan is for travel; in a fight the bot
        // stands and shoots from where it is until its position is decided.
        if self.task.kind == TaskKind::Fight && self.intent.path == PathOutcome::BudgetExhausted {
            self.intent.move_mode = MoveMode::Hold;
        }
        if obs.methodical {
            self.apply_pace(obs, world);
            self.apply_grenades(obs, world);
        }
        self.apply_hit_reaction(obs);
        if obs.methodical {
            self.apply_cover_fight(obs, world, nav);
        }
        if self.cover.is_none() {
            self.apply_combat_strafe(obs);
        }
        self.apply_ads(obs);
        self.apply_trigger_rate(obs);
        // Frozen for the buy period: stand still, facing where it is.
        if obs.frozen {
            self.intent.look_at = None;
            self.intent.move_mode = MoveMode::Hold;
            self.intent.move_goal = Some(obs.self_state.origin);
            self.intent.crouch = false;
            self.intent.sprint = false;
            self.intent.jump = false;
            self.intent.fire = false;
            self.intent.ads = false;
            self.intent.throw_buttons = 0;
        }
        self.intent.precise = false;
        let use_now = self.press_use_properly(obs);
        if self.intent.move_mode != MoveMode::BreakGlass {
            self.glass_attempt = None;
        }
        if self.intent.move_mode == MoveMode::BreakGlass {
            self.intent.fire = false;
            self.intent.sprint = false;
            self.intent.use_button = false;
            let goal = self.intent.move_goal;
            let since = match (self.glass_attempt, goal) {
                (Some((at, since)), Some(goal)) if dist_xy(at, goal) < WAYPOINT_IN * 2.0 => since,
                _ => obs.tick,
            };
            self.glass_attempt = goal.map(|goal| (goal, since));
            if obs.tick.saturating_sub(since) >= GLASS_PATIENCE_TICKS {
                // Shots are not clearing it: whatever is left is not worth a
                // magazine. Walk on into it instead.
                self.intent.move_mode = MoveMode::Walk;
                self.intent.path = PathOutcome::Clear;
            } else if let Some(goal) = goal {
                self.intent.precise = true;
                world.enter(QuerySubsystem::Tactical);
                let eye = obs.eye();
                // Try several body-height points on the obstructed corridor.
                for height in [obs.self_state.view_height.max(40.0), 40.0, 20.0] {
                    let target = [goal[0], goal[1], goal[2] + height];
                    if let Ok(SightSample::Blocked {
                        fraction,
                        obstacle: crate::query::ObstacleKind::Glass,
                    }) = world.shot_ray(eye, target, obs.self_state.id)
                    {
                        self.intent.look_at = Some(std::array::from_fn(|i| {
                            eye[i] + (target[i] - eye[i]) * fraction
                        }));
                        // One player shoots out a pane; a teammate already
                        // closer to it is left to do it.
                        let mine = dist_xy(obs.self_state.origin, goal);
                        let nearer_mate = obs.teammates.iter().any(|(_, at)| {
                            let theirs = dist_xy(*at, goal);
                            theirs < mine && theirs < GLASS_SHARE_IN
                        });
                        self.intent.fire = obs.self_state.ammo_clip > 0
                            && !nearer_mate
                            && self.intent.weapon.is_none()
                            && !self.intent.reload
                            && (self.task.kind != TaskKind::Fight
                                || obs.tick >= self.reaction_until);
                        break;
                    }
                }
            }
        }
        // The use key held off this tick only; the bot still means to use.
        let cmd = if use_now == self.intent.use_button {
            self.motor.drive(obs, &self.intent, dt_ms)
        } else {
            let mut intent = self.intent;
            intent.use_button = use_now;
            self.motor.drive(obs, &intent, dt_ms)
        };
        self.report = self.motor.report(obs, &self.intent, &cmd);
        self.last_cmd = cmd;
        cmd
    }

    /// Walks instead of sprinting, holds and looks around after the freeze
    /// lifts, and stops to check its surroundings while travelling. Fights,
    /// interactions and a ticking bomb keep their full pace.
    fn apply_pace(&mut self, obs: &BotObservation, world: &mut impl WorldQuery) {
        if obs.frozen {
            self.pace_released = None;
            return;
        }
        let released = *self.pace_released.get_or_insert(obs.tick);
        self.spawn_at.get_or_insert(obs.self_state.origin);
        let style = Style::of(obs);
        let carrying = self
            .decision
            .objective
            .is_some_and(|o| o.action == ObjectiveAction::Plant);
        let defusing = self
            .decision
            .objective
            .is_some_and(|o| o.action == ObjectiveAction::Defuse);
        if defusing {
            return;
        }
        self.intent.sprint = false;
        let engaged = self.task.kind == TaskKind::Fight
            || self.task.kind == TaskKind::Investigate
            || self.memory.currently_seen().next().is_some()
            || self.intent.fire
            || self.intent.use_button;
        if engaged {
            self.look.release();
            return;
        }
        self.intent.calm = true;
        let seed = self.pace_seed;
        let here = obs.self_state.origin;
        self.look.refresh(world, obs.eye(), obs.self_state.id);
        // Enemies come along the way in from their start (through that
        // door or round that corner); failing that, from ahead of where the
        // team set out. Teammates standing close block the view their way.
        let threat = obs
            .enemy_approach
            .map(|at| (at[1] - here[1]).atan2(at[0] - here[0]))
            .or_else(|| {
                self.spawn_at
                    .filter(|spawn| dist_xy(*spawn, here) > 300.0)
                    .map(|spawn| (here[1] - spawn[1]).atan2(here[0] - spawn[0]))
            })
            .or_else(|| {
                self.decision
                    .objective
                    .map(|o| (o.origin[1] - here[1]).atan2(o.origin[0] - here[0]))
            });
        let mates: Vec<f32> = obs
            .teammates
            .iter()
            .filter(|(_, at)| dist_xy(*at, here) < LOOK_MATE_IN)
            .map(|(_, at)| (at[1] - here[1]).atan2(at[0] - here[0]))
            .collect();
        let look_roll = pace_roll(seed, obs.tick, 41);
        // A teammate's callout: aim at it where there is a line to it,
        // otherwise favour looking that way.
        let watch = self.enemy_watch(obs);
        let watch_yaw = watch.map(|w| (w[1] - here[1]).atan2(w[0] - here[0]));
        let watch = watch.filter(|_| watch_yaw.is_some_and(|y| self.look.open_toward(y) != Some(false)));
        if self.intent.move_mode == MoveMode::Hold {
            let crouch_pct = match style {
                Style::Overwatch => 5,
                Style::Rush => PACE_CROUCH_HOLD_PCT / 2,
                Style::Hold => 45,
            };
            if pace_roll(seed, obs.tick / PACE_CROUCH_HOLD_TICKS, 4) < crouch_pct {
                self.intent.crouch = true;
            }
            // Holding: mostly the angle it chose to hold, but checking the
            // other lines in and the corners now and then.
            let held = self
                .intent
                .look_at
                .map(|l| ((l[1] - here[1]).atan2(l[0] - here[0]), 1.2));
            let bias = crate::look::LookBias {
                forward: held,
                threat: watch_yaw.or(threat),
                cone: None,
                mates: &mates,
                view: obs.self_state.viewangles[1].to_radians(),
            dwell: (LOOK_HOLD_MIN_TICKS, LOOK_HOLD_SPREAD_TICKS),
            };
            if let Some(yaw) = self.look.pick(obs.tick, look_roll, &bias) {
                self.intent.look_at = Some(look_point(obs, yaw));
            }
            if let Some(watch) = watch {
                self.intent.look_at = Some(watch);
            }
            // Standing on top of a teammate: take a short step off them.
            let mut push = [0.0f32; 2];
            for (_, mate) in &obs.teammates {
                let (mx, my) = (here[0] - mate[0], here[1] - mate[1]);
                let d = mx.hypot(my);
                if (1.0..HOLD_ROOM_IN).contains(&d) && (mate[2] - here[2]).abs() <= 64.0 {
                    push[0] += mx / d;
                    push[1] += my / d;
                }
            }
            let size = push[0].hypot(push[1]);
            if size > 0.1 {
                self.intent.move_mode = MoveMode::Walk;
                self.intent.path = PathOutcome::Clear;
                self.intent.crouch = false;
                self.intent.move_goal = Some([
                    here[0] + push[0] / size * HOLD_STEP_IN,
                    here[1] + push[1] / size * HOLD_STEP_IN,
                    here[2],
                ]);
            }
            return;
        }
        if self.intent.move_mode != MoveMode::Walk {
            return;
        }
        let opening = PACE_OPENING_MIN + seed % PACE_OPENING_SPREAD;
        if obs.tick.saturating_sub(released) < opening {
            self.intent.move_mode = MoveMode::Hold;
            self.intent.move_goal = Some(here);
            self.progress_origin = Some(here);
            self.progress_tick = obs.tick;
            return;
        }
        let Some(goal) = self.intent.move_goal else {
            return;
        };
        let (dx, dy) = (goal[0] - here[0], goal[1] - here[1]);
        if dx.hypot(dy) < 1.0 {
            return;
        }
        let heading = dy.atan2(dx);
        // Players sprint out of spawn, then walk once contact is possible.
        // The carrier runs the bomb in ahead of the team; pushing guns sprint
        // for longer before they slow down near the enemy.
        let (sprint_window, sprint_pct, crouch_pct) = if carrying {
            (PACE_SPRINT_WINDOW * 3, 70, 0)
        } else {
            match style {
                Style::Rush => (PACE_SPRINT_WINDOW * 4, 60, 5),
                Style::Overwatch => (PACE_SPRINT_WINDOW, PACE_SPRINT_PCT, 0),
                Style::Hold => (PACE_SPRINT_WINDOW * 2, 45, PACE_CROUCH_WALK_PCT),
            }
        };
        // With an enemy known to be close, walk quietly instead.
        let enemy_near = self.memory.last_seen().any(|c| {
            obs.tick.saturating_sub(c.seen_tick) <= LAST_SEEN_FRESH_TICKS
                && dist_xy(here, c.origin) < QUIET_NEAR_IN
        });
        let sprinting = !enemy_near
            && obs.tick.saturating_sub(released) < sprint_window
            && pace_roll(seed, obs.tick / PACE_SPRINT_TICKS, 3) < sprint_pct;
        if enemy_near && !carrying {
            self.intent.quiet = true;
        }
        // Check the corners and open lines around the path instead of
        // staring down it (or at a wall); the body keeps walking the path, so
        // this strafes and walks diagonally. A sprint looks where it is going.
        // Pushing players and the bomb carrier lean toward looking ahead.
        let ahead = if carrying || style == Style::Rush { 1.2 } else { 0.5 };
        let bias = crate::look::LookBias {
            forward: Some((heading, ahead)),
            threat: watch_yaw.or(threat),
            cone: Some(LOOK_WALK_CONE),
            mates: &mates,
            view: obs.self_state.viewangles[1].to_radians(),
            dwell: (LOOK_WALK_MIN_TICKS, LOOK_WALK_SPREAD_TICKS),
        };
        let yaw = if sprinting {
            self.look.release();
            heading
        } else {
            self.look.pick(obs.tick, look_roll, &bias).unwrap_or(heading)
        };
        self.intent.look_at = Some(look_point(obs, yaw));
        if let Some(watch) = watch {
            self.intent.look_at = Some(watch);
        }
        if sprinting {
            self.intent.sprint = true;
        } else if pace_roll(seed, obs.tick / PACE_CROUCH_WALK_TICKS, 5) < crouch_pct {
            self.intent.crouch = true;
        }
        self.ease_off_teammates(obs, goal);
    }

    /// Teammates may walk together but each prefers some room: a bot drifts
    /// sideways off a teammate it is about to walk on top of, more the
    /// closer they are, and carries on along its route. Nothing stops them
    /// from standing or moving close when the way is narrow.
    fn ease_off_teammates(&mut self, obs: &BotObservation, goal: [f32; 3]) {
        let here = obs.self_state.origin;
        let (dx, dy) = (goal[0] - here[0], goal[1] - here[1]);
        let span = dx.hypot(dy);
        if span < 1.0 {
            return;
        }
        let ahead = [dx / span, dy / span];
        let mut push = [0.0f32; 2];
        for (_, mate) in &obs.teammates {
            let (mx, my) = (here[0] - mate[0], here[1] - mate[1]);
            let d = mx.hypot(my);
            if d >= ROOM_IN || d < 1.0 || (mate[2] - here[2]).abs() > 64.0 {
                continue;
            }
            let weight = (ROOM_IN - d) / ROOM_IN;
            push[0] += mx / d * weight;
            push[1] += my / d * weight;
        }
        // Only the sideways part: easing off never means stopping or
        // turning back.
        let side = [-ahead[1], ahead[0]];
        let lateral = (push[0] * side[0] + push[1] * side[1]).clamp(-1.0, 1.0);
        if lateral.abs() < 0.05 {
            return;
        }
        let step = span.min(ROOM_LOOKAHEAD_IN);
        self.intent.move_goal = Some([
            here[0] + ahead[0] * step + side[0] * lateral * ROOM_SIDESTEP_IN,
            here[1] + ahead[1] * step + side[1] * lateral * ROOM_SIDESTEP_IN,
            goal[2],
        ]);
    }

    /// Fights are taken down the sights, as players do past point-blank
    /// range; the shot waits until the sights are most of the way up.
    fn apply_ads(&mut self, obs: &BotObservation) {
        let here = obs.self_state.origin;
        let class = obs.self_state.weapon_class;
        let target = self
            .fire_target
            .and_then(|(id, _)| self.memory.currently_seen().find(|c| c.id == id))
            .or_else(|| {
                self.memory
                    .currently_seen()
                    .min_by(|a, b| dist_xy(here, a.origin).total_cmp(&dist_xy(here, b.origin)))
            })
            .map(|c| c.origin);
        let ready_gun = obs.held().is_some_and(|slot| slot.is_loaded_gun())
            && obs.self_state.weapon_action.is_settled()
            && self.intent.weapon.is_none()
            && !self.intent.reload;
        self.intent.ads = obs.methodical
            && ready_gun
            && self.task.kind == TaskKind::Fight
            && self.intent.move_mode != MoveMode::BreakGlass
            && target.is_some_and(|at| class == WeaponClass::Sniper || dist_xy(here, at) > ADS_MIN_IN);
        if !self.intent.ads {
            self.ads_since = None;
            return;
        }
        let since = *self.ads_since.get_or_insert(obs.tick);
        let quick = obs.held().is_some_and(|slot| slot.facts.quick_select);
        let ready = if quick {
            ADS_READY_PISTOL
        } else {
            match class {
                WeaponClass::Sniper => ADS_READY_SNIPER,
                WeaponClass::Lmg => ADS_READY_LMG,
                WeaponClass::Assault => ADS_READY_RIFLE,
                WeaponClass::Smg | WeaponClass::Shotgun => ADS_READY_SMG,
            }
        };
        if obs.tick.saturating_sub(since) < ready {
            self.intent.fire = false;
        }
        // Trigger discipline with an automatic weapon: full auto close in,
        // short bursts at mid range, single taps far away.
        let automatic = obs.held().is_some_and(|slot| !slot.facts.semi_auto);
        if let Some(range) = target.map(|at| dist_xy(here, at))
            && automatic
            && class != WeaponClass::Sniper
            && range > BURST_NEAR_IN
        {
            let (on, cycle) = if range > BURST_FAR_IN { (1, 6) } else { (5, 10) };
            if (obs.tick.wrapping_add(self.pace_seed)) % cycle >= on {
                self.intent.fire = false;
            }
        }
    }

    /// The bomb's defuse trigger works on a press of the use key made while
    /// looking at the bomb, as a player does it: the key goes down once the
    /// sights are on it, and a hold that has started nothing is let go and
    /// pressed again.
    fn press_use_properly(&mut self, obs: &BotObservation) -> bool {
        if !self.intent.use_button {
            self.use_since = None;
            return false;
        }
        let aimed = self
            .intent
            .look_at
            .is_none_or(|at| self.motor.aimed_at(obs, at, USE_AIM_CONE_DEG));
        if !aimed {
            self.use_since = None;
            return false;
        }
        let (since, weapon, mut started) =
            *self.use_since.get_or_insert((obs.tick, obs.self_state.weapon, false));
        // Begun: the bomb or the defuse kit is coming out or in hand, or
        // progress shows. Once begun the key stays down.
        started |= obs.objective.is_some_and(|o| o.progress > 0.0)
            || obs.self_state.weapon != weapon
            || obs.self_state.weapon_action != crate::observation::WeaponAction::Ready;
        self.use_since = Some((since, weapon, started));
        if !started && obs.tick.saturating_sub(since) >= USE_REPRESS_TICKS {
            self.use_since = None;
            return false;
        }
        true
    }

    /// Where a teammate just called an enemy: the angle to hold, at head
    /// height.
    fn enemy_watch(&self, obs: &BotObservation) -> Option<[f32; 3]> {
        let here = obs.self_state.origin;
        // Its own sightings first: where an enemy was seen in the last ten
        // seconds is where it is most likely to come from.
        let own = self
            .memory
            .last_seen()
            .filter(|c| {
                c.confidence >= CALLOUT_CONFIDENCE_MAX
                    && obs.tick.saturating_sub(c.seen_tick) <= MEMORY_WATCH_TICKS
                    && dist_xy(here, c.origin) < MEMORY_WATCH_IN
            })
            .max_by_key(|c| c.seen_tick);
        let called = self.memory.last_seen().filter(|c| {
            c.confidence < CALLOUT_CONFIDENCE_MAX
                && obs.tick.saturating_sub(c.seen_tick) <= LAST_SEEN_FRESH_TICKS
                && dist_xy(here, c.origin) < CALLOUT_WATCH_IN
        });
        own.or_else(|| called.min_by(|a, b| dist_xy(here, a.origin).total_cmp(&dist_xy(here, b.origin))))
            .map(|c| [c.origin[0], c.origin[1], c.origin[2] + 60.0])
    }

    /// The enemy it was fighting, if it lost sight of them only a few
    /// seconds ago.
    fn lost_fight(&self, obs: &BotObservation) -> Option<Contact> {
        let id = self.last_target?;
        if obs.tick.saturating_sub(self.fought_at) > FIGHT_MEMORY_TICKS {
            return None;
        }
        self.memory.last_seen().find(|c| c.id == id).copied()
    }

    /// Fights are taken from cover where there is any close by: peek out to
    /// shoot, duck back after a burst, to reload or when hurt, and peek again;
    /// an enemy that drops out of sight gets a quick look or two before the
    /// bot moves on.
    fn apply_cover_fight(
        &mut self,
        obs: &BotObservation,
        world: &mut impl WorldQuery,
        nav: Option<&NavGraph>,
    ) {
        let here = obs.self_state.origin;
        let seen_target = self
            .last_target
            .and_then(|id| self.memory.currently_seen().find(|c| c.id == id))
            .copied();
        // Keep an existing plan while its enemy is in sight or only just
        // gone; otherwise look for cover when a fight starts.
        if let Some(plan) = self.cover {
            let lost = obs.tick.saturating_sub(plan.last_seen);
            let target_seen = seen_target.is_some_and(|c| c.id == plan.target);
            let spent = plan.peeks > COVER_MAX_PEEKS && plan.peeking;
            // A hit enemy who ducks out of sight is chased down, not waited for.
            let finish = !target_seen && plan.target_hurt && lost > 4;
            if spent || finish {
                self.cover_search_at = obs.tick + COVER_RETRY_TICKS;
            }
            if spent
                || finish
                || (!target_seen && lost > COVER_LOST_TICKS)
                || plan.jiggles > COVER_MAX_JIGGLES
                || self.memory.currently_seen().any(|c| c.id != plan.target && dist_xy(here, c.origin) < 400.0)
                || dist_xy(here, plan.peek) > COVER_LEASH_IN
            {
                self.cover = None;
            }
        }
        if self.cover.is_none() {
            let Some(target) = seen_target else {
                return;
            };
            if self.task.kind != TaskKind::Fight
                || obs.tick < self.cover_search_at
                || obs.self_state.weapon_class == WeaponClass::Shotgun
                || dist_xy(here, target.origin) < COVER_CLOSE_IN
                || obs.hurt.contains(&target.id)
            {
                return;
            }
            self.cover_search_at = obs.tick + COVER_SEARCH_TICKS;
            // Decided once per enemy: some fights are just taken in the open.
            let pct = if Style::of(obs) == Style::Rush { COVER_USE_RUSH_PCT } else { COVER_USE_PCT };
            if pace_roll(self.pace_seed, target.id.0 + obs.tick / 400, 42) >= pct {
                self.cover_search_at = obs.tick + COVER_RETRY_TICKS;
                return;
            }
            let Some(graph) = nav.filter(|g| !g.is_empty()) else {
                return;
            };
            let Some(cover) = find_cover(obs, world, graph, target) else {
                return;
            };
            self.cover = Some(CoverFight {
                target: target.id,
                cover,
                peek: here,
                peeking: true,
                until: obs.tick + self.peek_ticks(obs),
                jiggles: 0,
                peeks: 1,
                last_seen: obs.tick,
                health_at_peek: obs.self_state.health,
                target_hurt: false,
            });
        }
        let Some(mut plan) = self.cover else {
            return;
        };
        if seen_target.is_some_and(|c| c.id == plan.target) {
            plan.last_seen = obs.tick;
            plan.target_hurt = obs.hurt.contains(&plan.target);
        }
        let target_seen = plan.last_seen == obs.tick;
        let clip = obs.self_state.ammo_clip;
        let size = obs.held().map_or(0, |slot| slot.facts.clip_size).max(1);
        let low = clip * 100 < size * COVER_LOW_CLIP_PCT && obs.self_state.ammo_stock > 0;
        let health = obs.self_state.health;
        let hurt = health < COVER_HURT_HP || plan.health_at_peek - health >= COVER_HURT_LOSS;
        // An enemy it has hit is kept under fire to finish them, unless the
        // bot itself is about to die or is out of bullets.
        if plan.peeking && target_seen && plan.target_hurt && !low && health >= COVER_HURT_HP {
            plan.until = plan.until.max(obs.tick + 1);
        }
        if plan.peeking {
            // Duck back after a burst, to reload, or when hit.
            if obs.tick >= plan.until || low || hurt {
                plan.peeking = false;
                plan.until = obs.tick
                    + COVER_HIDE_MIN_TICKS
                    + pace_roll(self.pace_seed, obs.tick, 31) * COVER_HIDE_SPREAD_TICKS / 100;
            }
        } else if obs.tick >= plan.until && !(obs.self_state.weapon_action == crate::observation::WeaponAction::Reloading) {
            plan.peeking = true;
            plan.peeks += 1;
            plan.health_at_peek = obs.self_state.health;
            if target_seen || obs.tick.saturating_sub(plan.last_seen) < 4 {
                plan.until = obs.tick + self.peek_ticks(obs);
            } else {
                // A jiggle: a quick look out to find them again.
                plan.jiggles += 1;
                plan.until = obs.tick + COVER_JIGGLE_TICKS + pace_roll(self.pace_seed, obs.tick, 32) % 5;
            }
        }
        self.cover = Some(plan);
        let spot = if plan.peeking { plan.peek } else { plan.cover };
        let far = dist_xy(here, spot) > 12.0;
        self.intent.move_mode = if far { MoveMode::Walk } else { MoveMode::Hold };
        self.intent.path = PathOutcome::Clear;
        self.intent.move_goal = Some(spot);
        self.intent.sprint = false;
        self.progress_origin = Some(here);
        self.progress_tick = obs.tick;
        if !plan.peeking {
            self.intent.fire = false;
            self.intent.ads = false;
            // Behind cover is the time to top up the magazine.
            if clip * 100 < size * COVER_RELOAD_PCT && obs.self_state.ammo_stock > 0 {
                self.intent.reload = true;
            }
        }
        // Keep the sights where the enemy was, ready for the next peek.
        if !target_seen
            && let Some(last) = self.memory.last_seen().find(|c| c.id == plan.target)
        {
            self.intent.look_at = Some([last.origin[0], last.origin[1], last.origin[2] + 52.0]);
        }
    }

    fn peek_ticks(&self, obs: &BotObservation) -> u32 {
        COVER_PEEK_MIN_TICKS + pace_roll(self.pace_seed, obs.tick, 30) * COVER_PEEK_SPREAD_TICKS / 100
    }

    /// Shot by someone it cannot see, a player turns toward where the hit
    /// came from instead of carrying on as if nothing happened.
    fn apply_hit_reaction(&mut self, obs: &BotObservation) {
        let hit = self
            .damage_seen
            .is_some_and(|seen| seen != obs.damage_event);
        self.damage_seen = Some(obs.damage_event);
        let blind = self.memory.currently_seen().next().is_none();
        if hit && blind {
            self.hit_look = Some((obs.damage_yaw, obs.tick + HIT_LOOK_TICKS));
        }
        let Some((yaw, until)) = self.hit_look else {
            return;
        };
        if obs.tick >= until || !blind {
            self.hit_look = None;
            return;
        }
        let here = obs.self_state.origin;
        let rad = yaw.to_radians();
        self.intent.look_at = Some([
            here[0] + rad.cos() * 600.0,
            here[1] + rad.sin() * 600.0,
            obs.eye()[2],
        ]);
        self.intent.calm = false;
        self.intent.sprint = false;
    }

    /// Standing still in a gunfight is how players die: while shooting, a
    /// bot strafes side to side in uneven steps (snipers hold still).
    fn apply_combat_strafe(&mut self, obs: &BotObservation) {
        if !obs.methodical
            || self.task.kind != TaskKind::Fight
            || self.intent.move_mode != MoveMode::Hold
            || self.intent.crouch
            || obs.self_state.weapon_class == WeaponClass::Sniper
        {
            return;
        }
        let here = obs.self_state.origin;
        let Some(target) = self
            .fire_target
            .and_then(|(id, _)| self.memory.currently_seen().find(|c| c.id == id))
            .map(|c| c.origin)
        else {
            return;
        };
        let (dx, dy) = (target[0] - here[0], target[1] - here[1]);
        let span = dx.hypot(dy);
        if span < 1.0 {
            return;
        }
        if obs.tick >= self.strafe.1 {
            let roll = pace_roll(self.pace_seed, obs.tick, 23);
            // Mostly change direction, sometimes keep going a little longer.
            if roll < 70 {
                self.strafe.0 = -self.strafe.0;
            }
            self.strafe.1 = obs.tick + STRAFE_MIN_TICKS + roll % STRAFE_SPREAD_TICKS;
        }
        let side = [-dy / span * self.strafe.0, dx / span * self.strafe.0];
        self.intent.move_mode = MoveMode::Walk;
        self.intent.path = PathOutcome::Clear;
        self.intent.move_goal = Some([
            here[0] + side[0] * STRAFE_STEP_IN,
            here[1] + side[1] * STRAFE_STEP_IN,
            here[2],
        ]);
        self.progress_origin = Some(here);
        self.progress_tick = obs.tick;
    }

    /// A player can only pull a trigger so fast, and slows down to keep a
    /// distant target in the sights: pistols most of all.
    fn apply_trigger_rate(&mut self, obs: &BotObservation) {
        // The wait runs from the shot that actually left the barrel; until
        // the gun is ready again the trigger keeps being worked.
        let clip = (obs.self_state.weapon, obs.self_state.ammo_clip);
        let fired = clip.0 == self.semi_clip.0 && clip.1 < self.semi_clip.1;
        self.semi_clip = clip;
        let Some(slot) = obs.held().filter(|slot| slot.facts.semi_auto) else {
            return;
        };
        if fired {
            // The next pull comes after an uneven wait, as a finger does,
            // longer the farther the target.
            let here = obs.self_state.origin;
            let range = self
                .fire_target
                .and_then(|(id, _)| self.memory.currently_seen().find(|c| c.id == id))
                .map_or(0.0, |c| dist_xy(here, c.origin));
            let band = if range < SEMI_NEAR_IN {
                0
            } else if range < SEMI_FAR_IN {
                1
            } else {
                2
            };
            let table = if slot.facts.quick_select {
                SEMI_PISTOL_TICKS[band]
            } else {
                SEMI_GUN_TICKS[band]
            };
            let pick = pace_roll(self.pace_seed, obs.tick, 21) as usize % table.len();
            self.last_semi_shot = obs.tick;
            self.semi_gap = table[pick];
        }
        if self.intent.move_mode != MoveMode::BreakGlass
            && obs.tick.saturating_sub(self.last_semi_shot) < self.semi_gap
        {
            self.intent.fire = false;
        }
    }

    /// Where to go after an enemy lost from sight or heard: straight at them,
    /// or, for a pushing gun or a bold player some way off, first to a point
    /// beside them so the approach comes from the flank.
    fn flank_point(&mut self, obs: &BotObservation, nav: Option<&NavGraph>, last: Contact) -> [f32; 3] {
        let here = obs.self_state.origin;
        // A sound is only placed roughly: aim for walkable ground near it.
        let near_ground = nav
            .filter(|graph| !graph.is_empty())
            .and_then(|graph| {
                nav::nearest_within(graph, last.origin, 300.0).map(|n| graph.nodes[n as usize])
            })
            .unwrap_or(last.origin);
        let range = dist_xy(here, last.origin);
        let flanker = Style::of(obs) == Style::Rush || self.aggression > 0.6;
        if !obs.methodical || !flanker || range < FLANK_MIN_IN {
            return near_ground;
        }
        if let Some((id, point, reached)) = self.flank.as_mut()
            && *id == last.id
        {
            if !*reached && dist_xy(here, *point) <= FLANK_REACHED_IN {
                *reached = true;
            }
            return if *reached { near_ground } else { *point };
        }
        let Some(graph) = nav.filter(|graph| !graph.is_empty()) else {
            return near_ground;
        };
        let back = (here[1] - last.origin[1]).atan2(here[0] - last.origin[0]);
        let side = if pace_roll(self.pace_seed, last.id.0, 15) < 50 { 1.0 } else { -1.0 };
        let radius = (range * 0.6).min(FLANK_MAX_IN);
        let angle = back + side * FLANK_ANGLE;
        let want = [
            last.origin[0] + angle.cos() * radius,
            last.origin[1] + angle.sin() * radius,
            last.origin[2],
        ];
        let point = nav::nearest_within(graph, want, 160.0)
            .map(|node| graph.nodes[node as usize])
            .unwrap_or(near_ground);
        self.flank = Some((last.id, point, false));
        point
    }

    /// Starts, continues or gives up a grenade throw, overriding the intent
    /// while the bot aims and throws.
    fn apply_grenades(&mut self, obs: &BotObservation, world: &mut impl WorldQuery) {
        // The planter or defuser at the bomb gets on with it.
        let at_bomb = self.intent.use_button
            || obs.objective.is_some_and(|o| {
                o.role == TeamRole::Actor
                    && matches!(o.action, ObjectiveAction::Plant | ObjectiveAction::Defuse)
                    && (o.touching || dist_xy(obs.self_state.origin, o.origin) < NADE_ACTOR_CLEAR_IN)
            });
        if obs.frozen || (at_bomb && self.throw.is_none_or(|t| t.stage == grenade::Stage::Aim)) {
            self.throw = None;
            return;
        }
        let seen = self.memory.currently_seen().next().is_some();
        if self.throw.is_none()
            && !seen
            && obs.tick >= self.nade_check
            && obs.self_state.weapon_action.is_settled()
        {
            self.nade_check = obs.tick + NADE_CHECK_TICKS;
            self.throw = self.pick_throw(obs, world);
        }
        let Some(mut throw) = self.throw else {
            return;
        };
        // An enemy in view before the pin is out wins over the throw.
        if seen && throw.stage == grenade::Stage::Aim {
            self.throw = None;
            return;
        }
        let aimed = self.motor.aimed_at(obs, throw.aim, grenade::AIM_CONE_DEG);
        let done = throw.step(obs.tick, aimed);
        let here = obs.self_state.origin;
        self.intent.look_at = Some(throw.aim);
        self.intent.move_mode = MoveMode::Hold;
        self.intent.move_goal = Some(here);
        self.intent.fire = false;
        self.intent.sprint = false;
        self.intent.calm = false;
        self.intent.throw_buttons = if throw.holding_button() {
            throw.button()
        } else {
            0
        };
        self.progress_origin = Some(here);
        self.progress_tick = obs.tick;
        self.throw = match done {
            None => Some(throw),
            Some(_) => None,
        };
    }

    /// A throw a player would make now: utility onto the site before going
    /// in, or a grenade at an enemy that was just lost behind cover.
    fn pick_throw(&mut self, obs: &BotObservation, world: &mut impl WorldQuery) -> Option<grenade::Throw> {
        let seed = self.pace_seed;
        let here = obs.self_state.origin;
        let attacking = obs
            .objectives
            .iter()
            .any(|o| !matches!(o.action, ObjectiveAction::Defend | ObjectiveAction::Defuse));
        let mut wants: Vec<(Grenade, [f32; 3])> = Vec::new();
        // Before entering: the site the push is heading for.
        // Utility before going in: attackers onto the site they push,
        // defenders onto the planted site they retake.
        if attacking != obs.planted {
            let toward = obs.objective.map_or(here, |o| o.origin);
            if let Some(site) = obs
                .sites
                .iter()
                .copied()
                .min_by(|a, b| dist_xy(*a, toward).total_cmp(&dist_xy(*b, toward)))
                .filter(|site| {
                    let d = dist_xy(here, *site);
                    (NADE_SITE_NEAR_IN..=NADE_SITE_FAR_IN).contains(&d)
                        && dist_xy(toward, *site) < NADE_SITE_FAR_IN + 200.0
                })
            {
                let spot = |salt: u32| {
                    let a = pace_roll(seed, salt, 9) as f32 * 0.0628;
                    let r = pace_roll(seed, salt, 10) as f32 * 1.5;
                    [site[0] + a.cos() * r, site[1] + a.sin() * r, site[2]]
                };
                for (kind, pct, salt) in [
                    (Grenade::Frag, 60, 1),
                    (Grenade::Stun, 50, 2),
                    (Grenade::Smoke, 50, 3),
                    (Grenade::Flash, 40, 4),
                ] {
                    if pace_roll(seed, salt, 11) < pct {
                        wants.push((kind, spot(salt)));
                    }
                }
            }
        }
        // An enemy that ducked out of sight a moment ago.
        if let Some(lost) = self.memory.last_seen().find(|c| {
            let age = obs.tick.saturating_sub(c.seen_tick);
            let d = dist_xy(here, c.origin);
            (NADE_LOST_MIN_TICKS..=NADE_LOST_MAX_TICKS).contains(&age)
                && (NADE_LOST_NEAR_IN..=NADE_LOST_FAR_IN).contains(&d)
        }) {
            wants.push((Grenade::Frag, lost.origin));
            wants.push((Grenade::Stun, lost.origin));
            // Defenders smoke off an approach they cannot hold.
            if !attacking && dist_xy(here, lost.origin) > 700.0 {
                wants.push((Grenade::Smoke, lost.origin));
            }
        }
        for (kind, target) in wants {
            let bit = 1u8 << (kind as u8);
            if self.nades_used & bit != 0 || obs.grenade(kind).is_none() {
                continue;
            }
            let eye = obs.eye();
            let Some(pitch) = grenade::solve_pitch(eye, target) else {
                continue;
            };
            let (aim, apex) = grenade::aim_and_apex(eye, target, pitch);
            if !grenade::arc_clear(world, obs, apex, target) {
                continue;
            }
            self.nades_used |= bit;
            return Some(grenade::Throw {
                kind,
                aim,
                stage: grenade::Stage::Aim,
                since: obs.tick,
            });
        }
        None
    }

    /// After a few seconds on one angle a defender shifts to a neighbouring
    /// covered spot, alternating with the one it settled on first.
    fn guard_shift(&mut self, obs: &BotObservation) {
        let seed = self.pace_seed;
        let Some(scan) = self.guard.as_mut() else {
            return;
        };
        let Some(chosen) = scan.chosen else {
            return;
        };
        let home = *scan.home.get_or_insert(chosen);
        let since = *scan.held_since.get_or_insert(obs.tick);
        let (min, spread) = match Style::of(obs) {
            Style::Overwatch => (GUARD_HOLD_MIN * 4, GUARD_HOLD_SPREAD * 3),
            Style::Rush => (GUARD_HOLD_MIN / 2, GUARD_HOLD_SPREAD / 2),
            Style::Hold => (GUARD_HOLD_MIN, GUARD_HOLD_SPREAD),
        };
        let hold = min + pace_roll(seed, scan.shifts, 6) * spread / 100;
        if obs.tick.saturating_sub(since) < hold {
            return;
        }
        scan.shifts += 1;
        scan.held_since = None;
        let near: Vec<[f32; 3]> = scan
            .candidates
            .iter()
            .map(|(_, pos)| *pos)
            .filter(|pos| {
                dist_xy(*pos, home) <= GUARD_SHIFT_IN
                    && dist_xy(*pos, chosen) > 1.0
                    && !obs.teammates.iter().any(|(_, at)| dist_xy(*at, *pos) < 150.0)
            })
            .collect();
        scan.chosen = if dist_xy(chosen, home) > 1.0 || near.is_empty() {
            Some(home)
        } else {
            Some(near[pace_roll(seed, scan.shifts, 7) as usize % near.len()])
        };
    }

    /// The point to walk to now on the way to `goal`: this bot's detour point
    /// until it has been passed, then the goal itself. Short trips go direct,
    /// and the first of the squad always takes the straight line.
    /// `roam` lets the detour swing wide, a different corridor each round;
    /// otherwise (following a teammate) it stays a lane beside the direct line.
    fn detour_leg(
        &mut self,
        obs: &BotObservation,
        graph: &NavGraph,
        goal: [f32; 3],
        roam: bool,
    ) -> [f32; 3] {
        let here = obs.self_state.origin;
        if self.detour.is_none_or(|d| dist_xy(d.goal, goal) > 64.0) {
            let span = dist_xy(here, goal);
            // A fresh roll each life (each round in this mode).
            let life = obs.self_state.life_sequence.0;
            let roll = |salt| pace_roll(self.pace_seed ^ obs.squad.0.wrapping_mul(0x51), life, salt);
            let d = [(goal[0] - here[0]) / span.max(1.0), (goal[1] - here[1]) / span.max(1.0)];
            let side = [-d[1], d[0]];
            let at = |along: f32, offset: f32| {
                [
                    here[0] + (goal[0] - here[0]) * along + side[0] * offset,
                    here[1] + (goal[1] - here[1]) * along + side[1] * offset,
                    (here[2] + goal[2]) * 0.5,
                ]
            };
            let want = if roam {
                let reach = (span * DETOUR_ROAM_SHARE).min(DETOUR_ROAM_MAX_IN);
                let offset = (roll(13) as f32 / 50.0 - 1.0) * reach;
                (offset.abs() > DETOUR_STEP_IN * 0.5).then(|| at(0.3 + roll(14) as f32 / 250.0, offset))
            } else if roll(15) < DETOUR_ENTRY_PCT && span > DETOUR_MIN_IN * 1.5 {
                // Come into the goal from another side this round: a point
                // off to one side of it, swung 30-80 degrees from straight
                // on, so the last stretch runs in along a different line.
                let away = (here[1] - goal[1]).atan2(here[0] - goal[0]);
                let swing = (0.5 + roll(16) as f32 / 120.0) * if roll(17) < 50 { 1.0 } else { -1.0 };
                let reach = (span * 0.45).min(DETOUR_ENTRY_MAX_IN);
                Some([
                    goal[0] + (away + swing).cos() * reach,
                    goal[1] + (away + swing).sin() * reach,
                    goal[2],
                ])
            } else {
                // A lane beside the straight line, a different one each round.
                let lane = match (obs.squad.0 + roll(18)) % 5 {
                    0 => 0.0,
                    1 => 1.0,
                    2 => -1.0,
                    3 => 2.0,
                    _ => -2.0,
                };
                let offset = lane * DETOUR_STEP_IN * (0.7 + roll(19) as f32 / 160.0);
                (offset.abs() > DETOUR_STEP_IN * 0.5).then(|| at(0.35 + roll(20) as f32 / 330.0, offset))
            };
            let via = want
                .filter(|_| span > DETOUR_MIN_IN)
                .and_then(|want| {
                    // Only a support on the goal's own component can lead on to it.
                    let comp = nav::nearest_within(graph, goal, nav::SNAP_IN)
                        .map(|n| graph.component[n as usize])?;
                    graph
                        .nodes
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| graph.component[*i] == comp)
                        .map(|(_, pos)| {
                            (dist_xy(*pos, want) + 3.0 * (pos[2] - want[2]).abs(), *pos)
                        })
                        .filter(|(score, _)| *score <= DETOUR_SNAP_IN * 2.0)
                        .min_by(|a, b| a.0.total_cmp(&b.0))
                        .map(|(_, pos)| pos)
                });
            self.detour = Some(Detour { goal, via });
        }
        let detour = self.detour.as_mut().expect("set above");
        if let Some(via) = detour.via {
            if dist_xy(here, via) > DETOUR_REACHED_IN {
                return via;
            }
            detour.via = None;
        }
        goal
    }

    /// A covered support near `obj` that this bot holds, distinct from its
    /// teammates': each prefers its own sector around the site, avoids spots a
    /// teammate already stands on, and wants walls close by and a view of the
    /// site. `None` while the candidates are still being scored.
    fn guard_spot(
        &mut self,
        obs: &BotObservation,
        world: &mut impl WorldQuery,
        graph: &NavGraph,
        obj: ModeObjective,
    ) -> Option<[f32; 3]> {
        let key = (obj.id, obs.squad.0, obs.squad.1);
        let style = Style::of(obs);
        let spawn_at = self.spawn_at;
        if self.guard.as_ref().is_none_or(|scan| scan.key != key) {
            self.guard = Some(GuardScan {
                key,
                candidates: nav::guard_supports(
                    graph,
                    obj.origin,
                    GUARD_SECTORS,
                    if obs.methodical {
                        Style::of(obs).guard_range()
                    } else {
                        nav::GUARD_DEFAULT
                    },
                ),
                cursor: 0,
                best: None,
                chosen: None,
                started: obs.tick,
                home: None,
                held_since: None,
                shifts: 0,
            });
        }
        let scan = self.guard.as_mut()?;
        if scan.chosen.is_some() {
            return scan.chosen;
        }
        if scan.candidates.is_empty() {
            scan.chosen = Some(obj.origin);
            return scan.chosen;
        }
        // Each bot's own side of the site, shifted a little each round so a
        // defender is not always in the same spot.
        let shift = pace_roll(self.pace_seed, obs.self_state.life_sequence.0, 21) as usize % 5;
        let preferred = (obs.squad.0 as usize * 5 + (self.pace_seed as usize % 3) + shift) % GUARD_SECTORS;
        let sector_gap = |sector: usize| {
            let gap = sector.abs_diff(preferred);
            gap.min(GUARD_SECTORS - gap)
        };
        // The shared trace quota can starve the scoring for a while; past that,
        // the best spot scored so far, or failing that this bot's own sector,
        // still keeps teammates apart.
        if obs.tick.saturating_sub(scan.started) > GUARD_PATIENCE {
            scan.chosen = scan.best.map(|(_, pos)| pos).or_else(|| {
                scan.candidates
                    .iter()
                    .min_by_key(|(sector, _)| sector_gap(*sector))
                    .map(|(_, pos)| *pos)
            });
            return scan.chosen;
        }
        world.enter(QuerySubsystem::Tactical);
        let id = obs.self_state.id;
        let site = [obj.origin[0], obj.origin[1], obj.origin[2] + 40.0];
        let mut done = 0;
        while scan.cursor < scan.candidates.len() && done < GUARD_PER_THINK {
            let (sector, cand) = scan.candidates[scan.cursor];
            let eye = [cand[0], cand[1], cand[2] + 60.0];
            let Ok(view) = world.shot_ray(eye, site, id) else {
                return None;
            };
            let sees = match view {
                SightSample::Clear | SightSample::HitPlayer { .. } => true,
                SightSample::Blocked { fraction, .. } => fraction >= 0.9,
                _ => false,
            };
            let chest = [cand[0], cand[1], cand[2] + 40.0];
            let mut cover: f32 = 0.0;
            for ray in 0..COVER_RAYS {
                let a = ray as f32 * core::f32::consts::TAU / COVER_RAYS as f32;
                let end = [
                    chest[0] + a.cos() * COVER_REACH,
                    chest[1] + a.sin() * COVER_REACH,
                    chest[2],
                ];
                match world.shot_ray(chest, end, id) {
                    Ok(SightSample::Blocked { .. }) => cover += 1.0,
                    Ok(_) => {}
                    Err(_) => return None,
                }
            }
            let off = sector_gap(sector) as f32;
            // Pushing guns hold the side the enemy comes from; snipers stand
            // back toward their own spawn and watch the site from range.
            let lean = spawn_at.map_or(0.0, |spawn| {
                ((dist_xy(cand, spawn) - dist_xy(obj.origin, spawn)) / 250.0).clamp(-4.0, 4.0)
            });
            let (lean, off_weight) = match style {
                Style::Rush => (lean, 1.0),
                Style::Overwatch => (-lean, 1.0),
                Style::Hold => (0.0, 4.0),
            };
            let taken = obs
                .teammates
                .iter()
                .any(|(_, at)| dist_xy(*at, cand) < 150.0);
            // Some cover beats none, but a spot boxed in on every side sees nothing.
            let shelter = cover.min(3.0);
            let score = shelter * 2.0 + if sees { 4.0 } else { 0.0 } + lean * 2.0
                - off * off_weight
                - if taken { 8.0 } else { 0.0 };
            if scan.best.is_none_or(|(best, _)| score > best) {
                scan.best = Some((score, cand));
            }
            scan.cursor += 1;
            done += 1;
        }
        if scan.cursor >= scan.candidates.len() {
            scan.chosen = scan.best.map(|(_, pos)| pos).or(Some(obj.origin));
        }
        scan.chosen
    }

    /// A retained target is never a standing permission to shoot: the shot has
    /// to stay backed by a positive observation of that same client, refreshed
    /// on every tick perception actually looked.
    fn gate_fire(&mut self, obs: &BotObservation) {
        if !self.intent.fire {
            return;
        }
        let Some((id, since)) = self.fire_target.as_mut() else {
            self.intent.fire = false;
            return;
        };
        if obs.seen.iter().any(|contact| contact.id == *id) {
            *since = obs.tick;
        }
        if obs.tick.saturating_sub(*since) > FIRE_FRESH_TICKS {
            self.intent.fire = false;
        }
    }

    /// Records what a shot decision rests on. `None` withdraws the
    /// authorization, which the emitted-command path then enforces.
    fn authorize_fire(&mut self, fire: bool, target: Contact) -> bool {
        self.fire_target = fire.then_some((target.id, target.seen_tick));
        fire
    }

    fn should_think(&self, tick: u32, id: u32) -> bool {
        match self.last_think_tick {
            None => true,
            Some(last) if last == tick => false,
            Some(_) => (tick.wrapping_add(id)).is_multiple_of(THINK_PERIOD_TICKS),
        }
    }

    fn urgent_think(&self, obs: &BotObservation) -> bool {
        if self.last_think_tick == Some(obs.tick) {
            return false;
        }
        if self.decision.objective.is_some_and(|old| {
            !obs.objectives.iter().chain(obs.objective.iter()).any(|o| {
                o.id == old.id
                    && o.round == old.round
                    && o.action == old.action
                    && o.use_button == old.use_button
                    && o.touching == old.touching
            })
        }) {
            return true;
        }
        obs.events_since(&self.prev_seen)
            .iter()
            .any(|event| matches!(event, BotEvent::Spotted { .. }))
    }

    fn think(
        &mut self,
        obs: &BotObservation,
        world: &mut impl WorldQuery,
        nav: Option<&NavGraph>,
        astar_budget: &mut u32,
    ) {
        let started = std::time::Instant::now();
        let events = self.memory.ingest(obs);
        let touching_use = obs
            .objectives
            .iter()
            .copied()
            .chain(obs.objective)
            .find(|obj| obj.touching && obj.use_button);
        if touching_use.is_none()
            && let Some(intent) = self.unstick(obs, world)
        {
            self.intent = intent;
            self.task.reason = SwitchReason::ProgressLost;
            self.decision.stage = ActionStage::Approach;
            self.apply_weapon_skill(obs);
            return;
        }
        let mut objective = touching_use.or_else(|| {
            obs.objectives
                .iter()
                .copied()
                .chain(obs.objective)
                .max_by(|a, b| {
                    objective_utility(obs, *a, None)
                        .total_cmp(&objective_utility(obs, *b, None))
                        .then_with(|| b.id.cmp(&a.id))
                })
        });
        let mut next = self.pick_task(obs, objective);
        if next.kind == TaskKind::TouchObj && touching_use.is_none() {
            objective = choose_objective(
                obs,
                world,
                nav,
                astar_budget,
                &mut self.committed,
                &mut self.route_goal,
                &mut self.route_work,
                &mut self.objective_held,
            );
            next = self.pick_task(obs, objective);
        }
        let switched = next.kind != self.task.kind;
        // Routes are keyed by destination. A temporary combat fallback must not
        // discard navigation progress on every think tick.
        self.task = next;
        let previous_stage = self.decision.stage;
        let previous = self.decision.objective;
        self.decision.objective = objective.filter(|_| self.task.kind == TaskKind::TouchObj);
        self.intent = self.intent_for(obs, world, nav, astar_budget, objective);
        self.decision.stage = if matches!(
            self.intent.path,
            PathOutcome::Blocked | PathOutcome::Unreachable
        ) {
            ActionStage::Failed
        } else if previous.is_some_and(|old| {
            old.use_button
                && obs
                    .objectives
                    .iter()
                    .chain(obs.objective.iter())
                    .any(|new| {
                        old.id == new.id
                            && old.round == new.round
                            && old.action != new.action
                            && new.action == ObjectiveAction::Defend
                    })
        }) {
            ActionStage::Complete
        } else if self.intent.use_button {
            ActionStage::Interact
        } else if self.intent.move_mode == MoveMode::Hold {
            ActionStage::Hold
        } else if objective
            .is_some_and(|o| dist2(obs.self_state.origin, o.origin) < o.radius * o.radius)
        {
            ActionStage::Position
        } else {
            ActionStage::Approach
        };
        self.intent.sprint = self.intent.move_mode == MoveMode::Walk
            && self.intent.path == PathOutcome::Clear
            && self.memory.currently_seen().next().is_none()
            && !self.intent.use_button
            && !self.intent.fire
            && !self.intent.reload
            && !self.intent.crouch
            && self
                .intent
                .move_goal
                .is_some_and(|g| dist_xy(obs.self_state.origin, g) > 160.0);
        // Standing clearance also fits a crouched hull. Require a shot at crouched eye height.
        if self.intent.fire
            && self.intent.move_mode == MoveMode::Hold
            && obs.self_state.health < 60
            && let Some(target) = self.pick_fight_target(obs)
            && world.walk_hull(obs.self_state.origin, obs.self_state.origin)
                == Ok(WalkSample::Clear)
        {
            let mut eye = obs.self_state.origin;
            eye[2] += 40.0;
            self.intent.crouch = shot_allowed(
                world.shot_ray(eye, self.intent.look_at.unwrap(), obs.self_state.id),
                target.id,
            );
        }
        if (self.intent.path == PathOutcome::Unreachable
            || self.intent.path == PathOutcome::Blocked)
            && (self.task.kind == TaskKind::TouchObj || self.task.kind == TaskKind::Investigate)
        {
            self.task.reason = SwitchReason::PathFailed;
        }
        let touching_now = objective.is_some_and(|obj| obj.touching);
        if self.task.kind == TaskKind::TouchObj
            && self.was_touching
            && !touching_now
            && self.decision.stage != ActionStage::Complete
        {
            self.intent.path = PathOutcome::ProgressLost;
            self.task.reason = SwitchReason::ProgressLost;
        }
        self.was_touching = touching_now && self.task.kind == TaskKind::TouchObj;
        self.apply_weapon_skill(obs);
        self.prev_seen = self
            .memory
            .currently_seen()
            .map(|contact| contact.id)
            .collect();
        let weapon_act = self.weapon.act();
        if switched || previous_stage != self.decision.stage {
            let left = self
                .intent
                .move_goal
                .map(|goal| dist2(obs.self_state.origin, goal).sqrt())
                .unwrap_or(0.0);
            let know = self
                .memory
                .currently_seen()
                .next()
                .or_else(|| self.memory.last_seen().next())
                .map(|c| c.source);
            diag::info!(
                Sim,
                "bots: id={} tick={} task={:?} reason={:?} path={:?} know={know:?} origin=({:.0},{:.0},{:.0}) left={left:.0} events={events:?} decision={:?} weapon={weapon_act:?} think_us={} report={:?}",
                obs.self_state.id.0,
                obs.tick,
                self.task.kind,
                self.task.reason,
                self.intent.path,
                obs.self_state.origin[0],
                obs.self_state.origin[1],
                obs.self_state.origin[2],
                self.decision,
                started.elapsed().as_micros(),
                self.report,
            );
        }
    }

    /// The weapon channel has one owner. The skill commits to reloading or
    /// switching and keeps its request until the simulation confirms the
    /// outcome; movement and looking stay with the task.
    fn apply_weapon_skill(&mut self, obs: &BotObservation) {
        // An interaction already under way is not displaced by an empty magazine.
        // MW2 plants and defuses on the same key it reloads with: a reload
        // queued as the bot reaches the bomb would hold the interaction off
        // for as long as it lasts. Using the bomb comes first.
        if self.intent.use_button && self.weapon.wants_reload() {
            self.weapon.reset();
        }
        self.weapon.update(obs, !self.intent.use_button);
        self.intent.weapon = self.weapon.requested_weapon();
        self.intent.reload = self.weapon.wants_reload() && !self.intent.use_button;
        if self.intent.weapon.is_some() {
            self.intent.fire = false;
        }
        if self.intent.weapon.is_some() || self.intent.reload {
            self.intent.sprint = false;
        }
    }

    fn unstick(&mut self, obs: &BotObservation, world: &mut impl WorldQuery) -> Option<BotIntent> {
        let here = obs.self_state.origin;
        if self
            .progress_origin
            .is_none_or(|old| dist_xy(old, here) >= 16.0)
        {
            self.progress_origin = Some(here);
            self.progress_tick = obs.tick;
        }
        if let Some((goal, until)) = self.escape {
            if obs.tick < until && dist_xy(here, goal) > 8.0 {
                return Some(BotIntent {
                    look_at: Some([goal[0], goal[1], obs.eye()[2]]),
                    move_goal: Some(goal),
                    move_mode: MoveMode::Walk,
                    path: PathOutcome::Clear,
                    crouch: self.escape_crouch,
                    // Tap jump while pushing: a press every few ticks.
                    jump: self.escape_jump && obs.tick.is_multiple_of(6),
                    ..BotIntent::default()
                });
            }
            self.escape_crouch = false;
            self.escape_jump = false;
            self.escape = None;
            self.committed.clear();
            self.roam = None;
        }
        let moving = matches!(
            self.intent.move_mode,
            MoveMode::Walk | MoveMode::Drop | MoveMode::BreakGlass
        ) || matches!(self.intent.path, PathOutcome::Blocked | PathOutcome::Unreachable);
        if !moving || obs.tick.saturating_sub(self.progress_tick) < 40 {
            return None;
        }
        self.progress_tick = obs.tick;
        self.stuck_tries += 1;
        // A low gap the standing hull clears in the graph but the body does
        // not: duck and push on toward where it was going, every other try.
        // A slope or lip too steep to walk is jumped: the next try after that.
        if !self.stuck_tries.is_multiple_of(3)
            && let Some(goal) = self.intent.move_goal.filter(|g| dist_xy(*g, here) > 8.0)
        {
            let crouch = self.stuck_tries % 3 == 1;
            self.escape = Some((goal, obs.tick + ESCAPE_CROUCH_TICKS));
            self.escape_crouch = crouch;
            self.escape_jump = !crouch;
            return Some(BotIntent {
                look_at: Some([goal[0], goal[1], obs.eye()[2]]),
                move_goal: Some(goal),
                move_mode: MoveMode::Walk,
                path: PathOutcome::Clear,
                crouch,
                jump: !crouch,
                ..BotIntent::default()
            });
        }
        // Pmove can be blocked by other players even when world traces are clear.
        // Different clients yield in different directions before rejoining their route.
        let salt = obs
            .self_state
            .id
            .0
            .wrapping_mul(3)
            .wrapping_add(obs.tick / 40);
        for offset in 0..4 {
            let angle = ((salt + offset * 2) % 8) as f32 * core::f32::consts::FRAC_PI_4;
            let goal = [
                here[0] + angle.cos() * 64.0,
                here[1] + angle.sin() * 64.0,
                here[2],
            ];
            if world.walk_hull(here, goal) != Ok(WalkSample::Clear) {
                continue;
            }
            let Ok(floor) = world.hull_trace(
                [goal[0], goal[1], goal[2] + 2.0],
                [goal[0], goal[1], goal[2] - nav::STEP_Z_IN],
            ) else {
                continue;
            };
            if floor.startsolid || floor.fraction >= 1.0 {
                continue;
            }
            self.escape = Some((floor.endpos, obs.tick + 16));
            return Some(BotIntent {
                look_at: Some(floor.endpos),
                move_goal: Some(floor.endpos),
                move_mode: MoveMode::Walk,
                path: PathOutcome::Clear,
                ..BotIntent::default()
            });
        }
        None
    }

    fn pick_task(&mut self, obs: &BotObservation, objective: Option<ModeObjective>) -> Task {
        let seen = self.memory.currently_seen().next().is_some();
        let fresh = self
            .memory
            .last_seen()
            .any(|c| obs.tick.saturating_sub(c.seen_tick) <= LAST_SEEN_FRESH_TICKS);
        let here = obs.self_state.origin;
        let near_seen = self
            .memory
            .currently_seen()
            .map(|c| dist_xy(here, c.origin))
            .fold(f32::MAX, f32::min);
        // A teammate's callout is pushed only by the nearest teammate with a
        // pushing gun; everyone else holds an angle on it.
        let push_callout = |c: &&Contact| {
            c.confidence >= CALLOUT_CONFIDENCE_MAX
                || (Style::of(obs) == Style::Rush
                    && !obs
                        .teammates
                        .iter()
                        .any(|(_, at)| dist_xy(*at, c.origin) < dist_xy(here, c.origin)))
        };
        let near_fresh = self
            .memory
            .last_seen()
            .filter(|c| obs.tick.saturating_sub(c.seen_tick) <= LAST_SEEN_FRESH_TICKS)
            .filter(push_callout)
            .map(|c| dist_xy(here, c.origin))
            .fold(f32::MAX, f32::min);
        // A trade waits for the fight in front of the bot to be over.
        let trading = !seen
            && obs.trade.is_some_and(|(id, _)| !self.memory.currently_seen().any(|c| c.id == id));
        let lost_fight = self.lost_fight(obs);
        // The bomb carrier or defuser leaves sounds to the team.
        let actor = objective.is_some_and(|o| {
            o.role == TeamRole::Actor
                && matches!(o.action, ObjectiveAction::Plant | ObjectiveAction::Defuse)
        });
        // A lost or heard enemy close by is pushed; a sniper keeps its angle.
        let investigate = if trading && !actor {
            // Trade the teammate who just went down.
            95.0
        } else if lost_fight.is_some() && !actor && !seen {
            // Finish the fight it was in rather than wander off mid-way.
            FIGHT_MEMORY_SCORE
        } else if !fresh || near_fresh == f32::MAX {
            -1.0
        } else if actor {
            40.0
        } else if obs.methodical && near_fresh <= THREAT_HEARD_IN {
            if Style::of(obs) == Style::Overwatch { 50.0 } else { 85.0 }
        } else {
            55.0
        };
        let mut scores = [
            10.0,
            investigate,
            if seen {
                80.0 + self.aggression * 10.0
            } else {
                -1.0
            },
            if self.wounded(obs) {
                if seen { 60.0 } else { 95.0 }
            } else {
                -1.0
            },
            -1.0,
        ];
        // An empty gun, not the bomb in hand while planting.
        let gun_empty = obs.self_state.ammo_clip == 0
            && obs.held().is_some_and(|slot| slot.facts.clip_size > 0);
        if seen && gun_empty {
            scores[2] = 30.0;
            scores[3] = 100.0;
        }
        if seen && obs.tick < self.stalemate_until {
            scores[2] = 20.0;
            scores[1] = -1.0;
        }
        if let Some(obj) = objective {
            let route_length = if self.route_goal == Some(obj.origin) && !self.committed.is_empty()
            {
                let mut from = obs.self_state.origin;
                let mut length = 0.0;
                for point in &self.committed {
                    length += dist2(from, point.position).sqrt();
                    from = point.position;
                }
                Some(length + dist2(from, obj.origin).sqrt())
            } else {
                None
            };
            scores[4] = objective_utility(obs, obj, route_length);
            // Nearby enemies first; the bomb only when it cannot wait.
            let threat = if actor {
                near_seen <= THREAT_ACTOR_SEEN_IN
            } else {
                near_seen <= THREAT_SEEN_IN || near_fresh <= THREAT_HEARD_IN
            };
            // Once planting or defusing has started the actor keeps at it
            // unless an enemy is right there.
            let interacting = actor && obj.touching && near_seen > PLANT_FIGHT_IN;
            let urgent = interacting
                || obj.progress >= 0.5
                || (obj.action == ObjectiveAction::Defuse
                    && obj.remaining_ms.is_some_and(|ms| {
                        let needed = obj.interaction_ms as f32 * (1.0 - obj.progress.clamp(0.0, 1.0))
                            + dist_xy(here, obj.origin) / 180.0 * 1000.0;
                        (ms as f32) - needed < URGENT_SLACK_MS
                    }));
            if obs.methodical && threat && !urgent {
                scores[4] = scores[4].min(THREAT_OBJECTIVE_CAP);
            }
        }
        let kinds = [
            TaskKind::Hunt,
            TaskKind::Investigate,
            TaskKind::Fight,
            TaskKind::Recover,
            TaskKind::TouchObj,
        ];
        let mut best = 0;
        for i in 1..scores.len() {
            if scores[i] > scores[best] {
                best = i;
            }
        }
        let current = kinds.iter().position(|k| *k == self.task.kind).unwrap();
        if current != 0 && scores[current] >= 0.0 && scores[best] < scores[current] + SWITCH_MARGIN
        {
            best = current;
        }
        self.decision.scores = scores;
        Task {
            kind: kinds[best],
            reason: match kinds[best] {
                TaskKind::Hunt => {
                    if obs.objective.is_some() || !obs.objectives.is_empty() {
                        SwitchReason::PathFailed
                    } else {
                        SwitchReason::Spawned
                    }
                }
                TaskKind::Investigate => SwitchReason::LostSight,
                TaskKind::Fight => SwitchReason::SawEnemy,
                TaskKind::Recover => {
                    if self.wounded(obs) {
                        SwitchReason::LowHealth
                    } else {
                        SwitchReason::ClipEmpty
                    }
                }
                TaskKind::TouchObj => SwitchReason::ObjectiveReachable,
            },
        }
    }

    fn intent_for(
        &mut self,
        obs: &BotObservation,
        world: &mut impl WorldQuery,
        nav: Option<&NavGraph>,
        astar_budget: &mut u32,
        objective: Option<ModeObjective>,
    ) -> BotIntent {
        match self.task.kind {
            TaskKind::Fight => self.fight(obs, world, nav, astar_budget),
            TaskKind::Investigate => self.seek(obs, world, nav, astar_budget, SeekKind::LastSeen),
            TaskKind::TouchObj => self.seek(
                obs,
                world,
                nav,
                astar_budget,
                SeekKind::Objective(objective),
            ),
            TaskKind::Recover => self.seek(obs, world, nav, astar_budget, SeekKind::BackOff),
            TaskKind::Hunt => self.hunt(obs, world, nav, astar_budget),
        }
    }

    fn hunt(
        &mut self,
        obs: &BotObservation,
        world: &mut impl WorldQuery,
        nav: Option<&NavGraph>,
        astar_budget: &mut u32,
    ) -> BotIntent {
        let Some(goal) = self.ensure_roam(obs, nav) else {
            return BotIntent::default();
        };
        let (move_mode, path, move_goal) =
            self.route(obs.self_state.origin, goal, world, nav, astar_budget);
        // Incomplete coverage is not a wall: pick another roam destination
        // instead of holding in front of the truncated region forever.
        if matches!(
            path,
            PathOutcome::Blocked | PathOutcome::Unreachable | PathOutcome::IncompleteGraph
        ) {
            self.roam = None;
            self.committed.clear();
        }
        BotIntent {
            look_at: Some([goal[0], goal[1], obs.eye()[2]]),
            move_goal,
            desired_range: None,
            move_mode,
            path,
            ..BotIntent::default()
        }
    }

    fn ensure_roam(&mut self, obs: &BotObservation, nav: Option<&NavGraph>) -> Option<[f32; 3]> {
        if let Some(goal) = self.roam
            && dist2(obs.self_state.origin, goal) > 64.0 * 64.0
        {
            return Some(goal);
        }
        if self.roam.is_some() {
            self.committed.clear();
        }
        self.roam_gen = self.roam_gen.saturating_add(1);
        let salt = obs.self_state.id.0 ^ self.roam_gen;
        let next = if let Some(graph) = nav.filter(|graph| !graph.is_empty()) {
            nav::roam_node(graph, obs.self_state.origin, salt)
        } else {
            let ang = (salt as f32) * 0.37;
            Some([
                obs.self_state.origin[0] + 160.0 * ang.cos(),
                obs.self_state.origin[1] + 160.0 * ang.sin(),
                obs.self_state.origin[2],
            ])
        };
        self.roam = next;
        next
    }

    fn fight(
        &mut self,
        obs: &BotObservation,
        world: &mut impl WorldQuery,
        nav: Option<&NavGraph>,
        astar_budget: &mut u32,
    ) -> BotIntent {
        let Some(target) = self.pick_fight_target(obs) else {
            return BotIntent::default();
        };
        if self.last_target != Some(target.id) {
            self.last_target = Some(target.id);
            // Quick up close, slower to pick out a distant figure, unless
            // looking through a sniper scope.
            let range = dist_xy(obs.self_state.origin, target.origin);
            let sniper = obs.self_state.weapon_class == WeaponClass::Sniper;
            let base = if range < 800.0 {
                12.0
            } else if range < 2000.0 || sniper {
                13.5
            } else {
                16.0
            };
            let casual = if obs.casual { CASUAL_REACTION_TICKS } else { 0.0 };
            // Already aiming where they appeared: no turn to make first.
            let bearing = (target.origin[1] - obs.self_state.origin[1])
                .atan2(target.origin[0] - obs.self_state.origin[0]);
            let off = crate::look::angle(obs.self_state.viewangles[1].to_radians(), bearing).abs();
            let placed = if off <= PREAIM_REACTION_DEG.to_radians() { PREAIM_REACTION_TICKS } else { 0.0 };
            let reaction = (base + casual - placed + 2.0 * (1.0 - self.aggression)).round() as u32;
            self.reaction_until = obs.tick.saturating_add(reaction);
        }
        let aim_at = [target.origin[0], target.origin[1], target.origin[2] + 48.0];
        if self.wounded(obs) {
            return self.fight_break(obs, world, nav, astar_budget, target);
        }
        // Perception can see a head over cover while the chest is occluded.
        // Aim only at a point validated by the weapon trace, not sight alone.
        world.enter(QuerySubsystem::Tactical);
        let mut aim_at = aim_at;
        let mut shot = world.shot_ray(obs.eye(), aim_at, obs.self_state.id);
        if matches!(
            shot,
            Ok(SightSample::Blocked {
                obstacle: crate::query::ObstacleKind::Glass,
                ..
            })
        ) {
            // Shooting out a window for a long shot across the map is not
            // worth it; watch them through it instead.
            if dist_xy(obs.self_state.origin, target.origin) > FIGHT_GLASS_MAX_IN {
                return BotIntent {
                    look_at: Some(aim_at),
                    move_goal: Some(obs.self_state.origin),
                    move_mode: MoveMode::Hold,
                    path: PathOutcome::Clear,
                    ..BotIntent::default()
                };
            }
            return BotIntent {
                look_at: Some(aim_at),
                move_goal: Some(target.origin),
                move_mode: MoveMode::BreakGlass,
                path: PathOutcome::Clear,
                ..BotIntent::default()
            };
        }
        if definitely_obstructed(shot, target.id) {
            for height in [64.0, 8.0] {
                let candidate = [
                    target.origin[0],
                    target.origin[1],
                    target.origin[2] + height,
                ];
                shot = world.shot_ray(obs.eye(), candidate, obs.self_state.id);
                if shot_allowed(shot, target.id) {
                    aim_at = candidate;
                    break;
                }
                if !definitely_obstructed(shot, target.id) {
                    break;
                }
            }
        }
        let range = dist2(obs.self_state.origin, target.origin).sqrt();
        // A shotgun past its reach only scatters pellets: close in instead
        // (or the weapon skill draws the pistol for a far target).
        let in_reach = !(obs.self_state.weapon_class == WeaponClass::Shotgun
            && range > SHOTGUN_FIRE_IN);
        let fire = self.authorize_fire(
            obs.tick >= self.reaction_until
                && obs.self_state.ammo_clip != 0
                && in_reach
                && fresh_contact(obs, target)
                && shot_allowed(shot, target.id),
            target,
        );
        let hold = self.hold_range(obs);
        let (move_mode, path, move_goal) = self.fight_position(
            obs,
            world,
            nav,
            astar_budget,
            aim_at,
            target,
            shot,
            range <= hold,
        );
        if matches!(
            path,
            PathOutcome::Unreachable | PathOutcome::Blocked | PathOutcome::IncompleteGraph
        ) && !shot_allowed(shot, target.id)
        {
            if let Some(obj) = choose_objective(
                obs,
                world,
                nav,
                astar_budget,
                &mut self.committed,
                &mut self.route_goal,
                &mut self.route_work,
                &mut self.objective_held,
            ) {
                self.task = Task {
                    kind: TaskKind::TouchObj,
                    reason: SwitchReason::PathFailed,
                };
                self.decision.objective = Some(obj);
                return self.seek(
                    obs,
                    world,
                    nav,
                    astar_budget,
                    SeekKind::Objective(Some(obj)),
                );
            }
            self.task = Task {
                kind: TaskKind::Hunt,
                reason: SwitchReason::PathFailed,
            };
            return self.hunt(obs, world, nav, astar_budget);
        }
        BotIntent {
            look_at: Some(aim_at),
            move_goal,
            desired_range: Some(hold),
            move_mode,
            fire,
            path,
            ..BotIntent::default()
        }
    }

    fn pick_fight_target(&self, obs: &BotObservation) -> Option<Contact> {
        let here = obs.self_state.origin;
        let range = |c: &Contact| dist_xy(here, c.origin);
        let closest = self
            .memory
            .currently_seen()
            .min_by(|a, b| range(a).total_cmp(&range(b)))
            .copied()?;
        // Finish the fight in hand: only a much closer enemy pulls the aim
        // off the one being shot at.
        if let Some(current) = self
            .last_target
            .and_then(|id| self.memory.currently_seen().find(|c| c.id == id))
            .copied()
        {
            let (now, near) = (range(&current), range(&closest));
            if closest.id == current.id
                || near > now * TARGET_SWITCH_SHARE
                || now - near < TARGET_SWITCH_IN
            {
                return Some(current);
            }
        }
        Some(closest)
    }

    fn wounded(&self, obs: &BotObservation) -> bool {
        obs.self_state.health > 0
            && (obs.self_state.health as f32) <= 20.0 + 20.0 * self.self_preserve
    }

    fn hold_range(&self, obs: &BotObservation) -> f32 {
        if obs.methodical {
            // Recorded players hold an angle and win fights at range with
            // rifles, LMGs and snipers, and close in with SMGs and shotguns.
            let quick = obs.held().is_some_and(|slot| slot.facts.quick_select);
            let base = if quick {
                650.0
            } else {
                match obs.self_state.weapon_class {
                    WeaponClass::Sniper => 4000.0,
                    WeaponClass::Assault | WeaponClass::Lmg => 1800.0,
                    WeaponClass::Smg => 700.0,
                    WeaponClass::Shotgun => 300.0,
                }
            };
            return (base * (1.0 - 0.4 * self.aggression)).max(150.0);
        }
        (obs.self_state.weapon_class.fight_hold() - 40.0 * self.aggression).max(48.0)
    }

    fn fight_break(
        &mut self,
        obs: &BotObservation,
        world: &mut impl WorldQuery,
        nav: Option<&NavGraph>,
        astar_budget: &mut u32,
        target: Contact,
    ) -> BotIntent {
        let aim_at = [target.origin[0], target.origin[1], target.origin[2] + 48.0];
        let away = away_from(obs.self_state.origin, target.origin, 192.0);
        world.enter(QuerySubsystem::Tactical);
        let fire = self.authorize_fire(
            self.aggression > self.self_preserve
                && obs.tick >= self.reaction_until
                && obs.self_state.ammo_clip != 0
                && fresh_contact(obs, target)
                && shot_allowed(
                    world.shot_ray(obs.eye(), aim_at, obs.self_state.id),
                    target.id,
                ),
            target,
        );
        let (move_mode, path, move_goal) =
            self.route(obs.self_state.origin, away, world, nav, astar_budget);
        BotIntent {
            look_at: Some(aim_at),
            move_goal,
            desired_range: None,
            move_mode,
            fire,
            path,
            ..BotIntent::default()
        }
    }

    /// Where to stand for this engagement. Both the reposition inside weapon
    /// range and the approach from outside it end at the same committed support:
    /// the bot moves to a place it can shoot from, never to the enemy's own
    /// coordinates, which change with every observation.
    #[allow(clippy::too_many_arguments)]
    fn fight_position(
        &mut self,
        obs: &BotObservation,
        world: &mut impl WorldQuery,
        nav: Option<&NavGraph>,
        astar_budget: &mut u32,
        aim_at: [f32; 3],
        contact: Contact,
        shot: QueryResult<SightSample>,
        in_hold: bool,
    ) -> (MoveMode, PathOutcome, Option<[f32; 3]>) {
        let here = obs.self_state.origin;
        let target = contact.origin;
        let target_id = contact.id;
        world.enter(QuerySubsystem::Tactical);
        if shot.is_err() {
            return (MoveMode::Hold, PathOutcome::BudgetExhausted, Some(here));
        }
        // Already in range with a line of fire: this is the position.
        if in_hold && shot_allowed(shot, target_id) {
            return (MoveMode::Hold, PathOutcome::Clear, Some(here));
        }
        let hold = self.hold_range(obs);
        // The engagement, not the two exact coordinates: a threat that shifts a
        // little leaves the committed position standing. The bot's own position
        // is deliberately absent — walking to the position it chose must not
        // retire the choice.
        let key = FightScanKey {
            target: target_id,
            hold_in: hold as i32,
        };
        let mut scan = match self.fight_scan.take() {
            Some(scan) if scan.answers(key, target, obs.tick) => scan,
            _ => FightScan::begin(
                key,
                target,
                obs.tick,
                // Look for a firing spot no farther out than the bot already is.
                self.firing_candidates(obs, nav, target, hold.min(dist_xy(here, target)).max(120.0)),
            ),
        };
        // A committed position is walked to through the ordinary route executor,
        // which may take a detour; aim keeps following observations meanwhile.
        if let Some(pos) = scan.chosen {
            // Standing on the chosen support without a shot means it did not
            // deliver what it was chosen for; an unreachable one is no better.
            // Either way the next candidate is taken, never the same one again.
            let arrived = dist2(here, pos) <= WAYPOINT_IN * WAYPOINT_IN;
            let moved = (!arrived).then(|| self.route(here, pos, world, nav, astar_budget));
            let usable = moved.is_some_and(|moved| {
                !matches!(
                    moved.1,
                    PathOutcome::Unreachable | PathOutcome::IncompleteGraph
                )
            });
            if let Some(moved) = moved.filter(|_| usable) {
                scan.tick = obs.tick;
                self.fight_scan = Some(scan);
                return moved;
            }
            scan.commit_next(here);
            self.route_goal = None;
            self.route_pending = None;
            self.committed.clear();
            if let Some(pos) = scan.chosen {
                let moved = self.route(here, pos, world, nav, astar_budget);
                scan.tick = obs.tick;
                self.fight_scan = Some(scan);
                return moved;
            }
        }
        let mut denied = false;
        let opened = scan.cursor;
        while (scan.cursor as usize) < scan.candidates.len() {
            let cand = scan.candidates[scan.cursor as usize];
            let eye = [
                cand[0],
                cand[1],
                cand[2]
                    + if obs.self_state.view_height > 1.0 {
                        obs.self_state.view_height
                    } else {
                        60.0
                    },
            ];
            let shot = world.shot_ray(eye, aim_at, obs.self_state.id);
            if shot.is_err() {
                denied = true;
                break;
            }
            if !shot_allowed(shot, target_id) {
                scan.cursor += 1;
                continue;
            }
            let threat_eye = [target[0], target[1], target[2] + 60.0];
            let chest = [cand[0], cand[1], cand[2] + 48.0];
            let return_fire = world.shot_ray(threat_eye, chest, target_id);
            if return_fire.is_err() {
                denied = true;
                break;
            }
            let exposed = return_fire_clear(return_fire, obs.self_state.id);
            // Travel and exposure in one comparable scale; reachability is left
            // to the route, so a position behind a corner is not thrown away.
            let score = dist2(here, cand).sqrt() + if exposed { 320.0 } else { 0.0 };
            scan.accepted.push((score, cand));
            scan.cursor += 1;
        }
        if !denied && scan.cursor as usize >= scan.candidates.len() {
            scan.commit_next(here);
        }
        let chosen = scan.chosen;
        let anchor = scan.anchor;
        // A visit that neither advanced the scan nor holds a position is not
        // progress, so it must not keep an exhausted scan alive: letting it age
        // out is what allows the engagement to be reconsidered.
        if scan.cursor != opened || chosen.is_some() {
            scan.tick = obs.tick;
        }
        self.fight_scan = Some(scan);
        let Some(pos) = chosen else {
            let path = if denied {
                PathOutcome::BudgetExhausted
            } else {
                PathOutcome::Blocked
            };
            // A visible enemy is not necessarily shootable. In range, let the
            // caller resume another goal when every firing position failed; out
            // of range, close on the engagement's anchor — a destination that
            // does not move with every observation and invalidate its own search.
            if in_hold || denied {
                return (MoveMode::Hold, path, Some(here));
            }
            return self.route(here, anchor, world, nav, astar_budget);
        };
        self.route(here, pos, world, nav, astar_budget)
    }

    /// The bounded candidate set a scan is frozen on. Supports come from the
    /// baked graph; the ring is only for a runtime that has no graph at all,
    /// where there is no support to choose from.
    fn firing_candidates(
        &self,
        obs: &BotObservation,
        nav: Option<&NavGraph>,
        target: [f32; 3],
        hold: f32,
    ) -> Vec<[f32; 3]> {
        let here = obs.self_state.origin;
        if let Some(graph) = nav.filter(|graph| !graph.is_empty()) {
            let supports = nav::firing_supports(graph, here, target, hold, FIGHT_CANDIDATES);
            if !supports.is_empty() {
                return supports;
            }
        }
        (0..FIGHT_RING)
            .map(|step| {
                let ang = (step as f32) * (core::f32::consts::TAU / FIGHT_RING as f32);
                [
                    target[0] + hold * ang.cos(),
                    target[1] + hold * ang.sin(),
                    here[2],
                ]
            })
            .collect()
    }

    fn seek(
        &mut self,
        obs: &BotObservation,
        world: &mut impl WorldQuery,
        nav: Option<&NavGraph>,
        astar_budget: &mut u32,
        kind: SeekKind,
    ) -> BotIntent {
        let (goal, use_button, look) = match kind {
            SeekKind::LastSeen => {
                let trade = obs.trade.map(|(id, _)| id);
                let fought = self.lost_fight(obs).map(|c| c.id);
                let Some(last) = self
                    .memory
                    .last_seen()
                    .filter(|c| {
                        Some(c.id) == trade
                            || Some(c.id) == fought
                            || c.confidence >= CALLOUT_CONFIDENCE_MAX
                    })
                    .max_by_key(|c| (Some(c.id) == trade, Some(c.id) == fought, c.seen_tick))
                    .or_else(|| self.memory.last_seen().max_by_key(|c| c.seen_tick))
                    .copied()
                else {
                    return BotIntent::default();
                };
                // Just lost the enemy it was shooting: hold the angle where
                // they went, sights up, for the re-peek, instead of running
                // after them straight away.
                if Some(last.id) == fought
                    && obs.tick.saturating_sub(self.fought_at) <= FIGHT_PREAIM_TICKS
                {
                    let here = obs.self_state.origin;
                    return BotIntent {
                        look_at: Some([last.origin[0], last.origin[1], last.origin[2] + 52.0]),
                        move_goal: Some(here),
                        move_mode: MoveMode::Hold,
                        path: PathOutcome::Clear,
                        ads: obs.methodical,
                        ..BotIntent::default()
                    };
                }
                // A defender before the plant lets the attackers come to it:
                // it holds where it stands, sights on where the enemy was,
                // and only closes on one that is near or already hurt.
                let here = obs.self_state.origin;
                if holds_site(obs)
                    && dist_xy(here, last.origin) > DEFENDER_CHASE_IN
                    && !obs.hurt.contains(&last.id)
                {
                    return BotIntent {
                        look_at: Some([last.origin[0], last.origin[1], last.origin[2] + 52.0]),
                        move_goal: Some(here),
                        move_mode: MoveMode::Hold,
                        path: PathOutcome::Clear,
                        ads: true,
                        ..BotIntent::default()
                    };
                }
                let goal = self.flank_point(obs, nav, last);
                (goal, false, [last.origin[0], last.origin[1], last.origin[2] + 48.0])
            }
            SeekKind::Objective(obj) => {
                let Some(obj) = obj else {
                    return BotIntent::default();
                };
                if obj.touching && obj.use_button {
                    return BotIntent {
                        look_at: Some(obj.origin),
                        move_goal: Some(obs.self_state.origin),
                        move_mode: MoveMode::Hold,
                        path: PathOutcome::Clear,
                        use_button: true,
                        ..BotIntent::default()
                    };
                }
                if obj.action == ObjectiveAction::Defend
                    && obs.methodical
                    && let Some(graph) = nav.filter(|graph| !graph.is_empty())
                {
                    let here = obs.self_state.origin;
                    if let Some(spot) = self.guard_spot(obs, world, graph, obj) {
                        if dist_xy(here, spot) <= WAYPOINT_IN * 2.0 {
                            self.guard_shift(obs);
                            // Watch the site, holding one angle beside it per spot
                            // and turning to the next only when the spot changes.
                            // The way attackers come in, else the site itself.
                            let watch = obs.enemy_approach.unwrap_or(obj.origin);
                            let toward = (watch[1] - here[1]).atan2(watch[0] - here[0]);
                            let shifts = self.guard.as_ref().map_or(0, |scan| scan.shifts);
                            let sway = (pace_roll(self.pace_seed, shifts, 8) as f32 / 100.0 - 0.5) * 0.9;
                            let yaw = toward + sway;
                            return BotIntent {
                                look_at: Some([
                                    here[0] + yaw.cos() * 512.0,
                                    here[1] + yaw.sin() * 512.0,
                                    obs.eye()[2],
                                ]),
                                move_goal: Some(here),
                                move_mode: MoveMode::Hold,
                                path: PathOutcome::Clear,
                                ..BotIntent::default()
                            };
                        }
                        let mut leg = self.detour_leg(obs, graph, spot, true);
                        let (mut move_mode, mut path, mut move_goal) =
                            self.route(here, leg, world, nav, astar_budget);
                        if path == PathOutcome::Unreachable && dist_xy(leg, spot) > 1.0 {
                            if let Some(d) = self.detour.as_mut() {
                                d.via = None;
                            }
                            leg = spot;
                            (move_mode, path, move_goal) =
                                self.route(here, leg, world, nav, astar_budget);
                        }
                        return BotIntent {
                            look_at: Some([leg[0], leg[1], obs.eye()[2]]),
                            move_goal,
                            move_mode,
                            path,
                            ..BotIntent::default()
                        };
                    }
                }
                // A sniper trails the bomb and watches the way it is going.
                if obj.role == TeamRole::Cover
                    && obs.methodical
                    && Style::of(obs) == Style::Overwatch
                    && dist_xy(obs.self_state.origin, obj.origin) <= OVERWATCH_TRAIL_IN
                {
                    let here = obs.self_state.origin;
                    let watch = obs
                        .sites
                        .iter()
                        .copied()
                        .min_by(|a, b| dist_xy(*a, obj.origin).total_cmp(&dist_xy(*b, obj.origin)))
                        .unwrap_or(obj.origin);
                    return BotIntent {
                        look_at: Some([watch[0], watch[1], obs.eye()[2]]),
                        move_goal: Some(here),
                        move_mode: MoveMode::Hold,
                        path: PathOutcome::Clear,
                        ..BotIntent::default()
                    };
                }
                // Teammates heading for the same place fan out around it.
                let obj = match nav.filter(|graph| !graph.is_empty()) {
                    Some(graph) if obs.methodical && obj.spread && obs.squad.1 > 1 => {
                        let angle = obs.squad.0 as f32 * 2.399_963;
                        let radius = if obj.role == TeamRole::Cover {
                            SPREAD_IN * 1.25
                        } else {
                            SPREAD_IN * 1.4
                        };
                        let want = [
                            obj.origin[0] + angle.cos() * radius,
                            obj.origin[1] + angle.sin() * radius,
                            obj.origin[2],
                        ];
                        match nav::nearest_within(graph, want, SPREAD_IN) {
                            Some(node) => ModeObjective {
                                origin: graph.nodes[node as usize],
                                ..obj
                            },
                            None => obj,
                        }
                    }
                    _ => obj,
                };
                if obj.action == ObjectiveAction::Defend
                    || (obj.role == TeamRole::Cover && obj.active_user.is_some())
                {
                    // Scan separate sectors while guarding a site or an active teammate.
                    let angle = ((obs.self_state.id.0 + obs.tick / 80) % 8) as f32
                        * std::f32::consts::TAU
                        / 8.0;
                    let guard = [
                        obj.origin[0] + angle.cos() * 144.0,
                        obj.origin[1] + angle.sin() * 144.0,
                        obj.origin[2],
                    ];
                    if dist_xy(obs.self_state.origin, obj.origin) <= 220.0
                        && (obs.self_state.origin[2] - obj.origin[2]).abs()
                            <= nav::STEP_Z_IN * 2.0 + 8.0
                    {
                        return BotIntent {
                            look_at: Some([guard[0], guard[1], guard[2] + 60.0]),
                            path: PathOutcome::Clear,
                            ..BotIntent::default()
                        };
                    }
                }
                let goal = match nav.filter(|graph| !graph.is_empty()) {
                    // Escorts keep a lane beside the carrier; the carrier and a
                    // push with no carrier take a corridor of their own choosing.
                    Some(graph)
                        if obs.methodical
                            && (obj.spread || obj.action == ObjectiveAction::Plant) =>
                    {
                        self.detour_leg(obs, graph, obj.origin, obj.role != TeamRole::Cover)
                    }
                    _ => obj.origin,
                };
                (goal, false, [goal[0], goal[1], obs.eye()[2]])
            }
            SeekKind::BackOff => {
                let threat = self
                    .memory
                    .last_seen()
                    .next()
                    .or_else(|| self.memory.currently_seen().next())
                    .map(|c| c.origin)
                    .unwrap_or([
                        obs.self_state.origin[0] + 64.0,
                        obs.self_state.origin[1],
                        obs.self_state.origin[2],
                    ]);
                let away = away_from(obs.self_state.origin, threat, 192.0);
                (away, false, threat)
            }
        };
        let (mut move_mode, mut path, mut move_goal) =
            self.route(obs.self_state.origin, goal, world, nav, astar_budget);
        // A side point that cannot be reached from here (a one-way drop on
        // the way, say) is given up for the goal itself.
        if path == PathOutcome::Unreachable {
            let detoured = self
                .detour
                .as_mut()
                .filter(|d| d.via.is_some_and(|via| dist_xy(via, goal) < 1.0))
                .map(|d| {
                    d.via = None;
                    d.goal
                });
            let flanked = self
                .flank
                .as_mut()
                .filter(|f| !f.2 && dist_xy(f.1, goal) < 1.0)
                .map(|f| {
                    f.2 = true;
                    goal
                });
            let retry = detoured.or(flanked.and_then(|_| {
                self.memory.last_seen().max_by_key(|c| c.seen_tick).map(|c| c.origin)
            }));
            if let Some(target) = retry {
                (move_mode, path, move_goal) =
                    self.route(obs.self_state.origin, target, world, nav, astar_budget);
            }
        }
        BotIntent {
            look_at: Some(look),
            move_goal,
            desired_range: None,
            move_mode,
            use_button,
            crouch: matches!(kind, SeekKind::BackOff),
            path,
            ..BotIntent::default()
        }
    }

    fn route(
        &mut self,
        from: [f32; 3],
        to: [f32; 3],
        world: &mut impl WorldQuery,
        nav: Option<&NavGraph>,
        astar_budget: &mut u32,
    ) -> (MoveMode, PathOutcome, Option<[f32; 3]>) {
        if let Some(graph) = nav.filter(|g| !g.is_empty()) {
            return self.route_nav(from, to, world, graph, astar_budget);
        }
        walk_step(from, to, world)
    }

    fn route_nav(
        &mut self,
        from: [f32; 3],
        to: [f32; 3],
        world: &mut impl WorldQuery,
        graph: &NavGraph,
        astar_budget: &mut u32,
    ) -> (MoveMode, PathOutcome, Option<[f32; 3]>) {
        // A moving goal (a teammate carrying the bomb) is only re-planned for
        // once it has drifted a fair share of the way off: re-planning on every
        // step would restart the search before it ever finishes.
        let drift = (dist_xy(from, to) * ROUTE_DRIFT_SHARE).clamp(WAYPOINT_IN, ROUTE_DRIFT_MAX_IN);
        let need = self.committed.is_empty()
            || self.route_goal.is_none_or(|g| dist2(g, to) > drift * drift);
        // The old path is only worth walking while it heads roughly the same way.
        if need
            && self
                .route_goal
                .is_some_and(|g| dist_xy(g, to) > (drift * 2.0).max(ROUTE_STALE_IN))
        {
            self.committed.clear();
        }
        if need {
            // The search is also keyed on where it started; a bot walking on
            // meanwhile keeps that start while it is still close by.
            let (from, to) = match self.route_pending {
                Some((start, pending))
                    if dist2(pending, to) <= drift * drift
                        && dist2(start, from) <= ROUTE_START_PIN_IN * ROUTE_START_PIN_IN =>
                {
                    (start, pending)
                }
                _ => (from, to),
            };
            self.route_pending = Some((from, to));
            let objective = self.decision.objective.filter(|o| o.origin == to);
            match nav::find_route_resumable(
                world,
                graph,
                from,
                to,
                objective,
                astar_budget,
                &mut self.route_work,
            ) {
                Ok(path) => {
                    self.committed = path;
                    self.route_goal = Some(to);
                    self.route_pending = None;
                }
                Err(PathError::IncompleteGraph) => {
                    self.route_pending = None;
                    return (MoveMode::Hold, PathOutcome::IncompleteGraph, Some(to));
                }
                Err(PathError::BudgetExhausted { .. }) => {
                    // While the new plan waits for its share of the search, keep
                    // walking: the old path if there is one, else straight at
                    // the goal when that is open ground.
                    if self.committed.is_empty() {
                        return match world.walk_hull(from, to) {
                            Ok(WalkSample::Clear) => {
                                (MoveMode::Walk, PathOutcome::BudgetExhausted, Some(to))
                            }
                            _ => (MoveMode::Hold, PathOutcome::BudgetExhausted, Some(to)),
                        };
                    }
                }
                Err(
                    PathError::GraphChanged
                    | PathError::Unreachable
                    | PathError::EmptyGraph
                    | PathError::NoStartSupport
                    | PathError::NoGoalSupport,
                ) => {
                    self.committed.clear();
                    self.route_pending = None;
                    return (MoveMode::Hold, PathOutcome::Unreachable, Some(to));
                }
            }
        }
        world.enter(QuerySubsystem::Execution);
        while let Some(step) = self.committed.first().copied() {
            let tolerance = if self.committed.len() == 1 && self.decision.objective.is_some() {
                4.0
            } else {
                WAYPOINT_IN
            };
            if dist2(from, step.position) > tolerance * tolerance {
                break;
            }
            if let Some(next) = self.committed.get(1) {
                // A special transition starts at its entry; do not cut its corner.
                if next.kind != nav::TraversalKind::Walk && dist2(from, next.entry) > 4.0 * 4.0 {
                    break;
                }
                if next.kind == nav::TraversalKind::Walk
                    && world.walk_hull(from, next.position) != Ok(WalkSample::Clear)
                    && dist_xy(from, step.position) > 4.0
                {
                    break;
                }
            }
            self.committed.remove(0);
        }
        let Some(step) = self.committed.first().copied() else {
            return (MoveMode::Hold, PathOutcome::Clear, Some(from));
        };
        let sample = if step.kind == nav::TraversalKind::Drop {
            nav::drop_clear(world, from, step.position)
        } else {
            world.walk_hull(from, step.position)
        };
        match sample {
            Ok(WalkSample::Clear) => (
                if step.kind == nav::TraversalKind::Drop {
                    MoveMode::Drop
                } else {
                    MoveMode::Walk
                },
                PathOutcome::Clear,
                Some(step.position),
            ),
            Ok(WalkSample::BreakGlass) => (
                MoveMode::BreakGlass,
                PathOutcome::Clear,
                Some(step.position),
            ),
            Ok(WalkSample::Blocked) => (MoveMode::Hold, PathOutcome::Blocked, Some(step.position)),
            // A denied check is not a wall: the committed step was walkable when
            // the route was validated, so keep going and look again next tick.
            Err(_) => (
                if step.kind == nav::TraversalKind::Drop {
                    MoveMode::Drop
                } else {
                    MoveMode::Walk
                },
                PathOutcome::BudgetExhausted,
                Some(step.position),
            ),
        }
    }
}

/// The engagement a firing position was chosen for: a different threat or a
/// different weapon band retires the choice. The bot's own movement towards the
/// position does not, and neither does a small move by the threat — that is the
/// `anchor` distance below, not a grid cell, so no boundary makes one step
/// count as a new engagement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FightScanKey {
    target: ClientId,
    hold_in: i32,
}

/// One bounded evaluation of firing positions and the position it committed to.
/// The candidate list is frozen when the scan begins, so a denied slice resumes
/// at the same candidate instead of re-deriving the set.
#[derive(Clone, Debug, PartialEq)]
struct FightScan {
    key: FightScanKey,
    /// Where the threat was when this set was derived.
    anchor: [f32; 3],
    tick: u32,
    cursor: u32,
    candidates: Vec<[f32; 3]>,
    /// Candidates that passed their shot and exposure checks, with their score.
    /// A position that is taken or rejected leaves this list, so the next choice
    /// costs no queries and the same one is never re-picked.
    accepted: Vec<(f32, [f32; 3])>,
    chosen: Option<[f32; 3]>,
}

impl FightScan {
    fn begin(key: FightScanKey, anchor: [f32; 3], tick: u32, candidates: Vec<[f32; 3]>) -> Self {
        Self {
            key,
            anchor,
            tick,
            cursor: 0,
            candidates,
            accepted: Vec::new(),
            chosen: None,
        }
    }

    /// Whether this scan still answers the engagement in front of the bot.
    fn answers(&self, key: FightScanKey, at: [f32; 3], tick: u32) -> bool {
        self.key == key
            && dist2(self.anchor, at) <= FIGHT_COMMIT_MOVE * FIGHT_COMMIT_MOVE
            && !self.expired(tick)
    }

    /// Commits to the cheapest remaining accepted position the bot is not
    /// already standing on — one that close has just failed its own shot test
    /// from here, and the motor could not travel to it anyway.
    fn commit_next(&mut self, here: [f32; 3]) -> Option<[f32; 3]> {
        self.chosen = None;
        loop {
            let at = self
                .accepted
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.0.total_cmp(&b.1.0))
                .map(|(index, _)| index)?;
            let pos = self.accepted.remove(at).1;
            if dist2(here, pos) > WAYPOINT_IN * WAYPOINT_IN {
                self.chosen = Some(pos);
                return self.chosen;
            }
        }
    }

    /// An unfinished scan is short-lived; a commitment lives as long as the
    /// engagement it was made for, and no longer.
    fn expired(&self, tick: u32) -> bool {
        let age = tick.saturating_sub(self.tick);
        if self.chosen.is_some() {
            age > FIGHT_COMMIT_MAX_AGE_TICKS
        } else {
            age > FIGHT_SCAN_MAX_AGE_TICKS
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum SeekKind {
    LastSeen,
    Objective(Option<ModeObjective>),
    BackOff,
}

/// A defender guarding a site before the bomb is down.
fn holds_site(obs: &BotObservation) -> bool {
    obs.methodical
        && !obs.planted
        && obs.objective.is_some_and(|o| o.action == ObjectiveAction::Defend)
}

fn objective_utility(obs: &BotObservation, obj: ModeObjective, route_length: Option<f32>) -> f32 {
    let travel_ms = route_length.unwrap_or_else(|| dist2(obs.self_state.origin, obj.origin).sqrt())
        / 180.0
        * 1000.0;
    let cost = (travel_ms / 1000.0).min(25.0);
    let interaction_left = obj.interaction_ms as f32 * (1.0 - obj.progress.clamp(0.0, 1.0));
    if obj.action == ObjectiveAction::Defuse
        && obj.role == TeamRole::Actor
        && obj.remaining_ms.is_some_and(|ms| {
            (ms as f32) < interaction_left + if obj.touching { 0.0 } else { travel_ms }
        })
    {
        return -1.0;
    }
    let mut value = match obj.action {
        ObjectiveAction::Capture => 40.0 - cost,
        ObjectiveAction::Plant => 70.0 - cost,
        ObjectiveAction::Defend => 55.0 - cost,
        ObjectiveAction::Defuse => {
            let needed =
                travel_ms + obj.interaction_ms as f32 * (1.0 - obj.progress.clamp(0.0, 1.0));
            let slack = obj.remaining_ms.unwrap_or(u32::MAX) as f32 - needed;
            if slack < 0.0 {
                20.0
            } else {
                110.0 + (30.0 - slack / 1000.0).clamp(0.0, 30.0)
            }
        }
    };
    if obj.role == TeamRole::Cover {
        value = value.min(62.0);
    }
    if obj.touching && obj.use_button {
        value = 200.0;
    }
    value
}

#[allow(clippy::too_many_arguments)]
fn choose_objective(
    obs: &BotObservation,
    world: &mut impl WorldQuery,
    nav: Option<&NavGraph>,
    astar_budget: &mut u32,
    committed: &mut Vec<nav::RouteStep>,
    route_goal: &mut Option<[f32; 3]>,
    route_work: &mut nav::RouteWork,
    held: &mut Option<[f32; 3]>,
) -> Option<ModeObjective> {
    let mut candidates = obs.objectives.clone();
    if candidates.is_empty()
        && let Some(obj) = obs.objective
    {
        candidates.push(obj);
    }
    candidates.sort_by(|a, b| {
        let score = |o: &ModeObjective| {
            objective_utility(obs, *o, None)
                + if *route_goal == Some(o.origin) {
                    6.0
                } else {
                    0.0
                }
        };
        score(b).total_cmp(&score(a)).then_with(|| a.id.cmp(&b.id))
    });
    let Some(graph) = nav.filter(|graph| !graph.is_empty()) else {
        return candidates.first().copied();
    };
    let mut deferred: Option<ModeObjective> = None;
    for obj in candidates {
        // Goals the bot approaches by its own spot or line are routed by `seek`
        // to that point, so their route goal never equals the objective. Once
        // one was found reachable it stays chosen without a search per think.
        let indirect = obj.spread
            || obj.action == ObjectiveAction::Defend
            || (obs.methodical && obj.action == ObjectiveAction::Plant && !obj.touching);
        if indirect && *held == Some(obj.origin) {
            return Some(obj);
        }
        if *route_goal == Some(obj.origin)
            && (obj.touching || path_ends_in_use_volume(world, committed, obj))
        {
            return Some(obj);
        }
        match nav::find_route_resumable(
            world,
            graph,
            obs.self_state.origin,
            obj.origin,
            Some(obj),
            astar_budget,
            route_work,
        ) {
            Ok(path) => {
                if indirect {
                    *held = Some(obj.origin);
                    return Some(obj);
                }
                if path_ends_in_use_volume(world, &path, obj) {
                    *committed = path;
                    *route_goal = Some(obj.origin);
                    return Some(obj);
                }
            }
            // Budget-deferred work stays pending: stop here and keep the slot.
            Err(PathError::BudgetExhausted { .. }) => {
                deferred = Some(obj);
                break;
            }
            // Coverage that cannot answer this request is not proof of global
            // unreachability, and waiting does not extend the published graph.
            // Record it and let a lower-scoring, routable objective be tried.
            Err(
                PathError::GraphChanged
                | PathError::IncompleteGraph
                | PathError::Unreachable
                | PathError::EmptyGraph
                | PathError::NoStartSupport
                | PathError::NoGoalSupport,
            ) => {}
        }
    }
    deferred
}

/// A positive observation of this contact, recent enough to shoot at. A contact
/// held through unsensed ticks still aims and moves; it does not fire.
fn fresh_contact(obs: &BotObservation, target: Contact) -> bool {
    target.source == KnowledgeSource::CurrentlySeen
        && obs.tick.saturating_sub(target.seen_tick) <= FIRE_FRESH_TICKS
}

fn shot_allowed(sample: QueryResult<SightSample>, target: ClientId) -> bool {
    match sample {
        Ok(SightSample::Clear) => true,
        Ok(SightSample::HitPlayer { client, .. }) => client == target,
        Ok(SightSample::Unknown | SightSample::Blocked { .. }) | Err(_) => false,
    }
}

/// A trace that ran and found something in the way. A denied or unclassified
/// trace says nothing about the geometry, so it is not an obstruction.
fn definitely_obstructed(sample: QueryResult<SightSample>, target: ClientId) -> bool {
    matches!(
        sample,
        Ok(SightSample::Blocked { .. }) | Ok(SightSample::HitPlayer { .. })
    ) && !shot_allowed(sample, target)
}

fn return_fire_clear(sample: QueryResult<SightSample>, bot: ClientId) -> bool {
    match sample {
        Ok(SightSample::Clear) => true,
        Ok(SightSample::HitPlayer { client, .. }) => client == bot,
        Ok(SightSample::Unknown | SightSample::Blocked { .. }) | Err(_) => false,
    }
}

fn away_from(here: [f32; 3], threat: [f32; 3], dist: f32) -> [f32; 3] {
    let dx = here[0] - threat[0];
    let dy = here[1] - threat[1];
    let len = (dx * dx + dy * dy).sqrt().max(1.0);
    [
        here[0] + dx / len * dist,
        here[1] + dy / len * dist,
        here[2],
    ]
}

fn path_ends_in_use_volume(
    world: &impl WorldQuery,
    path: &[nav::RouteStep],
    obj: ModeObjective,
) -> bool {
    path.last()
        .is_some_and(|end| world.objective_contains(obj, end.position))
}

fn walk_step(
    from: [f32; 3],
    to: [f32; 3],
    world: &mut impl WorldQuery,
) -> (MoveMode, PathOutcome, Option<[f32; 3]>) {
    let d = [to[0] - from[0], to[1] - from[1]];
    let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
    if len < 8.0 {
        return (MoveMode::Hold, PathOutcome::Clear, Some(to));
    }
    let scale = (STEP_IN / len).min(1.0);
    let step = [from[0] + d[0] * scale, from[1] + d[1] * scale, from[2]];
    match world.walk_hull(from, step) {
        Ok(WalkSample::Clear) => (MoveMode::Walk, PathOutcome::Clear, Some(to)),
        Ok(WalkSample::BreakGlass) => (MoveMode::BreakGlass, PathOutcome::Clear, Some(step)),
        Ok(WalkSample::Blocked) if len > STEP_IN => {
            (MoveMode::Hold, PathOutcome::Unreachable, Some(to))
        }
        Ok(WalkSample::Blocked) => (MoveMode::Hold, PathOutcome::Blocked, Some(to)),
        Err(_) => (MoveMode::Hold, PathOutcome::BudgetExhausted, Some(to)),
    }
}

/// An eye-level point along `yaw`.
fn look_point(obs: &BotObservation, yaw: f32) -> [f32; 3] {
    let eye = obs.eye();
    [eye[0] + yaw.cos() * PACE_LOOK_IN, eye[1] + yaw.sin() * PACE_LOOK_IN, eye[2]]
}

/// A spot within a step or two where `target` cannot see the bot's chest:
/// somewhere to duck behind between bursts. Nearest first, a few per search.
fn find_cover(
    obs: &BotObservation,
    world: &mut impl WorldQuery,
    graph: &NavGraph,
    target: Contact,
) -> Option<[f32; 3]> {
    let here = obs.self_state.origin;
    let mut near: Vec<(f32, [f32; 3])> = graph
        .nodes
        .iter()
        .map(|pos| (dist_xy(*pos, here), *pos))
        .filter(|(d, pos)| *d > 24.0 && *d <= COVER_SEARCH_IN && (pos[2] - here[2]).abs() < 40.0)
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    world.enter(QuerySubsystem::Tactical);
    let eye = [target.origin[0], target.origin[1], target.origin[2] + 60.0];
    for (_, pos) in near.into_iter().take(COVER_CANDIDATES) {
        let chest = [pos[0], pos[1], pos[2] + 48.0];
        let Ok(sample) = world.shot_ray(eye, chest, target.id) else {
            return None;
        };
        let hidden = matches!(
            sample,
            SightSample::Blocked {
                obstacle: crate::query::ObstacleKind::World | crate::query::ObstacleKind::StaticOrLinked,
                fraction,
            } if fraction < 0.95
        );
        // The way there has to be walkable in a straight step.
        if hidden && matches!(world.walk_hull(here, pos), Ok(WalkSample::Clear)) {
            return Some(pos);
        }
    }
    None
}

/// A stable percentile roll for one bot in one time window.
fn pace_roll(seed: u32, window: u32, salt: u32) -> u32 {
    let mut x = seed ^ window.wrapping_mul(0x9e37_79b9) ^ salt.wrapping_mul(0x85eb_ca6b);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2c1b_3c6d);
    x ^= x >> 12;
    x = x.wrapping_mul(0x297a_2d39);
    x ^= x >> 15;
    x % 100
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    dx * dx + dy * dy + dz * dz
}

fn dist_xy(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    (dx * dx + dy * dy).sqrt()
}
