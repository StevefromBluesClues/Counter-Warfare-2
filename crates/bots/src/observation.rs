use sim::{ClientId, ClientLifecycle};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stance {
    Stand,
    Crouch,
    Prone,
}

impl Stance {
    pub fn from_view_height(height: f32) -> Self {
        if height >= 50.0 {
            Self::Stand
        } else if height >= 30.0 {
            Self::Crouch
        } else {
            Self::Prone
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KnowledgeSource {
    CurrentlySeen,
    LastSeen,
    PublicMode,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    pub id: ClientId,
    pub origin: [f32; 3],
    pub source: KnowledgeSource,
    pub seen_tick: u32,
    pub confidence: f32,
}

/// What perception established about one client this tick. `NotSeen` is an
/// answer; `Unknown` is the absence of one, and the two must not be confused —
/// a probe that was never performed cannot end a contact or authorize a shot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    Seen,
    NotSeen,
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WeaponClass {
    #[default]
    Assault,
    Smg,
    Lmg,
    Sniper,
    Shotgun,
}

impl WeaponClass {
    pub fn fight_hold(self) -> f32 {
        match self {
            Self::Sniper => 360.0,
            Self::Lmg => 220.0,
            Self::Assault => 160.0,
            Self::Smg => 112.0,
            Self::Shotgun => 72.0,
        }
    }

    pub fn from_script_name(name: &str) -> Self {
        if name.is_empty() {
            return Self::Assault;
        }
        let stem = crate::unique_loadout::family_stem(name);
        for row in crate::unique_loadout::UNIQUE_WEAPONS {
            if crate::unique_loadout::family_stem(row.key) == stem {
                return Self::from_group(row.group);
            }
        }
        Self::Assault
    }

    fn from_group(group: crate::unique_loadout::UniqueGroup) -> Self {
        use crate::unique_loadout::UniqueGroup;
        match group {
            UniqueGroup::Sniper => Self::Sniper,
            UniqueGroup::Lmg => Self::Lmg,
            UniqueGroup::Shotgun => Self::Shotgun,
            UniqueGroup::Smg | UniqueGroup::MachinePistol | UniqueGroup::Pistol => Self::Smg,
            UniqueGroup::Assault
            | UniqueGroup::Riot
            | UniqueGroup::Projectile
            | UniqueGroup::Cqb
            | UniqueGroup::Special => Self::Assault,
        }
    }
}

/// What the weapon simulator is doing with the active hand. The bot reads it;
/// it never reimplements the transitions behind it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WeaponAction {
    #[default]
    Ready,
    Raising,
    Dropping,
    Firing,
    Reloading,
    /// Melee, offhand, rechamber, sprint or a stun — not a readiness decision.
    Busy,
}

impl WeaponAction {
    pub fn from_weaponstate(raw: i32) -> Self {
        use weapon_iw4::WeaponState as State;
        match State::from_i32(raw) {
            Ok(State::Ready) => Self::Ready,
            Ok(State::Raising | State::RaisingAltswitch) => Self::Raising,
            Ok(State::Dropping | State::DroppingQuick | State::DroppingAltswitch) => Self::Dropping,
            Ok(State::Firing) => Self::Firing,
            Ok(state) if state.is_reload_family() => Self::Reloading,
            _ => Self::Busy,
        }
    }

    /// An active weapon id alone does not mean the hand finished raising.
    pub fn is_settled(self) -> bool {
        matches!(self, Self::Ready | Self::Firing)
    }
}

/// Timing and magazine facts for one weapon id, read from the content the
/// simulator uses. Not a second weapon model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WeaponFacts {
    /// A carried gun the player can select rather than an offhand or an item.
    pub selectable: bool,
    /// Pistol-class selection uses the quick raise and drop timings.
    pub quick_select: bool,
    pub clip_size: i32,
    pub raise_time_ms: i32,
    pub quick_raise_time_ms: i32,
    pub drop_time_ms: i32,
    pub quick_drop_time_ms: i32,
    pub reload_time_ms: i32,
    pub reload_empty_time_ms: i32,
    /// Fires once per trigger press (single shot or burst), so a held
    /// trigger has to be released between shots.
    pub semi_auto: bool,
    /// Set for a thrown offhand: which kind, and so which button throws it.
    pub grenade: Option<Grenade>,
}

/// Thrown offhands the bots know how to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grenade {
    Frag,
    Smoke,
    Stun,
    Flash,
}

