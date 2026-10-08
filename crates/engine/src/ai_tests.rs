use super::*;
fn session() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}
#[test]
fn hostile_parameters_never_start_jobs_or_change_project() {
    let mut s = session();
    let before = s.project.clone();
    for (cmd, p) in [
        ("ai.configure", json!({"kokoroUrl":"file:///etc/passwd"})),
        ("ai.configure", json!({"model":"x","unknown":0})),
        ("ai.configure", json!({"plannerUrl":"http://u:p@localhost:1234"})),
        ("ai.speak", json!({"text":"hola","voice":"../../x","language":"es"})),
        ("ai.speak", json!({"text":"hola","voice":"af_heart","language":"es"})),
        ("ai.speak", json!({"text":"hi","voice":"af_heart","language":"en","speed":0})),
        ("ai.comfy", json!({"workflow":{"nodes":[]}})),
        ("ai.autopilot", json!({"instructions":["corta"],"apply":false})),
        ("ai.autopilot", json!({"instructions":vec!["cut";9],"apply":true})),
        ("ai.import", json!({"token":1,"seconds":-1})),
    ] {
        assert!(s.execute(cmd, p).is_err(), "{cmd}");
        assert!(!s.ai.busy());
        assert!(Arc::ptr_eq(&before, &s.project));
    }
}
#[test]
fn reviewed_example_uses_exact_validated_actions() {
    let mut s = session();
    s.execute("assistant.plan", json!({"schemaVersion":1,"actions":[{"type":"extract","start":1.0,"end":2.0}]})).unwrap();
    let row = s.execute("ai.trainingExample", json!({"instruction":"Remove the interval between one and two seconds"})).unwrap();
    let plan: Value = serde_json::from_str(row["messages"][2]["content"].as_str().unwrap()).unwrap();
    assert_eq!(plan["schemaVersion"], 1);
    let action = &plan["actions"][0];
    assert_eq!(action["type"], "extract");
    assert!(action["start"].as_f64().unwrap() >= 1.0 && action["end"].as_f64().unwrap() <= 2.0);
    let preview = s.execute("assistant.inspect", json!({})).unwrap();
    let range = &preview["plan"]["ranges"][0];
    assert_eq!(action["start"], json!(Tick(range["start"].as_i64().unwrap()).seconds()));
    assert_eq!(action["end"], json!(Tick(range["end"].as_i64().unwrap()).seconds()));
    s.execute("sequence.addEdit", json!({"seconds":3.0})).unwrap();
    assert!(s.execute("ai.trainingExample", json!({"instruction":"cut"})).is_err());
}
#[test]
fn generated_audio_import_place_save_reopen_undo_redo() {
    let mut s = session();
    let before = s.project.clone();
    let history = s.history.undo.len();
    let duration = s.active_sequence().unwrap().duration().seconds();
    let frames = 24481;
    let bytes = filmcraft_media::wav::write_wav16(&vec![0.2; frames], 1, 24000);
    s.ai.ready =
        Some(Ready { token: 1, snapshot: s.project.clone(), sequence: s.state.active_sequence, assets: vec![Asset { name: "voice.wav".into(), bytes }] });
    let result = s.execute("ai.import", json!({"token":1,"place":true,"seconds":duration})).unwrap();
    assert_eq!(s.history.undo.len(), history + 1);
    let id = ItemId(result["items"][0].as_u64().unwrap());
    let path = match &s.project.item(id).unwrap().as_media().unwrap().media {
        MediaRef::File { path } => path.clone(),
        _ => panic!(),
    };
    assert!(std::path::Path::new(&path).exists());
    let expected = Tick::from_units(frames as i64, 24000);
    let clip = s.active_sequence().unwrap().audio_tracks.iter().flat_map(|t| &t.items).find(|c| c.item == id).unwrap();
    assert_eq!(clip.duration, expected);
    assert_eq!(clip.source_out() - clip.source_in, expected);
    assert!((s.active_sequence().unwrap().duration().seconds() - duration - expected.seconds()).abs() < 0.05);
    let encoded = filmcraft_format::encode(&s.project, false);
    let reloaded = filmcraft_format::decode(&encoded).unwrap();
    assert_eq!(reloaded.project, s.project.as_ref().clone());
    s.undo().unwrap();
    assert_eq!(*s.project, *before);
    s.redo().unwrap();
    assert!(s.project.item(id).is_some());
    std::fs::remove_file(path).unwrap();
}
#[test]
fn invalid_media_and_occupied_track_are_atomic() {
    let mut s = session();
    s.edit_sequence("Lock all audio destinations", |q, _, _| {
        for t in &mut q.audio_tracks {
            t.locked = true;
        }
        Ok(())
    })
    .unwrap();
    let before = s.project.clone();
    for (name, bytes) in [("invalid.wav", vec![0; 10]), ("voice.wav", filmcraft_media::wav::write_wav16(&vec![0.2; 24000], 1, 24000))] {
        s.ai.ready = Some(Ready { token: 1, snapshot: s.project.clone(), sequence: s.state.active_sequence, assets: vec![Asset { name: name.into(), bytes }] });
        assert!(s.execute("ai.import", json!({"token":1,"place":true})).is_err());
        assert!(Arc::ptr_eq(&before, &s.project));
    }
}
#[test]
fn stale_generation_cannot_edit_new_project() {
    let mut s = session();
    let old = s.project.clone();
    s.execute("file.newProject", json!({})).unwrap();
    s.ai.ready = Some(Ready { token: 1, snapshot: old, sequence: s.state.active_sequence, assets: vec![] });
    assert!(s.execute("ai.import", json!({"token":1})).is_err());
}
#[test]
fn autopilot_stop_and_outside_edit_are_bounded() {
    let mut s = session();
    let before = s.project.clone();
    s.execute("ai.autopilot", json!({"instructions":["no such task"],"local":true,"apply":true})).unwrap();
    poll(&mut s);
    assert!(!s.ai.busy());
    assert_eq!(s.ai.status["state"], "error");
    assert!(Arc::ptr_eq(&before, &s.project));
    s.execute("ai.autopilot", json!({"instructions":["corta los silencios"],"local":true,"apply":true})).unwrap();
    assert!(s.execute("ai.voices", json!({})).is_err());
    assert!(s.execute("ai.plan", json!({"instruction":"cut"})).is_err());
    s.execute("sequence.addEdit", json!({"seconds":3.0})).unwrap();
    let edited = s.project.clone();
    poll(&mut s);
    assert!(!s.ai.busy());
    assert!(Arc::ptr_eq(&edited, &s.project));
    assert!(s.ai.status["error"].as_str().unwrap().contains("outside"));
}

