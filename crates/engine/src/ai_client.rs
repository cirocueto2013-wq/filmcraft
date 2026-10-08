//! HTTP protocols documented by Kokoro-FastAPI, ComfyUI and OpenAI-compatible planners.
use super::{Asset, Config, Output};
use serde_json::{Value, json};
use std::{
    io::Read,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, String>;
const JSON_MAX: usize = 3 * 1024 * 1024;
const MEDIA_MAX: usize = 64 * 1024 * 1024;
fn stopped(stop: &AtomicBool) -> Result<()> {
    if stop.load(Ordering::Relaxed) { Err("cancelled".into()) } else { Ok(()) }
}
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::Rustls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .unversioned_rustls_crypto_provider(std::sync::Arc::new(rustls_rustcrypto::provider()))
                .build(),
        )
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(120)))
        .max_redirects(0)
        .build()
        .into()
}
fn read(mut r: ureq::http::Response<ureq::Body>, max: usize, stop: &AtomicBool) -> Result<Vec<u8>> {
    let status = r.status();
    let mut reader = r.body_mut().as_reader();
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        stopped(stop)?;
        let n = reader.read(&mut chunk).map_err(|e| format!("AI response body: {e}"))?;
        if n == 0 {
            break;
        }
        if bytes.len().checked_add(n).is_none_or(|size| size > max) {
            return Err(format!("AI response exceeds {max} bytes"));
        }
        let part = chunk.get(..n).ok_or_else(|| "invalid response buffer".to_string())?;
        bytes.extend_from_slice(part);
    }
    if !status.is_success() {
        return Err(format!("AI service returned HTTP {status}: {}", String::from_utf8_lossy(bytes.get(..bytes.len().min(2048)).unwrap_or_default())));
    }
    Ok(bytes)
}
fn request(a: &ureq::Agent, method: &str, url: &str, body: Option<&Value>, max: usize, stop: &AtomicBool) -> Result<Vec<u8>> {
    stopped(stop)?;
    let response = match method {
        "GET" => a.get(url).config().http_status_as_error(false).build().call(),
        "POST" => {
            a.post(url).header("Content-Type", "application/json").config().http_status_as_error(false).build().send(body.unwrap_or(&Value::Null).to_string())
        }
        _ => return Err("unsupported HTTP method".into()),
    }
    .map_err(|e| format!("AI service request failed: {e}"))?;
    read(response, max, stop)
}
fn json_request(a: &ureq::Agent, method: &str, url: &str, body: Option<&Value>, stop: &AtomicBool) -> Result<Value> {
    let b = request(a, method, url, body, JSON_MAX, stop)?;
    serde_json::from_slice(&b).map_err(|e| format!("AI service returned invalid JSON: {e}"))
}
fn url(base: &str, path: &str) -> String {
    format!("{}{path}", base.trim_end_matches('/'))
}
fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
/// Kokoro-FastAPI WAVs may carry RIFF/data streaming sentinels. Replace only those sizes;
/// reject truncation, invalid headers and oversized chunks before the media decoder sees them.
fn wav(mut b: Vec<u8>) -> Result<Vec<u8>> {
    if b.get(..4) != Some(b"RIFF") || b.get(8..12) != Some(b"WAVE") {
        return Err("Kokoro did not return a RIFF WAVE file".into());
    }
    let riff = u32::try_from(b.len().saturating_sub(8)).map_err(|_| "WAV is too large")?;
    let size = b.get(4..8).ok_or("incomplete RIFF header")?;
    if size == u32::MAX.to_le_bytes() {
        b.get_mut(4..8).ok_or("incomplete RIFF header")?.copy_from_slice(&riff.to_le_bytes());
    } else if size != riff.to_le_bytes() {
        return Err("WAV RIFF length does not match response".into());
    }
    let mut at = 12usize;
    let mut data = false;
    for _ in 0..1024 {
        if at == b.len() {
            break;
        }
        let h = b.get(at..at.saturating_add(8)).ok_or("truncated WAV chunk")?;
        let tag = h.get(..4).ok_or("truncated WAV chunk")?;
        let raw: u32 = u32::from_le_bytes(h.get(4..8).ok_or("truncated WAV size")?.try_into().map_err(|_| "invalid WAV size")?);
        let is_data = tag == b"data";
        let begin = at.checked_add(8).ok_or("WAV offset overflow")?;
        let len = if raw == u32::MAX && is_data { b.len().saturating_sub(begin) } else { usize::try_from(raw).map_err(|_| "WAV size overflow")? };
        let end = begin.checked_add(len).filter(|e| *e <= b.len()).ok_or("truncated WAV payload")?;
        if is_data {
            if data || len == 0 {
                return Err("empty or multiple WAV data chunks".into());
            }
            data = true;
            if raw == u32::MAX {
                let n = u32::try_from(len).map_err(|_| "WAV data too large")?;
                b.get_mut(at.saturating_add(4)..begin).ok_or("invalid WAV size field")?.copy_from_slice(&n.to_le_bytes());
            }
        }
        at = end.checked_add(len % 2).ok_or("WAV padding overflow")?;
    }
    if !data || at != b.len() {
        return Err("invalid or excessively fragmented WAV".into());
    }
    Ok(b)
}
fn cancel_comfy(a: &ureq::Agent, base: &str, id: &str) -> Result<()> {
    let stop = AtomicBool::new(false);
    // New ComfyUI provides an atomic per-job cancellation route. Never interrupt all users.
    match json_request(a, "POST", &url(base, &format!("/api/jobs/{}/cancel", encode(id))), Some(&json!({})), &stop) {
        Ok(_) => Ok(()),
        Err(e) if e.contains("HTTP 404") => {
            json_request(a, "POST", &url(base, "/queue"), Some(&json!({"delete":[id]})), &stop)?;
            Err("This ComfyUI version can remove queued jobs but cannot safely cancel a running job by id; its running generation may continue.".into())
        }
        Err(e) => Err(format!("ComfyUI job cancellation failed: {e}")),
    }
}
fn comfy(a: &ureq::Agent, c: &Config, p: &Value, stop: &AtomicBool) -> Result<Output> {
    stopped(stop)?;
    // Once queued, finish reading its id even if cancellation arrives during submission. This
    // lets us cancel that specific job instead of abandoning an unidentified remote generation.
    let submitted = json_request(a, "POST", &url(&c.comfy_url, "/prompt"), Some(p), &AtomicBool::new(false))?;
    if submitted.get("error").is_some() || submitted.get("node_errors").and_then(Value::as_object).is_some_and(|o| !o.is_empty()) {
        return Err(format!("ComfyUI rejected workflow: {submitted}"));
    }
    let id = submitted["prompt_id"].as_str().filter(|id| !id.is_empty() && id.len() <= 256).ok_or("ComfyUI did not return a prompt_id")?;
    let started = Instant::now();
    let result = (|| -> Result<Output> {
        loop {
            stopped(stop)?;
            if started.elapsed() > Duration::from_secs(600) {
                return Err("ComfyUI job exceeded ten minutes".into());
            }
            let history = json_request(a, "GET", &url(&c.comfy_url, &format!("/history/{}", encode(id))), None, stop)?;
            if let Some(h) = history.get(id) {
                if h["status"]["status_str"] == "error" {
                    return Err(format!("ComfyUI execution failed: {}", h["status"]));
                }
                if h["status"]["completed"] == true {
                    let outputs = h["outputs"].as_object().ok_or("ComfyUI completed without outputs")?;
                    let mut assets = Vec::new();
                    let mut total = 0usize;
                    for node in outputs.values() {
                        for key in ["images", "audio", "gifs", "videos"] {
                            if let Some(list) = node.get(key).and_then(Value::as_array) {
                                for item in list {
                                    if assets.len() >= 8 {
                                        return Err("ComfyUI returned more than eight media files; reduce workflow batch size".into());
                                    }
                                    let name = item["filename"].as_str().ok_or("ComfyUI output has no filename")?;
                                    let folder = item["subfolder"].as_str().unwrap_or("");
                                    let kind = item["type"].as_str().unwrap_or("output");
                                    if name.is_empty()
                                        || name.len() > 256
                                        || name.contains(['/', '\\'])
                                        || folder.len() > 1024
                                        || folder.split(['/', '\\']).any(|s| s == "..")
                                        || !matches!(kind, "output" | "temp")
                                    {
                                        return Err("ComfyUI returned an invalid output location".into());
                                    }
                                    let ext = std::path::Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
                                    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "wav" | "mp3" | "flac" | "mp4" | "mov") {
                                        return Err(format!("unsupported ComfyUI output format: {ext}"));
                                    }
                                    let view =
                                        url(&c.comfy_url, &format!("/view?filename={}&subfolder={}&type={}", encode(name), encode(folder), encode(kind)));
                                    let remaining = MEDIA_MAX.saturating_sub(total);
                                    let bytes = request(a, "GET", &view, None, remaining, stop)?;
                                    total = total.checked_add(bytes.len()).ok_or("ComfyUI output size overflow")?;
                                    assets.push(Asset { name: name.into(), bytes });
                                }
                            }
                        }
                    }
                    if assets.is_empty() {
                        return Err("ComfyUI produced no supported media files; add a save/output node".into());
                    }
                    return Ok(Output::Assets(assets));
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    })();
    if result.is_err()
        && let Err(e) = cancel_comfy(a, &c.comfy_url, id)
    {
        return Err(format!("{}; {e}", result.err().unwrap_or_else(|| "generation failed".into())));
    }
    result
}
fn run_inner(kind: &str, c: &Config, p: &Value, stop: &AtomicBool) -> Result<Output> {
    let a = agent();
    match kind {
        "voices" => {
            let v = json_request(&a, "GET", &url(&c.kokoro_url, "/v1/audio/voices"), None, stop)?;
            let voices = v["voices"].as_array().filter(|v| v.len() <= 512).ok_or("invalid Kokoro voice list")?;
            let mut list = Vec::new();
            for voice in voices {
                let id = voice.as_str().or_else(|| voice["id"].as_str()).ok_or("invalid Kokoro voice id")?;
                if id.len() > 128 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    return Err("invalid Kokoro voice id".into());
                }
                if id.starts_with(['a', 'b', 'e']) {
                    list.push(json!({"id":id,"language":if id.starts_with('e'){"es"}else{"en"}}));
                }
            }
            Ok(Output::Json(json!({"voices":list})))
        }
        "speak" => {
            let bytes = wav(request(&a, "POST", &url(&c.kokoro_url, "/v1/audio/speech"), Some(p), MEDIA_MAX, stop)?)?;
            Ok(Output::Assets(vec![Asset { name: format!("{}.wav", p["voice"].as_str().unwrap_or("voice")), bytes }]))
        }
        "connectComfy" => Ok(Output::Json(json_request(&a, "GET", &url(&c.comfy_url, "/system_stats"), None, stop)?)),
        "comfy" => comfy(&a, c, p, stop),
        "plan" => {
            stopped(stop)?;
            let endpoint = url(&c.planner_url, "/chat/completions");
            let mut req = a.post(&endpoint).header("Content-Type", "application/json");
            let key = &c.api_key;
            if !key.is_empty() {
                req = req.header("Authorization", format!("Bearer {key}"));
            }
            let response = req.config().http_status_as_error(false).build().send(p.to_string()).map_err(|e| format!("Planner request: {e}"))?;
            let body = read(response, JSON_MAX, stop).map_err(|e| if key.is_empty() { e } else { e.replace(key, "[redacted]") })?;
            let v: Value = serde_json::from_slice(&body).map_err(|e| format!("Planner returned invalid JSON: {e}"))?;
            let text = v["choices"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|a| a["message"]["content"].as_str())
                .ok_or("planner returned no assistant content")?;
            if text.len() > 128 * 1024 {
                return Err("planner response too large".into());
            }
            let proposal: Value = serde_json::from_str(text).map_err(|e| format!("planner must return the FilmCraft JSON editing schema: {e}"))?;
            if let Some(e) = proposal["error"].as_str() {
                return Err(format!("planner cannot perform this task: {e}"));
            }
            Ok(Output::Plan(proposal))
        }
        _ => Err("unsupported AI job".into()),
    }
}
pub(super) fn run(kind: &str, c: &Config, p: &Value, stop: &AtomicBool) -> Result<Output> {
    let mut config = c.clone();
    // Resolve environment credentials here so every error path, including parsed model errors,
    // redacts them. Never send an OpenAI environment key to another configured endpoint.
    if kind == "plan" && config.api_key.is_empty() && config.planner_url.trim_end_matches('/') == "https://api.openai.com/v1" {
        config.api_key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
        if config.api_key.len() > 8192 || config.api_key.chars().any(char::is_control) {
            return Err("invalid OPENAI_API_KEY".into());
        }
    }
    run_inner(kind, &config, p, stop).map_err(|e| if config.api_key.is_empty() { e } else { e.replace(&config.api_key, "[redacted]") })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn streaming_wav_is_canonical_and_truncation_is_rejected() {
        let mut original = filmcraft_media::wav::write_wav16(&vec![0.1; 100], 1, 24000);
        let canonical = original.clone();
        original[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        original[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(wav(original).unwrap(), canonical);
        for len in 0..canonical.len() {
            assert!(wav(canonical[..len].to_vec()).is_err());
        }
    }
    #[test]
    fn output_query_preserves_spaces_and_delimiters() {
        assert_eq!(encode("a b&é.png"), "a%20b%26%C3%A9.png");
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    struct Reply {
        path: &'static str,
        status: u16,
        body: Vec<u8>,
    }
    fn server(replies: Vec<Reply>) -> (String, std::thread::JoinHandle<Vec<Vec<u8>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for reply in replies {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut request = Vec::new();
                let mut buf = [0; 1024];
                let (header_end, len) = loop {
                    let n = stream.read(&mut buf).unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&buf[..n]);
                    if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&request[..at]);
                        let len = header
                            .lines()
                            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|n| n.trim().parse::<usize>().unwrap()))
                            .unwrap_or(0);
                        assert!(header.lines().next().unwrap().contains(reply.path), "{header}");
                        break (at + 4, len);
                    }
                };
                while request.len() < header_end + len {
                    let n = stream.read(&mut buf).unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&buf[..n]);
                }
                requests.push(request);
                write!(stream, "HTTP/1.1 {} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", reply.status, reply.body.len()).unwrap();
                stream.write_all(&reply.body).unwrap();
            }
            requests
        });
        (url, handle)
    }
    #[test]
    fn kokoro_protocol_generates_wav_and_preserves_spanish_language() {
        let bytes = filmcraft_media::wav::write_wav16(&vec![0.2; 100], 1, 24000);
        let (url, thread) = server(vec![Reply { path: "/v1/audio/speech", status: 200, body: bytes.clone() }]);
        let config = Config { kokoro_url: url, ..Default::default() };
        let stop = AtomicBool::new(false);
        let Output::Assets(assets) = run("speak", &config, &json!({"input":"Hola","voice":"ef_dora","lang_code":"e","response_format":"wav"}), &stop).unwrap()
        else {
            panic!()
        };
        assert_eq!(assets[0].bytes, bytes);
        let requests = thread.join().unwrap();
        assert!(String::from_utf8_lossy(&requests[0]).contains("\"lang_code\":\"e\""));
    }
    #[test]
    fn planner_authorization_is_sent_but_errors_are_redacted() {
        let secret = "test-key-not-a-credential";
        let (url, thread) = server(vec![Reply { path: "/v1/chat/completions", status: 401, body: format!("{{\"error\":\"{secret}\"}}").into_bytes() }]);
        let config = Config { planner_url: format!("{url}/v1"), api_key: secret.into(), ..Default::default() };
        let error = run("plan", &config, &json!({}), &AtomicBool::new(false)).err().unwrap();
        assert!(error.contains("401") && !error.contains(secret));
        let requests = thread.join().unwrap();
        assert!(String::from_utf8_lossy(&requests[0]).to_ascii_lowercase().contains(&format!("authorization: bearer {secret}")));
    }
    #[test]
    fn successful_http_response_cannot_echo_credentials_in_a_planner_error() {
        let secret = "test-echoed-key-not-a-credential";
        let proposal = json!({"error":secret}).to_string();
        let (url, thread) = server(vec![Reply {
            path: "/v1/chat/completions",
            status: 200,
            body: json!({"choices":[{"message":{"content":proposal}}]}).to_string().into_bytes(),
        }]);
        let config = Config { planner_url: format!("{url}/v1"), api_key: secret.into(), ..Default::default() };
        let error = run("plan", &config, &json!({}), &AtomicBool::new(false)).err().unwrap();
        assert!(error.contains("[redacted]") && !error.contains(secret));
        thread.join().unwrap();
    }
    #[test]
    fn comfy_history_and_output_download_use_documented_protocol() {
        let id = "owned-job";
        let (url,thread)=server(vec![
            Reply{path:"POST /prompt ",status:200,body:json!({"prompt_id":id,"node_errors":{}}).to_string().into_bytes()},
            Reply{path:"GET /history/owned-job ",status:200,body:json!({id:{"status":{"status_str":"success","completed":true},"outputs":{"9":{"images":[{"filename":"frame one.png","subfolder":"","type":"output"}]}}}}).to_string().into_bytes()},
            Reply{path:"/view?filename=frame%20one.png&subfolder=&type=output",status:200,body:b"encoded-media-protocol-fixture".to_vec()},
        ]);
        let config = Config { comfy_url: url, ..Default::default() };
        let Output::Assets(assets) = run("comfy", &config, &json!({"prompt":{"9":{"class_type":"SaveImage","inputs":{}}}}), &AtomicBool::new(false)).unwrap()
        else {
            panic!()
        };
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].name, "frame one.png");
        thread.join().unwrap();
    }
    #[test]
    fn comfy_cancellation_targets_only_the_owned_job() {
        let (url, thread) = server(vec![Reply { path: "POST /api/jobs/owned/cancel ", status: 200, body: json!({"cancelled":true}).to_string().into_bytes() }]);
        cancel_comfy(&agent(), &url, "owned").unwrap();
        thread.join().unwrap();
    }
    #[test]
    fn old_comfy_cancellation_dequeues_without_global_interrupt() {
        let (url, thread) = server(vec![
            Reply { path: "/api/jobs/owned/cancel", status: 404, body: b"not found".to_vec() },
            Reply { path: "POST /queue ", status: 200, body: b"{}".to_vec() },
        ]);
        assert!(cancel_comfy(&agent(), &url, "owned").unwrap_err().contains("may continue"));
        let r = thread.join().unwrap();
        assert!(String::from_utf8_lossy(&r[1]).contains("\"delete\":[\"owned\"]"));
    }
    #[test]
    fn response_allocation_is_bounded() {
        let (url, thread) = server(vec![Reply { path: "GET /", status: 200, body: vec![0; 1024] }]);
        assert!(request(&agent(), "GET", &url, None, 100, &AtomicBool::new(false)).unwrap_err().contains("exceeds"));
        thread.join().unwrap();
    }
}
