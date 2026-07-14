// 대화 기록 파일 감시: Claude Code(CLI+데스크탑) / Codex 의 완료를 훅 없이 감지
//   - Claude:  ~/.claude/projects/*/*.jsonl  (마지막이 '도구호출 없는 assistant 텍스트'면 턴 완료)
//   - Codex:   ~/.codex/sessions/**/*.jsonl  (payload.type == "task_complete")
// 앱 시작 이후 타임스탬프의 완료만 알림(과거 완료 무시). 파일은 증분으로만 읽음.
use crate::{dispatch_notification, short, TaskDone};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};
use tauri::AppHandle;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

const POLL_MS: u64 = 1200;
const RECENT_SECS: u64 = 60; // 최근 이만큼 수정된 파일만 처리
const CLAUDE_QUIET: u32 = 1; // 완료 후보가 이만큼 안정되면 알림(스트리밍 오탐 방지)
const APPROVAL_QUIET: u32 = 12; // 도구 호출 후 이만큼(≈14초) 결과 없으면 '승인 필요' 추정(생각/처리중 오탐 방지)
const TAIL_BYTES: u64 = 512 * 1024; // 최초 목격 시 훑을 꼬리 크기

static START: OnceLock<OffsetDateTime> = OnceLock::new();

#[derive(Default)]
struct FState {
    initialized: bool,
    offset: u64,
    partial: String,
    // Claude
    custom_title: String,
    ai_title: String,
    last_user: String,
    last_assistant: String,
    cand_marker: String,
    cand_ts: String,
    tail_candidate: bool,
    notified: String,
    quiet: u32,
    // 승인 대기 추정 (마지막이 도구호출 tool_use)
    pending_tool: bool,
    pending_marker: String,
    pending_ts: String,
    pending_detail: String,
    pending_quiet: u32,
    approval_notified: String,
    // Codex
    codex_id: String, // 세션 UUID (파일명에서 추출)
    codex_title: String,
    codex_notified: String,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into()))
}

/// 완료 타임스탬프가 앱 시작 이후인지 (과거 완료 무시)
fn after_start(ts: &str) -> bool {
    let start = match START.get() {
        Some(s) => *s,
        None => return true,
    };
    match OffsetDateTime::parse(ts, &Rfc3339) {
        Ok(t) => t > start,
        Err(_) => true,
    }
}

pub fn spawn(app: AppHandle) {
    let _ = START.set(OffsetDateTime::now_utc());
    std::thread::spawn(move || {
        let mut states: HashMap<PathBuf, FState> = HashMap::new();
        let claude_root = home().join(".claude").join("projects");
        let codex_root = home().join(".codex").join("sessions");
        loop {
            let mut files: Vec<(PathBuf, bool)> = Vec::new();
            collect_recent(&claude_root, false, &mut files, 0);
            collect_recent(&codex_root, true, &mut files, 0);
            for (path, is_codex) in files {
                let st = states.entry(path.clone()).or_default();
                process(&app, &path, is_codex, st);
            }
            std::thread::sleep(Duration::from_millis(POLL_MS));
        }
    });
}

fn collect_recent(dir: &Path, is_codex: bool, out: &mut Vec<(PathBuf, bool)>, depth: u32) {
    if depth > 6 {
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let now = SystemTime::now();
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_recent(&p, is_codex, out, depth + 1);
        } else if p.extension().map(|x| x == "jsonl").unwrap_or(false) {
            if let Ok(meta) = e.metadata() {
                if let Ok(modified) = meta.modified() {
                    if now
                        .duration_since(modified)
                        .map(|d| d.as_secs() <= RECENT_SECS)
                        .unwrap_or(false)
                    {
                        out.push((p, is_codex));
                    }
                }
            }
        }
    }
}

