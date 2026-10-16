use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::tasks::{Task, futures_lite::future};
use net::{
    AUTHORITY_MS, AuthorityClock, AuthorityWorld, ClientActionInbox, ClientCommandInbox,
    LocalPresentClient, ReliableEventHub, authority_should_tick, look_angles_from_degrees,
};
use sim::{ClientAction, ClientLifecycle, SimWorld, Tick};

use crate::nav::{self, NAV_HULL, NAV_SCHEMA, NavGraph, RouteStats};
use crate::query::{Budgeted, QueryCounters, QuerySubsystem, TraceBudget};
use crate::roster::{
    BotAddQueue, BotFireQueue, BotHold, BotRoster, BotTpQueue, BotTpTarget, BotTpWhere,
    default_class_index,
};
use crate::sensor;
use frame::{AuthoritySet, BotNavigationReady, ClientSet, HasWorld, MatchTornDown, RuntimeRole};

const VIEW_PITCH_DOWN: f32 = 85.0;
const TRACE_QUOTA: u32 = 96;
const ASTAR_QUOTA: u32 = 2048;
/// Ten seconds of authority ticks between aggregates. Compact enough to leave
/// in a match, coarse enough not to be a per-query log in the hot path.
const METER_PERIOD_TICKS: u32 = 200;

#[derive(Resource, Default)]
struct BotNav {
    graph: NavGraph,
    pending: Option<(u64, Task<NavGraph>, Option<asset_transport::StageHandle>)>,
}

/// Where the shared quota went and whether pending work actually advanced.
/// Reported per window; not a per-query trace.
#[derive(Resource, Default)]
struct BotMeter {
    queries: QueryCounters,
    controller_us: u64,
    ticks: u32,
    stuck_peak: usize,
    live_peak: usize,
    max_age: u32,
    max_starved: u32,
    /// Route totals at the last report, so the line carries this window's work.
    routes_at_report: RouteStats,
}

impl BotMeter {
    fn report(&mut self, totals: RouteStats, bots: usize) {
        let was = self.routes_at_report;
        let routes = RouteStats {
            completed: totals.completed.saturating_sub(was.completed),
            terminal: totals.terminal.saturating_sub(was.terminal),
            cancelled: totals.cancelled.saturating_sub(was.cancelled),
            discarded_attachments: totals
                .discarded_attachments
                .saturating_sub(was.discarded_attachments),
            denied_slices: totals.denied_slices.saturating_sub(was.denied_slices),
            ..RouteStats::default()
        };
        self.routes_at_report = totals;
        let per = |row: &[u32; QuerySubsystem::COUNT]| {
            QuerySubsystem::ALL
                .iter()
                .map(|s| format!("{}={}", s.label(), row[s.index()]))
                .collect::<Vec<_>>()
                .join(" ")
        };
        diag::info!(
            Sim,
            "bots: window={} bots={bots} attempted[{}] denied[{}] primitives={} \
             routes done={} terminal={} cancelled={} discarded={} denied_slices={} \
             age={} starve={} live_peak={} stuck_peak={} controller_us={}",
            self.ticks,
            per(&self.queries.attempted),
            per(&self.queries.denied),
            self.queries.primitives,
            routes.completed,
            routes.terminal,
            routes.cancelled,
            routes.discarded_attachments,
            routes.denied_slices,
            self.max_age,
            self.max_starved,
            self.live_peak,
            self.stuck_peak,
            self.controller_us,
        );
        self.queries = QueryCounters::ZERO;
        self.controller_us = 0;
        self.ticks = 0;
        self.stuck_peak = 0;
        self.live_peak = 0;
        self.max_age = 0;
        self.max_starved = 0;
    }
}

pub struct BotsPlugin;

