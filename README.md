# Runout

Runout is an offline Linux desktop app that takes records, tapes and digital sources from raw capture to clean, level-matched, tagged FLAC files, for one person at a desk with a turntable, a tape deck or a DAT machine.

Capture or import, split into tracks, clean, level, export. Click and crackle repair, hiss reduction, hum and rumble removal, wow and flutter correction, azimuth alignment, dropout repair, noise reduction decoding, bit-perfect digital capture, loudness matching and tagging. It is non-destructive: source audio is never touched, the project stores settings, and audio is rendered on playback and on export. It is a restoration tool. It is not a DAW.

It ships as an AppImage, built by GitHub Actions. There is also a headless `runout` command for batch work and for CI.

## What it does

**Input.** Record straight from the source with auto-stop at the end (the run-out groove, the end of the tape, or digital silence), import WAV, FLAC, AIFF, OGG or MP3 files (one per side works best), or import a folder of tracks that are already split. Folder import reads disc and track tags, falls back to natural filename order (`2 - Song.flac` before `10 - Song.flac`), treats subfolders like `Disc 1` as discs, and asks whether the files are Raw or already Cleaned. A capture that passes 4 GB switches to RF64 on its own, so a long reel at 96 kHz never hits the WAV size limit.

**Split.** Nothing splits by itself. A capture or an import arrives as one track per side (or program, reel, tape), and you split it. Press M while listening to drop a boundary at the playhead, drag boundaries, add, merge and delete tracks, or press the Auto-split button, which runs the medium's strategy: silence for records, gaps relative to the hiss floor for tape, program changes for 8-track, fixed-length chapters for long recordings. Short automatic fades at each boundary (50 ms in, 500 ms out). A tracklist lookup on MusicBrainz fills in titles, filtered by the medium's format, and can snap boundaries to the track lengths.

**Clean.** A fixed chain, in this order, because the order is part of the result. The medium decides which stages show; vinyl shows exactly what Runout always had.

