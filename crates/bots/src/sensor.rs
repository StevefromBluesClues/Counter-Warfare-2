use math_iw4::angle_vectors;
use sim::{ClientId, ClientLifecycle, Snapshot};

use crate::observation::{
    BotObservation, Contact, DEFAULT_OBJECTIVE_RADIUS, KnowledgeSource, ModeObjective,
    ObjectiveAction, SelfState, TeamRole, Visibility, WeaponAction, WeaponClass, WeaponSlot,
};
use crate::query::{QuerySubsystem, SightSample, WorldQuery};

pub const SIGHT_RANGE_IN: f32 = 5906.0;
pub const SIGHT_FOV_DEG: f32 = 90.0;
const PROBE_CHEST_Z: f32 = 48.0;
const PROBE_HEAD_Z: f32 = 64.0;
const PROBE_FEET_Z: f32 = 8.0;
/// Perception's share of the shared quota, so routing and firing keep theirs.
const SIGHT_PROBES: u32 = 6;

pub fn observe(
    snapshot: &Snapshot,
    bot: ClientId,
    world: &mut impl WorldQuery,
) -> Option<BotObservation> {
    observe_focused(snapshot, bot, world, None, &[])
}

/// Teammates call out what they see: an enemy a teammate spotted becomes,
/// half a second on, a rough position this bot knows, as voice or ping would
/// give it. Only enemies this bot does not see itself, and not too stale.
pub fn hear_callouts(obs: &mut BotObservation, callouts: &[(i32, ClientId, [f32; 3], u32)]) {
    for (team, id, origin, at) in callouts {
        let age = obs.tick.saturating_sub(*at);
        if *team != obs.self_state.team
            || !(CALLOUT_DELAY_TICKS..CALLOUT_KEEP_TICKS).contains(&age)
            || obs.seen.iter().any(|c| c.id == *id)
            || obs.heard.iter().any(|c| c.id == *id)
            || dist2(obs.self_state.origin, *origin) > CALLOUT_RANGE_IN * CALLOUT_RANGE_IN
        {
            continue;
        }
        // "One near the truck": close enough, not exact.
        let a = (id.0 * 37 + at / 20) as f32 * 1.7;
        obs.heard.push(Contact {
            id: *id,
            origin: [
                origin[0] + a.cos() * CALLOUT_SLOP_IN,
                origin[1] + a.sin() * CALLOUT_SLOP_IN,
                origin[2],
            ],
            source: KnowledgeSource::LastSeen,
            seen_tick: *at,
            confidence: 0.4,
        });
    }
}

/// A teammate killed in the last two seconds by an enemy still alive within
/// reach: the killer, to go and trade.
fn trade_target(
    snapshot: &Snapshot,
    bot: ClientId,
    team: i32,
    here: [f32; 3],
) -> Option<(ClientId, [f32; 3])> {
    snapshot.meta.journal.iter().rev().find_map(|record| {
        let sim::SimEvent::Died {
            victim,
            attacker: Some(attacker),
            ..
        } = &record.event
        else {
            return None;
        };
        if snapshot.tick.0.saturating_sub(record.tick.0) > TRADE_WINDOW_TICKS
            || *victim == bot
            || attacker == victim
        {
            return None;
        }
        let victim_team = snapshot.meta.for_client(*victim)?.client_state_team;
        let killer = snapshot.meta.for_client(*attacker)?;
        if victim_team != team
            || killer.client_state_team == team
            || killer.lifecycle != ClientLifecycle::Alive
        {
            return None;
        }
        let (_, ps) = snapshot.players.iter().find(|(id, _)| id == attacker)?;
        (dist2(here, ps.origin) <= TRADE_RANGE_IN * TRADE_RANGE_IN).then_some((*attacker, ps.origin))
    })
}

const TRADE_WINDOW_TICKS: u32 = 40;
const TRADE_RANGE_IN: f32 = 1500.0;

pub const CALLOUT_KEEP_TICKS: u32 = 60;
const CALLOUT_DELAY_TICKS: u32 = 10;
const CALLOUT_RANGE_IN: f32 = 3500.0;
const CALLOUT_SLOP_IN: f32 = 120.0;

