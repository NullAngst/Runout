# Runout

Runout is an offline Linux desktop app that takes vinyl captures from raw recording to clean, level-matched, tagged FLAC files, for one person at a desk with a USB turntable.

Capture or import, split into tracks, clean, level, export. Click and crackle repair, hiss reduction, hum and rumble removal, track splitting, loudness matching and tagging. It is non-destructive: source audio is never touched, the project stores settings, and audio is rendered on playback and on export. It is a restoration tool. It is not a DAW.

It ships as an AppImage, built by GitHub Actions. There is also a headless `runout` command for batch work and for CI.

## What it does

**Input.** Record straight from the turntable (16-bit, 48 kHz when the device offers it, side by side, auto-stop at the run-out groove), import WAV, FLAC, AIFF, OGG or MP3 files (one per side works best), or import a folder of tracks that are already split. Folder import reads disc and track tags, falls back to natural filename order (`2 - Song.flac` before `10 - Song.flac`), treats subfolders like `Disc 1` as discs, and asks whether the files are Raw or already Cleaned.

**Split.** Auto-split on silence with a threshold and minimum gap, drag boundaries, add, merge and delete tracks, and press M while listening to drop a boundary at the playhead. Short automatic fades at each boundary (50 ms in, 500 ms out). A tracklist lookup on MusicBrainz fills in titles and can snap boundaries to the track lengths.

**Clean.** A fixed chain, in this order, because the order is part of the result:

1. DC offset removal (always on)
2. Speed and pitch correction (33 1/3 and 45, plus cents)
3. Rumble filter (12 to 48 dB/octave, linear-phase option)
4. Hum removal (50 or 60 Hz plus harmonics, detected automatically)
5. Declick (autoregressive model, LSAR interpolation)
6. Decrackle (same idea, tuned for dense tiny ticks)
7. Manual repair (select a region, or paint a band on the spectrogram)
8. Declip (off unless clipping is found)
9. Denoise (spectral subtraction, profile learned from the lead-in or run-out)
10. Channel tools (balance, swap, azimuth)
11. EQ (shelves plus parametric bands)
12. Leveling and limiter (always last)

Every stage has an Auto button. Any track can get its own settings. A/B switches between original and processed at the same playhead, and Listen to removed plays only what the chain took out. If you hear music in it, a setting is too aggressive.

**Level.** Each track is measured in LUFS (ITU-R BS.1770), gained to a target (default -14 LUFS) and run through a 4x oversampled true-peak limiter with a hard ceiling (default -1.0 dBTP). The limiter may take at most 3 dB off a peak by default; a track that needs more stops short of the target instead of getting squashed, and the report flags it. Track mode or album mode.

**Export.** Tagged FLAC per track, 16-bit with TPDF dither by default or 24-bit, into `~/Music/Runout/Artists/{album artist} - {album}/{track:02}. {title}.flac`. The untouched raw capture of each side goes in `Raw/` as WAV. Cover art is embedded and saved as `cover.jpg`. A processing report lists every track's settings, clicks repaired, noise floor and loudness before and after, and true peak. After export, every file is measured again; anything over the ceiling is re-rendered with a lower ceiling.

**Extras.** Batch mode for folders, presets by record condition (Clean modern pressing, Light wear, Played-to-death 70s LP, plus your own), a 20-second capture level check, the headless CLI, the before/after report, and crash recovery (autosave every 30 seconds, and a capture in progress is recovered up to the last second written).

The app never touches the network unless you press Look up tracklist.

## Getting the AppImage

Grab `Runout_*.AppImage` from the Releases page, then:

```
chmod +x Runout_*.AppImage
./Runout_*.AppImage
```

That's it. It bundles WebKitGTK, so it's big, roughly 80 to 120 MB. It runs on any distro with a glibc at least as new as Ubuntu 22.04's, which covers any current Kubuntu, openSUSE, Fedora or Arch.

## Using it

1. Create a project. It's a `Name.runout/` folder under `~/Music/Runout/Projects/`, holding `project.json` and the captures.

   DO NOT MOVE OR RENAME ANYTHING INSIDE THE `.runout` FOLDER BY HAND. The project points at `captures/Side A.wav` and friends by path; break that and the project opens with no audio. Moving the whole `.runout` folder is fine.
2. **Capture/Import.** Pick your turntable in the device list. The AT-LP120XUSB shows up as a USB Audio device and the app remembers it. Press **Capture level check** (C), play the loudest part of the record for 20 seconds, and read the headroom. Then **Record Side A** (Ctrl+R), flip, record Side B.
3. **Split.** Check the boundaries. Drag them, or press M where a song ends. **Look up tracklist** (Ctrl+L) if MusicBrainz knows the record.
4. **Clean.** Press **Analyze** (Ctrl+Enter). That fills the click map and the loudness numbers. Listen, flip A/B (A), and check Listen to removed (R). Use `[` and `]` to hop between clicks.
5. **Level.** Track mode at -14 LUFS is the default. Analyze again if you changed settings.
6. **Export** (Ctrl+E). Every track needs a title; export does not start while one is blank.

Press `?` in the app for every keyboard shortcut.

The spectrogram matters more than it looks: clicks show as vertical lines and hum as a flat horizontal one. To fix a scratch the declicker missed, drag across it on the waveform (or paint the band on the spectrogram) and press X.

## The command line

The AppImage is the app. The `runout` command is built from the same engine:

```
cargo build --release -p runout
./target/release/runout --help
```