fn process(app: &AppHandle, path: &Path, is_codex: bool, st: &mut FState) {
    let len = match fs::metadata(path) {
        Ok(m) => m.len(),
        Err(_) => return,
    };

    let lines: Vec<String> = if !st.initialized {
        st.initialized = true;
        if is_codex {
            st.codex_id = session_id_from_path(path);
        }
        // 제목은 head에서, 최근 완료는 꼬리에서 (타임스탬프로 과거 완료는 걸러짐)
        let head = read_head(path, 65536);
        load_titles(&head, is_codex, st);
        let tail = read_tail(path, len);
        st.offset = len;
        tail
    } else {
        read_new(path, st, len)
    };

    if !lines.is_empty() {
        st.quiet = 0;
        st.pending_quiet = 0;
        for line in &lines {
            let v: Value = match serde_json::from_str(line.trim_start_matches('\u{feff}')) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if is_codex {
                process_codex_line(app, st, &v);
            } else {
                process_claude_line(st, &v);
            }
        }
    }

    if is_codex {
        return;
    }

    let title = if !st.custom_title.is_empty() {
        st.custom_title.clone()
    } else if !st.ai_title.is_empty() {
        st.ai_title.clone()
    } else if !st.last_user.is_empty() {
        st.last_user.clone()
    } else {
        "작업".to_string()
    };

    if st.tail_candidate && st.cand_marker != st.notified && after_start(&st.cand_ts) {
        // 완료: 도구호출 없는 텍스트가 안정되면
        st.quiet += 1;
        st.pending_quiet = 0;
        if st.quiet >= CLAUDE_QUIET {
            dispatch_notification(
                app,
                TaskDone {
                    source: "claude".into(),
                    kind: "completed".into(),
                    message: short(&title, 30),
                    detail: short(&st.last_assistant, 55),
                    hwnd: 0,
                },
            );
            st.notified = st.cand_marker.clone();
            st.quiet = 0;
        }
    } else if st.pending_tool
        && st.pending_marker != st.approval_notified
        && after_start(&st.pending_ts)
    {
        // 승인 대기 추정: 도구호출 후 결과 없이 조용하면
        st.pending_quiet += 1;
        st.quiet = 0;
        if st.pending_quiet >= APPROVAL_QUIET {
            dispatch_notification(
                app,
                TaskDone {
                    source: "claude".into(),
                    kind: "approval".into(),
                    message: short(&title, 30),
                    detail: short(&st.pending_detail, 55),
                    hwnd: 0,
                },
            );
            st.approval_notified = st.pending_marker.clone();
            st.pending_quiet = 0;
        }
    } else {
        st.quiet = 0;
        st.pending_quiet = 0;
    }
}

/// offset부터 끝까지 새 완전한 줄들
fn read_new(path: &Path, st: &mut FState, len: u64) -> Vec<String> {
    if len < st.offset {
        st.offset = 0;
        st.partial.clear();
    }
    if len == st.offset {
        return Vec::new();
    }
    let mut f = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    if f.seek(SeekFrom::Start(st.offset)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    if f.take(len - st.offset).read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    st.offset = len;
    let text = String::from_utf8_lossy(&buf);
    let combined = format!("{}{}", st.partial, text);
    let mut parts: Vec<String> = combined.split('\n').map(|s| s.to_string()).collect();
    st.partial = parts.pop().unwrap_or_default();
    parts.into_iter().filter(|l| !l.trim().is_empty()).collect()
}

/// 파일 끝에서 최대 TAIL_BYTES 를 읽어 완전한 줄들 (첫 부분 잘린 줄은 버림)
fn read_tail(path: &Path, len: u64) -> Vec<String> {
    let start = len.saturating_sub(TAIL_BYTES);
    let mut f = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    if f.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&buf);
    let mut parts: Vec<&str> = text.split('\n').collect();
    if start > 0 && !parts.is_empty() {
        parts.remove(0); // 잘린 첫 줄 버림
    }
    parts
        .into_iter()
        .filter(|l| !l.trim().is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn read_head(path: &Path, max: usize) -> String {
    let mut f = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return String::new(),
    };
    let mut buf = vec![0u8; max];
    let n = f.read(&mut buf).unwrap_or(0);
    String::from_utf8_lossy(&buf[..n]).to_string()
}

fn load_titles(head: &str, is_codex: bool, st: &mut FState) {
    for line in head.split('\n') {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(line.trim_start_matches('\u{feff}')) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if is_codex {
            if st.codex_title.is_empty() {
                if let Some(t) = codex_user_text(&v) {
                    if !t.is_empty() {
                        st.codex_title = t;
                    }
                }
            }
        } else {
            match v["type"].as_str() {
                Some("custom-title") => {
                    if let Some(t) = v["customTitle"].as_str() {
                        st.custom_title = t.to_string();
                    }
                }
                Some("ai-title") => {
                    if let Some(t) = v["aiTitle"].as_str() {
                        st.ai_title = t.to_string();
                    }
                }
                Some("user") => {
                    let t = user_text(&v["message"]["content"]);
                    if !t.is_empty() && st.last_user.is_empty() {
                        st.last_user = t;
                    }
                }
                _ => {}
            }
        }
    }
}

fn process_claude_line(st: &mut FState, v: &Value) {
    match v["type"].as_str() {
        Some("custom-title") => {
            if let Some(t) = v["customTitle"].as_str() {
                st.custom_title = t.to_string();
            }
        }
        Some("ai-title") => {
            if let Some(t) = v["aiTitle"].as_str() {
                st.ai_title = t.to_string();
            }
        }
        Some("user") => {
            let t = user_text(&v["message"]["content"]);
            if !t.is_empty() {
                st.last_user = t;
            }
            st.tail_candidate = false;
            st.pending_tool = false; // tool_result 도착 = 승인/실행됨
        }
        Some("assistant") => {
            let (text, has_tool, tool_desc) = assistant_content(&v["message"]["content"]);
            if !text.is_empty() {
                st.last_assistant = text.clone();
            }
            let ts = v["timestamp"].as_str().unwrap_or("").to_string();
            let marker = v["uuid"]
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| ts.clone());
            st.tail_candidate = !text.is_empty() && !has_tool;
            st.cand_ts = ts.clone();
            st.cand_marker = marker.clone();
            // 마지막이 도구호출이면 승인 대기 후보
            st.pending_tool = has_tool;
            if has_tool {
                st.pending_marker = marker;
                st.pending_ts = ts;
                st.pending_detail = tool_desc;
            }
        }
        _ => {}
    }
}