impl Plugin for BotsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BotRoster>()
            .init_resource::<BotAddQueue>()
            .init_resource::<BotHold>()
            .init_resource::<BotTpQueue>()
            .init_resource::<BotFireQueue>()
            .init_resource::<BotNav>()
            .init_resource::<BotNavigationReady>()
            .init_resource::<BotMeter>()
            .add_systems(
                Update,
                (
                    fill_bots_from_rules,
                    drain_bot_add_queue,
                    evict_bots_claiming_local_client,
                    boot_bots,
                    apply_bot_tp,
                )
                    .chain()
                    .after(ClientSet::Load),
            )
            .add_systems(
                Update,
                (reset_roster_on_match_torn_down, prepare_navigation)
                    .chain()
                    .in_set(ClientSet::Load),
            )
            .add_systems(
                FixedUpdate,
                (evict_bots_claiming_local_client, think_bots)
                    .chain()
                    .in_set(AuthoritySet::Ingress)
                    .run_if(authority_should_tick),
            );
    }
}

fn reset_roster_on_match_torn_down(
    mut torn: MessageReader<MatchTornDown>,
    mut roster: ResMut<BotRoster>,
    mut nav: ResMut<BotNav>,
    mut ready: ResMut<BotNavigationReady>,
) {
    if torn.read().len() == 0 {
        return;
    }
    *roster = BotRoster::default();
    *nav = BotNav::default();
    *ready = BotNavigationReady::default();
}

fn fill_bots_from_rules(
    mut roster: ResMut<BotRoster>,
    mut queue: ResMut<BotAddQueue>,
    installed: Option<Res<HasWorld>>,
    rules: Option<Res<frame::HostMatchRules>>,
    world: Option<Res<AuthorityWorld>>,
) {
    if roster.rules_filled || !installed.is_some_and(|installed| installed.0) {
        return;
    }
    let Some(world) = world else {
        return;
    };
    roster.rules_filled = true;
    let count = |dvar: &str| {
        rules
            .as_ref()
            .and_then(|rules| rules.0.iter().find(|(name, _)| name == dvar))
            .and_then(|(_, value)| value.trim().parse::<u32>().ok())
            .unwrap_or(0)
    };
    let kind = world.0.game_mode_kind();
    let enemies = count(sim::ENEMY_BOTS_DVAR);
    let friends = if kind == gamemode_iw4::GameModeKind::FreeForAll {
        0
    } else {
        count(sim::FRIENDLY_BOTS_DVAR)
    };
    // Competitive Bomb Defusal is played five against five, Casual nine
    // against nine; an unset lobby fills it. These bots take the
    // auto-assigned team: a requested side arrives after the first spawn,
    // and the switch would kill them for the rest of the round.
    if kind.is_bomb_defusal() && enemies == 0 && friends == 0 {
        use gamemode_iw4::bomb_defusal as bd;
        let count = if kind == gamemode_iw4::GameModeKind::BombDefusalCasual {
            bd::CASUAL_ENEMY_BOTS + bd::CASUAL_FRIENDLY_BOTS
        } else {
            bd::DEFAULT_ENEMY_BOTS + bd::DEFAULT_FRIENDLY_BOTS
        };
        queue.push(count);
        diag::info!(Sim, "bots: bomb defusal adds {count} auto-assigned bots");
        return;
    }
    queue.push_side(enemies, crate::BotSide::Enemy);
    queue.push_side(friends, crate::BotSide::Friendly);
    if enemies > 0 || friends > 0 {
        diag::info!(
            Sim,
            "bots: game rules add {enemies} enemy and {friends} friendly"
        );
    }
}

fn drain_bot_add_queue(
    mut queue: ResMut<BotAddQueue>,
    mut roster: ResMut<BotRoster>,
    local: Res<LocalPresentClient>,
    world: Option<Res<AuthorityWorld>>,
    hub: Option<Res<net::UdpAuthorityHub>>,
) {
    let requests = queue.drain();
    if requests.is_empty() {
        return;
    }
    let mut taken = vec![local.0];
    if let Some(world) = world.as_ref() {
        taken.extend(world.0.clients_scoreboard().into_iter().map(|(id, _)| id));
    }
    if let Some(hub) = hub.as_ref() {
        taken.extend(hub.connections.clients().map(sim::ClientId));
    }
    for request in requests {
        let count = request.count;
        let added = roster.add_bots(count, &taken, request.dummy, request.side);
        if added.len() < count as usize {
            diag::warn!(
                Sim,
                "bots: add {count} — only {} minted, the roster is full",
                added.len()
            );
        }
        diag::info!(
            Sim,
            "bots: add {count} (dummy={}) → clients {:?}",
            request.dummy,
            added.iter().map(|id| id.0).collect::<Vec<_>>()
        );
    }
}