Some examples:

```
# the CI smoke test: generated sweep with hum, hiss and 40 clicks through the full chain
runout --self-test

# one folder of old rips, worn record settings
runout process --preset worn-70s ~/Music/Old/Album/

# a folder of album folders, one export each
runout process --batch --preset light-wear ~/Music/Old/

# already cleaned rips: no restoration, no leveling, new tags only, audio bit for bit
runout process --cleaned ~/Music/Old/Album/

# two sides recorded elsewhere, split on silence
runout process --artist "Artist" --album "Album" --year 1979 side_a.wav side_b.wav

runout analyze side_a.wav
runout presets
```

## Building from source

### Prerequisites

- Rust, latest stable (rustup is easiest)
- Node.js, current LTS
- The WebKitGTK 4.1 and ALSA development packages

### Steps

1. Install the system packages. I use apt, so in my case:

   ```
   sudo apt install libwebkit2gtk-4.1-dev libasound2-dev librsvg2-dev patchelf build-essential file
   ```

   It depends on your system. On openSUSE it's roughly `sudo zypper in webkit2gtk3-devel alsa-devel librsvg-devel libopenssl-devel file` plus `sudo zypper in -t pattern devel_basis`; check Tauri's Linux prerequisites page for your distro's names.

2. Clone the repo and go into it:

   ```
   git clone https://github.com/NullAngst/Runout.git
   cd Runout
   ```

3. Install the npm packages, since the UI and the Tauri CLI come from npm:

   ```
   npm install
   ```

4. Run the engine tests, which check the DSP against the reference clips in `tests/`:

   ```
   cargo test --release -p runout-engine
   ```

5. Build the AppImage:

   ```
   npm run tauri build -- --bundles appimage
   ```

   It lands in `target/release/bundle/appimage/`.

5.5. For working on the UI, `npm run tauri dev` runs the app with live reload instead.

Now the AppImage is built, the engine has passed its tests against the reference clips, and everything is working as it should be. Tinker as you see fit.

### Releases

Push a tag and Actions does the rest:

```
git tag v0.1.0
git push origin v0.1.0
```

The workflow in `.github/workflows/build.yml` installs the build packages, runs the engine tests, builds the AppImage, runs `runout --self-test`, and attaches the AppImage to the release for that tag. Run it by hand from the Actions tab to get the AppImage as a build artifact without making a release. Everything is done from the GitHub web UI, no `gh` needed.

It builds on `ubuntu-22.04`, the oldest runner GitHub still offers, since the AppImage only runs where glibc is at least as new as the build machine's. That runner is in deprecated support until 17 April 2027. After that, change `runs-on` to `ubuntu-24.04`.

Nothing is pinned. There is no `Cargo.lock` or `package-lock.json` in the repo, so every build resolves the newest versions that fit the ranges in `Cargo.toml` and `package.json`. You always ship current libraries. The cost: an upstream release that breaks something shows up as a failed build, or worse, a passing build with a behavior change. The engine tests and the self-test exist to catch the second case.

## Notes and known limits

- **Wayland and AMD.** On Linux, Tauri draws through WebKitGTK, which has a history of blank or flickering windows on some AMD and Wayland setups. The app sets `WEBKIT_DISABLE_DMABUF_RENDERER=1` at startup, the known workaround, and all heavy drawing (waveform, spectrogram) happens in Rust, so the web view only shows pictures. KDE Plasma on Wayland is the primary test target. If WebKitGTK still proves unworkable, the plan is to swap the UI layer for Slint and keep the engine.
- **Real-time preview.** Playback renders the whole chain in 1 second blocks ahead of the playhead on all cores. Change a setting and it re-renders from the playhead; you hear the change within about a quarter second. The self-test measures the full chain at about 40x real time on 2 threads, so a 16-thread desktop has plenty of headroom. If rendering ever falls behind, playback pauses with a warning instead of stuttering.
- **Leveling in preview** uses the gains from the last Analyze. Until you analyze, preview plays at unity gain with the limiter on, and the track list marks tracks that are stale.
- **Folder imports get no automatic fades**, since those files are already trimmed and fading them would clip gapless albums. Set fades per track if you want them.
- **Cleaned imports pass through bit for bit**: no DC filter, no fades, no dither. Turning any stage on for that file brings the full path back.
- **Album mode loudness** is the duration-weighted energy mean of the tracks' integrated loudness, which stays within a fraction of a LU of a gated whole-album measurement and can be recomputed from stored numbers.
- **The spectrogram shows the original audio**, since that's where you hunt for the scratch to repair. Judge the result by ear with A/B.
- **FLAC encoding** uses flacenc's default settings with 4096-sample blocks. flacenc has no numbered levels; on the test material its files came out about 1% smaller than ffmpeg's level 5, so "level 5 or better" is fair.
- **Speed correction** uses the engine's own windowed-sinc interpolator rather than rubato, because preview blocks need to read the source at exact absolute positions.
- **Off-center pressing correction and turntable speed calibration** from the feature list are not in v1.

## Layout

```
engine/   all DSP, the project model, file I/O, export (one crate, shared)
cli/      the headless runout command
app/      the Tauri shell: commands, playback, capture, MusicBrainz lookup
ui/       the Svelte front end
tests/    short reference clips: synthetic clicks and hum on a sine sweep
```

Regenerate the reference clips with `runout gen-test-clips tests`.

## License

GPL-3.0. See `LICENSE`.
