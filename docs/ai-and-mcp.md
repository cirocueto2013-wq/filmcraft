# AI and MCP

FilmCraft connects to external AI services from the native app and native MCP server. The editor,
HTTP/TLS clients, validation and commands remain Rust; no Python runtime or model weights ship with
FilmCraft. Browser connections currently return an explicit unsupported error.

## Editing assistant

Open **Window → Text → Assistant**. Under **AI connections**, select OpenAI cloud (default model
`gpt-4.1`) or set an OpenAI-compatible base URL and a model supported by your service. For example,
Ollama's base URL is `http://127.0.0.1:11434/v1`. For OpenAI, enter a key and **Apply connections**, or
start the native app with `OPENAI_API_KEY` set. Keys are session-only: not in projects, saved UI state,
command journals or inspect responses. Environment keys are sent only to OpenAI's exact base URL.
A configured endpoint must not contain credentials, query parameters or fragments. Redirects are
refused. Authenticated proxies for other cloud models may use the same OpenAI-compatible protocol.

Generate a preview: the planner receives bounded timeline metadata and transcript text, not footage
or audio. It returns a restricted JSON plan. FilmCraft validates it, previews the operations, and
requires the current token to apply. Project edits or sequence changes invalidate that token.
Clip plans run against an isolated project before adoption, and apply as one undo step.

Supported actions:

| Action | Fields | Meaning |
| --- | --- | --- |
| `extract` | `start`, `end` | Ripple-remove an interval across all tracks |
| `seek` | `time` | Navigate within the sequence |
| `split` | `clip`, `time` | Split a clip and its linked partners |
| `move` | `clip`, `start`, `track` | Move to an existing compatible free span |
| `trim` | `clip`, `edge`, `delta` | Regular trim; delta in seconds, edge `in`/`out` |
| `gain` | `clip`, `db` | Set audio gain between −96 and +24 dB |

The envelope is `{"schemaVersion":1,"actions":[...]}`. Up to 256 actions; sequence/time positions
bounded to ten minutes. Clip edits can combine in order. They cannot mix with extracts or navigation.
Extract ranges use the original timeline and snap inward to frames. Moves refuse overwriting other
clips, including collisions caused by linked partners. Trims that exceed source/neighbour bounds
are refused. Locks and synchronized captions are respected. Arbitrary commands, scripts, paths,
exports and project overwrites are not part of the model's vocabulary.

**Local audio/transcript analysis** works without a model: `corta los silencios`, `remove silence`,
`busca <texto>`, `find <text>`, `version de 60 segundos`. Quiet audio is measured with RMS from the
actual audible mix; it is not semantic speech recognition. Missing/invalid audio produces an error.

**Autopilot** runs up to eight instructions in order and applies validated plans. Starting it explicitly
requires `apply:true` (or the UI's **Run Autopilot · apply edits** button). Each task is undoable. It
stops on an error, cancellation, or an outside edit. Completed tasks stay visible and undoable; a
failure is not silently rolled back over user edits.

## Kokoro narration

Run an external [Kokoro-FastAPI](https://github.com/remsky/Kokoro-FastAPI) service, then set its base URL
(default `http://127.0.0.1:8880`). Load its actual voice list. Spanish defaults to `ef_dora`; English to
`af_heart`, with US/UK alternatives available from the server. Choose a voice matching the language.
Generate narration, then import it or add it at the playhead on an unlocked free span. Import and
placement are one undo step. Generated files are persisted beside a saved project, or under the
FilmCraft data directory's `Generated Media` for an unsaved project. Save the project beside
its media or copy/consolidate media before transferring it to another machine.

The client uses `/v1/audio/voices` and `/v1/audio/speech`, requests WAV, and handles Kokoro's streaming
RIFF/data size sentinels. Text is capped at 12000 bytes and downloads at 64 MiB. Model weights are
Apache-2.0; the external service/runtime has its own installation and dependencies. No model,
phonemizer or voice pack is bundled. There is no claim that a voice is best for every script.

## ComfyUI

Run your [ComfyUI](https://github.com/Comfy-Org/ComfyUI) server (default `http://127.0.0.1:8188`). Test the
connection, paste a workflow exported using **Export (API)**, and generate. The server must have the
workflow's models and nodes. FilmCraft uses `/prompt`, `/history/{prompt_id}` and `/view`, surfaces
node/execution errors, and downloads up to eight supported image/audio/movie outputs, 64 MiB total.
It validates all media and stages placement before committing. An occupied span or invalid asset
leaves the project unchanged.

Cancellation uses the server's per-job `/api/jobs/{id}/cancel`. Older ComfyUI versions fall back to
removing only that queued job; a running generation may continue, which is reported. FilmCraft never
sends a global `/interrupt` that could stop someone else's workflow. Jobs have request and overall
time limits. No shared ComfyUI installation is changed by FilmCraft.

## MCP

Use the existing `filmcraft-cli mcp` server, or `filmcraft-cli mcp --bridge 127.0.0.1:9876` for the live
app. `tools/list` provides typed schemas for:

- `ai_configure`, `assistant_context`, `assistant_preview`, `assistant_status`, `assistant_apply`;
- `ai_voices`, `ai_generate_voice`, `comfy_connect`, `comfy_generate`;
- `ai_job_status`, `ai_import_media`, `ai_cancel`;
- `editor_autopilot`, `editor_training_example`.

Generation returns promptly with a job token. Poll status, inspect the proposal/media, then apply or
import. Polling also advances an explicitly authorized Autopilot. The same functionality is available
as `assistant.*` and `ai.*` engine commands through CLI/control/`command_run`.

## Editing specialization and fine-tuning

A fine-tuned language model is still a language model. Here its job is a constrained editing planner,
not a free-form chatbot: domain instructions, timeline context, JSON actions, deterministic validation,
undo and evaluation define that boundary. OpenAI works without training a new model.

**Copy reviewed training example** / `editor_training_example` returns a JSONL-compatible `messages`
row from a current valid proposal, using the exact actions the editor previewed. Collect real edits
with permission to use their transcript/content. Correct bad plans before including them. Keep a held-out
evaluation set grouped by source project so related clips/languages do not leak across splits.

For an original synthetic bilingual seed/evaluation set:

```sh
cargo run -p filmcraft-engine --example editor_dataset -- target/editor-dataset
```

The generator creates 96 training and 24 held-out rows, validates every plan through the editor,
and refuses overwriting existing datasets. It is seed data for explicit interval edits, not evidence
of general editing skill. Add reviewed split/move/trim/gain examples and semantic edits before fine-tuning.
Evaluate JSON validity, successful FilmCraft validation, actual timeline changes and preservation of
material that should stay; do not use text similarity as the editing-quality score. Compare against
the original model on held-out projects before selecting trained weights/model IDs in AI connections.
No fine-tuning job or upload is automatically started, and this contribution does not ship trained weights.