// The link hands the local player its real id during signon, which can land
// after a bot was already minted. A slot holding that id would make `is_bot`
// claim the player, so retire it — loudly, because by then it is a bug.
fn evict_bots_claiming_local_client(mut roster: ResMut<BotRoster>, local: Res<LocalPresentClient>) {
    if !roster.is_bot(local.0) {
        return;
    }
    roster.bots.retain(|bot| bot.id != local.0);
    diag::warn!(
        Sim,
        "bots: retired the slot on client {} — that id is the local player",
        local.0.0
    );
}

fn boot_bots(
    mut roster: ResMut<BotRoster>,
    mut actions: ResMut<ClientActionInbox>,
    mut request_ids: ResMut<net::ActionRequestIds>,
    installed: Option<Res<HasWorld>>,
    world: Option<ResMut<AuthorityWorld>>,
    local: Res<LocalPresentClient>,
) {
    let Some(mut world) = world else {
        return;
    };
    if !installed.is_some_and(|installed| installed.0) {
        return;
    }
    let seed = roster.seed;
    let team_based = world.0.game_mode_kind() != gamemode_iw4::GameModeKind::FreeForAll;
    let local_team = match world.0.client_state_team(local.0) {
        Some(entity_iw4::TEAM_AXIS) => Some(("axis", "allies")),
        Some(entity_iw4::TEAM_ALLIES) => Some(("allies", "axis")),
        _ => None,
    };
    for bot in &mut roster.bots {
        if bot.joined {
            continue;
        }
        let team = match bot.side.filter(|_| team_based) {
            None => None,
            Some(side) => {
                let Some((mine, theirs)) = local_team else {
                    continue;
                };
                Some(match side {
                    crate::BotSide::Friendly => mine,
                    crate::BotSide::Enemy => theirs,
                })
            }
        };
        if world.0.gsc_realm() == Some(sim::script::Realm::Iw4)
            && let Err(error) = world
                .0
                .persistent_data_mut()
                .admit_temporary(bot.id, bot.account)
        {
            diag::warn!(
                Sim,
                "bots: client {} account admission failed: {error:?}",
                bot.id.0
            );
            continue;
        }
        let index = default_class_index(seed, bot.id, bot.class_picks);
        let join_id = request_ids.allocate();
        let class_request = request_ids.allocate();
        let name_request = request_ids.allocate();
        let queued = [
            actions.push(
                bot.id,
                ClientAction::JoinMatch {
                    request_id: join_id,
                },
            ),
            team.map_or(Ok(()), |team| {
                let request_id = request_ids.allocate();
                match (
                    sim::menu_response_field(sim::TEAM_MENU),
                    sim::menu_response_field(team),
                ) {
                    (Some(menu), Some(response)) => actions.push(
                        bot.id,
                        ClientAction::MenuResponse {
                            request_id,
                            menu,
                            response,
                        },
                    ),
                    _ => Ok(()),
                }
            }),
            actions.push(
                bot.id,
                ClientAction::ChooseDefaultClass {
                    request_id: class_request,
                    index,
                },
            ),
            actions.push(
                bot.id,
                ClientAction::SetName {
                    request_id: name_request,
                    name: entity_iw4::pack_client_state_name(bot.name),
                },
            ),
        ];
        if let Some(error) = queued.into_iter().find_map(Result::err) {
            diag::warn!(Sim, "bots: client {} not booted — {error}", bot.id.0);
            continue;
        }
        bot.joined = true;
        bot.class_picks += 1;
        diag::info!(
            Sim,
            "bots: JoinMatch+changeclass client={} pick={index} request_id={class_request}",
            bot.id.0,
        );
    }
}

