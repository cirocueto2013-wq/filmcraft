//! Reviewed editing assistance inside the existing Text panel.
use crate::FilmcraftApp;
use egui::{Rect, Ui, UiBuilder};
use serde_json::json;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AiUi {
    pub config: filmcraft_engine::ai::Config,
    pub local: bool,
    pub language: String,
    pub voice: String,
    pub speech: String,
    pub workflow: String,
    pub autopilot_instructions: String,
    pub speed: f64,
    pub voices: Vec<String>,
    #[serde(skip)]
    pub api_key: String,
}
impl std::fmt::Debug for AiUi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiUi").field("config", &self.config).field("api_key", &"[redacted]").finish_non_exhaustive()
    }
}
impl Default for AiUi {
    fn default() -> Self {
        Self {
            config: Default::default(),
            local: false,
            language: "es".into(),
            voice: "ef_dora".into(),
            speech: String::new(),
            workflow: String::new(),
            autopilot_instructions: String::new(),
            speed: 1.0,
            voices: Vec::new(),
            api_key: String::new(),
        }
    }
}

pub fn show(app: &mut FilmcraftApp, ui: &mut Ui, rect: Rect) {
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect.shrink(8.0)).id_salt("assistant-panel"));
    let mut actions = Vec::new();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut child, |ui| {
        ui.heading("AI & Editing Assistant");
        services(app, ui);
        ui.separator();
        ui.label("Preview first. Cuts affect all tracks and apply as one undoable edit.");
        let r = ui.checkbox(&mut app.ui.ai.local, "Local audio/transcript analysis");
        app.auto.add("assistant.local", r.rect, "Use local analysis instead of the model");
        ui.small(if app.ui.ai.local {
            "Quiet-audio analysis uses RMS, not speech recognition. No upload."
        } else {
            "The configured planner receives timeline metadata and transcript text when you generate a preview."
        });
        let r = ui.add(
            egui::TextEdit::multiline(&mut app.ui.assistant_instruction)
                .desired_width(ui.available_width())
                .desired_rows(2)
                .hint_text("corta los silencios · busca <texto> · version de 60 segundos"),
        );
        app.auto.add("assistant.instruction", r.rect, "Editing instruction");
        if r.changed() {
            actions.push(("assistant.cancel", json!({})));
        }
        let state = app.session.execute("assistant.inspect", json!({})).unwrap_or_else(|e| json!({"error":e.to_string()}));
        let busy = state["analysing"].as_bool().unwrap_or(false) || app.session.ai.busy();
        ui.horizontal(|ui| {
            let r = ui.add_enabled(
                !busy && !app.ui.assistant_instruction.trim().is_empty() && app.session.active_sequence().is_some(),
                egui::Button::new("Generate preview"),
            );
            app.auto.add("assistant.preview", r.rect, "Generate edit preview");
            if r.clicked() {
                if !app.ui.ai.local {
                    actions.push(("ai.configure", json!(app.ui.ai.config)));
                }
                actions.push((if app.ui.ai.local { "assistant.plan" } else { "ai.plan" }, json!({"instruction":app.ui.assistant_instruction})));
            }
            let r = ui.add_enabled(busy || !state["plan"].is_null(), egui::Button::new("Discard / cancel"));
            app.auto.add("assistant.cancel", r.rect, "Discard assistant proposal");
            if r.clicked() {
                actions.push(("ai.cancel", json!({})));
            }
        });
        if busy {
            ui.spinner();
            ui.label("Working… Cancel stops pending edits.");
            ui.ctx().request_repaint();
        }
        if let Some(error) = state["error"].as_str() {
            ui.colored_label(app.tokens.text, error);
        }
        let p = &state["plan"];
        if p.is_null() {
            return;
        }
        ui.separator();
        ui.label(p["label"].as_str().unwrap_or("Edit preview"));
        let stale = p["stale"].as_bool().unwrap_or(true);
        if stale {
            ui.label("The project changed. Generate a fresh preview.");
        }
        let ranges = p["ranges"].as_array();
        let cuts = ranges.map_or(0, Vec::len);
        ui.label(format!(
            "{cuts} cuts · remove {:.2}s · resulting duration {:.2}s",
            p["removedSeconds"].as_f64().unwrap_or(0.0),
            p["outputSeconds"].as_f64().unwrap_or(0.0)
        ));
        if let Some(ranges) = ranges {
            for (i, range) in ranges.iter().enumerate() {
                let (a, b) = (range["start"].as_i64().unwrap_or(0), range["end"].as_i64().unwrap_or(0));
                let r = ui.add_enabled(
                    !stale,
                    egui::Button::new(format!("Cut {}: {:.3}s – {:.3}s · view", i + 1, filmcraft_time::Tick(a).seconds(), filmcraft_time::Tick(b).seconds())),
                );
                app.auto.add(&format!("assistant.range.{i}"), r.rect, "View proposed cut");
                if r.clicked() {
                    actions.push(("playhead.set", json!({"time":a})));
                }
            }
        }
        if let Some(matches) = p["matches"].as_array() {
            for (i, hit) in matches.iter().enumerate() {
                let a = hit["start"].as_i64().unwrap_or(0);
                let r = ui.add_enabled(!stale, egui::Button::new(format!("Match {} · {:.3}s · go", i + 1, filmcraft_time::Tick(a).seconds())));
                app.auto.add(&format!("assistant.match.{i}"), r.rect, "Go to transcript match");
                if r.clicked() {
                    actions.push(("playhead.set", json!({"time":a})));
                }
            }
        }
        let edits = p["actions"].as_array().map_or(0, Vec::len);
        if let Some(actions) = p["actions"].as_array() {
            for a in actions {
                let label = match a["type"].as_str().unwrap_or("") {
                    "split" => format!("Split clip {} at {}s", a["clip"], a["time"]),
                    "move" => format!("Move clip {} to {}s on track {}", a["clip"], a["start"], a["track"]),
                    "trim" => format!("Trim clip {} {} edge by {}s", a["clip"], a["edge"].as_str().unwrap_or(""), a["delta"]),
                    "gain" => format!("Set clip {} audio gain to {} dB", a["clip"], a["db"]),
                    _ => "Editing action".into(),
                };
                ui.label(label);
            }
        }
        let navigate = p["seek"].as_i64().is_some();
        let r = ui.add_enabled(
            !stale && (cuts > 0 || navigate || edits > 0),
            egui::Button::new(if navigate { "Go to transcript match" } else { "Apply editing plan · one undo step" }),
        );
        app.auto.add("assistant.apply", r.rect, "Apply reviewed proposal");
        if r.clicked() {
            actions.push(("assistant.apply", json!({"token":p["token"]})));
        }
        let r = ui.add_enabled(!stale && (cuts > 0 || navigate || edits > 0), egui::Button::new("Copy reviewed training example"));
        app.auto.add("assistant.trainingExample", r.rect, "Copy reviewed editing training example");
        if r.clicked() {
            match app.session.execute("ai.trainingExample", json!({"instruction":app.ui.assistant_instruction})) {
                Ok(v) => ui.ctx().copy_text(v.to_string()),
                Err(e) => app.ui.status = e.to_string(),
            }
        }
    });
    for (command, params) in actions {
        if let Err(e) = crate::menus::invoke(app, ui.ctx(), command, params) {
            app.ui.status = e;
            break;
        }
    }
}