fn process_codex_line(app: &AppHandle, st: &mut FState, v: &Value) {
    // 제목: 첫 사용자 입력에서
    if st.codex_title.is_empty() {
        if let Some(t) = codex_user_text(v) {
            if !t.is_empty() {
                st.codex_title = t;
            }
        }
    }
    // 완료: task_complete 이벤트
    let payload = &v["payload"];
    if payload["type"].as_str() == Some("task_complete") {
        let turn_id = payload["turn_id"].as_str().unwrap_or("").to_string();
        if turn_id.is_empty() || turn_id == st.codex_notified {
            return;
        }
        let ts = v["timestamp"].as_str().unwrap_or("");
        if !after_start(ts) {
            st.codex_notified = turn_id; // 과거 완료는 기록만
            return;
        }
        st.codex_notified = turn_id;
        let msg = payload["last_agent_message"].as_str().unwrap_or("");
        let mt = msg.trim_start();
        // 실제 완료 메시지가 아니면(빈 값 / 승인 판정 JSON 등) 알림 안 함
        if mt.is_empty() || mt.starts_with('{') {
            return;
        }
        // 제목 우선순위: 사용자가 설정한 스레드 제목 > 첫 사용자 입력 > 기본값
        let title = codex_thread_name(&st.codex_id)
            .filter(|t| !is_noise(t))
            .or_else(|| {
                if st.codex_title.is_empty() {
                    None
                } else {
                    Some(st.codex_title.clone())
                }
            })
            .unwrap_or_else(|| "Codex 작업".to_string());
        dispatch_notification(
            app,
            TaskDone {
                source: "codex".into(),
                kind: "completed".into(),
                message: short(&title, 30),
                detail: short(msg, 55),
                hwnd: 0,
            },
        );
    }
}

fn assistant_content(content: &Value) -> (String, bool, String) {
    if let Some(s) = content.as_str() {
        return (s.to_string(), false, String::new());
    }
    let mut text = String::new();
    let mut has_tool = false;
    let mut tool_desc = String::new();
    if let Some(arr) = content.as_array() {
        for p in arr {
            match p["type"].as_str() {
                Some("text") => {
                    if let Some(t) = p["text"].as_str() {
                        if !text.is_empty() {
                            text.push(' ');
                        }
                        text.push_str(t);
                    }
                }
                Some("tool_use") => {
                    has_tool = true;
                    if tool_desc.is_empty() {
                        tool_desc = tool_brief(p);
                    }
                }
                _ => {}
            }
        }
    }
    (text, has_tool, tool_desc)
}