// Defenders rotate, as Counter-Strike CTs do: enough attackers called out
// at one site and the other site's defenders head over to help, one anchor
// staying behind unless the whole push has been seen there. The call holds
// a while after the last sighting so the rotation is not abandoned midway.
const ROTATE_CALL_IN: f32 = 1300.0;
const ROTATE_MIN_CALLED: usize = 2;
const ROTATE_ALL_CALLED: usize = 3;
const ROTATE_KEEP_TICKS: u32 = 600;
// Rotated defenders get an objective id of their own, so they pick fresh
// guard spots at the new site.
const ROTATE_SITE_ID: u32 = 1000;

/// A defending team's call: which site is being hit, how many attackers
/// have been seen there at once, and when it was last confirmed.
#[derive(Clone, Copy, Debug)]
pub struct Rotation {
    site: usize,
    called: usize,
    at: u32,
}

/// Send a defender holding the quiet site to the one being hit.
pub fn rotate_on_call(
    obs: &mut BotObservation,
    callouts: &[(i32, ClientId, [f32; 3], u32)],
    rotations: &mut std::collections::HashMap<i32, Rotation>,
) {
    let team = obs.self_state.team;
    if obs.frozen {
        rotations.remove(&team);
        return;
    }
    let [mine] = obs.objectives.as_slice() else {
        return;
    };
    let mine = *mine;
    if !obs.methodical || mine.action != ObjectiveAction::Defend || obs.sites.len() < 2 {
        return;
    }
    // What this team knows now: its own sightings and teammates' callouts.
    let mut counts = vec![Vec::<ClientId>::new(); obs.sites.len()];
    let known = obs.seen.iter().map(|c| (c.id, c.origin)).chain(
        callouts
            .iter()
            .filter(|(t, _, _, at)| {
                *t == team && (CALLOUT_DELAY_TICKS..CALLOUT_KEEP_TICKS).contains(&obs.tick.saturating_sub(*at))
            })
            .map(|(_, id, origin, _)| (*id, *origin)),
    );
    for (id, origin) in known {
        let nearest = obs
            .sites
            .iter()
            .enumerate()
            .map(|(i, site)| (i, dist2(*site, origin)))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((site, d2)) = nearest
            && d2 <= ROTATE_CALL_IN * ROTATE_CALL_IN
            && !counts[site].contains(&id)
        {
            counts[site].push(id);
        }
    }
    if let Some((site, ids)) = counts.iter().enumerate().max_by_key(|(_, ids)| ids.len())
        && ids.len() >= ROTATE_MIN_CALLED
    {
        let called = rotations
            .get(&team)
            .filter(|r| r.site == site && obs.tick.saturating_sub(r.at) <= ROTATE_KEEP_TICKS)
            .map_or(0, |r| r.called)
            .max(ids.len());
        rotations.insert(team, Rotation { site, called, at: obs.tick });
    }
    let Some(call) = rotations
        .get(&team)
        .filter(|r| obs.tick.saturating_sub(r.at) <= ROTATE_KEEP_TICKS)
        .copied()
    else {
        return;
    };
    let hot = obs.sites[call.site];
    let here = obs.self_state.origin;
    if dist2(hot, mine.origin) < 64.0 * 64.0 {
        return;
    }
    // The defender nearest the quiet site anchors it against a split.
    let anchor = call.called < ROTATE_ALL_CALLED
        && !obs
            .teammates
            .iter()
            .any(|(_, at)| dist2(*at, mine.origin) < dist2(here, mine.origin));
    if anchor {
        return;
    }
    let flat = dist2([here[0], here[1], 0.0], [hot[0], hot[1], 0.0]);
    let goal = ModeObjective {
        id: ROTATE_SITE_ID + call.site as u32,
        origin: hot,
        action: ObjectiveAction::Defend,
        radius: DEFEND_RADIUS,
        touching: flat <= DEFEND_RADIUS * DEFEND_RADIUS,
        ..ModeObjective::at(hot)
    };
    obs.objectives = vec![goal];
    obs.objective = Some(goal);
}