#[test]
fn api_keys_are_not_returned_saved_or_journalled() {
    let mut s = session();
    let secret = "private-test-key-not-a-real-credential";
    let result = s.execute("ai.configure", json!({"apiKey":secret})).unwrap();
    assert!(!result.to_string().contains(secret));
    assert!(!s.execute("ai.inspect", json!({})).unwrap().to_string().contains(secret));
    assert!(!serde_json::to_string(&s.ai.config).unwrap().contains(secret));
    assert!(s.journal.iter().all(|(_, v)| !v.to_string().contains(secret)));
    s.execute("ai.configure", json!({"model":"other"})).unwrap();
    assert_eq!(s.ai.config.api_key, secret);
    s.execute("ai.configure", json!({"apiKey":""})).unwrap();
    assert!(s.ai.config.api_key.is_empty());
}

#[test]
fn local_autopilot_executes_multiple_validated_tasks_and_keeps_undo() {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"width":160,"height":90,"fps":24,"video":1,"audio":1})).unwrap();
    let bytes = filmcraft_media::wav::write_wav16(
        &(0..48000 * 4)
            .map(|i| if (48000..96000).contains(&i) || i >= 144000 { 0.2 * (i as f32 * std::f32::consts::TAU * 440.0 / 48000.0).sin() } else { 0.0 })
            .collect::<Vec<_>>(),
        1,
        48000,
    );
    let item = crate::commands::import_bytes(&mut s, "original-autopilot-fixture.wav", bytes.into(), None).unwrap();
    s.execute("timeline.place", json!({"item":item.0,"track":"A1","duration":Tick::from_seconds_f64(4.0).0})).unwrap();
    let before = s.project.clone();
    let history = s.history.undo.len();
    s.execute("ai.autopilot", json!({"instructions":["corta los silencios","version de 1 segundos"],"apply":true,"local":true})).unwrap();
    for _ in 0..200 {
        s.poll_persistence();
        if !s.ai.busy() {
            break;
        }
    }
    assert_eq!(s.ai.status["state"], "complete", "{}", s.ai.status);
    assert_eq!(s.ai.status["completed"], 2);
    assert!((s.active_sequence().unwrap().duration().seconds() - 1.0).abs() < 0.05);
    assert_eq!(s.history.undo.len(), history + 2);
    s.undo().unwrap();
    s.undo().unwrap();
    assert_eq!(*s.project, *before);
    s.redo().unwrap();
    s.redo().unwrap();
    assert!((s.active_sequence().unwrap().duration().seconds() - 1.0).abs() < 0.05);
}