fn apply_bot_tp(
    mut queue: ResMut<BotTpQueue>,
    roster: Res<BotRoster>,
    mut actions: ResMut<ClientActionInbox>,
    mut request_ids: ResMut<net::ActionRequestIds>,
    world: Option<Res<AuthorityWorld>>,
    local: Res<LocalPresentClient>,
) {
    let requests = queue.drain();
    if requests.is_empty() {
        return;
    }
    let Some(world) = world else {
        return;
    };
    let snapshot = world.0.snapshot(Tick(0));
    let local_ps = snapshot
        .meta
        .for_client(local.0)
        .is_some_and(|m| m.lifecycle == ClientLifecycle::Alive)
        .then(|| {
            snapshot
                .players
                .iter()
                .find(|(id, _)| *id == local.0)
                .map(|(_, ps)| ps)
        })
        .flatten();
    for request in requests {
        let ids: Vec<sim::ClientId> = match request.target {
            BotTpTarget::All => roster.bots.iter().map(|b| b.id).collect(),
            BotTpTarget::Id(id) => {
                if roster.is_bot(id) {
                    vec![id]
                } else {
                    diag::warn!(Sim, "bots: tp skipped — client {} is not a bot", id.0);
                    Vec::new()
                }
            }
        };
        for id in ids {
            let current = snapshot
                .players
                .iter()
                .find(|(c, _)| *c == id)
                .map(|(_, ps)| ps.viewangles)
                .unwrap_or([0.0, 0.0, 0.0]);
            let (origin, angles) = match request.where_ {
                BotTpWhere::Absolute { origin, yaw, pitch } => {
                    let mut angles = current;
                    if let Some(yaw) = yaw {
                        angles[1] = yaw;
                    }
                    if let Some(pitch) = pitch {
                        angles[0] = pitch;
                    }
                    (origin, angles)
                }
                BotTpWhere::Above { height } => {
                    let Some(ps) = local_ps else {
                        diag::warn!(Sim, "bots: tp above skipped — local not Alive");
                        continue;
                    };
                    let origin = [ps.origin[0], ps.origin[1], ps.origin[2] + height];
                    (origin, aim_viewangles(origin, ps.origin))
                }
            };
            let request_id = request_ids.allocate();
            if let Err(error) = actions.push(
                id,
                ClientAction::Move {
                    request_id,
                    origin,
                    angles,
                },
            ) {
                diag::warn!(Sim, "bots: tp client={} not queued — {error}", id.0);
                continue;
            }
            diag::info!(
                Sim,
                "bots: tp client={} ({:.1} {:.1} {:.1}) request_id={request_id}",
                id.0,
                origin[0],
                origin[1],
                origin[2]
            );
        }
    }
}

const APPROACH_REFRESH_TICKS: u32 = 10;

/// Below this an enemy has clearly been hit.
const HURT_ENEMY_HP: i32 = 70;

/// Confidence marking a contact learned from a teammate's death.
pub(crate) const TRADE_CONFIDENCE: f32 = 0.9;

fn bots_contact(id: sim::ClientId, origin: [f32; 3], tick: u32, confidence: f32) -> crate::observation::Contact {
    crate::observation::Contact {
        id,
        origin,
        source: crate::observation::KnowledgeSource::LastSeen,
        seen_tick: tick,
        confidence,
    }
}

#[derive(SystemParam)]
struct ThinkBots<'w> {
    clock: Res<'w, AuthorityClock>,
    world: ResMut<'w, AuthorityWorld>,
    nav: Res<'w, BotNav>,
    roster: ResMut<'w, BotRoster>,
    cmds: ResMut<'w, ClientCommandInbox>,
    actions: ResMut<'w, ClientActionInbox>,
    request_ids: ResMut<'w, net::ActionRequestIds>,
    hold: Res<'w, BotHold>,
    fire: ResMut<'w, BotFireQueue>,
    reliable: ResMut<'w, ReliableEventHub>,
    meter: ResMut<'w, BotMeter>,
}

