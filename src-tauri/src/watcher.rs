// 대화 기록 파일 감시: Claude Code(CLI+데스크탑) / Codex 의 완료를 훅 없이 감지
//   - Claude:  ~/.claude/projects/*/*.jsonl  (마지막이 '도구호출 없는 assistant 텍스트'면 턴 완료)
//   - Codex:   ~/.codex/sessions/**/*.jsonl  (payload.type == "task_complete")
// 앱 시작 이후 타임스탬프의 완료만 알림(과거 완료 무시). 파일은 증분으로만 읽음.
use crate::{dispatch_notification, short, TaskDone};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Emitter};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

const POLL_MS: u64 = 1200;
const RECENT_SECS: u64 = 60; // 최근 이만큼 수정된 파일만 처리
const CLAUDE_QUIET: u32 = 1; // 완료 후보가 이만큼 안정되면 알림(스트리밍 오탐 방지)
const TAIL_BYTES: u64 = 512 * 1024; // 최초 목격 시 훑을 꼬리 크기
/// 도구 호출 후 이만큼(13 × 1.2s ≈ 15.6초) 결과 없이 조용하면 '승인 대기'로 추정.
/// **데스크탑 앱 세션에만 적용**한다(아래 설명 참고).
const APPROVAL_QUIET: u32 = 13;

// 승인 대기 감지가 entrypoint 별로 다른 이유:
//   - cli: 승인 프롬프트가 떠 있는 동안 transcript 에 tool_use 를 아직 쓰지 않는다
//          (승인 후에야 기록) → 파일 감시로는 감지 불가 → Notification 훅으로 처리(lib.rs).
//   - claude-desktop: 승인 프롬프트 중에도 tool_use 가 기록된다. 대신 데스크탑 앱은
//          훅을 실행하지 않으므로, 파일 감시로 추정하는 수밖에 없다.
// CLI 세션에 추정을 적용하면 자동 실행되는 긴 명령(빌드 등)을 승인 대기로 오탐하므로
// 데스크탑 세션에만 켠다.

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
    /// 마지막 '실제 사용자 프롬프트' 시각 = 턴 시작 (소요 시간 계산용).
    /// tool_result 는 텍스트가 없어 자연히 걸러진다.
    turn_start_ts: String,
    /// entrypoint == "claude-desktop" (데스크탑 앱 세션). 승인 추정은 여기서만 한다.
    is_desktop: bool,
    /// 세션 cwd 기준으로 로드한 permissions.allow 규칙(자동승인 명령 제외용). 세션당 1회 로드.
    allow_rules: Vec<String>,
    allow_loaded: bool,
    /// 이번 턴에 쓴 토큰 합. 사용자 프롬프트에서 리셋.
    turn_tokens: u64,
    /// 이번 턴에 이미 집계한 requestId. 한 응답이 thinking/text/tool_use 여러 줄로
    /// 쪼개지면서 usage 가 그대로 복제되므로, 줄마다 더하면 중복 합산된다.
    turn_reqs: HashSet<String>,
    // 승인 대기 추정 (데스크탑 전용): 마지막이 '권한 필요' 도구호출이고 결과 없이 조용할 때
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
    codex_turn_start_ts: String,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into()))
}

/// usage → 이번 응답에서 '새로 처리한' 토큰 (입력 + 캐시 생성 + 출력).
/// 캐시 읽기(cache_read_input_tokens)는 컨텍스트 재사용분이라 제외한다 — 입력의
/// 대부분(수십만)을 차지해서 포함하면 정작 이 작업이 얼마나 무거웠는지 가늠이 안 된다.
fn usage_tokens(u: &Value) -> u64 {
    let g = |k: &str| u[k].as_u64().unwrap_or(0);
    g("input_tokens") + g("cache_creation_input_tokens") + g("output_tokens")
}