/// Smoke grenades the bots have seen pop. The cloud is only drawn, so the
/// simulation's traces pass through it; a bot treats the space around where a
/// smoke grenade burst as blocking sight while the smoke lasts.
#[derive(Debug, Default)]
pub struct SmokeTracker {
    flying: Vec<(sim::ProjectileId, [f32; 3])>,
    clouds: Vec<([f32; 3], u32)>,
}

impl SmokeTracker {
    pub fn update(&mut self, snapshot: &Snapshot, is_smoke: impl Fn(u32) -> bool) {
        let tick = snapshot.tick.0;
        let now: Vec<(sim::ProjectileId, [f32; 3])> = snapshot
            .projectiles
            .iter()
            .filter(|p| is_smoke(p.weapon))
            .map(|p| (p.id, p.origin))
            .collect();
        for (id, origin) in &self.flying {
            if !now.iter().any(|(other, _)| other == id) {
                self.clouds.push((*origin, tick));
            }
        }
        self.flying = now;
        self.clouds
            .retain(|(_, popped)| tick.saturating_sub(*popped) < SMOKE_FILL_TICKS + SMOKE_LAST_TICKS);
    }

    /// Clouds thick enough to hide behind at `tick`: centre and radius.
    pub fn clouds(&self, tick: u32) -> Vec<([f32; 3], f32)> {
        self.clouds
            .iter()
            .filter_map(|(at, popped)| {
                let age = tick.saturating_sub(*popped);
                if age >= SMOKE_FILL_TICKS + SMOKE_LAST_TICKS {
                    return None;
                }
                // The cloud billows out over the first second.
                let grow = (age as f32 / SMOKE_FILL_TICKS as f32).min(1.0);
                Some(([at[0], at[1], at[2] + SMOKE_LIFT_IN], SMOKE_RADIUS_IN * grow))
            })
            .filter(|(_, radius)| *radius > 24.0)
            .collect()
    }
}

/// `screenType flashed` in a shellshock file.
const FLASHED_SCREEN: i32 = 1;
const FLASH_BLIND_LEFT: f32 = 0.3;
const SMOKE_FILL_TICKS: u32 = 20;
const SMOKE_LAST_TICKS: u32 = 260;
const SMOKE_RADIUS_IN: f32 = 230.0;
const SMOKE_LIFT_IN: f32 = 60.0;

fn smoke_blocks(smoke: &[([f32; 3], f32)], from: [f32; 3], to: [f32; 3]) -> bool {
    smoke.iter().any(|(centre, radius)| {
        let d: [f32; 3] = std::array::from_fn(|i| to[i] - from[i]);
        let len2 = d.iter().map(|v| v * v).sum::<f32>().max(1e-3);
        let t = ((0..3).map(|i| (centre[i] - from[i]) * d[i]).sum::<f32>() / len2).clamp(0.0, 1.0);
        let near: [f32; 3] = std::array::from_fn(|i| from[i] + d[i] * t - centre[i]);
        near.iter().map(|v| v * v).sum::<f32>() < radius * radius
    })
}

