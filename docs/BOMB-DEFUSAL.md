# Bomb Defusal (`bd`, `bdc`)

Search and Destroy with a Counter-Strike 2 economy, in two modes:
Competitive Bomb Defusal (`bd`) and Casual Bomb Defusal (`bdc`). Competitive
is the default mode: new lobbies and `map` launches start in it unless another
mode is picked or `IW4L_GAMETYPE` names one.

```bash
IW4L_GAMETYPE=bd cargo run --profile play -p launcher -- map mp_rust
IW4L_GAMETYPE=bdc cargo run --profile play -p launcher -- map mp_rust
```

| | Competitive (`bd`) | Casual (`bdc`) |
|---|---|---|
| Teams (default bots) | 5 v 5 | 9 v 9 (CS plays 10 v 10; the engine seats 18) |
| Match | first to 13 of 24, sides switch at 12 | first to 8 of 15, no halftime |
| Round time / freeze / buy time | 1:55 / 15 s / 20 s | 2:15 / 6 s / 45 s |
| Money | start $800, cap $16000 | start $1000, cap $10000 |
| Armor | bought | free Kevlar + helmet every round |
| Defuse kit | bought by CTs ($400) | free for every CT |
| Team damage | on: 33% of bullet, 85% of grenade damage | off |
| Bots | full skill | slightly easier: 12.5% shakier aim (wider first miss, slower settle, more wander) and 0.1 s slower to react to a new target |

Both share everything else below. The game type picks the rules through the
`scr_bd_mode` script dvar (`competitive` / `casual`); each setting can still
be overridden by its own dvar.

## How it is built

The mode runs the map's stock `maps/mp/gametypes/sd` script (rounds, bomb,
spawns, HUD) and layers the built-in `iw4l/bomb_defusal` module on top
(`crates/assets/src/bomb_defusal.gsc.txt`). `GameModeKind::script_overlay`
adds that module to the startup entries after the map's `main`. The buy menus
are menuDefs in `crates/ui/menus/bomb_defusal.json`; item buttons answer with
`scriptMenuResponse bd:<id>`. Host bots send `bd:autobuy` on every new life.

## Rules

* Money: start and cap per mode (table above), CS2 prices and kill awards per weapon (knife
  $1500, SMG $600, P90 $300, shotgun $900, XM1014 $600, AWP/CZ75 $100, Zeus $0).
* Rounds: win $3250, bomb detonated or defused $3500. Loss bonus $1400–$3400
  with the CS2 counter (starts at one step, a win removes one step). Planting
  pays the planter $300 and every Terrorist $800 if the round is lost; a defuse
  pays $300. Terrorists alive when time runs out get no income. Teamkill −$300.
* Competitive is MR12: sides switch after 12 rounds, first to 13. The switch
  resets money, loss bonuses and gear. Casual has no switch.
* Survivors keep their weapons (refilled to starting ammo), grenades, armor and
  kit; the dead respawn
  with the side's starting pistol: the USP .45 for Terrorists and the
  USP .45 Silencer for Counter-Terrorists (both $200). The $300 step up for
  either side is the M9 (CS's P250: FMJ rounds for armor penetration, Steady
  Aim, −10% damage, listed in the bonus column); the full-auto G18 is the CZ75-Auto. No killstreaks, perks or deathstreaks.
* No music and no "time is almost up / winning / losing" announcer lines;
  objective callouts stay. Prices you can't afford show in red.
* The attacking team sees a bomb waypoint over the carrier and a
  `BOMB: <name>` line under the money; only the carrier gets the plant prompt.
  Bodies are cleared between rounds.
* While spectating, Fire / Aim switch to the next / previous teammate.
  The followed player's name shows above the HUD.