/// 두 RFC3339 타임스탬프 사이 초. 파싱 불가하거나 음수면 0(=표시 안 함).
fn elapsed_secs(start: &str, end: &str) -> u64 {
    let s = match OffsetDateTime::parse(start, &Rfc3339) {
        Ok(t) => t,
        Err(_) => return 0,
    };
    let e = match OffsetDateTime::parse(end, &Rfc3339) {
        Ok(t) => t,
        Err(_) => return 0,
    };
    let secs = (e - s).whole_seconds();
    if secs < 0 {
        0
    } else {
        secs as u64
    }
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
        let mut first_pass = true;
        loop {
            let mut files: Vec<(PathBuf, bool)> = Vec::new();
            collect_recent(&claude_root, false, &mut files, 0);
            collect_recent(&codex_root, true, &mut files, 0);
            for (path, is_codex) in files {
                let st = states.entry(path.clone()).or_default();
                process(&app, &path, is_codex, st);
            }
            // 최초 스캔 완료 → 프론트에 알림(로딩 화면 종료용). 어떤 도구 기록을 찾았는지도 전달.
            if first_pass {
                first_pass = false;
                let _ = app.emit(
                    "scan-ready",
                    serde_json::json!({
                        "claude": claude_root.is_dir(),
                        "codex": codex_root.is_dir(),
                    }),
                );
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
                    // detail 은 이제 알림에 표시하지 않으므로 짧게만 넘긴다(로컬 엔드포인트
                    // 수동 테스트 등 호환용).
                    detail: short(&st.last_assistant, 55),
                    hwnd: 0,
                    elapsed_secs: elapsed_secs(&st.turn_start_ts, &st.cand_ts),
                    tokens: st.turn_tokens,
                },
            );
            st.notified = st.cand_marker.clone();
            st.quiet = 0;
        }
    } else if st.pending_tool
        && st.pending_marker != st.approval_notified
        && after_start(&st.pending_ts)
    {
        // 승인 대기 추정(데스크탑 전용): 권한 필요 도구 호출 후 결과 없이 조용하면
        st.pending_quiet += 1;
        st.quiet = 0;
        if st.pending_quiet >= APPROVAL_QUIET {
            dispatch_notification(
                app,
                TaskDone {
                    source: "claude".into(),
                    kind: "approval".into(),
                    message: short(&title, 30),
                    // 승인 detail = "도구: 명령/경로"(tool_brief). 디스코드에 표시되므로 넉넉히.
                    detail: short(&st.pending_detail, 300),
                    hwnd: 0,
                    elapsed_secs: 0,
                    tokens: 0, // 승인 대기 시점엔 아직 집계할 게 없다
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
    // 세션 출처: "cli" | "claude-desktop". 같은 세션을 양쪽에서 열 수 있어 최신 줄 기준으로 갱신.
    if let Some(ep) = v["entrypoint"].as_str() {
        st.is_desktop = ep == "claude-desktop";
    }
    // 세션 cwd 를 처음 알게 되면 그 프로젝트의 allow 규칙을 로드(자동승인 명령 제외용).
    if !st.allow_loaded {
        if let Some(cwd) = v["cwd"].as_str() {
            st.allow_rules = load_allow_rules(cwd);
            st.allow_loaded = true;
        }
    }
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
                // 텍스트가 있는 user 줄 = 실제 사용자 프롬프트(tool_result 아님) → 턴 시작
                st.turn_start_ts = v["timestamp"].as_str().unwrap_or("").to_string();
                st.turn_tokens = 0;
                st.turn_reqs.clear();
            }
            st.tail_candidate = false;
            st.pending_tool = false; // tool_result 도착 = 승인되어 실행됨
        }
        Some("assistant") => {
            // 토큰 집계: 같은 requestId 는 한 번만(usage 가 줄마다 복제되므로).
            // requestId 가 없는 기록은 그대로 합산한다.
            let req = v["requestId"].as_str().unwrap_or("");
            if req.is_empty() || st.turn_reqs.insert(req.to_string()) {
                st.turn_tokens += usage_tokens(&v["message"]["usage"]);
            }
            let (text, has_tool, tool_desc, has_sensitive) =
                assistant_content(&v["message"]["content"]);
            if !text.is_empty() {
                st.last_assistant = text.clone();
            }
            let ts = v["timestamp"].as_str().unwrap_or("").to_string();
            let marker = v["uuid"]
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| ts.clone());
            // 완료 후보 판정: 가능하면 stop_reason 사용(가장 견고).
            //   CLI 는 thinking/text/tool_use 를 각각 별도 줄로 기록하므로, 도구 호출
            //   직전의 중간 설명 텍스트가 "text 있고 도구 없음"으로 남아 완료로 오탐된다.
            //   같은 응답의 각 줄에는 동일한 stop_reason 이 복제되므로 이를 기준 삼는다:
            //     end_turn = 턴 종료(완료) / tool_use = 도구로 계속.
            //   stop_reason 이 없는 구형 기록은 기존 휴리스틱으로 폴백.
            let stop = v["message"]["stop_reason"].as_str().unwrap_or("");
            st.tail_candidate = if stop.is_empty() {
                !text.is_empty() && !has_tool
            } else {
                stop == "end_turn" && !text.is_empty()
            };
            st.cand_ts = ts.clone();
            st.cand_marker = marker.clone();
            // 승인 대기 후보: 데스크탑 세션에서 마지막이 '권한 필요' 도구호출일 때만.
            // (CLI 는 훅이 정확히 잡으므로 추정하지 않는다 — 긴 자동 실행 오탐 방지)
            st.pending_tool = st.is_desktop && has_tool && has_sensitive;
            if st.pending_tool {
                // 자동승인(allow 매칭)되는 도구 호출은 승인 프롬프트가 뜨지 않으므로 제외.
                // 대표적으로 `Bash(npm run *)` 같은 규칙에 걸리는 빌드·설치 명령의 오탐을 막는다.
                if let Some((tname, targ)) = first_tool_arg(&v["message"]["content"]) {
                    if is_auto_approved(&st.allow_rules, &tname, &targ) {
                        st.pending_tool = false;
                    }
                }
            }
            if st.pending_tool {
                st.pending_marker = marker;
                st.pending_ts = ts;
                st.pending_detail = tool_desc;
            }
        }
        _ => {}
    }
}