/// `focus` is the client the bot has already committed to. It is probed before
/// the rotation spends the same allowance on acquisition candidates, so a
/// committed target is the last thing perception gives up, not the first.
pub fn observe_focused(
    snapshot: &Snapshot,
    bot: ClientId,
    world: &mut impl WorldQuery,
    focus: Option<ClientId>,
    smoke: &[([f32; 3], f32)],
) -> Option<BotObservation> {
    let meta = snapshot.meta.for_client(bot)?;
    let ps = snapshot
        .players
        .iter()
        .find(|(id, _)| *id == bot)
        .map(|(_, ps)| ps)?;
    let team = meta.client_state_team;
    world.enter(QuerySubsystem::Perception);
    let self_state = SelfState {
        life_sequence: meta.life_sequence,
        id: bot,
        lifecycle: meta.lifecycle,
        origin: ps.origin,
        viewangles: ps.viewangles,
        delta_angles: ps.delta_angles,
        view_height: ps.view_height_current,
        stance: crate::observation::Stance::from_view_height(ps.view_height_current),
        health: ps.health,
        weapon: ps.weapon as u16,
        weapon_class: world.weapon_class(ps.weapon as u16),
        weapon_action: WeaponAction::from_weaponstate(ps.weaponstate_primary),
        ammo_clip: meta.ammo_clip,
        ammo_stock: meta.ammo_stock,
        team,
    };
    let inventory = own_inventory(ps, meta, world);
    let reach = engage_range(&inventory, ps.weapon as u16);
    let sniper = inventory
        .iter()
        .any(|slot| slot.weapon == ps.weapon as u16 && slot.class == WeaponClass::Sniper);
    let mut heard = Vec::new();
    // A flashbang whites the bot out for most of its effect; a stun leaves it
    // sluggish for the rest of the shock.
    let now = sim::level_time_ms(snapshot.tick);
    let elapsed = now.wrapping_sub(ps.shellshock_time);
    let shocked = ps.shellshock_duration > 0 && (0..ps.shellshock_duration).contains(&elapsed);
    let screen = meta.shellshock.as_ref().map(|shock| shock.screen_type);
    let left = if shocked {
        1.0 - elapsed as f32 / ps.shellshock_duration as f32
    } else {
        0.0
    };
    let blinded = screen == Some(FLASHED_SCREEN) && left > FLASH_BLIND_LEFT;
    let stun = if screen == Some(FLASHED_SCREEN) { left * 0.5 } else { left };
    let mut seen = Vec::new();
    let mut unsensed = Vec::new();
    if meta.lifecycle != ClientLifecycle::Alive {
        // A bot that is not alive performs no probe, so it makes no negative
        // observation either.
        unsensed.extend(
            snapshot
                .players
                .iter()
                .map(|(id, _)| *id)
                .filter(|id| *id != bot),
        );
    } else {
        let eye = [
            ps.origin[0],
            ps.origin[1],
            ps.origin[2]
                + if ps.view_height_current > 1.0 {
                    ps.view_height_current
                } else {
                    60.0
                },
        ];
        let (forward, _, _) = angle_vectors(ps.viewangles);
        let half = (SIGHT_FOV_DEG * 0.5).to_radians().cos();
        // Rotate probes so occluded clients at the front of the snapshot cannot
        // monopolize perception.
        let mut probes = SIGHT_PROBES;
        let count = snapshot.players.len();
        let start = (snapshot.tick.0 as usize / 2 + bot.0 as usize) % count.max(1);
        for index in probe_order(snapshot, start, count, focus) {
            let (id, other) = &snapshot.players[index];
            if *id == bot {
                continue;
            }
            let Some(other_meta) = snapshot.meta.for_client(*id) else {
                continue;
            };
            if other_meta.lifecycle != ClientLifecycle::Alive {
                continue;
            }
            if snapshot.meta.kind.is_team() && other_meta.client_state_team == team {
                continue;
            }
            let range = dist2(ps.origin, other.origin).sqrt();
            // Hearing needs no line of sight: unsuppressed gunfire carries far,
            // running and sprinting feet carry a little; crouched or walking
            // players are quiet.
            let firing = WeaponAction::from_weaponstate(other.weaponstate_primary)
                == WeaponAction::Firing;
            let loud = if world.weapon_silenced(other.weapon as u16) {
                HEAR_SUPPRESSED_IN
            } else {
                HEAR_GUNFIRE_IN
            };
            let pace = other.velocity[0].hypot(other.velocity[1]);
            let feet = if other.view_height_current < 50.0 || pace < HEAR_PACE_MIN {
                0.0
            } else if pace > HEAR_SPRINT_PACE {
                HEAR_SPRINT_IN
            } else {
                HEAR_RUN_IN
            };
            if (firing && range <= loud) || range <= feet {
                // Ears place a sound only roughly.
                let slop = range * HEAR_SLOP_SHARE;
                let a = (snapshot.tick.0 / 20 + id.0 * 7) as f32 * 2.39;
                heard.push(Contact {
                    id: *id,
                    origin: [
                        other.origin[0] + a.cos() * slop,
                        other.origin[1] + a.sin() * slop,
                        other.origin[2],
                    ],
                    source: KnowledgeSource::LastSeen,
                    seen_tick: snapshot.tick.0,
                    confidence: 0.5,
                });
            }
            // Out of range or outside the cone is a complete answer on its own;
            // only a client that needed a probe it did not get is unsensed.
            // Close by, movement at the edge of vision is noticed too; far
            // away, only a sniper watching through a scope picks out a
            // figure off the centre of view.
            let cone = if range <= PERIPHERAL_IN {
                PERIPHERAL_HALF_COS
            } else if range > LONG_SIGHT_IN && !sniper {
                LONG_HALF_COS
            } else {
                half
            };
            if !in_range_and_fov(ps.origin, other.origin, forward, cone) {
                continue;
            }
            // An enemy beyond what the gun in hand can hit is left alone, as a
            // player would rather than empty a pistol across the map.
            if dist2(ps.origin, other.origin) > reach * reach {
                continue;
            }
            if probes == 0 {
                unsensed.push(*id);
                continue;
            }
            // Whited out, or the line runs through a smoke cloud: nothing seen.
            if blinded
                || smoke_blocks(
                    smoke,
                    eye,
                    [other.origin[0], other.origin[1], other.origin[2] + 48.0],
                )
            {
                continue;
            }
            match probe_visible(world, bot, eye, other.origin, *id, &mut probes) {
                Visibility::Seen => seen.push(Contact {
                    id: *id,
                    origin: other.origin,
                    source: KnowledgeSource::CurrentlySeen,
                    seen_tick: snapshot.tick.0,
                    confidence: 1.0,
                }),
                Visibility::NotSeen => {}
                Visibility::Unknown => unsensed.push(*id),
            }
        }
    }
    let teammates: Vec<(ClientId, [f32; 3])> = snapshot
        .players
        .iter()
        .filter(|(id, _)| *id != bot)
        .filter(|(id, _)| {
            snapshot.meta.for_client(*id).is_some_and(|other| {
                other.lifecycle == ClientLifecycle::Alive && other.client_state_team == team
            })
        })
        .map(|(id, ps)| (*id, ps.origin))
        .collect();
    let squad = (
        teammates.iter().filter(|(id, _)| id.0 < bot.0).count() as u32,
        teammates.len() as u32 + 1,
    );
    let objectives = public_objectives(snapshot, bot, team, squad, &teammates);
    let (sites, planted) = bomb_sites(snapshot, bot, team);
    Some(BotObservation {
        tick: snapshot.tick.0,
        time_ms: snapshot.tick.0 as i32 * 50,
        self_state,
        inventory,
        seen,
        unsensed,
        events: Vec::new(),
        objective: objectives
            .iter()
            .copied()
            .min_by(|a, b| dist2(ps.origin, a.origin).total_cmp(&dist2(ps.origin, b.origin))),
        objectives,
        methodical: snapshot.meta.kind.is_bomb_defusal(),
        casual: snapshot.meta.kind == gamemode_iw4::GameModeKind::BombDefusalCasual,
        frozen: meta.controls.frozen,
        teammates,
        squad,
        heard,
        stun,
        trade: trade_target(snapshot, bot, team, ps.origin),
        hurt: Vec::new(),
        enemy_approach: None,
        damage_event: ps.damage_event,
        damage_yaw: ps.damage_yaw as f32 * 360.0 / 256.0,
        sites,
        planted,
    })
}

