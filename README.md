# Vice-Tay

Vice-Tay runs an Xbox Series title natively on Windows, from files you supply,
through [VIctory](https://github.com/Marvisak/VIctory), a compatibility layer:
it loads the console executable, answers its system calls and translates its
GPU work to Direct3D 12. This tree is tuned and fixed on one PC, an RTX 4090
with a Ryzen 9 9950X3D and 48 GB of RAM, toward one goal: the open world at
30 frames a second, with the picture right, fully playable.

**No game files, code or assets are in this repository.** You need your own
copy of the game. Vice-Tay is not affiliated with or endorsed by the game's
developer or publisher.

> [!TIP]
> **[View the gameplay screenshots](https://github.com/TaynanGT/Vice-Tay-Screenshots#readme)**
>
> Ten captures from an earlier Series S playtest, on build `672cf831`.

## This branch: an 8 GB NVIDIA card

Branch `vice-tay-nvidia-8gb` is upstream Vice-Tay plus changes for a smaller
PC: an RTX 5060 Ti 8 GB, a Ryzen 5 7600X (6 cores) and 31 GB of RAM, on the
Series S profile. Measured in the prison yard (opening cutscene), diagnostics
build:

| Setup | fps | Notes |
|---|---:|---|
| Upstream `-Machine 4070` profile | 4.2 | card 470 MB over its 6.3 GB budget, 5.2 GB read from the PC's memory |
| + the 8 GB settings below | 16.5 | picture unchanged |
| + `-RayTracing` (with the watchdog fix) | 13.3-14.0 | ray-traced indirect light; colour banding on walls |
| Release build, ray tracing off, `VICTORY_WINDOW_KB=1024` | 17.3-18.3 | release instead of diagnostics: +1-2 fps; the 1 MiB window grid: same fps, 0.5 GB less RAM |
| + ports from trustno1-coder/VIctory-NVIDIA (below) | 18.6-19.5 | the submitting thread's helpers at its priority; picture unchanged |

What it adds:

- **8 GB settings** (passed as environment variables): `VICTORY_LEVELS_ON_DEMAND=1`,
  `VICTORY_TEXTURE_BUDGET_MB=1024`, `VICTORY_CARD_BUFFERS_MB=1536`,
  `VICTORY_CARD_WINDOWS_MB=1024` with the 4070 profile's worker counts.
  Capping the copies of textures picked by index frees room for card copies
  of the title's buffers, which shaders otherwise read over PCIe. Each change
  alone did nothing; together, 4.2 -> 16.5 fps. A 512 MB texture cap frees
  600-850 MB more but leaves textures missing (clothes render black).
- **The title's hang watchdog no longer ends a run.** A slow first world load
  (ray tracing on, an 8 GB card) outlasted it, and it wrote to address 0.
  `VICTORY_TITLE_WATCHDOG=1` keeps it.
- **`VICTORY_WINDOW_AT=<x>,<y>`**: the title's window and the presenter's open
  there (a virtual monitor), never full screen and never taking focus, so a run
  can be stepped with `VICTORY_KEYS_FILE` while the desktop is in use.
- **`VICTORY_CAPTURE`**: a file named `want` in the folder saves the next frame
  (it was read twice per present, and the second read missed it).
- **`VICTORY_REFRESH_FEW=<n>`**: how many of a submission's refreshes are checked
  for "nothing new" on the submitting thread before the threads beside are
  woken (256 upstream). With all of them there, the submitting thread's work fell
  from 71-85 to 61-63 ms a frame and the title's wait at presents from 39 to 29 ms;
  the cutscene's own swings (14-17 fps) hide the frame-rate change in one run.
- **Textures the card has no room for are made at low resolution** (none of their
  levels over 64 KiB) instead of being refused, which showed them black: walls,
  the cell window, the yard and cloth. 23,621 such textures in the prison yard,
  the card kept under 98 % of its budget, fps unchanged. `VICTORY_TEXTURE_LOW=0`
  refuses them as before. With that, the indirect-light stand-in
  (`VICTORY_AMBIENT`) only darkened the outdoors and made hair glow: the
  launcher here runs without it, and with `VICTORY_CARD_WINDOWS_MB=512`.
- **Ported from [trustno1-coder/VIctory-NVIDIA](https://github.com/trustno1-coder/VIctory-NVIDIA)**
  (their authors kept): the submitting thread's helpers run at its priority
  (Indy2l, +1 fps here); violet probe leftovers dropped before the ambient
  raise (Ahyildiz, the purple blotch); `VICTORY_SKIP_BINK=1` skips the intro
  movies (C-GDev via CuteBooty: the world 3:00 after launch instead of 5:28);
  raw buffer views kept inside their buffer (lichenlang89 via CuteBooty,
  `VICTORY_RAW_VIEW_CLAMP=0` restores). Their DLSS and frame generation come
  with another renderer (CGBJ's) and are not ported; the BC7 and descriptor
  slot fixes were here already.
- **`VICTORY_WINDOW_KB=1024`** (an upstream option, measured here): copies of title
  memory outside buffers on a 1 MiB grid instead of 4 MiB. Same fps, 0.5 GB less
  private memory, and the PC's free memory 0.9 -> 1.8 GB in the prison yard.
  256 KiB fell to 4.4 fps; `VICTORY_WINDOW_TAILS=1` to 3.0 fps.
- **Pipelines from the title's pipeline cache** (device methods 0x1c8, 0x218;
  `VICTORY_PIECE_PIPELINES=1`, off by default): every fork leaves them
  unimplemented, and the title then makes the same pipelines from its shader
  containers, so nothing is missing. Implemented here (pieces decoded, vertex
  fetches turned into input layouts, host pipelines per bound targets) but the
  picture broke with them on, so they stay off.
- **`VICTORY_TEXTURE_DROP_LEVELS=<n>`**: textures with levels on demand go without
  their n largest levels. None in the prison yard qualified, so it had no effect there.

Tried and not kept: `VICTORY_SEND_CHANGED=1` (copies slower: 15 against 16.5 fps),
`VICTORY_SYNC_FINE=1` (no change), `VICTORY_BVH_SIZES_WAIT=1` (late structure
sizes mostly fixed, banding unchanged, 15 % slower).

## At a glance

- **It plays.** On the Series S profile the prologue runs from the prison
  through the boat section to an island, driven by hand, with failed attempts
  picked up again from checkpoints ([Gameplay](#gameplay)).
- **It is not fast enough yet.** 21.1 fps at the median over the latest
  30-minute session, short of the 30 fps goal ([Tests](#tests)).
- **Every change is tested twice:** by CI, and by playing it on this PC with
  the frame rate, memory and card measured from the game's own frame lines.

## Gameplay

What a session plays, on the Series S profile, as in the latest 30-minute
session (minutes in the world):

1. **Loading.** The start screen after 72 s, the world after 120 s.
2. **The prison and the ride out** (0-4 min). Cutscenes carry the story from
   the prison to a boat on open water.
3. **The boat, by hand** (4-25 min). Following a plane to the package it drops,
   with Focus to find it, then the route the map draws: under a road bridge and
   an iron bridge, along a mangrove channel, past a broken pier and across a bay
   to an island with a crashed plane.
4. **Failing and trying again.** Running aground ends the mission ("the boat is
   no longer usable"). The game offers to repeat the checkpoint, skip it or
   restart the mission; repeating and skipping both work, and the run goes on.
5. **On foot** (25-30 min). Off the boat on the island's beach, with the
   inventory tutorial; the pause menu opens and closes.

The HUD, minimap, route, mission prompts and subtitles work throughout. Since
[#57](https://github.com/TaynanGT/Vice-Tay/pull/57) a performance panel sits in the top-left corner of every session: FPS and
frame times with a graph, resolution, draws, GPU and CPU with their load, VRAM
and RAM. **F3** hides it, `-NoPanel` starts without it. Every session is
recorded to video on the card's encoder.

### Series S gameplay clip

The boat section on the RTX 4090 test PC, with the performance panel visible.

https://github.com/user-attachments/assets/bd07891c-c53f-4f46-9b7e-80463c379b84

## Tests

Every change goes through the same two steps before it is merged:

1. **CI.** Clippy with and without the diagnostics feature, every test in both
   configurations, and the repository policy check (`cargo xtask`), locally and
   on GitHub.
2. **A session in the game.** The Series S profile in full screen, played by
   hand or by a scripted route (walk, run, turn the camera, a capture every
   minute). Nothing compiles meanwhile. A change that could cost frames is run
   against a control of the same base, the same route and the same length.

### Latest sessions

Newest first. fps from the game's once-a-second frame lines in the world (over
1,000 draws a frame); memory is the game's private bytes, sampled every 20 s.

| Session | Build | Played | In the world | fps: median (p10-p90; 1% low) | Game memory, median / max | What it showed |
|---|---|---|---|---|---|---|
| Performance panel | `5963f415` ([#57](https://github.com/TaynanGT/Vice-Tay/pull/57)) | scripted route | 5 min | 20.8 (16.8-23.9; 1.4) | 26.2 / 27.1 GB | The panel on screen at no cost in frame rate; GPU load read from Windows (87% in the boat scene) |
| The boat section | `bc090416` | by hand | 30 min | 21.1 (19.1-23.2; 17.0) | 23.5 / 24.1 GB | The prologue through the boat section; frame rate eased from 21.7 to 20.6 fps and memory grew by about 1 GB from the first ten minutes to the last |

Each session's timeline, the frame broken into its parts and the card's
readings: [docs/tests.md](docs/tests.md) and
[docs/measurements.md](docs/measurements.md). What earlier sessions found and
fixed: [docs/findings-rtx4090.md](docs/findings-rtx4090.md).

### Under test, not merged yet

| Change | From | Status |
|---|---|---|
| Title textures uploaded at the start of each submission (frame order, flickering) | [Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD) `5d6bd841` | CI green; Series S session against a control |
| Wave64 groups over 64 threads keep exact barriers through divergence (translator version 3) | the C01 translator batch | Reviewed; Series S session against a control |
| PC FSR 4.1.2 to 3840×2160 (`-Fsr4K`, Series X profile) | Belobug, from [DexNom/VIctory](https://github.com/DexNom/VIctory) `ab34a8b2` | Ported; needs AMD's signed upscaler DLL |
| DLSS 4.5 Super Resolution (`-Dlss4K`) on the same bridge | this tree | Being written |

### Test reliability

Four `victory-gpu` tests failed CI now and then. Three shared global state with
tests running beside them (the one asking of `watch::ask_once`, the heap-sync
thread) and were fixed in [#55](https://github.com/TaynanGT/Vice-Tay/pull/55);
the fourth assumed where a heap would land and was fixed with the panel. Over
the whole `victory-gpu` suite run 15 times per commit: 3 failures before, none
in 30 runs after.

## Test PC

| Component | Configuration |
| --- | --- |
| CPU | AMD Ryzen 9 9950X3D, 16 cores and 32 threads |
| Motherboard | MSI MAG X870 TOMAHAWK WIFI (MS-7E51), revision 1.0 |
| BIOS | MSI 1.A93 |
| GPU | NVIDIA GeForce RTX 4090, 24 GB GDDR6X |
| NVIDIA driver | 621.40 |
| Integrated graphics | AMD Radeon Graphics |
| RAM | 48 GiB DDR5 (2 × 24 GiB), configured at 8,000 MT/s; 43.4 GiB visible to Windows |
| Page file | 128 GiB |
| System SSD | Samsung 990 PRO 4 TB NVMe |
| Additional SSDs | Samsung 990 PRO 2 TB NVMe and Samsung 970 PRO 1 TB NVMe |
| Monitor | Samsung Odyssey OLED G8 (G81SF), 3840 × 2160, 240 Hz |
| OS | Windows 11 Pro 26H2 x64, build 26300.10075 |

## Known problems

- **Frame rate.** Below 30 fps in the world. Of a 47 ms frame, about 34 ms go to
  preparing the GPU's work on the CPU and 13 ms to copying data back.
- **Memory.** The game holds 23-27 GB on the Series S profile. On a PC with less
  free RAM than that, Windows pages part of it out and frames stall.
- **Picture.** Skin, hair and some walls show flat bands of colour; some objects
  (tanks, tarps, plants, a skylight) glow in flat purple, pink or blue; some faces
  show scrambled colour patches and some eyes are lit white; clouds and reflections
  break into square blocks; a few shapes render as flat black.
  For banding that varies between launches, pass `-More @{VICTORY_RTV_FORMAT_LOG='1'}`
  to `scripts\play.ps1` on a diagnostics build. Compare the `RTV format check`
  totals and `RTV format mismatch` examples at the same checkpoint in a clean and a banded run.
- **Ray tracing.** The ray-traced structures are not built by default, so a few
  hundred dispatches a frame are skipped and some light is missing; `-RayTracing`
  builds them.
- **Loading.** The first run of a new build translates every shader while it plays;
  translate them ahead (below).
- **Measuring.** On the Series S profile the log's GPU-time counter reads near
  zero, so the tables leave it out; the panel's GPU load comes from Windows instead.

## Getting started

You need Windows 10 or 11 x64, a CPU with AVX2, a Direct3D 12 GPU with shader
model 6.6 and mesh shaders, Rust (the toolchain is pinned in the repository),
the MSVC build tools with a Windows SDK, and your own dump of the title.

```powershell
git clone https://github.com/TaynanGT/Vice-Tay
cd Vice-Tay
copy victory.example.toml victory.toml      # then set your paths, outside this folder
cargo build --locked --profile diag -p victory --features diagnostics
```

From the repository root, translate the shaders and warm the pipelines (each can
be stopped and resumed). With `game_dir` set in `victory.toml`, the shader
command scans that directory. To scan one archive, pass its quoted path, for
example `.\target\diag\victory.exe shaders "D:\My Game Dump\shaders.rpf"`.

```powershell
.\target\diag\victory.exe shaders       # scans game_dir in victory.toml
.\target\diag\victory.exe warm --used   # pipelines earlier runs drew with
```

Then play on the Series S profile, the one every session above uses:

```powershell
scripts\play.ps1 -Exe <path of your copy's executable> -Console series-s
```

If the shader cache contains fewer than 1,000 stored translations,
`play.ps1` runs `victory shaders` before launching the title. It scans the
configured game directory when available, or the executable's directory.
The driver may still build pipelines during the first run. Use `-SkipShaders`
to skip the translation step.

`Play-VIctory.cmd` takes the same options from a Command Prompt; on a double-click
PowerShell asks for the executable's path.

| Option | What it does |
|---|---|
| (none) | Full screen, Series X profile, settings picked for your graphics card, performance panel on |
| **F11** or **Alt+Enter** | Switch between full screen and a window while it runs |
| **F3** | Hide or show the performance panel |
| `-NoPanel` | Start without the performance panel |
| `-Windowed` | Start in a window |
| `-Console series-s` | Run as a Series S: a smaller picture, lighter on the card, faster; the profile every session uses |
| `-Machine 4090` / `7900xtx` | Force the settings for NVIDIA (measured on the RTX 4090) or AMD |
| `-Safe` | Leave out the changes still on trial; try it first when something goes wrong |
| `-RayTracing` | Build the ray-traced structures, so the title's ray-traced indirect light runs |
| `-Build release` | Run `target\release\victory.exe` (`cargo build --release --locked -p victory`); it has no panel |

Every `VICTORY_...` switch is described in a comment where the code reads it.
More commands and debugging switches:
[docs/README-fxceless.md](docs/README-fxceless.md) and [docs/findings.md](docs/findings.md);
recording one frame and running it again: [docs/replay.md](docs/replay.md).

### Direct3D 12 on Windows 10

If the Windows runtime rejects root signature flags `c00` or `c01`, build an
executable that requests the application-local DirectX Agility SDK. Download
[Microsoft.Direct3D.D3D12 1.620.2](https://www.nuget.org/packages/Microsoft.Direct3D.D3D12/1.620.2),
open the `.nupkg` as a ZIP and copy `build/native/bin/x64/D3D12Core.dll` into
`target\diag\D3D12\`. Build from the repository root:

```powershell
$env:VICTORY_AGILITY_SDK = '1'
try { cargo build --locked --profile diag -p victory --features diagnostics }
finally { Remove-Item Env:VICTORY_AGILITY_SDK }
```

Launch with `scripts\play.ps1` as usual. The `D3D12` folder goes next to the
executable. Use `target\release\D3D12\` for a release build. Keep the folder
beside the executable if you move it. For the optional D3D12 debug layer,
use `D3D12SDKLayers.dll` from the same package. A normal rebuild with
`VICTORY_AGILITY_SDK` unset uses the Windows runtime.

## How this tree is built

Vice-Tay is published as a repository of its own, not as a GitHub fork, with the
whole history of the trees it comes from. It started from codec-d/cool_fox (now
taken down), which is [avatarmanz/VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX)
with work for NVIDIA cards on top. New work from avatarmanz is merged as it lands,
and changes from other forks and our own fixes come in through pull requests,
each tested as described under [Tests](#tests) before it is merged. What each
change does: [docs/this-fork.md](docs/this-fork.md). How the layer works:
[docs/architecture.md](docs/architecture.md).

## Credits

This is other people's work with more on top. The history is kept, and commits
taken from elsewhere name the original commit and its author. Everyone with a
commit here is listed in [docs/credits.md](docs/credits.md).

- **[VIctory](https://github.com/Marvisak/VIctory)** by Marvisak and its
  contributors: the loader, the runtime services, the Direct3D 12 back end and
  the shader recompiler.
- **[fxceless/victory-fork](https://github.com/fxceless/victory-fork)** and the
  people whose pull requests it carries.
- **[avatarmanz/VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX)**:
  list calls recorded on the title's threads and translated by workers,
  submissions prepared on threads of their own, cheaper write guards, textures on
  a copy queue, frame record and replay.
- **codec-d/cool_fox** (taken down by its author): two-plane composition, fallback
  heaps, table plans, the residency guard, fence wakes on `WaitOnAddress`, wave32
  lowering for NVIDIA, movie-plane uploads and XMA audio fixes.
- **[DexNom/VIctory](https://github.com/DexNom/VIctory)**: ray tracing fixes,
  cube gradients, sparse textures.
- **[Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD)**:
  the performance panel, the ambient light idea, pipeline workers, and work by
  romainhedouin.
- **[FQY55kai/ZJOU-VICTORY](https://github.com/FQY55kai/ZJOU-VICTORY)**.
- **[myokex/victory-amd](https://github.com/myokex/victory-amd)**: the guard over
  every view of a page (with Spexivus, joroyt and tightfall), and Joshua Odom's
  reads into title memory.
- **Geckomacabre** and **cxy0072019-sketch**: the texture read-back.
- Pull requests to this repository by **meshahid973** (relocation checks),
  **nullexxx** (file logging; an empty pipeline library on vkd3d-proton),
  **cxy0072019-sketch** (saved pipeline scheduling left out on request) and
  **welisonzika14-jpg** (placed-filler bookkeeping kept out of default builds).
- **[XCL](https://github.com/ivall/xcl)** (GPL-2.0-only) was read for how the same
  title behaves under another layer; no code of it is here.

## License

GPL-3.0-or-later, as VIctory. See [LICENSE](LICENSE); the one file it does not
cover is named in [docs/asset-policy.md](docs/asset-policy.md).