fn process_codex_line(app: &AppHandle, st: &mut FState, v: &Value) {
    if let Some(t) = codex_user_text(v) {
        if !t.is_empty() {
            // 제목: 첫 사용자 입력에서
            if st.codex_title.is_empty() {
                st.codex_title = t;
            }
            // 턴 시작: 가장 최근 사용자 입력 (소요 시간 계산용)
            st.codex_turn_start_ts = v["timestamp"].as_str().unwrap_or("").to_string();
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
                elapsed_secs: elapsed_secs(&st.codex_turn_start_ts, ts),
                tokens: 0, // Codex 기록엔 usage 가 없다
            },
        );
    }
}

/// 승인(권한 확인)이 흔히 필요한 도구인지. 읽기 전용 도구는 제외해 오탐을 줄인다.
/// (Bash 등 오래 걸리는 도구의 오탐은 permissions.allow 매칭으로 별도 제거 — is_auto_approved)
fn is_approval_tool(name: &str) -> bool {
    const APPROVAL_TOOLS: &[&str] = &[
        "Bash",
        "PowerShell",
        "Write",
        "Edit",
        "MultiEdit",
        "NotebookEdit",
        "WebFetch",
    ];
    APPROVAL_TOOLS.iter().any(|t| t.eq_ignore_ascii_case(name))
}