fn think_bots(mut p: ThinkBots) {
    // A bot has no client reading its reliable channel to ack it, so nothing
    // else ever drains it — leave this out and match broadcasts (deaths, hit
    // markers, ...) overflow the queue and the bot gets retired as if its
    // connection had died.
    for bot in &p.roster.bots {
        p.reliable.ack_all(bot.id);
    }
    if p.roster.bots.is_empty() {
        p.fire.drain();
        return;
    }

    let snapshot = p.world.0.snapshot(Tick(p.clock.tick.saturating_sub(1)));
    {
        let world = &p.world.0;
        p.roster.smoke.update(&snapshot, |weapon| {
            world.weapon_script_name(weapon).starts_with("smoke_grenade")
        });
    }
    let smoke = p.roster.smoke.clouds(snapshot.tick.0);
    // Where each team starts: noted while everyone is frozen for the buy.
    {
        let mut sums: std::collections::HashMap<i32, ([f32; 3], f32)> = Default::default();
        for (id, ps) in &snapshot.players {
            let Some(meta) = snapshot.meta.for_client(*id) else {
                continue;
            };
            if meta.lifecycle != ClientLifecycle::Alive || !meta.controls.frozen {
                continue;
            }
            let entry = sums.entry(meta.client_state_team).or_insert(([0.0; 3], 0.0));
            for axis in 0..3 {
                entry.0[axis] += ps.origin[axis];
            }
            entry.1 += 1.0;
        }
        for (team, (sum, n)) in sums {
            p.roster.threat.note_spawn(team, sum.map(|v| v / n));
        }
        p.roster.threat.refresh(&p.nav.graph);
    }
    let threat = std::mem::take(&mut p.roster.threat);
    let mut approach = std::mem::take(&mut p.roster.approach);
    let mut rotations = std::mem::take(&mut p.roster.rotations);
    // Last tick's sightings are this tick's callouts; this tick's are
    // gathered as the bots look.
    let callouts = std::mem::take(&mut p.roster.callouts);
    let mut heard_now: Vec<(i32, sim::ClientId, [f32; 3], u32)> = Vec::new();
    let fires = p.fire.drain();
    let mut budget = TraceBudget::new(TRACE_QUOTA);
    let mut astar = ASTAR_QUOTA;
    let count = p.roster.bots.len();
    let start = (p.clock.tick as usize / 2) % count.max(1);
    let mut controller_us = 0u64;
    let seed = p.roster.seed;
    for offset in 0..count {
        let bot = &mut p.roster.bots[(start + offset) % count];
        let Some(meta) = snapshot.meta.for_client(bot.id) else {
            continue;
        };
        // Bomb Defusal marks its bots by their buy request, and moves one to
        // make room when a player picks a full side: send it as soon as the
        // bot is in, not at its first spawn, or a player picking a side
        // straight after joining finds no bot to move. The script holds the
        // buy until the bot spawns.
        let bomb_defusal = p.world.0.game_mode_kind().is_bomb_defusal();
        if bomb_defusal && !bot.announced {
            bot.announced = true;
            push_botbuy(&mut p.actions, &mut p.request_ids, bot.id);
        }
        if meta.lifecycle != ClientLifecycle::Alive {
            if let Some(brain) = bot.brain.as_mut() {
                brain.cancel_navigation();
            }
            continue;
        }
        if bot.class_picked_in != Some(meta.life_sequence) {
            bot.class_picked_in = Some(meta.life_sequence);
            let index = default_class_index(seed, bot.id, bot.class_picks);
            let request_id = p.request_ids.allocate();
            match p.actions.push(
                bot.id,
                ClientAction::ChooseDefaultClass { request_id, index },
            ) {
                Ok(()) => bot.class_picks += 1,
                Err(error) => diag::warn!(
                    Sim,
                    "bots: client {} class pick not queued — {error}",
                    bot.id.0
                ),
            }
            if bomb_defusal {
                push_botbuy(&mut p.actions, &mut p.request_ids, bot.id);
            }
        }
        let mut cmd = if p.hold.0 || bot.brain.is_none() {
            let Some((_, ps)) = snapshot.players.iter().find(|(id, _)| *id == bot.id) else {
                continue;
            };
            playerstate_iw4::UserCmd {
                server_time: p.clock.time_ms,
                angles: look_angles_from_degrees(std::array::from_fn(|axis| {
                    ps.viewangles[axis] - ps.delta_angles[axis]
                })),
                weapon: ps.weapon as u16,
                weapon_mapped: ps.weapon as u16,
                ..playerstate_iw4::UserCmd::default()
            }
        } else if let Some(brain) = bot.brain.as_mut() {
            let mut queried = Budgeted::new(&mut p.world.0, &mut budget);
            let Some(mut obs) =
                sensor::observe_focused(
                    &snapshot,
                    bot.id,
                    &mut queried,
                    brain.focus_target(),
                    &smoke,
                )
            else {
                continue;
            };
            let team = obs.self_state.team;
            for contact in &obs.seen {
                heard_now.push((team, contact.id, contact.origin, obs.tick));
            }
            sensor::hear_callouts(&mut obs, &callouts);
            sensor::rotate_on_call(&mut obs, &callouts, &mut rotations);
            obs.hurt = obs
                .seen
                .iter()
                .filter(|c| {
                    snapshot
                        .players
                        .iter()
                        .any(|(id, ps)| *id == c.id && ps.health > 0 && ps.health < HURT_ENEMY_HP)
                })
                .map(|c| c.id)
                .collect();
            // The killfeed and the dying teammate's last words: where the
            // killer is.
            if let Some((killer, at)) = obs.trade
                && !obs.seen.iter().any(|c| c.id == killer)
            {
                obs.heard.retain(|c| c.id != killer);
                obs.heard.push(bots_contact(killer, at, obs.tick, TRADE_CONFIDENCE));
            }
            // Where an enemy would come from, refreshed every half second.
            let enemy_team = match obs.self_state.team {
                1 => Some(2),
                2 => Some(1),
                _ => None,
            };
            let tick_now = snapshot.tick.0;
            let cached = approach.get(&bot.id).copied();
            obs.enemy_approach = match cached {
                Some((at, point)) if tick_now.saturating_sub(at) < APPROACH_REFRESH_TICKS => point,
                _ => {
                    let point = enemy_team.and_then(|team| {
                        threat.approach(&p.nav.graph, team, obs.self_state.origin)
                    });
                    approach.insert(bot.id, (tick_now, point));
                    point
                }
            };
            let started = std::time::Instant::now();
            let mut cmd = brain.drive_nav(
                &obs,
                &mut queried,
                Some(&p.nav.graph),
                &mut astar,
                AUTHORITY_MS,
            );
            controller_us += started.elapsed().as_micros() as u64;
            cmd.server_time = p.clock.time_ms;
            cmd
        } else {
            continue;
        };
        if fires.iter().any(|target| match target {
            BotTpTarget::All => true,
            BotTpTarget::Id(id) => *id == bot.id,
        }) {
            cmd.buttons |= playerstate_iw4::buttons::ATTACK;
        }
        p.cmds.push(bot.id, None, cmd, None);
    }
    p.roster.threat = threat;
    p.roster.approach = approach;
    p.roster.rotations = rotations;
    // Keep a sighting a few seconds so a callout survives the caller losing
    // sight for a moment.
    let tick_now = snapshot.tick.0;
    heard_now.extend(
        callouts
            .into_iter()
            .filter(|(_, _, _, at)| tick_now.saturating_sub(*at) < sensor::CALLOUT_KEEP_TICKS),
    );
    p.roster.callouts = heard_now;
    if count == 0 {
        return;
    }
    // Only a bot with a movement task that did not move counts as stuck; a
    // hold, an interaction or waiting to respawn is not a navigation failure.
    let tick = p.clock.tick;
    let mut routes = RouteStats::default();
    let mut stuck = 0;
    let mut live = 0;
    let (mut age, mut starved) = (0, 0);
    for bot in &p.roster.bots {
        let Some(brain) = bot.brain.as_ref() else {
            continue;
        };
        let stats = brain.route_stats();
        routes.completed += stats.completed;
        routes.terminal += stats.terminal;
        routes.cancelled += stats.cancelled;
        routes.discarded_attachments += stats.discarded_attachments;
        routes.denied_slices += stats.denied_slices;
        let (bot_age, bot_starved) = brain.route_age();
        if bot_age != 0 {
            live += 1;
        }
        age = age.max(bot_age);
        starved = starved.max(bot_starved);
        stuck += usize::from(brain.stuck(tick));
    }
    let meter = &mut p.meter;
    meter.queries.merge(&budget.counters);
    meter.controller_us += controller_us;
    meter.ticks += 1;
    meter.stuck_peak = meter.stuck_peak.max(stuck);
    meter.live_peak = meter.live_peak.max(live);
    meter.max_age = meter.max_age.max(age);
    meter.max_starved = meter.max_starved.max(starved);
    if meter.ticks >= METER_PERIOD_TICKS {
        meter.report(routes, count);
    }
}