* Each gun's bullet damage is scaled so the two sides' counterparts are
  even and a pricier gun out-damages a cheaper one of its kind. The scale
  was solved from MW2's own damage, falloff ranges and fire rates against
  a target time to kill through Kevlar (mean of 300, 700 and 1200 units,
  per-weapon armor penetration):

  | Kind | T | CT | Shared |
  |---|---|---|---|
  | Starter ($200) | USP .45 −13% | USP .45 Silencer −5% (shorter falloff) | |
  | Pistols $300–700 | TMP −2% | M93 Raffica −5% | M9 −10%, Akimbo M9 −2%, G18 −2%, .44 Magnum −2%, Desert Eagle −2% |
  | SMGs | Mini-Uzi +13% | Vector +8% | UMP45 +3%, PP2000 +44%, MP5K +5%, MP5K Silencer +5%, P90 +23% |
  | Rifles | TAR-21 −9%, AK-47 +8%, F2000 −6% | FAMAS +5%, M4A1 +39%, M4A1 Silencer +38%, AUG +9% | RPD +1% |

  MW2's M4A1 does 30 to the AK-47's 40 per hit at the same falloff, so it
  needs the larger scale to land just ahead of the cheaper AK, as in
  Counter-Strike; the silenced M4A1 falls off sooner and so sits behind both.
  The $5200 MG4 (+17%) hits about as hard as the AK-47 and pays for its
  100-round belt; snipers and shotguns keep +0–5%. Pistols come with triple
  reserve ammo.
* Price tiers: guns above the starter pistols carry bonuses while held —
  Steady Aim (M9, SMGs, shotguns, TAR-21/FAMAS, RPD), Sleight of
  Hand (AK/M4/SG 553/AUG, auto-snipers, M249), Quickdraw (snipers), FMJ or
  Grip variants on rifles, snipers and heavy guns. The buy menu lists each
  gun's bonuses and damage change.
* A match with no bot counts set in the lobby starts with 9 bots (Competitive,
  a 5v5 with you) or 17 (Casual, 9v9), auto-assigned. Lobby bot settings
  override this.
* Host bots buy on every life, pick up and plant the bomb, and defuse it. In
  this mode they move like a recorded human match. Out of a fight they look
  around rather than at the nearest wall: each bot keeps a rough picture of
  how far it can see in 16 directions and checks long sightlines, corners and
  doorways, the way enemies would come and whatever it has not checked
  lately (well behind that way scores low), nearer angles first, for about 0.6-1.7 s walking and 1-3 s
  holding. Walking keeps the look within about 135 degrees of the path (so
  they strafe and walk diagonally), sprinting looks ahead, and a hold
  favours the angle chosen to hold. Routes lean toward walls and cover: a
  step out in the open costs 30% more, so a much shorter open route still
  wins. They crouch now and then, sprint in short bursts out of spawn, move
  at full speed on diagonals like movement keys do, and pause for a moment
  after the freeze (tuned from recorded human matches on Afghan and Favela); attackers
  pick a site per round from the score and escort a teammate carrying the
  bomb. Teammates heading the same way each take their own line, a
  different one each round: in most rounds a lane beside the direct route
  (its side and width rolled per round), in about 40% an entry from another
  side (a point off to one side of the goal, swung 30-80 degrees from
  straight on). Defenders shift their guard spot around the site each round
  too. They fan out at the goal
  (about 240 units around a site, 210 around the bomb carrier). While
  walking, a bot drifts sideways off a teammate closer than about 150 units
  rather than walking on top of them; it never stops or turns back for
  this, so teammates can still move and stand close together.
  Defenders split across both sites and each holds its own covered spot with a
  view of the site (distance by weapon, below), shifting between it and
  neighbouring spots; after a plant only the nearest defender defuses while the
  rest guard, and the attackers guard the planted bomb the same way.
  Defenders rotate: once two attackers are seen or called out within 1300
  units of one site, the other site's defenders head over, the one nearest
  the quiet site staying as its anchor unless three or more were seen at the
  hit site. The call holds for 30 s after the last sighting, and a later
  call at the other site turns them back.
  Until the bomb is down, defenders let the attackers come to them: an
  enemy heard, called out or lost from sight more than 500 units away is
  held with sights up from where the defender stands rather than chased;
  only a near or already hurt enemy is pushed.