/// permissions.allow 규칙 하나가 (도구, 명령/인자)에 매칭되는지 (근사).
/// 규칙 형식: `Tool(pattern)` 또는 `Tool`. pattern 의 `*` 는 임의 문자열(앵커됨).
/// 예: `Bash(npm run *)` → 도구=Bash 이고 명령이 "npm run " 으로 시작하면 매칭.
fn rule_matches(rule: &str, tool: &str, arg: &str) -> bool {
    let (rtool, rpat) = match rule.find('(') {
        Some(open) if rule.ends_with(')') => (&rule[..open], Some(&rule[open + 1..rule.len() - 1])),
        _ => (rule, None),
    };
    if !rtool.eq_ignore_ascii_case(tool) {
        return false;
    }
    match rpat {
        None => true, // `Bash` → 모든 Bash 허용
        Some(pat) => glob_match(pat, arg),
    }
}

/// `*` 와일드카드 glob 매칭(앵커). `*` 없으면 정확 일치.
fn glob_match(pat: &str, s: &str) -> bool {
    let parts: Vec<&str> = pat.split('*').collect();
    if parts.len() == 1 {
        return pat == s;
    }
    // 첫 조각 = prefix
    if !s.starts_with(parts[0]) {
        return false;
    }
    let mut pos = parts[0].len();
    for (i, part) in parts.iter().enumerate().skip(1) {
        if i == parts.len() - 1 {
            // 마지막 조각 = suffix (빈 문자열이면 항상 참)
            return s[pos..].ends_with(part);
        }
        match s[pos..].find(part) {
            Some(idx) => pos += idx + part.len(),
            None => return false,
        }
    }
    true
}

/// 이 도구 호출이 자동승인(allow 규칙 매칭)되는지 → 그렇다면 승인 프롬프트가 뜨지 않으므로
/// 승인 대기 추정에서 제외한다.
fn is_auto_approved(rules: &[String], tool: &str, arg: &str) -> bool {
    rules.iter().any(|r| rule_matches(r, tool, arg))
}

/// content 배열에서 첫 tool_use 의 (도구명, 주요 인자) 추출 (allow 매칭용).
fn first_tool_arg(content: &Value) -> Option<(String, String)> {
    for p in content.as_array()? {
        if p["type"].as_str() == Some("tool_use") {
            let name = p["name"].as_str().unwrap_or("").to_string();
            let input = &p["input"];
            let arg = input["command"]
                .as_str()
                .or_else(|| input["file_path"].as_str())
                .or_else(|| input["path"].as_str())
                .or_else(|| input["url"].as_str())
                .unwrap_or("")
                .to_string();
            return Some((name, arg));
        }
    }
    None
}

/// 세션 cwd 기준으로 permissions.allow 규칙을 모은다(글로벌 + 프로젝트, settings + local).
fn load_allow_rules(cwd: &str) -> Vec<String> {
    let home = home();
    let paths = [
        home.join(".claude").join("settings.json"),
        home.join(".claude").join("settings.local.json"),
        Path::new(cwd).join(".claude").join("settings.json"),
        Path::new(cwd).join(".claude").join("settings.local.json"),
    ];
    let mut rules = Vec::new();
    for p in paths {
        if let Ok(s) = fs::read_to_string(&p) {
            if let Ok(v) = serde_json::from_str::<Value>(s.trim_start_matches('\u{feff}')) {
                if let Some(arr) = v["permissions"]["allow"].as_array() {
                    for r in arr {
                        if let Some(rs) = r.as_str() {
                            rules.push(rs.to_string());
                        }
                    }
                }
            }
        }
    }
    rules
}

/// tool_use → "도구명: 주요인자" (승인 알림 상세용)
fn tool_brief(p: &Value) -> String {
    let name = p["name"].as_str().unwrap_or("도구");
    let input = &p["input"];
    let arg = input["command"]
        .as_str()
        .or_else(|| input["file_path"].as_str())
        .or_else(|| input["path"].as_str())
        .or_else(|| input["url"].as_str())
        .or_else(|| input["description"].as_str())
        .unwrap_or("");
    if arg.is_empty() {
        name.to_string()
    } else {
        format!("{}: {}", name, arg)
    }
}