fn prepare_navigation(
    world: Option<Res<AuthorityWorld>>,
    installed: Option<Res<HasWorld>>,
    role: Res<RuntimeRole>,
    generation: Res<frame::WorldGeneration>,
    mut nav: ResMut<BotNav>,
    mut ready: ResMut<BotNavigationReady>,
    load: Option<Res<assets::MapLoadProcess>>,
) {
    ready.0 = frame::WorldReadiness::new(*generation, frame::ReadinessState::Pending);
    if !matches!(*role, RuntimeRole::Listen | RuntimeRole::Dedicated) {
        ready.0.state = frame::ReadinessState::Ready;
        return;
    }
    if !installed.is_some_and(|installed| installed.0) {
        return;
    }
    let Some(world) = world else {
        return;
    };
    if refresh_nav(&world.0, &mut nav, load.as_deref()) {
        ready.0.state = frame::ReadinessState::Ready;
    }
}

fn refresh_nav(world: &SimWorld, nav: &mut BotNav, load: Option<&assets::MapLoadProcess>) -> bool {
    let digest = world.content_digest();
    if nav.graph.digest == digest && nav.graph.schema == NAV_SCHEMA && nav.graph.hull == NAV_HULL {
        return true;
    }
    if let Some((pending_digest, task, stage)) = nav.pending.as_mut()
        && *pending_digest == digest
    {
        if let Some(graph) = future::block_on(future::poll_once(task)) {
            nav.graph = graph;
            // The bake is only navigation once this graph is the one the bots
            // will read, which is here and not on the worker.
            if let Some(stage) = stage.take() {
                // What the walk actually produced: the links bots route over.
                // Nodes alone say how finely the grid was sampled; edges say
                // how much of the map turned out to be connected.
                let edges: usize = nav.graph.adj.iter().map(Vec::len).sum();
                stage.set_completed(edges as u64);
                stage.done();
            }
            nav.pending = None;
            return true;
        }
        return false;
    }
    // A bake is one indivisible walk of the grid, so nothing counts up while it
    // runs; the edge count is written once, when the graph it produced is the
    // one the bots read.
    let stage = load.map(|load| {
        load.progress
            .begin(asset_transport::StageId::Navigation, None)
    });
    let mut snapshot = world.clone();
    let generation = nav.graph.generation.wrapping_add(1);
    nav.pending = Some((
        digest,
        assets::load_pool().spawn(async move {
            let mut graph = navigation_for(&mut snapshot, digest);
            graph.generation = generation;
            graph
        }),
        stage,
    ));
    false
}

