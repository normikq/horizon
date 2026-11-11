# Native libraries and video in the world

A native mod ([native-mods.md](native-mods.md)) can declare a third-party library that the game loads at run
time. The first one is libmpv: a mod plays a local video file on a surface in the world, a screen block or a
panel, drawn by the game itself and not in a window of mpv's.

This page covers the declaration, what the launcher and the game check, the helpers a mod calls, what a frame
costs, and what is still open before a release.

- [Where it plugs in](#where-it-plugs-in)
- [Declaring a library](#declaring-a-library)
- [What is checked, and when](#what-is-checked-and-when)
- [The helpers](#the-helpers)
- [Sound](#sound)
- [Frame cost](#frame-cost)
- [Trying it](#trying-it)
- [Limits and open questions](#limits-and-open-questions)

## Where it plugs in

The audit of the engine found that no new render pipeline is needed.

| Question | Finding | Where |
|---|---|---|
| Can a mod's helper reach the engine's internals? | Yes. Helpers are methods of `Game`; `native_draw_quad` reads `self.gui.atlas` directly. | `crates/horizon-client/src/game/native_block_entity_draw.rs` |
| Is there a texture a mod can write at run time? | Yes. The atlas has empty slots written by patches: maps, the minimap (`horizon/minimap`) and a mod's own image (`horizon/modimage`, `native_mod_image`). | `HudAtlas::slots` in `crates/horizon-client-render/src/hud.rs` |
| Is that atlas the one world geometry samples? | Yes. `native_draw_quad` looks its sprite up in the same atlas, and the entity batch draws it. | `BlockEntityDraw::quad` |
| How do patches reach the GPU? | `Game::map_patches` joins the HUD batch each frame; `HudRenderer::upload_patches` stages them into the atlas texture. | `crates/horizon-client/src/game/extras.rs`, `hud.rs` |
| Can a mod's crate bind libmpv itself? | No. A mod's crate has no build script and uses only crates in Horizon's `Cargo.lock`, so the binding lives in the engine's tree. | [native-mods.md](native-mods.md) |
| Does the binding need a new registry crate? | No. `libloading` and `sha2` are already in `Cargo.lock`. | `crates/horizon-mpv/Cargo.toml` |

So the pieces are:

- **`crates/horizon-mpv`** opens libmpv by its full path at run time, checks its SHA-256, and plays a file through
  mpv's software render API into RGBA frames on a thread of its own. Nothing links against libmpv, and a game
  that plays no video never loads it.
- **Video slots** (`crates/horizon-client-render/src/video_slots.rs`): two 1280x720 atlas slots, `horizon/video0`
  and `horizon/video1`, present only in a game built with native mods, like the mod image slot.
- **The helpers** (`crates/horizon-client/src/game/extras/video.rs`) open a surface, drive the playback and draw
  the slot on a quad.
- **The launcher** (`launcher/src/native_mods/libraries.rs`) reads the declaration, checks the file's hash and
  installs it beside the built game.

## Declaring a library

A bundle that needs a library has a `native-libraries.toml` beside its `horizon-mixin.toml`:

```toml
[[library]]
name = "libmpv"
version = "0.40.0"
platform = "windows-x64"
file = "libmpv-2.dll"
sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
```

| Key | Meaning |
|---|---|
| `name` | What the game asks for. Video needs `libmpv`. Lowercase letters, digits, `-` and `_`. |
| `version` | The library's version, for the player and the build's record. |
| `platform` | `windows-x64`, `windows-arm64`, `linux-x64`, `linux-arm64`, `macos-x64` or `macos-arm64`. One entry for each platform the mod supports. |
| `file` | The file name it is installed under: one name, no folders. |
| `sha256` | The file's SHA-256, 64 lowercase hex digits. |
| `path` | Optional. Where the file is in the bundle. Without it the launcher looks for `data/<file>`. |

Any other key is refused, so a manifest cannot carry an address to download from.

The file itself comes from one of two places, in this order:

1. the bundle (`path`, or `data/<file>`). A `.hnmod` is limited to 64 MiB, which a full libmpv build exceeds, so
   this suits small libraries and folder bundles;
2. the player's library folder, `native/libraries/` in the launcher's build folder. The build's error names the
   folder, the file and the hash it must have.

Horizon never downloads a library. A mod that declares a library but has no entry for the platform being built
is refused. Two mods may declare the same library only as the same file with the same hash.

## What is checked, and when

A native mod already runs with the player's rights, and so does a library it brings. The checks below make sure
the file that runs is the file the mod's author named. They do not make that file safe: read who built it.

| When | Check |
|---|---|
| Build, before anything compiles | The manifest's keys and values; the file exists; its SHA-256 is the declared one. A mismatch stops the build with both hashes. |
| Install | The file is copied to `<version folder>/native-libraries/` and hashed again there. The list goes to `<version folder>/native-libraries.json` and into `native-mods.json`. |
| First video in the game | The game reads `native-libraries.json` beside its executable, refuses a file name with a folder in it, hashes the file again, and only then loads it. |
| Load | By full path only. On Windows the library's own imports resolve from its folder and System32, never from the game's folder, the working directory or `PATH`. |
| Each file mpv opens | mpv's `on_load` hook passes local files only. A URL, a network share, a device, or a playlist entry that names one is stopped before mpv opens it. |

mpv starts with no configuration file, no scripts, no `youtube-dl` hook and no input bindings of its own.

A new build never carries libraries over from the build it started from: `native-libraries/` and
`native-libraries.json` belong to the build that installed them.

## The helpers

All are methods of `Game`, called from a mod's helpers like `native_draw_quad`. They are pending API
(`tools/native-hook-fragments/world_video.json`) until the next stamp gives them a number.

| Helper | What it does |
|---|---|
| `native_video_open(path, width, height, looped) -> Result<u32, String>` | Opens a surface of `width` x `height` pixels (at most 1280 x 720) and plays the local file `path` on it. Returns the surface, or why there is none. |
| `native_video_load(video, path) -> Result<(), String>` | Plays another local file on the surface. |
| `native_video_pause(video, paused) -> bool` | Pauses or resumes. |
| `native_video_seek(video, seconds) -> bool` | Jumps to a time from the file's start. |
| `native_video_volume(video, volume) -> bool` | Sets the loudness, 0 to 100. |
| `native_video_close(video) -> bool` | Stops the video and frees its surface. |
| `native_video_state(video) -> Option<NativeVideoState>` | Position, duration, paused, ended, loaded, failed, sizes, and what the frames cost. |
| `native_draw_video(draw, video, corners) -> bool` | From `native_block_entity_submit`: draws the current frame on a quad, corners in `native_draw_quad`'s order. Full bright, whatever the light on the block. False until the first frame is there. |

Two surfaces play at once. The video keeps its shape: mpv fits it into the surface with black bars, so give the
quad the surface's proportions. Videos stop when the player leaves the world for the title screen.

The first `native_video_open` hashes and loads the library on the game's thread. For a 120 MB libmpv that is a
pause of a few hundred milliseconds, once per run. Open the first surface when the player asks for a video, not
in the middle of a fight.

A minimal screen, drawn by a mod's block entity:

```rust
// The mod's own state: the surface, opened once.
static SCREEN: std::sync::Mutex<Option<u32>> = std::sync::Mutex::new(None);

let mut screen = SCREEN.lock().unwrap();
if screen.is_none() {
    let file = game.native_instance_dir().join("videos").join("intro.mkv");
    match game.native_video_open(&file, 1280, 720, true) {
        Ok(video) => *screen = Some(video),
        Err(why) => log::warn!("no video: {why}"),
    }
}
if let Some(video) = *screen {
    // A 16:9 screen two blocks wide on the block's north face.
    game.native_draw_video(draw, video, [
        [1.5, 1.125, -0.01],
        [1.5, 0.0, -0.01],
        [-0.5, 0.0, -0.01],
        [-0.5, 1.125, -0.01],
    ]);
}
```

## Sound

mpv plays the sound itself, through its own audio output, on the system's default device. It does not go
through the game's mixer: the game's volume sliders, its positional audio and its mute do not apply. A mod sets
the loudness with `native_video_volume` and can lower it with distance itself. Routing the sound into the game's
mixer is not part of this work.

## Frame cost

The target is at most 2 ms of the game's thread for a frame of 720p video at 30 frames a second.

Decoding and rendering run on mpv's threads and the player's own thread. The game's thread does two things for
a frame that has a new video frame: it takes the finished pixels (a mutex and a pointer), and it stages them
for the GPU (one copy of `width * height * 4` bytes, 3.5 MiB at 720p). A game frame without a new video frame
pays for the mutex only.

Measured with `mpv_bench` ([Trying it](#trying-it)) on an Intel Core i5-14600KF, Windows 11, a release build:
1280x720 at 30 frames a second for 10 seconds, against a loop standing in for the game at 60 frames a second.

| What | Mean | Median | 99% | Worst |
|---|---|---|---|---|
| Game thread, a frame with a new video frame (take and one copy) | 0.53 ms | 0.47 ms | 1.17 ms | 1.46 ms |
| Game thread, a frame without one | under 0.001 ms | | | 0.03 ms |

296 of 300 frames arrived. That is inside the 2 ms target on this computer, with the copy standing in for the
GPU staging. It is one computer: the number that counts on a slower one is `upload_micros` below, read in the
game.

The same video in the game itself (the `video_screen` example, a creative world at 1280x720, RTX 5070 Ti):
taking the frame costs 1 microsecond and staging its patch 0.34 to 0.36 ms of the game's thread, for each of
the 30 video frames a second.

The player's thread spends a whole frame interval (33 ms at 30 frames a second) in mpv's render call, because
that call waits until the frame is due. That wait is what paces the video, and it is not CPU time: a frame rendered
without waiting, after a seek while paused, took 0.5 to 1.3 ms there.

`NativeVideoState` reports the costs in the game: `render_micros` (the player's thread, wait included),
`pump_micros` (the game's thread taking the frames) and `upload_micros` (the game's thread staging the frame's
patch). The `video_screen` example logs them every five seconds.

## Trying it

With a libmpv for your platform (client API 2, such as `libmpv-2.dll`):

```sh
# The version the library reports, opened the way the game opens it.
cargo run -p horizon-mpv --example mpv_version -- --file /full/path/to/libmpv-2.dll

# Plays a generated file: colours, pause, seek, a broken file, a playlist that names an address.
HORIZON_MPV_LIBRARY=/full/path/to/libmpv-2.dll cargo test -p horizon-mpv

# What a 720p frame costs on this computer.
cargo run --release -p horizon-mpv --example mpv_bench -- /full/path/to/libmpv-2.dll
```

Without `HORIZON_MPV_LIBRARY` the playback tests pass without playing anything and say so; CI needs a libmpv on
the runner to run them for real.

`crates/horizon-client/examples/video_screen.rs` is a whole mod on these helpers, as a trait mod
([native-mods.md](native-mods.md#trait-mods-draft)): a `/video` command opens a 16:9 screen in front of the
player and drives it (`open <file>`, `pause`, `play`, `seek <seconds>`, `volume <0-100>`, `close`, `info`).
The example is a game of its own, so it needs the library beside its executable the way the launcher installs
it:

```
target/fast/examples/
  video_screen.exe
  native-libraries.json      {"libraries":[{"mod":"video_screen","name":"libmpv","version":"…","file":"libmpv-2.dll","sha256":"…"}]}
  native-libraries/
    libmpv-2.dll
```

```sh
cargo build --profile fast -p horizon-client --example video_screen
target/fast/examples/video_screen --resources PATH --game-dir SCRATCH --world Video --creative
```

## Limits and open questions

- **Licence.** Horizon is MIT. libmpv is LGPL when built with `-Dgpl=false` and GPL otherwise, and the builds
  most people download are GPL builds. Horizon does not ship libmpv: the mod's author or the player supplies
  it. A fork that ships a libmpv in its installer takes on that build's licence terms for the bundle and needs
  that reviewed first. `horizon-mpv` is written against mpv's documented client API, whose headers are ISC, and
  contains no mpv code.
- **Platforms.** Built and tested on Windows x64. The loader has a Unix path, untested.
- **Renderer.** Frames come from mpv's software renderer and reach the GPU as a texture patch. A zero-copy path
  through mpv's OpenGL or D3D11 renderer would need the engine to share a device with mpv, which wgpu does not
  offer today.
- **Hardware decoding** is off: the software renderer needs frames in system memory.
- **Where videos come from.** Local files only. Streams would need a list of allowed protocols, a setting and a
  consent screen, and are refused until someone designs those.
- **Sound in the game's mixer.** Not done, see [Sound](#sound).
- **The launcher builds from GitHub.** A native build compiles the source of a Horizon revision downloaded from
  the repository, never a checkout on this computer. These helpers exist only in a fork's own tree, so the fork
  must publish its source and point its launcher at it before players can build a video mod.
- **Smart App Control** can block a locally built game ([native-mods.md](native-mods.md)); an unsigned library
  beside it is subject to the same policy.
- **Signing.** The launcher checks hashes, not signatures. Signing the fork's releases is its release process's
  job (`horizon-release-sign`).
