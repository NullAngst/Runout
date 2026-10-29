# Vinyl goldens

These files are the record of what Runout did to vinyl before the media
expansion. They were generated once, on main with the loop fix applied and
nothing else changed, by running the release `runout` binary and the engine
from that commit. `regression.rs` checks every build against them.

DO NOT REGENERATE THESE FILES. Regenerating them from changed code makes the
no-regression test prove nothing. If a vinyl golden fails, the code is wrong,
not the golden.

What's here:

- `vinyl_exports.json`: `runout process` over each clip in `tests/` with the
  default chain and each built-in condition preset (Clean modern pressing,
  Light wear, Played-to-death 70s LP), plus the three clips as one folder with
  `--cleaned` and with `--preset light-wear`. For every exported FLAC and raw
  WAV: a SHA-256 of the decoded PCM (integers at the file's bit depth, little
  endian, 4 bytes each), the format, every Vorbis tag, and the report lines
  that carry the analysis numbers. `{repo}` stands for the repo root and
  `{folder}` for a folder holding copies of the three clips. File paths are
  relative to the `--out` root and use the old layout; compare from the
  album folder down.
- `selftest.json`: `runout --self-test` check values (everything but timing),
  three throughput runs and their median, and the CPU and thread count they
  were measured on. The 5% throughput check only means something on the same
  machine, so it is enforced only when the CPU and thread count match.
- `v1-project.runout/project.json`: a version 1 project over
  `tests/hum_sweep.wav` (light-wear chain, hum notch, balance and azimuth,
  speed cents, a manual repair, a noise profile).
- `v1-user-preset.json`: a version 1 user condition preset as Runout saves it.
- `fixtures.json`: hashes of renders from those two fixtures (a 10-second
  preview block, the full source, and the preset's chain over
  `clicks_sweep.wav`; f32 bits, little endian, channel after channel), plus
  the chains as version 1 serialized them.
- `verdigris.json`: the `:root` tokens from `ui/src/styles.css` and the
  colors hardcoded in `engine/src/tiles.rs`, as they were.