const NAV_CACHE_KIND: &str = "nav";

/// The digest, schema and hull that decide in-memory reuse are the whole key,
/// so the same map read back from disk is the same graph the walk would bake.
fn nav_cache_key(digest: u64) -> String {
    format!("{digest:016x}-{NAV_SCHEMA}-{NAV_HULL:08x}")
}

/// Walking the grid costs seconds of load, and a match teardown drops the graph
/// with the roster, so the second start of a map would pay it again. Keep the
/// bake in the artifact cache under its own key and only walk on a miss.
fn navigation_for(world: &mut SimWorld, digest: u64) -> NavGraph {
    let key = nav_cache_key(digest);
    if let Some(graph) = assets::cache_get(NAV_CACHE_KIND, &key)
        .as_deref()
        .and_then(NavGraph::cache_decode)
        && graph.digest == digest
        && graph.schema == NAV_SCHEMA
        && graph.hull == NAV_HULL
    {
        diag::info!(
            Sim,
            "bots: navigation read from cache: {} nodes, {} drops{}",
            graph.nodes.len(),
            graph.drops,
            if graph.truncated { ", truncated" } else { "" }
        );
        return graph;
    }
    let started = std::time::Instant::now();
    let mut graph = NavGraph::default();
    bake_navigation(world, &mut graph);
    diag::info!(
        Sim,
        "bots: navigation prepared in {:.1}ms on worker",
        started.elapsed().as_secs_f64() * 1000.0
    );
    if let Err(error) = assets::cache_put(NAV_CACHE_KIND, &key, &graph.cache_encode()) {
        diag::warn!(Sim, "bots: navigation cache store {key}: {error}");
    }
    graph
}