/// How far the weapon in hand is worth firing.
fn engage_range(inventory: &[WeaponSlot], held: u16) -> f32 {
    let Some(slot) = inventory.iter().find(|slot| slot.weapon == held) else {
        return SIGHT_RANGE_IN;
    };
    if slot.facts.quick_select {
        return ENGAGE_PISTOL_IN;
    }
    match slot.class {
        WeaponClass::Sniper => SIGHT_RANGE_IN,
        WeaponClass::Assault | WeaponClass::Lmg => ENGAGE_RIFLE_IN,
        WeaponClass::Smg => ENGAGE_SMG_IN,
        WeaponClass::Shotgun => ENGAGE_SHOTGUN_IN,
    }
}

const ENGAGE_PISTOL_IN: f32 = 1800.0;
const HEAR_GUNFIRE_IN: f32 = 2500.0;
const HEAR_SUPPRESSED_IN: f32 = 500.0;
const HEAR_SPRINT_IN: f32 = 1000.0;
const HEAR_RUN_IN: f32 = 700.0;
const HEAR_PACE_MIN: f32 = 130.0;
const HEAR_SPRINT_PACE: f32 = 205.0;
const HEAR_SLOP_SHARE: f32 = 0.15;
const PERIPHERAL_IN: f32 = 1500.0;
/// cos(70 deg): a 140 degree field close by.
const PERIPHERAL_HALF_COS: f32 = 0.342;
const LONG_SIGHT_IN: f32 = 3000.0;
/// cos(25 deg): a 50 degree field far away.
const LONG_HALF_COS: f32 = 0.906;
const ENGAGE_SMG_IN: f32 = 1800.0;
const ENGAGE_SHOTGUN_IN: f32 = 900.0;
const ENGAGE_RIFLE_IN: f32 = 3000.0;