fn invoke(app: &mut FilmcraftApp, ui: &Ui, id: &str, params: serde_json::Value) -> bool {
    if let Err(e) = crate::menus::invoke(app, ui.ctx(), id, params) {
        app.ui.status = e;
        false
    } else {
        true
    }
}
fn services(app: &mut FilmcraftApp, ui: &mut Ui) {
    let state = app.session.execute("ai.inspect", json!({})).unwrap_or_else(|e| json!({"status":{"error":e.to_string()}}));
    let busy = state["busy"].as_bool().unwrap_or(false);
    if let Some(list) = state["status"]["result"]["voices"].as_array() {
        app.ui.ai.voices = list.iter().filter_map(|v| v["id"].as_str().map(str::to_string)).collect();
    }
    let section = egui::CollapsingHeader::new("AI connections").show(ui, |ui| {
        ui.label("Kokoro base URL");
        let r = ui.text_edit_singleline(&mut app.ui.ai.config.kokoro_url);
        app.auto.add("ai.kokoroUrl", r.rect, "Kokoro URL");
        ui.label("ComfyUI base URL");
        let r = ui.text_edit_singleline(&mut app.ui.ai.config.comfy_url);
        app.auto.add("ai.comfyUrl", r.rect, "ComfyUI URL");
        ui.label("Planner OpenAI-compatible base URL (including /v1)");
        let r = ui.text_edit_singleline(&mut app.ui.ai.config.planner_url);
        app.auto.add("ai.plannerUrl", r.rect, "Planner URL");
        ui.horizontal(|ui| {
            let r = ui.button("OpenAI cloud");
            app.auto.add("ai.provider.openai", r.rect, "Use OpenAI cloud");
            if r.clicked() {
                app.ui.ai.config.planner_url = "https://api.openai.com/v1".into();
                app.ui.ai.config.model = "gpt-4.1".into();
                app.ui.ai.local = false;
            }
            let r = ui.button("Ollama / local");
            app.auto.add("ai.provider.local", r.rect, "Use local OpenAI-compatible model");
            if r.clicked() {
                app.ui.ai.config.planner_url = "http://127.0.0.1:11434/v1".into();
                app.ui.ai.config.model.clear();
            }
        });
        ui.label("API key (session only, or OPENAI_API_KEY for OpenAI)");
        let r = ui.add(egui::TextEdit::singleline(&mut app.ui.ai.api_key).password(true));
        app.auto.add("ai.apiKey", r.rect, "Planner API key");
        ui.label("Planner model");
        let r = ui.text_edit_singleline(&mut app.ui.ai.config.model);
        app.auto.add("ai.model", r.rect, "Planner model");
        ui.small("Connections run on your configured services. FilmCraft does not bundle models. Native app / MCP only.");
        let r = ui.add_enabled(!busy, egui::Button::new("Apply connections"));
        app.auto.add("ai.configure", r.rect, "Apply AI connection settings");
        if r.clicked() {
            let mut config = json!(app.ui.ai.config);
            config["apiKey"] = json!(app.ui.ai.api_key);
            invoke(app, ui, "ai.configure", config);
        }
    });
    app.auto.add("ai.section.connections", section.header_response.rect, "AI connections");
    let section = egui::CollapsingHeader::new("AI voices · Kokoro").show(ui, |ui| {
        ui.horizontal(|ui| {
            for lang in ["es", "en"] {
                let r = ui.selectable_label(app.ui.ai.language == lang, lang);
                app.auto.add(&format!("ai.language.{lang}"), r.rect, lang);
                if r.clicked() {
                    app.ui.ai.language = lang.into();
                    app.ui.ai.voice = if lang == "es" { "ef_dora" } else { "af_heart" }.into();
                }
            }
            let r = ui.add_enabled(!busy, egui::Button::new("Load voices"));
            app.auto.add("ai.voices", r.rect, "Load service voice list");
            if r.clicked() && invoke(app, ui, "ai.configure", json!(app.ui.ai.config)) {
                invoke(app, ui, "ai.voices", json!({}));
            }
        });
        let r = ui.text_edit_singleline(&mut app.ui.ai.voice);
        app.auto.add("ai.voice", r.rect, "Voice id");
        let combo = egui::ComboBox::from_id_salt("ai-voice-list").selected_text(&app.ui.ai.voice).show_ui(ui, |ui| {
            for id in &app.ui.ai.voices {
                if (app.ui.ai.language == "es" && id.starts_with('e')) || (app.ui.ai.language == "en" && id.starts_with(['a', 'b'])) {
                    let r = ui.selectable_value(&mut app.ui.ai.voice, id.clone(), id);
                    app.auto.add(&format!("ai.voice.choose.{id}"), r.rect, id);
                }
            }
        });
        app.auto.add("ai.voiceList", combo.response.rect, "Available service voices");
        let r = ui.add(egui::TextEdit::multiline(&mut app.ui.ai.speech).desired_rows(3).hint_text("Narration text / Texto de la narración"));
        app.auto.add("ai.speechText", r.rect, "Narration text");
        let r = ui.add(egui::Slider::new(&mut app.ui.ai.speed, 0.5..=2.0).text("Speed"));
        app.auto.add("ai.speed", r.rect, "Voice speed");
        let r = ui.add_enabled(!busy && !app.ui.ai.speech.trim().is_empty(), egui::Button::new("Generate voice"));
        app.auto.add("ai.speak", r.rect, "Generate Kokoro voice");
        if r.clicked() && invoke(app, ui, "ai.configure", json!(app.ui.ai.config)) {
            invoke(app, ui, "ai.speak", json!({"text":app.ui.ai.speech,"voice":app.ui.ai.voice,"language":app.ui.ai.language,"speed":app.ui.ai.speed}));
        }
    });
    app.auto.add("ai.section.voices", section.header_response.rect, "AI voices · Kokoro");
    let section = egui::CollapsingHeader::new("ComfyUI workflow").show(ui, |ui| {
        ui.label("Paste the workflow exported with ComfyUI's Export (API). Models and nodes run on that server.");
        let r = ui.add(egui::TextEdit::multiline(&mut app.ui.ai.workflow).desired_rows(4).code_editor().desired_width(ui.available_width()));
        app.auto.add("ai.workflow", r.rect, "ComfyUI API workflow JSON");
        ui.horizontal(|ui| {
            let r = ui.add_enabled(!busy, egui::Button::new("Test connection"));
            app.auto.add("ai.connectComfy", r.rect, "Test ComfyUI connection");
            if r.clicked() && invoke(app, ui, "ai.configure", json!(app.ui.ai.config)) {
                invoke(app, ui, "ai.connectComfy", json!({}));
            }
            let r = ui.add_enabled(!busy && !app.ui.ai.workflow.trim().is_empty(), egui::Button::new("Generate media"));
            app.auto.add("ai.comfy", r.rect, "Run ComfyUI workflow");
            if r.clicked() {
                match serde_json::from_str::<serde_json::Value>(&app.ui.ai.workflow) {
                    Ok(w) => {
                        if invoke(app, ui, "ai.configure", json!(app.ui.ai.config)) {
                            invoke(app, ui, "ai.comfy", json!({"workflow":w}));
                        }
                    }
                    Err(e) => app.ui.status = format!("Invalid workflow JSON: {e}"),
                }
            }
        });
    });
    app.auto.add("ai.section.comfy", section.header_response.rect, "ComfyUI workflow");
    let section = egui::CollapsingHeader::new("Autopilot · editing tasks").show(ui, |ui| {
        ui.label("One instruction per line, up to eight. Executes in order on the current timeline. Each task is undoable; stops on errors or outside edits.");
        let r = ui.add(egui::TextEdit::multiline(&mut app.ui.ai.autopilot_instructions).desired_rows(3).hint_text("corta los silencios\nbusca despedida"));
        app.auto.add("ai.autopilotInstructions", r.rect, "Autopilot instructions");
        let r = ui.add_enabled(!busy && !app.ui.ai.autopilot_instructions.trim().is_empty(), egui::Button::new("Run Autopilot · apply edits"));
        app.auto.add("ai.autopilot", r.rect, "Run Autopilot and apply edits");
        if r.clicked() && invoke(app, ui, "ai.configure", json!(app.ui.ai.config)) {
            let steps: Vec<_> = app.ui.ai.autopilot_instructions.lines().filter(|s| !s.trim().is_empty()).map(str::to_string).collect();
            invoke(app, ui, "ai.autopilot", json!({"instructions":steps,"apply":true,"local":app.ui.ai.local}));
        }
    });
    app.auto.add("ai.section.autopilot", section.header_response.rect, "Autopilot · editing tasks");
    if let Some(e) = state["status"]["error"].as_str() {
        ui.label(e);
    }
    if let Some(v) = state["status"]["state"].as_str() {
        ui.label(format!("AI: {v}"));
    }
    if busy {
        ui.spinner();
        let r = ui.button("Cancel AI job");
        app.auto.add("ai.cancel", r.rect, "Cancel AI generation or autopilot");
        if r.clicked() {
            invoke(app, ui, "ai.cancel", json!({}));
        }
    }
    let ready = &state["ready"];
    if !ready.is_null() {
        if let Some(assets) = ready["assets"].as_array() {
            for a in assets {
                ui.label(format!("{} · {} bytes", a["name"].as_str().unwrap_or("Generated media"), a["bytes"]));
            }
        }
        let valid = ready["stale"] == false;
        ui.horizontal(|ui| {
            let r = ui.add_enabled(valid, egui::Button::new("Import into project"));
            app.auto.add("ai.import", r.rect, "Import generated media");
            if r.clicked() {
                invoke(app, ui, "ai.import", json!({"token":ready["token"]}));
            }
            let r = ui.add_enabled(valid && app.session.active_sequence().is_some(), egui::Button::new("Add at playhead · one undo"));
            app.auto.add("ai.place", r.rect, "Import and place generated media");
            if r.clicked() {
                invoke(app, ui, "ai.import", json!({"token":ready["token"],"place":true,"seconds":app.session.playhead().seconds()}));
            }
        });
        if !valid {
            ui.label("Project changed since generation. Generate again before importing.");
        }
    }
}