impl Grenade {
    pub fn from_script_name(name: &str) -> Option<Self> {
        let stem = name.strip_suffix("_mp").unwrap_or(name);
        match stem {
            "frag_grenade" | "semtex" => Some(Self::Frag),
            "smoke_grenade" => Some(Self::Smoke),
            "concussion_grenade" => Some(Self::Stun),
            "flash_grenade" => Some(Self::Flash),
            _ => None,
        }
    }

    /// Lethal grenades go on the frag button, tactical ones on the smoke button.
    pub fn lethal(self) -> bool {
        self == Self::Frag
    }
}

/// One owned weapon with the ammunition the match state holds for it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WeaponSlot {
    pub weapon: u16,
    pub class: WeaponClass,
    pub facts: WeaponFacts,
    pub clip: i32,
    pub stock: i32,
}

impl WeaponSlot {
    /// Milliseconds from committing to a change until this weapon is raised,
    /// leaving `from` behind. Both halves follow the incoming weapon's class.
    pub fn switch_ms(&self, from: &Self) -> i32 {
        if self.facts.quick_select {
            from.facts.quick_drop_time_ms + self.facts.quick_raise_time_ms
        } else {
            from.facts.drop_time_ms + self.facts.raise_time_ms
        }
    }

    pub fn reload_ms(&self) -> i32 {
        if self.clip == 0 {
            self.facts.reload_empty_time_ms
        } else {
            self.facts.reload_time_ms
        }
    }