* Each bot keeps a favourite kind of gun for the match (most rifles; about
  15% snipers, 20% shotguns/SMGs, 7% LMGs) and buys it when it can afford it
  with a vest, plus a frag and one tactical (smoke, stun or flash). Bots
  manage money as a CS team does: after the pistol round a bot without a
  gun full-buys (rifle, vest, and a kit for a CT) when it can, saves when
  the next loss bonus would get it a full buy or it cannot even afford a
  FAMAS/TAR-21 and vest, and otherwise force-buys the best it can; the last
  round of a half and match point are always forced. How a bot
  plays follows its primary, as in recorded human matches on Highrise:
  snipers trail the bomb about 800 units back and watch the site, and defend
  700–1500 units out toward their own spawn, holding each angle for a long
  time; shotguns and SMGs sprint longer, rarely crouch, and defend 600–1300
  units out on the side the attackers come from, moving every couple of
  seconds; rifles and LMGs hold 250–850 units from the site and crouch more.
  The bomb carrier sprints out of spawn and heads straight in without
  waiting for escorts. Escorts follow whatever route the carrier takes,
  including down stairs and through drops behind breakable windows. Routes
  avoid windows unless one saves a long way round (a pane costs as much as a
  1200-unit detour); only the bot nearest a pane shoots it out, aiming exactly
  at the intact glass, and gives up and pushes on after 2 s. Bots do not
  shoot out glass to start a fight more than 1200 units away. The carrier and
  any push not following a carrier take a different detour each round, up to
  60% of the trip (1600 units) off the direct line, so routes vary; a moving
  goal is only re-planned once it has drifted about 30% of the distance away
  (at most 600 units), so a fast carrier does not keep restarting the search.
* Bots throw grenades: on the attack, before going in, they put a frag,
  stun, smoke or flash onto the site from 600–1500 units away; on either
  side they frag or stun an enemy that ducked out of sight 1–6 s ago
  350–1300 units away, and defenders smoke off a far approach. A throw is
  only made along an open arc, and an enemy coming into view cancels it.
* Smoke, flashbangs and stuns work on bots: a bot cannot see through a
  smoke cloud (about 230 units across, for 14 s after the grenade pops), a
  flashbang blinds it for most of the flash, and a stun (or the tail of a
  flash) slows its turning and shakes its aim. Frozen in the buy period,
  bots stand still. A shotgun is not fired past 650 units; with the nearest
  enemy beyond 900 units a bot draws its pistol, and back to the shotgun
  inside 450 units. Idle bots step off a teammate they are standing on.
* Teamwork and awareness: an enemy one bot sees is called out to its
  teammates (within 3500 units, half a second later, placed roughly). Only
  the nearest teammate with an SMG or shotgun pushes a callout; the rest
  pre-aim toward it while they walk or hold (aiming straight at it where
  there is a line to it, otherwise looking that way). When a teammate is killed, bots
  within 1500 units learn where the killer is and go to trade them at once
  (ahead of anything but a fight in view). Shot by someone it
  cannot see, a bot turns toward where the hit came from. Bots strafe side
  to side in uneven steps while shooting in the open (snipers hold still),
  walk quietly
  (under footstep-hearing pace) with an enemy known within 1200 units, and
  defenders throw utility onto a planted site before retaking it.
* Cover: some fights are taken from cover (60%, 30% for SMG and shotgun
  users; none with a shotgun, against an enemy within 450 units or one
  already hit). The bot looks for a spot within 160 units that its target
  cannot see. If it finds one it peeks out to shoot for 1.8-3.2 s, then
  ducks back for about half a second. It ducks early when its magazine is
  nearly empty or it is down to 35 health (or has lost 50 since peeking),
  and reloads behind cover under 70%. An enemy it has visibly hit (under 70
  health) is kept under fire past the peek's end, and chased rather than
  waited for if they duck out of sight. Any other enemy who drops out of
  sight gets up to three quick 0.3 s jiggle peeks. After two peeks without a
  kill the bot gives up the cover and fights in the open for 10 s, so a
  duel cannot stall while both sides regenerate.
* A bot keeps shooting the enemy it is fighting while it can see them; a
  different enemy only takes over when under 60% of the distance and at
  least 250 units nearer. A new fight starts on the closest enemy in view.
* Bots only take on enemies their gun can reach: pistols and SMGs 1800
  units, shotguns 900, rifles and LMGs 3000, snipers any distance in sight.
* Perception: within 1500 units bots notice enemies across a 140 degree
  field; beyond 3000 units only snipers notice anyone off the centre of view
  (others need them within 25 degrees). They hear unsuppressed gunfire within
  2500 units (suppressed 500) and running or sprinting footsteps within
  700/1000 units, placing the sound roughly. Reaction to a new target is
  quicker up close (about 0.6-0.7 s) than far away (0.8-0.9 s), except for
  snipers, who react at long range as at mid range. Crosshair placement
  counts: an enemy appearing within 12 degrees of where a bot is aiming is
  shot 0.1 s sooner, and the first miss on a new target shrinks with the
  turn needed (down to 30% of it for a target already under the sights), so
  a defender holding the right angle wins the first shot.