/// tool_use → "도구명: 주요인자" (승인 알림 상세용)
fn tool_brief(p: &Value) -> String {
    let name = p["name"].as_str().unwrap_or("도구");
    let input = &p["input"];
    let arg = input["command"]
        .as_str()
        .or_else(|| input["file_path"].as_str())
        .or_else(|| input["path"].as_str())
        .or_else(|| input["pattern"].as_str())
        .or_else(|| input["url"].as_str())
        .or_else(|| input["description"].as_str())
        .unwrap_or("");
    if arg.is_empty() {
        name.to_string()
    } else {
        format!("{}: {}", name, arg)
    }
}

fn user_text(content: &Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    let mut text = String::new();
    if let Some(arr) = content.as_array() {
        for p in arr {
            if p["type"].as_str() == Some("text") {
                if let Some(t) = p["text"].as_str() {
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(t);
                }
            }
        }
    }
    text
}

/// 주입된 시스템/환경 컨텍스트인지 (제목으로 부적절)
fn is_noise(t: &str) -> bool {
    let s = t.trim_start();
    s.is_empty()
        || s.starts_with('<') // <environment_context>, <user_instructions> 등
        || s.starts_with('{') // JSON(승인 판정 등)
        || s.starts_with("You are ")
        || s.starts_with("The following") // Codex 에이전트/시스템 프롬프트
        || s.contains("environment_context")
        || s.contains("helpful assistant")
        || s.starts_with("[external_agent") // 도구 호출/결과
}

/// Codex 세션 파일명 끝의 UUID(session id) 추출: rollout-<ts>-<uuid>.jsonl
fn session_id_from_path(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let chars: Vec<char> = stem.chars().collect();
    if chars.len() >= 36 {
        chars[chars.len() - 36..].iter().collect()
    } else {
        String::new()
    }
}

/// ~/.codex/session_index.jsonl 에서 세션 id → 사용자가 설정한 스레드 제목 조회
fn codex_thread_name(id: &str) -> Option<String> {
    if id.is_empty() {
        return None;
    }
    let idx = home().join(".codex").join("session_index.jsonl");
    let content = fs::read_to_string(idx).ok()?;
    // 같은 id가 여러 줄일 수 있음(제목을 바꿀 때마다 append됨).
    // 마지막(=가장 최근) 항목의 thread_name을 사용한다.
    let mut latest: Option<String> = None;
    for line in content.lines() {
        if let Ok(v) = serde_json::from_str::<Value>(line.trim_start_matches('\u{feff}')) {
            if v["id"].as_str() == Some(id) {
                if let Some(name) = v["thread_name"].as_str() {
                    let n = name.trim();
                    if !n.is_empty() {
                        latest = Some(n.to_string());
                    }
                }
            }
        }
    }
    latest
}

fn codex_user_text(v: &Value) -> Option<String> {
    let p = &v["payload"];
    let text = match p["type"].as_str() {
        // 형식1: {"type":"user_message","message":"..."}
        Some("user_message") => p["message"].as_str().unwrap_or("").to_string(),
        // 형식2: {"type":"message","role":"user","content":[{"type":"input_text","text":"..."}]}
        Some("message") if p["role"].as_str() == Some("user") => {
            let mut text = String::new();
            if let Some(arr) = p["content"].as_array() {
                for c in arr {
                    match c["type"].as_str() {
                        Some("input_text") | Some("text") => {
                            if let Some(t) = c["text"].as_str() {
                                if !text.is_empty() {
                                    text.push(' ');
                                }
                                text.push_str(t);
                            }
                        }
                        _ => {}
                    }
                }
            } else if let Some(s) = p["content"].as_str() {
                text = s.to_string();
            }
            text
        }
        _ => return None,
    };
    if is_noise(&text) {
        None // 주입 텍스트는 제목으로 쓰지 않고 다음 후보를 찾게 함
    } else {
        Some(text)
    }
}
