# Workflow verification — 2026-10-07

Source baseline: `94a22d7a833d8897bce35291a406ce36b6ad4265`. The corrections are in the working
tree. Verification uses Rust 1.95.0, Linux/Xvfb, software Vulkan, Chromium/WebGPU and the real
desktop/browser applications. FFmpeg/FFprobe are external test oracles only, never product
dependencies. The oracle is `N-127233-g452820cba6-20261007` (APV support required by existing tests).

## Final gates

| Check | Result |
|---|---|
| Complete workspace suite, including doctests | 1,949 passed; 39 existing ignored tests; one AV1 download failure (HTTP 403) |
| Editing suite after the extreme-speed regression fix | 45 passed |
| `cargo fmt --check`, `git diff --check` | Passed |
| Clippy, workspace and all targets, `-D warnings` | Passed |
| Native application and CLI builds | Passed |
| Browser build and WASM check | Passed; 42 crates checked |
| Layers / assets | Passed; 67 assets attributed |
| Native and browser end-to-end workflows | Passed |

The AV1 failure is retained, not disabled or reclassified as a pass. Cargo compilation/checking
and the final test run sharing the native target directory were performed in sequence to keep
doctest artifacts intact.

## Implemented and corrected

- Project replacement now resets associated session state together; opening a demo cannot
  overwrite the previously open project. Failed Save As leaves the previous name/path/dirty
  state intact. Per-sequence playheads round-trip through project files. Extreme ripple-speed
  changes reject a result outside timeline bounds before committing the edit.
- Invalid dimensions, frame rates, sample rates, export integers, extreme export/timeline times
  and cyclic media references
  are rejected or bounded before allocation/traversal. Corrupt MXF index positions/channel
  counts no longer request huge buffers; mutation tests and media oracles exercise the fix.
- Native CPAL output/capture negotiates integer and floating-point device formats. Capture
  buffers are bounded, workers are joined, stream errors release the playback clock, and
  changing audio hardware recovers playback at its current position.
- Voice-over failures retain the captured take for retry or explicit discard. Project
  replacement is refused while a take remains pending; pre-roll/channel limits are enforced.
- Built-in shortcut conflicts are resolved for each supported platform/preset. Graphics text
  undo merges only the same editing gesture/layer. Text → Graphics and Libraries use the
  existing graphics commands/template catalogue, with actual search/navigation/edit/apply UI.
- Optical Flow time interpolation now estimates and applies bidirectional block motion;
  translated texture, alpha, static/end frames and scene cuts have regression coverage.
- File exports stage in the destination directory and replace the destination only on successful
  completion. Cancellation preserves an existing file and cleans the incomplete staging file.
  Multi-file exports commit each complete file individually.
- AAC presentation edits trim encoder priming and tail padding to the exact requested sample
  interval. Browser export retries deferred video/audio reads before consuming audio or limiter
  state; interleave groups are mixed as one transaction. The prior browser failure produced
  four seconds of video but only 65,536 audio samples; the corrected export has 192,000 samples.

## Real application checks

`apps/filmcraft/tests/e2e.py` drives the native control socket and generates its own original
fixtures. It creates a project/sequence, imports two H.264/AAC clips, a still and WAV, places
linked clips, trims/moves/splits/deletes, checks undo/redo, performs repeated seeks, plays/pauses,
saves/closes/reopens and checks the document/playhead. Export verification independently
decodes all 84 frames of the 160×90, 24 fps, 3.5-second output: red for frames 0–35, blue for
36–83, AAC presentation exactly 168,000 samples, and the expected 440/880 Hz tones after cuts.
It also verifies missing-media reopen/relink, mixed valid/invalid import, a corrupt project whose
open failure preserves the current edit, and cancellation over
an existing destination. Program-monitor and full-editor screenshots were inspected.

`apps/filmcraft-web/tests/smoke.mjs` drives Chromium through CDP. Import/playback uses WebCodecs
and AudioWorklet (`audioClock: true`); the imported clip displayed frames without a drop.
An analyser attached to the actual worklet output recorded the expected 440 Hz tone in three
live windows (amplitude approximately 0.125 at the browser's 44.1 kHz output rate).
Save/close/reopen preserves document and playhead. The downloaded MP4 has 96 video frames and
exactly four seconds of both video and audio. Independent decoding checks audio energy at four
positions, including the last 0.2 seconds. Import/Edit/Export modes, a small window and HiDPI
rendering also pass, without an application panic.

The real CPAL adapters also passed against ALSA file PCMs: 48 kHz stereo capture preserved
distinct 440 Hz left / 880 Hz right tones at 0.25 amplitude; output preserved the same tones and
advanced its sample clock. A `/dev/full` output induced an actual stream error and released
the clock. These are real device-adapter calls, not replacements for production implementations.

The headless CLI independently reopened the saved project and rendered its blue frame at 2.75 s.
An extreme custom range returned an ordinary parameter error (exit 1), with no panic.

Reproducible commands are in [testing.md](testing.md). Detailed evidence stays under
`target/verification/`: `native-acceptance/`, `web-acceptance-tone/`, `audio-*-verified.*`,
`workspace-acceptance.log`, `clippy-frozen.log`, `wasm-acceptance.log` and the build logs.
`acceptance-summary.json` summarizes the gates; `filmcraft-fixes.patch` is a recoverable patch
of all modified/new source files (reverse applicability checked), with hashes in
`source-manifest.json`.
`export-range-red.log` records the original overflow; the corresponding regression now requires
an ordinary error for negative, empty and overflowing ranges. Persisted timeline clips, captions,
transitions, marks and work areas are bounded before end-time arithmetic.

## Environment and scope limits

- The official AV1 conformance-vector download from `storage.googleapis.com/aom-test-data`
  is rejected by the session proxy with HTTP 403. The conformance test remains enabled and
  fails rather than silently skipping (`FILMCRAFT_REQUIRE_ORACLES=1`). Locally generated AV1
  oracle tests run normally. Evidence: `network-limitations.json` and the workspace log.
- Optional Whisper/download features compile and their unit tests pass. Actual Whisper
  inference is unverified: no weights are present and Hugging Face is also proxy-blocked
  with HTTP 403. The model test explicitly reports its missing weights; its harness success
  is not evidence of inference working. See `whisper-model-availability.log`.
- No physical GPU, sound card or microphone is exposed. Software rendering, browser audio,
  no-device playback fallback and real ALSA adapter conversion/error handling are verified;
  physical-device synchronization and native Windows/macOS behavior are not certified here.
- Some compatibility preferences (identified in the existing settings schema),
  plugin hosting, camera RAW and additional delivery encoders remain explicitly outside this
  implemented workflow. The existing project has no corresponding plugin/RAW/encoder APIs;
  this verification does not claim full professional-editor parity. Remaining source uses of
  “placeholder” are empty/offline panel states or container layout reservations, not substitutes
  for the implemented editing/export path. Existing ignored tests are fixture generators,
  performance measurements or manually generated screenshots; none were newly disabled.