fn in_range_and_fov(from: [f32; 3], to: [f32; 3], forward: [f32; 3], min_dot: f32) -> bool {
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let len2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    if len2 > SIGHT_RANGE_IN * SIGHT_RANGE_IN || len2 <= 0.0001 {
        return false;
    }
    let len = len2.sqrt();
    let dir = [d[0] / len, d[1] / len, d[2] / len];
    dir[0] * forward[0] + dir[1] * forward[1] + dir[2] * forward[2] >= min_dot
}

/// The bot's own carried weapons with the ammunition the match state holds for
/// them. Enemy inventories and positions are never read here.
fn own_inventory(
    ps: &playerstate_iw4::PlayerState,
    meta: &sim::ClientSnapshotMeta,
    world: &impl WorldQuery,
) -> Vec<WeaponSlot> {
    ps.weapons
        .iter()
        .copied()
        .filter(|&id| id > 0)
        .filter_map(|id| {
            let weapon = u16::try_from(id).ok()?;
            let facts = world.weapon_facts(weapon)?;
            let (clip, stock) = meta
                .ammo_by_weapon
                .iter()
                .find(|(owned, _, _)| *owned == id as u32)
                .map_or((0, 0), |(_, clip, stock)| (*clip, *stock));
            Some(WeaponSlot {
                weapon,
                class: world.weapon_class(weapon),
                facts,
                clip,
                stock,
            })
        })
        .collect()
}

/// Snapshot indices in the order perception spends its allowance: the committed
/// target first, then the tick's rotation.
fn probe_order(
    snapshot: &Snapshot,
    start: usize,
    count: usize,
    focus: Option<ClientId>,
) -> Vec<usize> {
    let mut order: Vec<usize> = (0..count).map(|offset| (start + offset) % count).collect();
    if let Some(focus) = focus
        && let Some(at) = order
            .iter()
            .position(|index| snapshot.players[*index].0 == focus)
    {
        let first = order.remove(at);
        order.insert(0, first);
    }
    order
}

fn probe_visible(
    world: &mut impl WorldQuery,
    bot: ClientId,
    eye: [f32; 3],
    origin: [f32; 3],
    target: ClientId,
    remaining: &mut u32,
) -> Visibility {
    let probes = [
        [origin[0], origin[1], origin[2] + PROBE_CHEST_Z],
        [origin[0], origin[1], origin[2] + PROBE_HEAD_Z],
        [origin[0], origin[1], origin[2] + PROBE_FEET_Z],
    ];
    // One positive probe settles the question; a negative one settles it only
    // when every probe actually ran and came back classified.
    let mut complete = true;
    for probe in probes {
        if *remaining == 0 {
            return Visibility::Unknown;
        }
        *remaining -= 1;
        match world.sight_ray(eye, probe, bot) {
            Ok(SightSample::Clear) => return Visibility::Seen,
            Ok(SightSample::HitPlayer { client, .. }) if client == target => {
                return Visibility::Seen;
            }
            Err(_) => {
                // The shared quota is gone; stop attributing denials to this bot.
                *remaining = 0;
                return Visibility::Unknown;
            }
            Ok(SightSample::Unknown) => complete = false,
            Ok(_) => {}
        }
    }
    if complete {
        Visibility::NotSeen
    } else {
        Visibility::Unknown
    }
}