    pub fn is_loaded_gun(&self) -> bool {
        self.facts.selectable && self.clip > 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelfState {
    pub life_sequence: sim::LifeSequence,
    pub id: ClientId,
    pub lifecycle: ClientLifecycle,
    pub origin: [f32; 3],
    pub viewangles: [f32; 3],
    pub delta_angles: [f32; 3],
    pub view_height: f32,
    pub stance: Stance,
    pub health: i32,
    pub weapon: u16,
    pub weapon_class: WeaponClass,
    pub weapon_action: WeaponAction,
    pub ammo_clip: i32,
    pub ammo_stock: i32,
    pub team: i32,
}

pub const DEFAULT_OBJECTIVE_RADIUS: f32 = 96.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ObjectiveAction {
    #[default]
    Capture,
    Plant,
    Defuse,
    Defend,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TeamRole {
    #[default]
    Actor,
    Cover,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModeObjective {
    pub id: u32,
    pub round: u32,
    pub action: ObjectiveAction,
    pub role: TeamRole,
    pub active_user: Option<ClientId>,
    pub remaining_ms: Option<u32>,
    pub interaction_ms: u32,
    pub progress: f32,
    pub origin: [f32; 3],
    pub touching: bool,
    pub use_button: bool,
    pub radius: f32,
    /// Several teammates may share this goal and should fan out around it.
    pub spread: bool,
}

impl ModeObjective {
    pub fn at(origin: [f32; 3]) -> Self {
        Self {
            id: 0,
            round: 0,
            action: ObjectiveAction::Capture,
            role: TeamRole::Actor,
            active_user: None,
            remaining_ms: None,
            interaction_ms: 0,
            progress: 0.0,
            origin,
            touching: false,
            use_button: false,
            radius: DEFAULT_OBJECTIVE_RADIUS,
            spread: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BotEvent {
    Spotted { id: ClientId },
    LostSight { id: ClientId },
}

#[derive(Clone, Debug, PartialEq)]
pub struct BotObservation {
    pub tick: u32,
    pub time_ms: i32,
    pub self_state: SelfState,
    /// The bot's own carried weapons. Enemy inventories are never projected.
    pub inventory: Vec<WeaponSlot>,
    pub seen: Vec<Contact>,
    /// Clients perception did not finish evaluating this tick, because its
    /// allowance ran out, a query was denied, or a trace came back
    /// unclassified. A client in neither `seen` nor here was evaluated and not
    /// seen, which is a real negative observation.
    pub unsensed: Vec<ClientId>,
    pub events: Vec<BotEvent>,
    pub objective: Option<ModeObjective>,
    pub objectives: Vec<ModeObjective>,
    /// Rounds with one life each reward a slower, checking pace.
    pub methodical: bool,
    /// Casual rules: bots aim a little shakier and react a little later.
    pub casual: bool,
    /// Script-frozen, as in a pre-round freeze.
    pub frozen: bool,
    /// Living teammates, which a team's spacing and sharing of positions needs.
    pub teammates: Vec<(ClientId, [f32; 3])>,
    /// This bot's place among its living team, by client number, and the
    /// team's size: a stable way to give teammates different jobs.
    pub squad: (u32, u32),
    /// Enemies heard but not necessarily seen this tick: gunfire and running
    /// feet, placed roughly where the sound came from.
    pub heard: Vec<Contact>,
    /// How dazed the bot is by a stun or the tail of a flash: 0 clear, 1 just
    /// hit. Slows its turning and aim.
    pub stun: f32,
    /// Counts hits taken; with `damage_yaw`, the world yaw (degrees) toward
    /// where the last hit came from.
    pub damage_event: u32,
    pub damage_yaw: f32,
    /// A teammate was just killed nearby: the killer and where they are, to
    /// be traded.
    pub trade: Option<(ClientId, [f32; 3])>,
    /// Enemies in sight that have visibly taken hits (hitmarkers, blood):
    /// worth pushing to finish.
    pub hurt: Vec<ClientId>,
    /// Where an enemy would come through on the way here from their start:
    /// the angle to watch when nothing is in sight.
    pub enemy_approach: Option<[f32; 3]>,
    /// Every bomb site marker this bot's team is shown, planted or not.
    pub sites: Vec<[f32; 3]>,
    /// The bomb is down and ticking.
    pub planted: bool,
}

impl BotObservation {
    /// A grenade of this kind still carried, if any.
    pub fn grenade(&self, kind: Grenade) -> Option<&WeaponSlot> {
        self.inventory
            .iter()
            .find(|slot| slot.facts.grenade == Some(kind) && slot.clip + slot.stock > 0)
    }

    pub fn held(&self) -> Option<&WeaponSlot> {
        self.slot(self.self_state.weapon)
    }

    pub fn slot(&self, weapon: u16) -> Option<&WeaponSlot> {
        self.inventory.iter().find(|slot| slot.weapon == weapon)
    }

    pub fn owns(&self, weapon: u16) -> bool {
        self.inventory.iter().any(|slot| slot.weapon == weapon)
    }

    pub fn eye(&self) -> [f32; 3] {
        let z = if self.self_state.view_height > 1.0 {
            self.self_state.view_height
        } else {
            60.0
        };
        [
            self.self_state.origin[0],
            self.self_state.origin[1],
            self.self_state.origin[2] + z,
        ]
    }

    pub fn visibility(&self, id: ClientId) -> Visibility {
        if self.seen.iter().any(|contact| contact.id == id) {
            Visibility::Seen
        } else if self.unsensed.contains(&id) {
            Visibility::Unknown
        } else {
            Visibility::NotSeen
        }
    }

    pub fn events_since(&self, prev_seen: &[ClientId]) -> Vec<BotEvent> {
        let mut events = Vec::new();
        for contact in &self.seen {
            if !prev_seen.contains(&contact.id) {
                events.push(BotEvent::Spotted { id: contact.id });
            }
        }
        for id in prev_seen {
            // Only a completed negative observation is a loss of sight. A skipped
            // probe leaves the previous knowledge exactly as it was.
            if self.visibility(*id) == Visibility::NotSeen {
                events.push(BotEvent::LostSight { id: *id });
            }
        }
        events
    }
}
