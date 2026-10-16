# Counter-Warfare 2

Counter-Warfare 2 brings a Counter-Strike style **Bomb Defusal** mode to Call of Duty:
Modern Warfare 2 (2009) multiplayer maps: a CS2 money economy, a buy menu with
CS-equivalent MW2 guns, CT vs T sides, halftime side switches and bots that buy, take
cover, peek, trade and play the bomb like a team.

It is a fork of [IW4L](https://github.com/vladtrc/iw4L) by vladtrc, an open-source
runtime for MW2 written in Rust with [Bevy](https://bevy.org/). Point it at a copy of
MW2 you already own and it loads that installation's maps, models, textures and weapons
into its own engine. No game assets are included.

<p align="center">
  <img src="docs/screenshots/bomb-plant.jpg" width="49%" alt="Bomb planting">
  <img src="docs/screenshots/tanker-explosion.jpg" width="49%" alt="Tanker explosion">
</p>

## The two modes

| | Competitive (`bd`) | Casual (`bdc`) |
|---|---|---|
| Teams (default bots) | 5 v 5 | 9 v 9 |
| Match | first to 13 of 24, sides switch at 12 | first to 8 of 15, no halftime |
| Round / freeze / buy time | 1:55 / 15 s / 20 s | 2:15 / 6 s / 45 s |
| Money | start $800, cap $16000 | start $1000, cap $10000 |
| Armor / defuse kit | bought | free Kevlar + helmet; free kit for every CT |
| Team damage | 33% bullets, 85% grenades | off |
| Bots | full skill | 12.5% shakier aim, 0.1 s slower reactions |

CT is the map's allied faction and defends; T is the opposing faction and attacks.
Press **B** to open the buy menu during freeze time and buy time, near your spawn.
Full rules, prices, weapon balance and the `scr_bd_*` settings are in
[docs/BOMB-DEFUSAL.md](docs/BOMB-DEFUSAL.md); what changed from IW4L is in
[CHANGES.md](CHANGES.md).

## Windows: prebuilt release

1. Download `counter-warfare-2-windows.zip` from [Releases](../../releases) and extract
   it into an empty writable folder.
2. Launch `counter-warfare2.exe`. It finds MW2 in your Steam libraries and creates a
   `Modern Warfare 2.lnk` shortcut next to itself. For an install outside Steam, create
   that shortcut to your MW2 folder yourself.
3. Pick COMPETITIVE or CASUAL BOMB DEFUSAL from the game modes, or start a map directly:

```bash
counter-warfare2.exe map mp_afghan
```

Online play through IW4L's servers is not supported: this build's game rules and
protocol differ from IW4L's. Do not put an IW4L `.iw4l-server` file next to the exe;
with one, the game would update itself to IW4L's build.

## Build and run

Install Rust through rustup and GNU Make. [Build dependencies](docs/BUILD.md) cover
Linux's C/C++ toolchain and system libraries, and macOS's Xcode command line tools.
[Windows instructions](docs/WINDOWS.md) cover building and arranging a portable folder.

You need your own installed MW2 Multiplayer data. Nothing here patches or replaces the
installation's files. Caches, demos and logs go under `iw4l-artifacts/`; Linux settings
use a separate configuration directory described in the [run guide](docs/RUN.md).

From the repository root:

```bash
cp .env.example .env
# Edit .env: set IW4L_GAMES to the folder containing your game installations.
IW4L_GAMETYPE=bd make map mp_invasion
```

`IW4L_GAMETYPE=bdc` starts Casual. `cargo build --profile play -p launcher` builds the
game alone, into `target/play/iw4l.exe`.

## Known issues

* mp_invasion favors the attackers: in bot-only tests T wins most rounds there, while
  mp_afghan and mp_derail are close to even.
* Gameplay outside Bomb Defusal is IW4L's and remains incomplete; expect missing
  behavior, bugs and desyncs.

More in [docs/BOMB-DEFUSAL.md](docs/BOMB-DEFUSAL.md#known-issues).

## Inside the engine

| Area | Implementation |
|---|---|
| Assets | Native FastFile readers convert game data into a shared intermediate representation. |
| Shaders | Retail Direct3D 9 Shader Model 3 bytecode is translated to WGSL. |
| Rendering | World geometry, models and effects feed one sorted draw-surface list. |
| Simulation | Server authority, client prediction and replay share one simulation step over explicit Bevy ECS state. |
| Networking | Custom UDP traffic; a QUIC master provides browsing and relaying. The host simulates the match. |
| Bomb Defusal | A GSC overlay on Search and Destroy (`crates/assets/src/bomb_defusal.gsc.txt`), buy and team menus (`crates/ui/menus/bomb_defusal.json`), and the bot AI in `crates/bots`. |

## Where to go next

- [Bomb Defusal rules](docs/BOMB-DEFUSAL.md) and [changes from IW4L](CHANGES.md).
- [Run guide](docs/RUN.md): console commands, classes and demo playback.
- [Rendering](docs/RENDER.md) and [simulation](docs/SIM-STEP.md): inspect the engine's implementation.
- [Map loading](docs/MAP-LOAD.md), [GSC runtime](docs/GSC-RUNTIME.md) and [bot AI](docs/BOTS.md): starting points for experiments and modifications.
- [Documentation index](docs/INDEX.md), [contributing](CONTRIBUTING.md) and [security reports](SECURITY.md).

IW4L, and the Bomb Defusal work on top of it, were written by an LLM.

## Acknowledgements and license

Counter-Warfare 2 is built on [IW4L](https://github.com/vladtrc/iw4L), created by
vladtrc. Thank you to IW4L's contributors, and to **ju1cedr1nker** and
**silvernote03** for QA testing IW4L's weapons, maps, attachments and other gameplay
features.

[OpenAssetTools](https://github.com/Laupetin/OpenAssetTools) and its [iw4x-x64
fork](https://github.com/iw4x-x64/oat) informed asset layouts;
[IW4x](https://github.com/iw4x/iw4x-client) informed asset and protocol behavior;
[KisakCOD](https://github.com/SwagSoftware/KisakCOD) informed engine structure.
[Ghidra](https://github.com/NationalSecurityAgency/ghidra) was used to inspect the
original binaries.

Movement implementation history and source boundaries are recorded in
[its provenance note](docs/provenance/movement-iw4.md).

The source is licensed under [Apache 2.0](LICENSE). Preserve required attribution and
bundled font license texts when redistributing; see [NOTICE](NOTICE).

Counter-Warfare 2 is a free fan project. It is not affiliated with or endorsed by
Activision, Infinity Ward or Valve. Call of Duty, Modern Warfare and Counter-Strike are
trademarks of their owners. You need your own copy of Modern Warfare 2 to play.