1. DC offset removal (always on, analog only)
2. Speed and pitch, plus wow and flutter (rpm, cm/s or ips; a deck's speed error, a reference tone or a known track length; wow tracked from recorded hum or a test tone)
3. Azimuth, static or tracked (tape)
4. Channel mode: stereo, mono sum, one channel, or best channel auto
5. Playback EQ correction (cassette 120 and 70 us, reel NAB and IEC, 78 curves, custom)
6. DNR: noise reduction decode, B-type or dbx II (tape)
7. Rumble filter, called High-pass for tape
8. Hum and buzz (50, 60 or 59.94 Hz and harmonics, detected automatically)
9. Dropout repair (tape)
10. Declick (autoregressive model, LSAR interpolation)
11. Decrackle (same idea, tuned for dense tiny ticks)
12. Manual repair (select a region, or paint a band on the spectrogram)
13. Declip (off unless clipping is found)
14. Denoise (spectral subtraction, profile learned from the lead-in or a stretch of plain hiss)
15. Band limit (the medium's real bandwidth, so the hiss above it goes)
16. Channel tools (balance, swap, and azimuth for vinyl)
17. EQ (shelves plus parametric bands)
18. Leveling and limiter (always last)

Digital media get their own short chain: error repair, de-emphasis, manual repair, leveling. With every stage off, the export is bit-identical to what came off the S/PDIF input.

Every stage has an Auto button where one makes sense. Any track can get its own settings. A/B switches between original and processed at the same playhead, and Listen to removed plays only what the chain took out. If you hear music in it, a setting is too aggressive.

**Level.** Each track is measured in LUFS (ITU-R BS.1770), gained to a target (default -14 LUFS) and run through a 4x oversampled true-peak limiter with a hard ceiling (default -1.0 dBTP). The limiter may take at most 3 dB off a peak by default; a track that needs more stops short of the target instead of getting squashed, and the report flags it. Track mode or album mode.

**Export.** Tagged FLAC per track into `~/Music/Runout`, laid out by the project's kind:

| Kind | For | Path |
|---|---|---|
| Album | Commercial releases, every vinyl project | `{artist}/{album artist} - {album}/{track:02}. {title}.flac` |
| Mixtape | Compilations, radio rips, mixes | `Mixtapes/{title}/{track:02}. {artist} - {title}.flac` |
| Recording | Dictation, family tapes, airchecks, lectures | `Recordings/{date} {title}/{part:02}. {title}.flac` |

Vinyl is 16-bit with TPDF dither by default, tape is 24-bit, digital keeps the source's word length. The untouched raw capture of each side goes in `Raw/` as WAV. Cover art is embedded and saved as `cover.jpg`. Files get a `MEDIA` tag (Vinyl, Cassette, Reel-to-reel, DAT and so on) and, where they say something, `RUNOUT_MEDIUM`, `RUNOUT_SPEED`, `RUNOUT_NR` and `RUNOUT_DECK`. A processing report lists every track's settings, clicks and dropouts repaired, noise floor and loudness before and after, and true peak, and a `preset.json` beside it records the medium, condition and deck the export used. After export, every file is measured again; anything over the ceiling is re-rendered with a lower ceiling.

**Extras.** Batch mode for folders, condition presets per medium (Clean modern pressing, Light wear, Played-to-death 70s LP for records, Clean, Worn and Rough for tape, plus your own), deck profiles, a 20-second capture level check, five themes, the headless CLI, the before/after report, and crash recovery (autosave every 30 seconds, and a capture in progress is recovered up to the last second written).

The app never touches the network unless you press Look up tracklist.

## Media

Every project has one medium, picked when you create it and changeable later with the Medium button (Ctrl+M). The medium decides the capture path, how speed is expressed, which stages show in the Clean panel, the split strategy, the side naming and the export kind. Projects from before the media expansion open as Vinyl LP, unchanged.

| Family | Media |
|---|---|
| Vinyl | LP 33 1/3, Single 45, Mono LP, 78 rpm |
| Cassette | Type I, Type II / IV, Spoken word, Microcassette |
| Cartridge | 8-track |
| Reel | 1/4" half-track, 1/4" quarter-track, full-track mono |
| Video | VHS / Beta Hi-Fi, VHS linear track, LaserDisc analog |
| Digital | DAT, MiniDisc, DCC |
| Generic | Analog line-in, Digital S/PDIF |

The generic presets cover the rest: wire recordings, dictation belts, radio airchecks, anything with a line out.

Settings come in three layers, applied in this order:

1. **Medium preset**: what the thing is. Sets the stages, their defaults and everything above.
2. **Condition preset**: how worn it is. Records and tapes each have their own list.
3. **Deck profile**: what your playback machine does wrong. Speed error, azimuth offset, balance and a little EQ, applied to every side you play on it.

Every field of a medium is editable in the Medium dialog's Parameters tab. Save as preset keeps your version under `~/.config/runout/media/`, and Reset to base goes back to the built-in one. Changing the medium keeps your captures, tracks and manual repairs.

Deck profiles live in Settings. Measure fills one in from the open project: capture a test tape with tones if you have one, otherwise a tape or record you know well, press Analyze, then Measure.

### Tape notes

- **Noise reduction.** Switch the deck's own NR off, capture the tape undecoded, then switch DNR on in Runout. Why? Plenty of decks decode badly: misaligned heads, drifted calibration, tired chips. The raw stays encoded, so you can redo the decode with other settings any time and A/B it. B-type targets tapes recorded with Dolby B. It is a model of B-type behavior built from the published encode curves, not the original circuit. It needs the tape's reference level in dBFS: Auto reads it from a level tone at the head of the tape, otherwise you type it in. A 2 dB miscalibration is audible as dull or spitty treble. dbx II needs no calibration.
- **Wow and flutter** needs a stable reference recorded on the tape: mains hum that got onto it when it was made, or a test tone. Without one it does nothing and says so. Hum from your own capture chain is rock steady and useless as a reference; Runout spots that and tells you. With a 3150 Hz tone it also reports weighted wow and flutter. The weighting follows IEC 60386 approximately, so treat it as a close reading, not a lab certificate.
- **Speed.** A deck profile, a reference tone at the head of the tape, or a known length (select a track, type `3:41` under Known length) all set the same speed trim.
- **8-track.** Press Find program changes in the Split step. When a song runs through a change, Runout offers to join the halves with a short crossfade, and you confirm each one.
- **Overlays.** Speed, azimuth and dropout maps show under the waveform once Analyze has run; toggle them in the timeline header. `[` and `]` hop between every event in view: clicks, dropouts, errors, program changes.

### Record notes

- **Mono LP** sums the channels by default, which cancels the vertical noise a mono groove cannot carry. Best channel auto is there for a record with one worn wall.
- **78s need a wider stylus than LPs.** A 78 played with an LP stylus rides the groove bottom and sounds awful no matter what the software does. The 78 preset offers 78.26, 77.92, 78 and 80 rpm plus custom, heavier declick and decrackle, best channel mono, the band limit on, and playback EQ correction from RIAA to the record's own curve.

### S/PDIF and bit-perfect capture

Digital media capture through the ALSA hw device directly, bypassing PipeWire and PulseAudio, so nothing resamples or adjusts volume on the way in. The hw device is exclusive: close anything else using the card first, or the capture refuses to start and says so. During capture, the bit-perfect light stays green while every sample is a clean 16-bit value; if the low bits of a 24-bit stream carry data, something between the source and the input changed the samples, and the light goes to the warning color. When the card exposes the IEC958 channel status, Runout reads the sample rate and the emphasis flag from it, and switches de-emphasis on when the source says it was pre-emphasized. `runout verify` runs the same check on a file.

## Themes

Five themes: Verdigris (the default, the palette Runout always had), Light, Dark, Kawaii Light and Kawaii Dark. Pick one in Settings (Ctrl+,). It switches live and is remembered. Themes change colors, corner radius and fonts only, never the layout. The Kawaii themes use Nunito, bundled with its OFL licence in `ui/src/assets/fonts/`, since the app works offline. Every text and background pair passes WCAG AA, checked by a test.

## Getting the AppImage

Grab `Runout_*.AppImage` from the Releases page, then:

```
chmod +x Runout_*.AppImage
./Runout_*.AppImage
```

That's it. It bundles WebKitGTK, so it's big, roughly 80 to 120 MB. It runs on any distro with a glibc at least as new as Ubuntu 22.04's, which covers any current Kubuntu, openSUSE, Fedora or Arch.

## Using it

1. Create a project. Pick the medium (the last one you used is already picked), then the condition and the deck if you have one. The project is a `Name.runout/` folder under `~/Music/Runout/Projects/`, holding `project.json` and the captures.

   DO NOT MOVE OR RENAME ANYTHING INSIDE THE `.runout` FOLDER BY HAND. The project points at `captures/Side A.wav` and friends by path; break that and the project opens with no audio. Moving the whole `.runout` folder is fine.
2. **Capture/Import.** Pick your input. My AT-LP120XUSB shows up as a USB Audio device and my Onkyo cassette deck goes into a line input over RCA; the app remembers the device per capture path. It depends on your setup. Press **Capture level check** (C), play the loudest part for 20 seconds, and read the headroom. Then **Record Side A** (Ctrl+R), flip, record Side B. The record button follows the medium's naming: Side A/B, Program 1 to 4, Reel 1, Tape.
3. **Split.** Each side arrives as one track. Press M where a song ends while listening, or press the Auto-split button and check the boundaries. **Look up tracklist** (Ctrl+L) if MusicBrainz knows the release.
4. **Clean.** Press **Analyze** (Ctrl+Enter). That fills the click and dropout maps, measures the wow and azimuth curves when those stages are on, and fills the loudness numbers. Listen, flip A/B (A), and check Listen to removed (R). Use `[` and `]` to hop between events.
5. **Level.** Track mode at -14 LUFS is the default. Analyze again if you changed settings.
6. **Export** (Ctrl+E). Albums and mixtapes need every title filled in before export starts. Recordings name untitled parts Part 1, Part 2 and so on.

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
# the CI smoke test: vinyl, tape and digital chains on generated signals
runout --self-test

# one folder of old rips, worn record settings
runout process --preset worn-70s ~/Music/Old/Album/

# a folder of album folders, one export each
runout process --batch --preset light-wear ~/Music/Old/

# already cleaned rips: no restoration, no leveling, new tags only, audio bit for bit
runout process --cleaned ~/Music/Old/Album/

# two sides recorded elsewhere, split on silence
runout process --artist "Artist" --album "Album" --year 1979 side_a.wav side_b.wav

# a Type II cassette played on a profiled deck, kept as one track per side
runout process --medium cassette-type-ii --deck onkyo --preset cassette-worn --no-split side_a.wav side_b.wav

# was this DAT capture bit-perfect?
runout verify dat_tape.wav

runout analyze side_a.wav
runout presets
runout media
```

The CLI still splits files on its own, since there is no one to press a button. `--no-split` turns that off. Exports go to `~/Music/Runout` unless you pass `--out`.

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

4. Run the engine tests, which check the DSP against the reference clips in `tests/`, vinyl against the goldens in `engine/tests/golden/`, and the media checks:

   ```
   cargo test --release -p runout-engine
   ```

5. Build the AppImage:

   ```
   npm run tauri build -- --bundles appimage
   ```

   It lands in `target/release/bundle/appimage/`.

5.5. For working on the UI, `npm run tauri dev` runs the app with live reload instead.

6. Run the app tests and the self-test, since the app tests need the UI built in step 5:

   ```
   cargo test --release -p runout-app
   ./target/release/runout --self-test
   ```

Now the AppImage is built, vinyl renders bit for bit as it did before the media expansion, the tape and digital chains pass their checks, and everything is working as it should be. Tinker as you see fit.

### The vinyl goldens

`engine/tests/golden/` holds what Runout did to vinyl before the media expansion: export hashes, tags, report numbers and self-test values, recorded once from the unmodified code. The regression tests check every build against them.

NEVER REGENERATE THE GOLDENS. If a vinyl golden fails, the code is wrong, not the golden. Regenerating them from changed code makes the no-regression test prove nothing.

### Releases

Push a tag and Actions does the rest:

```
git tag v0.2.0
git push origin v0.2.0
```

The workflow in `.github/workflows/build.yml` installs the build packages, runs the engine tests, builds the AppImage, runs the app tests and `runout --self-test`, and attaches the AppImage to the release for that tag. Run it by hand from the Actions tab to get the AppImage as a build artifact without making a release. Everything is done from the GitHub web UI, no `gh` needed.

It builds on `ubuntu-22.04`, the oldest runner GitHub still offers, since the AppImage only runs where glibc is at least as new as the build machine's. That runner is in deprecated support until 17 April 2027. After that, change `runs-on` to `ubuntu-24.04`.

Nothing is pinned. There is no `Cargo.lock` or `package-lock.json` in the repo, so every build resolves the newest versions that fit the ranges in `Cargo.toml` and `package.json`. You always ship current libraries. The cost: an upstream release that breaks something shows up as a failed build, or worse, a passing build with a behavior change. The engine tests, the goldens and the self-test exist to catch the second case.

## Notes and known limits

- **Wayland and AMD.** On Linux, Tauri draws through WebKitGTK, which has a history of blank or flickering windows on some AMD and Wayland setups. The app sets `WEBKIT_DISABLE_DMABUF_RENDERER=1` at startup, the known workaround, and all heavy drawing (waveform, spectrogram, overlays) happens in Rust, so the web view only shows pictures. KDE Plasma on Wayland is the primary test target. If WebKitGTK still proves unworkable, the plan is to swap the UI layer for Slint and keep the engine.
- **Real-time preview.** Playback renders the whole chain in 1 second blocks ahead of the playhead on all cores. Change a setting and it re-renders from the playhead; you hear the change within about a quarter second. The self-test measures the vinyl chain at about 29x real time and the full tape chain (wow, azimuth, NR decode, dropouts) at about 20x at 96 kHz, both on 2 threads, so a 16-thread desktop has plenty of headroom. If rendering ever falls behind, playback pauses with a warning instead of stuttering.
- **Wow and azimuth curves come from Analyze** and are stored in the project's `analysis/` folder. Preview only applies them. If those files go missing, the project still opens and Analyze rebuilds them.
- **Leveling in preview** uses the gains from the last Analyze. Until you analyze, preview plays at unity gain with the limiter on, and the track list marks tracks that are stale.
- **Folder imports get no automatic fades**, since those files are already trimmed and fading them would clip gapless albums. Set fades per track if you want them.
- **Cleaned imports and digital captures pass through bit for bit**: no DC filter, no fades, no dither. Turning any stage on for that file brings the full path back.
- **Dropouts longer than the repair limit** (300 ms by default) are flagged, not repaired. Short ones are rebuilt; longer ones get their level back but not their lost treble.
- **Print-through** has no automatic stage. Select the echo and use a manual repair.
- **Album mode loudness** is the duration-weighted energy mean of the tracks' integrated loudness, which stays within a fraction of a LU of a gated whole-album measurement and can be recomputed from stored numbers.
- **The spectrogram shows the original audio**, since that's where you hunt for the scratch to repair. Judge the result by ear with A/B.
- **FLAC encoding** uses flacenc's default settings with 4096-sample blocks. flacenc has no numbered levels; on the test material its files came out about 1% smaller than ffmpeg's level 5, so "level 5 or better" is fair.
- **Speed and wow correction** use the engine's own windowed-sinc interpolator rather than rubato, because preview blocks need to read the source at exact absolute positions.
- **Playback EQ curves** are built from analog time constants as first-order sections matched at DC and a reference frequency, which tracks the analog curve closer than a plain bilinear transform near the top of the band. The 78 curves come from a cited table, named in `engine/src/dsp/tape_eq.rs`.
- **Off-center pressing correction** is not in yet.

## Layout

```
engine/   all DSP, the media and project model, file I/O, export (one crate, shared)
cli/      the headless runout command
app/      the Tauri shell: commands, playback, capture (analog and S/PDIF), MusicBrainz lookup
ui/       the Svelte front end and its themes
tests/    short reference clips: synthetic clicks and hum on a sine sweep
```

Regenerate the reference clips with `runout gen-test-clips tests`. It also writes `tape_clip.wav`, the 96 kHz tape signal the self-test builds in memory (wow, azimuth error, B-type encoding, dropouts), for listening to or trying the tape stages by hand.

## License

GPL-3.0. See `LICENSE`. Nunito is under the SIL Open Font License; see `ui/src/assets/fonts/OFL.txt`.