fn bake_navigation(world: &mut SimWorld, graph: &mut NavGraph) {
    let digest = world.content_digest();
    if graph.digest == digest && graph.schema == NAV_SCHEMA && graph.hull == NAV_HULL {
        return;
    }
    let generation = graph.generation.wrapping_add(1);
    let mut seeds = world.authored_spawn_origins();
    seeds.extend(
        world
            .objectives
            .compass
            .iter()
            .map(|objective| objective.origin),
    );
    let Some(bounds) = nav::playable_bounds(world.clip_brushes(), &seeds) else {
        *graph = NavGraph {
            generation,
            digest,
            schema: NAV_SCHEMA,
            hull: NAV_HULL,
            ..NavGraph::default()
        };
        return;
    };
    *graph = nav::bake_seeded(world, bounds, digest, &seeds);
    graph.generation = generation;
}

fn aim_viewangles(from: [f32; 3], target: [f32; 3]) -> [f32; 3] {
    let dir = [
        target[0] - from[0],
        target[1] - from[1],
        target[2] - from[2],
    ];
    if dir[0] == 0.0 && dir[1] == 0.0 {
        let pitch = if dir[2] < 0.0 {
            VIEW_PITCH_DOWN
        } else {
            -VIEW_PITCH_DOWN
        };
        return [pitch, 0.0, 0.0];
    }
    let mut angles = math_iw4::vect_to_angles(dir);
    angles[0] = math_iw4::angle_normalize_360(angles[0]);
    angles[1] = math_iw4::angle_normalize_360(angles[1]);
    if angles[0] > 180.0 {
        angles[0] -= 360.0;
    }
    angles[0] = angles[0].clamp(-VIEW_PITCH_DOWN, VIEW_PITCH_DOWN);
    angles
}

/// Bomb Defusal's bot buy request, which also marks the client as a bot.
fn push_botbuy(
    actions: &mut ClientActionInbox,
    request_ids: &mut net::ActionRequestIds,
    bot: sim::ClientId,
) {
    let (Some(menu), Some(response)) = (
        sim::menu_response_field(gamemode_iw4::bomb_defusal::BUY_MENU),
        sim::menu_response_field(gamemode_iw4::bomb_defusal::BOTBUY_RESPONSE),
    ) else {
        return;
    };
    let request_id = request_ids.allocate();
    if let Err(error) = actions.push(
        bot,
        ClientAction::MenuResponse {
            request_id,
            menu,
            response,
        },
    ) {
        diag::warn!(Sim, "bots: client {} buy request not queued — {error}", bot.0);
    }
}