/// The bomb sites this team is shown, and whether the bomb is planted: before
/// a plant attackers see two targets and defenders two sites to defend; after
/// it, one site to defend or one bomb to defuse.
fn bomb_sites(snapshot: &Snapshot, bot: ClientId, team: i32) -> (Vec<[f32; 3]>, bool) {
    let Some(team) = gamemode_iw4::Team::from_packed_u8(team as u8) else {
        return (Vec::new(), false);
    };
    let mut sites = Vec::new();
    let (mut targets, mut defends, mut defuse) = (0, 0, false);
    for o in snapshot.meta.objectives.compass.iter().filter(|o| o.shows_to(team, bot.0)) {
        let site = if o.icon.contains("target") {
            targets += 1;
            true
        } else if o.icon.contains("defend") {
            defends += 1;
            true
        } else if o.icon.contains("defuse") {
            defuse = true;
            true
        } else {
            false
        };
        if site && !sites.iter().any(|s: &[f32; 3]| dist2(*s, o.origin) < 64.0 * 64.0) {
            sites.push(o.origin);
        }
    }
    (sites, defuse || (defends == 1 && targets == 0))
}

fn public_objectives(
    snapshot: &Snapshot,
    bot: ClientId,
    team: i32,
    squad: (u32, u32),
    teammates: &[(ClientId, [f32; 3])],
) -> Vec<ModeObjective> {
    let Some(team) = gamemode_iw4::Team::from_packed_u8(team as u8) else {
        return Vec::new();
    };
    let feet = snapshot
        .players
        .iter()
        .find(|(id, _)| *id == bot)
        .map(|(_, p)| p.origin);
    let visible = || {
        snapshot
            .meta
            .objectives
            .compass
            .iter()
            .filter(move |o| o.shows_to(team, bot.0))
    };
    let flat_dist2 = |a: [f32; 3], b: [f32; 3]| dist2([a[0], a[1], 0.0], [b[0], b[1], 0.0]);
    // A carried bomb's escort icon rides on its carrier.
    let carrying = feet.is_some_and(|f| {
        visible().any(|o| {
            o.icon.contains("escort") && flat_dist2(f, o.origin) <= CARRIER_RADIUS * CARRIER_RADIUS
        })
    });
    // A teammate carrying the bomb picks the site; everyone else escorts them.
    let escort = visible()
        .find(|o| o.icon.contains("escort"))
        .filter(|_| !carrying)
        .map(|o| o.origin);
    if let Some(at) = escort {
        let carrier = snapshot
            .players
            .iter()
            .filter(|(id, _)| *id != bot)
            .min_by(|a, b| dist2(a.1.origin, at).total_cmp(&dist2(b.1.origin, at)))
            .map(|(id, _)| *id);
        // Snap so the route request survives the carrier's small movements.
        let snap = |v: f32| (v / ESCORT_SNAP).round() * ESCORT_SNAP;
        let origin = [snap(at[0]), snap(at[1]), at[2]];
        let mut follow = ModeObjective {
            id: u32::MAX,
            origin,
            role: TeamRole::Cover,
            active_user: carrier,
            radius: ESCORT_RADIUS,
            spread: true,
            ..ModeObjective::at(origin)
        };
        follow.touching =
            feet.is_some_and(|f| flat_dist2(f, origin) <= ESCORT_RADIUS * ESCORT_RADIUS);
        return vec![follow];
    }
    let [_, a, b] = snapshot.meta.objectives.scores;
    let rounds = u32::try_from(a.max(0) + b.max(0)).unwrap_or(0);
    let mut mix = rounds.wrapping_mul(0x9E37_79B9).wrapping_add(0x7F4A_7C15);
    mix ^= mix >> 16;
    mix = mix.wrapping_mul(0x85EB_CA6B);
    mix ^= mix >> 13;
    mix = mix.wrapping_mul(0xC2B2_AE35);
    mix ^= mix >> 16;
    // Defenders split across the sites they hold; whoever holds a planted bomb
    // guards it. Neither applies while something can be defused.
    let defuse = visible().any(|o| o.icon.contains("defuse"));
    let mut sites: Vec<&sim::CompassObjective> = visible()
        .filter(|o| o.icon.contains("defend") && !defuse)
        .collect();
    if !sites.is_empty() && !visible().any(|o| o.icon.contains("target")) {
        sites.sort_by_key(|o| o.index);
        let site = sites[((squad.0 + mix) as usize) % sites.len()];
        let mut guard = ModeObjective {
            id: u32::from(site.index),
            origin: site.origin,
            action: ObjectiveAction::Defend,
            radius: DEFEND_RADIUS,
            ..ModeObjective::at(site.origin)
        };
        guard.touching =
            feet.is_some_and(|f| flat_dist2(f, site.origin) <= DEFEND_RADIUS * DEFEND_RADIUS);
        return vec![guard];
    }
    // Attackers commit to one site per round; the choice rotates with the score.
    let targets: Vec<u8> = visible()
        .filter(|o| o.icon.contains("target"))
        .map(|o| o.index)
        .collect();
    let preferred_target = (targets.len() > 1).then(|| targets[(mix as usize) % targets.len()]);
    visible()
        .filter(|o| !(o.icon.contains("target") && preferred_target.is_some_and(|p| p != o.index)))
        .filter(|o| {
            !o.icon.contains("defend")
                && !o.icon.contains("escort")
                && !o.icon.starts_with("compassping")
        })
        .map(|o| {
            let mut objective = ModeObjective {
                id: u32::from(o.index),
                origin: o.origin,
                touching: feet.is_some_and(|f| {
                    flat_dist2(f, o.origin) <= DEFAULT_OBJECTIVE_RADIUS * DEFAULT_OBJECTIVE_RADIUS
                }),
                ..ModeObjective::at(o.origin)
            };
            if o.icon.contains("defuse") {
                // The defender nearest the bomb defuses; the rest cover them.
                let mine = feet.map_or(f32::MAX, |f| flat_dist2(f, o.origin));
                if teammates
                    .iter()
                    .any(|(_, at)| flat_dist2(*at, o.origin) < mine)
                {
                    objective.action = ObjectiveAction::Defend;
                    objective.radius = DEFEND_RADIUS;
                    objective.touching = mine <= DEFEND_RADIUS * DEFEND_RADIUS;
                    return objective;
                }
                objective.action = ObjectiveAction::Defuse;
                objective.interaction_ms = DEFUSE_INTERACTION_MS;
            } else if o.icon.contains("target") && !carrying {
                objective.spread = true;
                return objective;
            } else if o.icon.contains("target") && carrying {
                objective.action = ObjectiveAction::Plant;
                objective.interaction_ms = PLANT_INTERACTION_MS;
            } else {
                return objective;
            }
            // Hold-to-use objectives only work inside the authored trigger,
            // which is much smaller than the capture radius.
            objective.use_button = true;
            objective.radius = USE_OBJECTIVE_RADIUS;
            objective.touching = feet.is_some_and(|f| {
                flat_dist2(f, o.origin) <= USE_OBJECTIVE_RADIUS * USE_OBJECTIVE_RADIUS
            });
            objective
        })
        .collect()
}

const CARRIER_RADIUS: f32 = 48.0;
const ESCORT_RADIUS: f32 = 220.0;
const DEFEND_RADIUS: f32 = 700.0;
const ESCORT_SNAP: f32 = 128.0;
const USE_OBJECTIVE_RADIUS: f32 = 56.0;
const PLANT_INTERACTION_MS: u32 = 5_000;
const DEFUSE_INTERACTION_MS: u32 = 10_000;

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    dx * dx + dy * dy + dz * dz
}
