# Changes from IW4L

Counter-Warfare 2 is based on IW4L at upstream commit
[`91217cd`](https://github.com/vladtrc/iw4L/commit/91217cd) (2026-10-08). This file
lists what it adds and every file it changes. `git diff 91217cd` shows the exact
changes.

## Bomb Defusal game mode

Two modes on top of MW2's Search and Destroy, Competitive (`bd`) and Casual (`bdc`),
with a CS2 economy: round rewards, loss-bonus ladder, kill rewards per weapon,
plant/defuse bonuses, a buy menu with CS-equivalent MW2 guns, armor and defuse kits,
a bomb carrier marker, spectator cycling and halftime side and score swaps. Rules:
[docs/BOMB-DEFUSAL.md](docs/BOMB-DEFUSAL.md).

* `crates/assets/src/bomb_defusal.gsc.txt` **(new)**: the mode script, run as the
  `iw4l/bomb_defusal` overlay after the map's stock `maps/mp/gametypes/sd`. Money, buy
  menu responses, items and their damage scaling, armor, kits, halftime, team picks
  and the bots' buying.
* `crates/assets/src/map_scripts.rs`, `crates/assets/src/session_load/match_walk.rs`:
  load the overlay.
* `crates/gamemode_iw4/src/bomb_defusal.rs` **(new)**, `kind.rs`, `lib.rs`: the two
  mode kinds, names, tokens and default bot counts.
* `crates/session/src/match_apply.rs`, `crates/sim/src/spawn.rs`: mode selection;
  `scr_bd_mode` is passed to the script.
* `crates/ui/menus/bomb_defusal.json` **(new)**, `crates/ui/menus/frontend.json`,
  `crates/ui/src/menu.rs`: buy and team menus, game-mode entries.
* `crates/hud/src/menus/{mod,host,runtime}.rs`, `crates/hud/src/scorebar.rs`: buy and
  team menu handling, the mode name on the HUD.
* `crates/console/assets/default_controls.cfg`, `crates/console/src/user_settings.rs`:
  **B** opens the buy menu.
* `crates/audio/src/{backend,event}.rs`: no music or "winning/losing" announcer lines
  in this mode.
* `docs/BOMB-DEFUSAL.md` **(new)**, `docs/INDEX.md`, `docs/GSC-RUNTIME.md`.

### Rules and balance

* Pistols: T start with the USP .45, CT with the silenced USP; the M9 FMJ is the $300
  upgrade.
* Halftime scores swap with the players, and the scoreboard is refreshed.
* Each buyable gun's damage is scaled so T and CT counterparts are even and a pricier
  gun beats a cheaper one of its kind, solved from MW2's damage, falloff and fire
  rates. Table in [docs/BOMB-DEFUSAL.md](docs/BOMB-DEFUSAL.md).
* Picking a full side in Casual moves a bot to the other side; MW2's per-team cap is
  lifted for that swap.

## Engine fixes and features the mode needed

* Spectating: Fire/Aim cycle teammates, the followed player's name is shown, no death
  blur or aim-down-sights focus on a spectated view
  (`crates/sim/src/script/host/spectators.rs`, `crates/sim/src/step.rs`,
  `crates/sim/src/lib.rs`, `crates/sim/src/match_state.rs`,
  `crates/net/src/client/{input,runtime}.rs`,
  `crates/net/src/client/presentation/{centity_runtime,presented}.rs`,
  `crates/hud/src/overhead_names.rs`,
  `crates/render_frontend/src/assemble/drawsurf/dof.rs`).
* Plant and defuse triggers need the player looking at them, as in MW2
  (`crates/sim/src/script/host/triggers.rs`); the bomb takes priority over guns on the
  ground (`crates/sim/src/item.rs`).
* Runtime fixes in `crates/sim/src/world.rs`,
  `crates/sim/src/script/profile/iw4_startup.rs`,
  `crates/render_anim/src/anim/remote_kit.rs`,
  `crates/render_anim/src/occupancy/remote_body.rs`,
  `crates/render_frontend/src/adapters/fx/system.rs`.

## Bots

Changed: `crates/bots/src/{controller,motor,sensor,plugin,roster,observation,memory,nav,intent,query,weapon}.rs`
and `lib.rs`. New: `look.rs`, `threat.rs`, `grenade.rs`.

* Movement: human-like pace (bursts of sprint, crouching, pauses), routes that prefer
  cover, a different lane or entry each round.
* Looking around (`look.rs`): each bot keeps a picture of how far it can see in 16
  directions and checks corners, long sightlines and the way enemies come.
* Where enemies come from (`threat.rs`): routes in from the team spawns, and the door or
  corner an enemy would come through.
* Fighting: cover fighting with peeks and jiggle peeks, reloading behind cover,
  finishing hurt targets, staying on the current target, remembering enemies for 15 s,
  trading teammates, callouts.
* Aim (`motor.rs`): sights land off a new target and settle in, recoil kick, dial-in
  that tightens over time and partly fades after 2 s without shooting, crosshair
  placement. Reactions about 0.6–0.7 s close up, 0.8–0.9 s far away.
* Objective play: reliable plants and defuses, escorts, rotations between sites,
  defenders holding angles before the plant instead of chasing.
* Economy: full-buy, save and force-buy decisions like a CS team.

## Branding

* `crates/bootstrap/src/launch.rs`, `crates/ui/src/{gap_hud,menu_load,launch_report}.rs`,
  `crates/diag/src/lib.rs`: the window title and messages say Counter-Warfare 2; the
  Windows release ships the game as `counter-warfare2.exe`.
* `README.md`, `NOTICE`, this file.