/// content → (텍스트, 도구호출 있음, 첫 도구 요약, 권한필요 도구 포함)
fn assistant_content(content: &Value) -> (String, bool, String, bool) {
    if let Some(s) = content.as_str() {
        return (s.to_string(), false, String::new(), false);
    }
    let mut text = String::new();
    let mut has_tool = false;
    let mut has_sensitive = false;
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
                    if is_approval_tool(p["name"].as_str().unwrap_or("")) {
                        has_sensitive = true;
                    }
                    if tool_desc.is_empty() {
                        tool_desc = tool_brief(p);
                    }
                }
                _ => {}
            }
        }
    }
    (text, has_tool, tool_desc, has_sensitive)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn v(json: &str) -> Value {
        serde_json::from_str(json).expect("test fixture must be valid json")
    }

    /// CLI 는 thinking/text/tool_use 를 각각 별도 줄로 기록한다. 도구 호출 직전의
    /// 중간 설명 텍스트는 "text 있고 도구 없음"이라 stop_reason 없이는 완료로 오탐된다.
    /// (v0.1.9 에서 실제로 발생했던 버그)
    #[test]
    fn cli_intermediate_text_before_tool_is_not_complete() {
        let mut st = FState::default();
        process_claude_line(
            &mut st,
            &v(r#"{"type":"assistant","uuid":"u1","timestamp":"2026-07-15T08:00:00.000Z",
                 "message":{"stop_reason":"tool_use","content":[{"type":"text","text":"확인해볼게요"}]}}"#),
        );
        assert!(!st.tail_candidate, "stop_reason=tool_use 는 완료가 아니다");
    }

    #[test]
    fn end_turn_with_text_is_complete() {
        let mut st = FState::default();
        process_claude_line(
            &mut st,
            &v(r#"{"type":"assistant","uuid":"u2","timestamp":"2026-07-15T08:00:01.000Z",
                 "message":{"stop_reason":"end_turn","content":[{"type":"text","text":"다 됐어요"}]}}"#),
        );
        assert!(st.tail_candidate);
        assert_eq!(st.cand_marker, "u2");
        assert_eq!(st.last_assistant, "다 됐어요");
    }

    /// 완료 응답이 thinking/text 두 줄로 쪼개질 때, 두 줄 모두 stop_reason=end_turn 을
    /// 갖는다. text 가 없는 thinking 줄에서 성급히 알리면 내용 없는 알림이 뜬다.
    #[test]
    fn end_turn_thinking_without_text_is_not_complete() {
        let mut st = FState::default();
        process_claude_line(
            &mut st,
            &v(r#"{"type":"assistant","uuid":"u3","timestamp":"2026-07-15T08:00:02.000Z",
                 "message":{"stop_reason":"end_turn","content":[{"type":"thinking","thinking":"음..."}]}}"#),
        );
        assert!(!st.tail_candidate, "text 없는 end_turn 은 완료 후보가 아니다");
    }

    #[test]
    fn tool_use_line_is_not_complete() {
        let mut st = FState::default();
        process_claude_line(
            &mut st,
            &v(r#"{"type":"assistant","uuid":"u4","timestamp":"2026-07-15T08:00:03.000Z",
                 "message":{"stop_reason":"tool_use","content":[{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#),
        );
        assert!(!st.tail_candidate);
    }

    /// stop_reason 이 없는 구형 기록은 "text 있고 도구 없음" 휴리스틱으로 폴백한다.
    #[test]
    fn legacy_without_stop_reason_uses_heuristic() {
        let mut st = FState::default();
        process_claude_line(
            &mut st,
            &v(r#"{"type":"assistant","uuid":"u5","timestamp":"2026-07-15T08:00:04.000Z",
                 "message":{"content":[{"type":"text","text":"끝"}]}}"#),
        );
        assert!(st.tail_candidate, "구형: text 만 있으면 완료");

        let mut st2 = FState::default();
        process_claude_line(
            &mut st2,
            &v(r#"{"type":"assistant","uuid":"u6","timestamp":"2026-07-15T08:00:05.000Z",
                 "message":{"content":[{"type":"text","text":"실행할게요"},{"type":"tool_use","name":"Bash","input":{}}]}}"#),
        );
        assert!(!st2.tail_candidate, "구형: text+도구 가 한 줄이면 완료 아님");
    }

    /// tool_result(user 줄)가 도착하면 이전 완료 후보는 무효가 된다.
    #[test]
    fn user_line_clears_candidate() {
        let mut st = FState::default();
        process_claude_line(
            &mut st,
            &v(r#"{"type":"assistant","uuid":"u7","timestamp":"2026-07-15T08:00:06.000Z",
                 "message":{"stop_reason":"end_turn","content":[{"type":"text","text":"완료"}]}}"#),
        );
        assert!(st.tail_candidate);
        process_claude_line(
            &mut st,
            &v(r#"{"type":"user","timestamp":"2026-07-15T08:00:07.000Z",
                 "message":{"content":[{"type":"tool_result","content":"ok"}]}}"#),
        );
        assert!(!st.tail_candidate);
    }

    #[test]
    fn titles_are_tracked() {
        let mut st = FState::default();
        process_claude_line(&mut st, &v(r#"{"type":"custom-title","customTitle":"내 작업"}"#));
        process_claude_line(&mut st, &v(r#"{"type":"ai-title","aiTitle":"AI 제목"}"#));
        assert_eq!(st.custom_title, "내 작업");
        assert_eq!(st.ai_title, "AI 제목");
    }

    #[test]
    fn assistant_content_extracts_text_and_tool_flag() {
        // Read 는 읽기 전용이라 권한 필요 도구가 아니다
        let (text, has_tool, _desc, sensitive) = assistant_content(&v(
            r#"[{"type":"text","text":"a"},{"type":"tool_use","name":"Read","input":{}}]"#,
        ));
        assert_eq!(text, "a");
        assert!(has_tool);
        assert!(!sensitive);

        let (text2, has_tool2, _, _) = assistant_content(&v(r#""그냥 문자열""#));
        assert_eq!(text2, "그냥 문자열");
        assert!(!has_tool2);

        // Bash/PowerShell 은 권한 필요
        let (_, _, desc, sensitive2) = assistant_content(&v(
            r#"[{"type":"tool_use","name":"PowerShell","input":{"command":"ls"}}]"#,
        ));
        assert!(sensitive2, "PowerShell 은 승인 대상(윈도우 CLI 셸 도구)");
        assert_eq!(desc, "PowerShell: ls");
    }

    /// 승인 추정은 데스크탑 세션에서만. CLI 는 훅이 정확히 잡으므로 추정하면
    /// 자동 실행되는 긴 명령(빌드 등)을 승인 대기로 오탐한다.
    #[test]
    fn approval_pending_only_for_desktop_sessions() {
        let cli_tool = r#"{"type":"assistant","uuid":"u1","timestamp":"2026-07-15T08:00:00.000Z",
             "entrypoint":"cli",
             "message":{"stop_reason":"tool_use","content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo build"}}]}}"#;
        let mut st = FState::default();
        process_claude_line(&mut st, &v(cli_tool));
        assert!(!st.is_desktop);
        assert!(!st.pending_tool, "CLI 세션은 승인 추정을 하지 않는다");

        let desktop_tool = r#"{"type":"assistant","uuid":"u2","timestamp":"2026-07-15T08:00:00.000Z",
             "entrypoint":"claude-desktop",
             "message":{"stop_reason":"tool_use","content":[{"type":"tool_use","name":"Bash","input":{"command":"rm x"}}]}}"#;
        let mut st2 = FState::default();
        process_claude_line(&mut st2, &v(desktop_tool));
        assert!(st2.is_desktop);
        assert!(st2.pending_tool, "데스크탑 세션은 승인 추정 대상");
        assert_eq!(st2.pending_marker, "u2");
        assert_eq!(st2.pending_detail, "Bash: rm x");
    }

    #[test]
    fn glob_match_basics() {
        assert!(glob_match("npm run *", "npm run tauri build"));
        assert!(!glob_match("npm run *", "cargo build"));
        assert!(glob_match("git push *", "git push origin main"));
        assert!(glob_match("git commit *", "git commit -m 'hello'"));
        assert!(glob_match("taskkill", "taskkill")); // * 없음 = 정확 일치
        assert!(!glob_match("taskkill", "taskkill foo"));
        assert!(glob_match("*.txt", "a/b/c.txt")); // suffix
        assert!(glob_match("*", "무엇이든"));
    }

    #[test]
    fn rule_matches_tool_and_pattern() {
        assert!(rule_matches("Bash(npm run *)", "Bash", "npm run x"));
        assert!(!rule_matches("Bash(npm run *)", "Write", "npm run x")); // 도구 불일치
        assert!(!rule_matches("Bash(npm run *)", "Bash", "cargo build")); // 패턴 불일치
        assert!(rule_matches("Bash", "Bash", "무엇이든")); // 패턴 없음 = 전체 허용
    }

    /// 자동승인(allow 매칭)되는 명령은 승인 프롬프트가 안 뜨므로 대기로 보지 않는다.
    #[test]
    fn auto_approved_command_is_not_pending() {
        let tool = |cmd: &str| {
            format!(
                r#"{{"type":"assistant","uuid":"u","timestamp":"2026-07-16T08:00:00.000Z",
                   "entrypoint":"claude-desktop",
                   "message":{{"stop_reason":"tool_use","content":[{{"type":"tool_use","name":"Bash","input":{{"command":"{}"}}}}]}}}}"#,
                cmd
            )
        };
        let mut st = FState::default();
        st.allow_rules = vec!["Bash(npm run *)".into()];
        st.allow_loaded = true;
        process_claude_line(&mut st, &v(&tool("npm run tauri build")));
        assert!(!st.pending_tool, "allow 매칭된 빌드 명령은 승인 후보가 아니다");

        let mut st2 = FState::default();
        st2.allow_rules = vec!["Bash(npm run *)".into()];
        st2.allow_loaded = true;
        process_claude_line(&mut st2, &v(&tool("rm -rf /important")));
        assert!(st2.pending_tool, "allow 안 된 명령은 승인 후보로 남는다");
    }

    /// 읽기 전용 도구는 데스크탑에서도 승인 대기로 보지 않는다.
    #[test]
    fn readonly_tool_is_not_pending_even_on_desktop() {
        let mut st = FState::default();
        process_claude_line(
            &mut st,
            &v(r#"{"type":"assistant","uuid":"u3","timestamp":"2026-07-15T08:00:00.000Z",
                 "entrypoint":"claude-desktop",
                 "message":{"stop_reason":"tool_use","content":[{"type":"tool_use","name":"Read","input":{"file_path":"a.txt"}}]}}"#),
        );
        assert!(!st.pending_tool);
    }

    /// tool_result 가 도착하면 승인 대기 상태는 해제된다(승인되어 실행됨).
    #[test]
    fn tool_result_clears_pending() {
        let mut st = FState::default();
        process_claude_line(
            &mut st,
            &v(r#"{"type":"assistant","uuid":"u4","timestamp":"2026-07-15T08:00:00.000Z",
                 "entrypoint":"claude-desktop",
                 "message":{"stop_reason":"tool_use","content":[{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#),
        );
        assert!(st.pending_tool);
        process_claude_line(
            &mut st,
            &v(r#"{"type":"user","timestamp":"2026-07-15T08:00:05.000Z",
                 "message":{"content":[{"type":"tool_result","content":"ok"}]}}"#),
        );
        assert!(!st.pending_tool);
    }

    /// 소요 시간은 '실제 사용자 프롬프트'부터 잰다. 도중의 tool_result 는 턴 시작이 아니다.
    #[test]
    fn elapsed_is_measured_from_user_prompt_not_tool_result() {
        let mut st = FState::default();
        process_claude_line(
            &mut st,
            &v(r#"{"type":"user","timestamp":"2026-07-15T08:00:00.000Z","message":{"content":"작업해줘"}}"#),
        );
        assert_eq!(st.turn_start_ts, "2026-07-15T08:00:00.000Z");

        process_claude_line(
            &mut st,
            &v(r#"{"type":"user","timestamp":"2026-07-15T08:00:30.000Z",
                 "message":{"content":[{"type":"tool_result","content":"ok"}]}}"#),
        );
        assert_eq!(
            st.turn_start_ts, "2026-07-15T08:00:00.000Z",
            "tool_result 는 턴 시작을 갱신하면 안 된다"
        );

        process_claude_line(
            &mut st,
            &v(r#"{"type":"assistant","uuid":"u1","timestamp":"2026-07-15T08:02:30.000Z",
                 "message":{"stop_reason":"end_turn","content":[{"type":"text","text":"끝"}]}}"#),
        );
        assert_eq!(elapsed_secs(&st.turn_start_ts, &st.cand_ts), 150);
    }

    /// 한 응답이 thinking/text/tool_use 여러 줄로 쪼개지면 usage 가 그대로 복제된다.
    /// requestId 로 dedup 하지 않으면 토큰이 2~3배로 부풀려진다.
    #[test]
    fn tokens_are_deduped_by_request_id() {
        let mut st = FState::default();
        process_claude_line(
            &mut st,
            &v(r#"{"type":"user","timestamp":"2026-07-16T08:00:00.000Z","message":{"content":"해줘"}}"#),
        );
        // 같은 requestId 를 가진 3줄 (usage 복제됨)
        for kind in [
            r#"{"type":"thinking","thinking":"음"}"#,
            r#"{"type":"text","text":"설명"}"#,
            r#"{"type":"tool_use","name":"Read","input":{}}"#,
        ] {
            let line = format!(
                r#"{{"type":"assistant","uuid":"x","requestId":"req_A","timestamp":"2026-07-16T08:00:01.000Z",
                   "message":{{"stop_reason":"tool_use","usage":{{"input_tokens":10,"cache_creation_input_tokens":5,"cache_read_input_tokens":99999,"output_tokens":100}},
                   "content":[{}]}}}}"#,
                kind
            );
            process_claude_line(&mut st, &v(&line));
        }
        assert_eq!(
            st.turn_tokens, 115,
            "같은 requestId 3줄은 한 번만 집계(10+5+100). 캐시 읽기는 제외"
        );

        // 다른 requestId 는 더해진다
        process_claude_line(
            &mut st,
            &v(r#"{"type":"assistant","uuid":"y","requestId":"req_B","timestamp":"2026-07-16T08:00:02.000Z",
                 "message":{"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":9},
                 "content":[{"type":"text","text":"끝"}]}}"#),
        );
        assert_eq!(st.turn_tokens, 125);

        // 새 사용자 프롬프트 = 새 턴 → 리셋
        process_claude_line(
            &mut st,
            &v(r#"{"type":"user","timestamp":"2026-07-16T08:01:00.000Z","message":{"content":"또 해줘"}}"#),
        );
        assert_eq!(st.turn_tokens, 0);
        assert!(st.turn_reqs.is_empty());
    }

    #[test]
    fn elapsed_secs_handles_bad_input() {
        assert_eq!(elapsed_secs("", ""), 0);
        assert_eq!(elapsed_secs("nonsense", "2026-07-15T08:00:00.000Z"), 0);
        assert_eq!(
            elapsed_secs("2026-07-15T08:01:00.000Z", "2026-07-15T08:00:00.000Z"),
            0,
            "역순이면 0"
        );
    }

    /// 주입된 시스템/환경 컨텍스트는 제목으로 쓰지 않는다.
    #[test]
    fn noise_is_rejected_as_title() {
        assert!(is_noise("<environment_context>"));
        assert!(is_noise("{\"approved\":true}"));
        assert!(is_noise("You are a helpful assistant"));
        assert!(is_noise("   "));
        assert!(!is_noise("메모리 확인하고 DevPet 이어서 작업하자"));
    }
}
