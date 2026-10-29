# Media expansion: build notes

This is the record of how the media expansion was built from the Leader build spec, and where the build differs from the letter of the spec. The README covers how to use it.

## Applying it

The build came as one `.tar` laid out like the repo: every changed and new file, no patch files, no lockfiles.

1. Go to your checkout and onto the expansion branch, since main stays shippable until the end:

   ```
   cd Runout
   git switch -c media-expansion
   ```

2. Extract the tar over the checkout:

   ```
   tar -xf runout-media-expansion.tar
   ```

3. Commit the goldens on their own first, since the spec wants them as their own commit before anything else:

   ```
   git add engine/tests/golden
   git commit -m "Record vinyl goldens before the media expansion"
   ```

   They were recorded from main as it stands (loop fix included, nothing else changed) before any expansion code was written. DO NOT REGENERATE THEM.

4. Commit the rest. The tar adds a `.gitignore`, so `git add -A` leaves out `target/`, `node_modules/`, `dist/`, `app/gen/` and the lockfiles:

   ```
   git add -A
   git commit -m "Media expansion"
   ```

5. Run the checks:

   ```
   npm install
   cargo test --release -p runout-engine
   npm run tauri build -- --bundles appimage
   cargo test --release -p runout-app
   ./target/release/runout --self-test
   ```

   The app tests come after the AppImage build, since the Tauri crate needs the built UI in `dist/` to compile.

6. Do the by-hand checks from the acceptance list on the AppImage, push the branch, and merge from the GitHub web UI.

Now the expansion is on the branch, vinyl renders bit for bit as before, and the tape and digital chains pass their checks.

## What was checked here

- Engine tests: unit tests, the reference clips, the vinyl goldens (exports, tags, report numbers, the v1 project and preset, self-test values), and the media checks (mono and 78 exports, the run-out detector at 78, every built-in medium saving and loading, RF64 past 4 GB, the three export layouts, the true-peak ceiling, WCAG AA on all five themes, Verdigris byte for byte, a digital source staying bit-perfect when added).
- App tests: the four loop scheduler tests and the IEC958 channel status parser.
- Self-test: every vinyl, tape and digital check passes. Vinyl chain 29x real time and tape chain 20x at 96 kHz, both on 2 threads.
- The AppImage builds (about 83 MB).
- The UI type-checks clean with svelte-check, and every panel and dialog was rendered in a browser against a mocked backend for vinyl, cassette, 8-track and DAT projects in three themes, with no runtime errors.

The goldens' self-test throughput was measured on a 2-thread Xeon, so the 5% throughput check only runs on a machine with the same CPU and thread count. Everywhere else it is skipped and says so.

## Where the build differs from the spec

- **Playback EQ sections.** The spec says analog time-constant prototypes through the bilinear transform. A plain bilinear transform warps the treble pole badly near Nyquist at 44.1 and 48 kHz, so each first-order section is matched at DC and at a reference frequency (10 kHz for disc curves, 16 kHz for tape) instead. Same prototypes, closer to the analog curve.
- **78 curves** come from the published turnover and rolloff table (the one Audacity and Graumann use), cited in `engine/src/dsp/tape_eq.rs`. Where the table gives a flat treble the 10 kHz check is skipped.
- **B-type** is a model of the published encode curves with an exact inverse decode, not Dolby's circuit. Its constants are tuned to those curves and a test checks the shape.
- **dbx II** emphasis corners and the detector time constant are tuning values that behave like a dbx II decoder. No dbx document with the exact figures could be found, and the code says so.
- **Weighted wow and flutter** uses an approximation of the IEC 60386 weighting curve. Treat the figure as a close reading.
- **Residual wow in the self-test** is measured before the first dropout in the tape clip, since a dropout knocks the hum reference out for its length.
- **The tape clip** is generated in memory by the self-test. `runout gen-test-clips` writes it to disk as `tape_clip.wav` for listening; it is not committed to `tests/`.
- **The loop fix** was already in main, so there was nothing to apply at spec step 4.
- **Program changes** are found by the Find program changes button and shown on the timeline; Split at program changes cuts there, and each Join asks first.
- **Analysis curves** (wow, azimuth, dropouts, errors) are stored as small binary files in the project's `analysis/` folder, named in `project.json`. A project whose files are missing opens fine and Analyze rebuilds them.
- **A digital source never gets automatic hum or declip** when it is added, since either one would end the bit-perfect passthrough.
