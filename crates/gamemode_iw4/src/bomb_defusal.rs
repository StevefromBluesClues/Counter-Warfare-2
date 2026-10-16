/// Competitive Bomb Defusal: five a side, first to 13, Counter-Strike's
/// competitive economy.
pub const GAMETYPE_TOKEN: &str = "bd";

pub const DISPLAY_NAME: &str = "COMPETITIVE BOMB DEFUSAL";

/// Casual Bomb Defusal: bigger teams, shorter match, free armor and kits.
pub const CASUAL_GAMETYPE_TOKEN: &str = "bdc";

pub const CASUAL_DISPLAY_NAME: &str = "CASUAL BOMB DEFUSAL";

/// Tells the overlay script which rules to play by.
pub const MODE_DVAR: &str = "scr_bd_mode";

/// Search and Destroy supplies the rounds, bomb and spawns; this module layers
/// the economy, buy menu and loadouts on top of it.
pub const SCRIPT_GAMETYPE: &str = "sd";

pub const SCRIPT_MODULE: &str = "iw4l/bomb_defusal";

pub const BUY_MENU: &str = "bd_buy";

pub const AUTOBUY_RESPONSE: &str = "bd:autobuy";

/// Bots buy toward a favourite kind of gun kept for the match.
pub const BOTBUY_RESPONSE: &str = "bd:botbuy";

pub const DEFAULT_ENEMY_BOTS: u32 = 5;

pub const DEFAULT_FRIENDLY_BOTS: u32 = 4;

/// Casual fills the server: nine a side (Counter-Strike plays ten, the
/// engine seats eighteen).
pub const CASUAL_ENEMY_BOTS: u32 = 9;

pub const CASUAL_FRIENDLY_BOTS: u32 = 8;