* Fighting follows recorded players: with a shot, rifles and LMGs hold their
  angle out to 1800 units, snipers at any range, SMGs close to about 700 and
  shotguns to 300 (less for bolder bots). The enemy a bot was fighting stays
  its business when it slips out of sight: for 1.5 s the bot holds the angle
  where they went, sights up, for the re-peek, then hunts them down, ahead
  of the objective, for up to 6 s; a teammate's death is only traded once
  the fight in front of the bot is over. Any other enemy lost from sight or
  heard within 1500 units is pushed for up to 4 s; SMG/shotgun users and bold bots
  come at them from the side when they are more than 400 units away, while
  snipers keep their angle. Enemies seen within 2000 units, or heard or lost
  within 1500, come before the objective; the bomb carrier and the defuser
  only stop for an enemy they can see within 1200 units, and nobody stops
  once a plant or defuse is half done or the bomb cannot wait (under 5 s
  slack). A carrier or defuser already in the zone keeps at it unless an
  enemy is in sight within 600 units, and the bomb in hand never counts as
  an empty gun. The use key goes down only once the sights are on the bomb
  (the defuse only starts on a press made while looking at it); a press
  that starts nothing within a second is let go and made again. A reload
  queued as the bot reaches the bomb is dropped (MW2 reloads and uses on
  the same key), and the planter or defuser starts no grenade within 700
  units of the bomb.
* Where enemies come from: during the buy freeze the bots note where each
  team starts, and once a round work out the ways in from the enemy start
  over the navigation graph. A bot looks along the route an enemy would
  actually take to reach it (the point 600 units back along it: the door
  or corner they would come through), defenders hold their angle on it,
  and sounds and sightings in the last 10 s take over when there are any. Bots remember where they saw enemies for 15 s: for the
  first 10 s, while walking or holding, they keep an eye on the most recent
  spot within 2500 units (aiming at it where there is a line to it).
  Ducking behind cover or reloading does not count toward giving up on a
  fight the bot cannot shoot in.
  A bot that stops making progress, or holds with no route, cycles a
  crouched push toward its goal (low gaps), a jumping push (slopes and
  lips) and a side step; a detour or flank point found unreachable is
  dropped for the goal itself.
* Semi-automatic weapons are fired at a player's pace with an uneven
  rhythm: shots land roughly 150-250 ms apart up close, 200-300 ms at mid
  range and 300-350 ms beyond 1200 units, pistols a little slower.
* Aim: on picking up a target a bot's sights land off it in a random
  direction (a few degrees, more for shakier bots) and close in over about
  half a second, leaving a small wander; early shots can miss but the error
  never stays put, so no fight is decided by a bot that cannot hit. Every
  shot a bot fires knocks its sights off (mostly up) and it has to pull
  back on, so a first hit leaves time to react. The longer it keeps shooting
  the same target the more it dials in: the kick shrinks and its error is
  capped (3.15 degrees at first, shrinking toward 0.6), so a bot never sprays
  far off a target for long. From 1.5 s after its first shot at a target it
  locks on harder: over the next second the kick, the error cap and the hand
  wander shrink by up to 2.5 times (the cap floor down to 0.35 degrees), so
  drawn-out fights finish. After 2 s without a shot at its target (behind
  cover, reloading, or the enemy gone) half the dial-in is lost: when the
  bot opens fire again its sights land off target by half a fresh
  acquisition, and it keeps half the shots and lock-on time it had built.
* Bots aim down the sights in fights past 250 units (snipers always) and
  fire once the sights are mostly up (0.15 s SMGs/shotguns/pistols, 0.2 s
  rifles, 0.3 s LMGs, 0.4 s snipers). Automatic weapons fire full auto
  inside 600 units, short bursts to 1500 and single taps beyond.
* After two seconds with no enemy in view a bot tops up a magazine below
  half and goes back from the pistol to its main gun if that has ammo. An
  empty main gun mid-fight is reloaded unless an enemy is within 800 units,
  when the pistol is drawn instead.
* Bots react to a new target in about 600–700 ms close up, 675–775 ms at
  mid range and 800–900 ms far away (snipers as at mid range; Casual adds
  100 ms), aim with a per-bot offset plus a slow drift, and bring the sights
  up before firing.
* Bots tap the trigger on semi-automatic and burst weapons. A bot that cannot
  get a shot off in a fight for 3 s goes back to its objective for a few
  seconds to find a new angle instead of holding forever.
* **B** (`+actionslot 2`) opens the buy menu during the freeze time and the
  buy time of the round (per mode, table above), within 1000 units of the
  spawn.
