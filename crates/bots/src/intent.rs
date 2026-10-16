#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathOutcome {
    None,
    Clear,
    Blocked,
    Unreachable,
    BudgetExhausted,
    IncompleteGraph,
    ProgressLost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveMode {
    Hold,
    Walk,
    Drop,
    BreakGlass,
    Mantle,
    Ladder,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BotIntent {
    pub look_at: Option<[f32; 3]>,
    pub move_goal: Option<[f32; 3]>,
    pub desired_range: Option<f32>,
    pub move_mode: MoveMode,
    pub fire: bool,
    pub use_button: bool,
    pub reload: bool,
    pub crouch: bool,
    pub sprint: bool,
    /// Turn the view at a relaxed, steady rate: nothing to react to.
    pub calm: bool,
    /// Offhand buttons (frag or smoke) held this tick to throw a grenade.
    pub throw_buttons: u32,
    /// Aim exactly, without the bot's personal aim error, and only fire when
    /// on the point: shooting out a pane rather than a person.
    pub precise: bool,
    /// Jump this tick (getting out of a hole the walk cannot climb).
    pub jump: bool,
    /// Aim down the sights.
    pub ads: bool,
    /// Walk slowly enough that footsteps make no sound.
    pub quiet: bool,
    /// The weapon the command asks for. `None` keeps the observed weapon; a
    /// selection is held through every phase of the change, not pulsed once.
    pub weapon: Option<u16>,
    pub path: PathOutcome,
}

impl Default for BotIntent {
    fn default() -> Self {
        Self {
            look_at: None,
            move_goal: None,
            desired_range: None,
            move_mode: MoveMode::Hold,
            fire: false,
            use_button: false,
            reload: false,
            crouch: false,
            sprint: false,
            calm: false,
            throw_buttons: 0,
            precise: false,
            jump: false,
            ads: false,
            quiet: false,
            weapon: None,
            path: PathOutcome::None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MotorReport {
    #[default]
    Executing,
    Arrived,
    Blocked,
    Unsupported,
}