* Kevlar/helmet absorb damage with per-weapon armor penetration; the defuse kit
  halves the 10 s defuse.

## Teams

* Joining (and "change team" in game) opens a Counter-Strike style CHOOSE
  TEAM menu in place of the class menu: COUNTER-TERRORISTS, TERRORISTS or
  AUTO-SELECT (the smaller team). Picking a side that is already a player
  ahead moves one of its bots to the other side, as Counter-Strike does, so
  the pick always goes through: the stock per-side cap (half the server) is
  lifted for that swap, since a full Casual server has both sides at it, and
  bots identify themselves to the script as soon as they join so there is
  always one to move. CT is always the map's allied ("good guy")
  faction — Rangers, Navy SEALs or Task Force 141 — and defends; T is always
  the opposing faction — OpFor, Militia, Spetsnaz or Airborne — and attacks.
  At halftime (after round `scr_bd_roundswitch`, 12) every player changes
  team as in Counter-Strike: CT players become T and take the T faction, and
  the other way round, keeping their team's score; money and gear reset.
  The scores move with the players when the next round starts: the round
  wins (what the scoreboard shows and the win limit counts) and the team
  scores are swapped together and pushed to the scoreboard.
  Classes are not used: the default class is taken and gear comes from the
  buy menu. Switching team mid-round waits out the round as a spectator of
  the new team.
* The team and buy menus use MW2's in-game menu look; the team cards show
  the map's faction logos and names, and each whole card is the button.
  Change Team in the pause menu opens the same screen.
* A completed defuse puts the kit away at once (the round ends and freezes
  everyone before the stock script would).
* Standing where the bomb can be planted or defused gives the use key to the
  bomb; guns lying on the ground there cannot be picked up instead.
* The carrier is marked by the BOMB icon over them and the HUD line; Search
  and Destroy's own escort waypoint is hidden.
* Spectating a teammate shows the names over other teammates and keeps the
  view sharp: the death blur and the aim-down-sights focus are not applied
  to a spectated view.

Dvars (set before the match): `scr_bd_startmoney`, `scr_bd_maxmoney`,
`scr_bd_freezetime`, `scr_bd_buytime`, `scr_bd_buyzone` (0 = anywhere),
`scr_bd_planttime`, `scr_bd_defusetime`, `scr_bd_bombtimer`,
`scr_bd_roundswitch`, `scr_bd_winlimit`, `scr_bd_roundlimit`, `scr_bd_timelimit`,
`scr_bd_freearmor`, `scr_bd_freekits`, `scr_bd_teamdamage` (each 0/1; their
defaults come from the mode).

MW2 has one lethal and one tactical offhand slot: buying a second lethal
(HE / Molotov / Zeus) or a different tactical replaces the first. Flashbangs
stack to two.

## Known issues

* mp_invasion favors the attackers: in bot-only tests T wins about 11 of 12
  rounds there, while mp_afghan and mp_derail are close to even. Defenders
  reach the sites first and lose the site fights; map-specific defender
  positions (further back, crossfires) are the likely fix.
* Switching team three times in quick succession once lost the third pick
  (mid-round switches sit out the round); not reproduced since.
* The game menu not opening while spectating was reported once and not
  reproduced.

## Testing

* Bot-only balance test: add a temporary `logPrint` of the round winner in
  `bdRoundEnd`, make the host spectate (a temporary `[[ level.spectator ]]()`
  for entity 0), and run several maps in parallel with short settings, e.g.
  `IW4L_SCRIPT_DVARS="set scr_bd_freezetime 6;set scr_bd_roundswitch 6;set scr_bd_roundlimit 12"`
  and `--cmds "wait world; wait 3; bot add 1; wait 110; ...; finish_run"`
  (console `wait` is capped at 120 s, chain several). Results swing a lot
  between runs; compare 30+ rounds.
* `--cmds` also takes `menukey up|down|enter|escape`, `openmenu <menu>`,
  `screenshot`, `record <name>` / `stoprecord`, `bot hold on`, `bot tp ...`.
* Script compile errors print `gsc: refused` on the console; `logPrint`
  output and script runtime errors are in `iw4l-artifacts/logs/latest.log`.
* When editing the GSC through a script, check that `"\n"` stayed an escape;
  the compiler refuses a newline inside a string.
* Checks: `cargo clippy -p bots` and `cargo test -p bots`.
