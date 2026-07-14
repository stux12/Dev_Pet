mod watcher;

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use sysinfo::{Disks, System};
use tauri::{Emitter, Manager};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Diagnostics::Debug::Beep;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, IsWindow, SetForegroundWindow,
    ShowWindow, SW_RESTORE,
};

/// 알림 소리 음소거 여부
static MUTED: AtomicBool = AtomicBool::new(false);
/// 디스코드 웹훅 URL (설정되면 알림을 디스코드로도 전송)
static DISCORD_WEBHOOK: Mutex<String> = Mutex::new(String::new());

/// 프론트로 보내는 시스템 메트릭
#[derive(Serialize, Clone)]
struct Metrics {
    cpu: f32,
    mem_used: u64,
    mem_total: u64,
    mem_pct: f32,
    disk_pct: f32,
}

/// 작업 완료 / 승인 필요 알림
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct TaskDone {
    #[serde(default = "unknown_source")]
    pub source: String,
    /// "completed" | "approval"
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub detail: String,
    /// 알림 클릭 시 포커스할 창 핸들 (있으면)
    #[serde(default)]
    pub hwnd: i64,
}

fn default_kind() -> String {
    "completed".to_string()
}

fn unknown_source() -> String {
    "unknown".to_string()
}

/// 외부 URL을 기본 브라우저로 열기
#[tauri::command]
fn open_url(url: String) {
    let _ = open::that(url);
}

/// 알림 소리 음소거 설정
#[tauri::command]
fn set_mute(muted: bool) {
    MUTED.store(muted, Ordering::Relaxed);
}

/// 알림에 연결된 창(작업하던 세션)을 앞으로 가져오기.
/// Windows의 포그라운드 잠금을 우회하려고 AttachThreadInput 기법 사용.
#[tauri::command]
fn focus_window(hwnd: i64) {
    if hwnd == 0 {
        return;
    }
    unsafe {
        let h = HWND(hwnd as usize as *mut core::ffi::c_void);
        if !IsWindow(Some(h)).as_bool() {
            return;
        }
        let fg = GetForegroundWindow();
        let fg_thread = GetWindowThreadProcessId(fg, None);
        let cur_thread = GetCurrentThreadId();
        let attach = fg_thread != 0 && fg_thread != cur_thread;
        if attach {
            let _ = AttachThreadInput(cur_thread, fg_thread, true);
        }
        let _ = ShowWindow(h, SW_RESTORE);
        let _ = BringWindowToTop(h);
        let _ = SetForegroundWindow(h);
        if attach {
            let _ = AttachThreadInput(cur_thread, fg_thread, false);
        }
    }
}

/// 알림 소리 재생 (완료: 상승음 / 승인: 주의음). 별도 스레드에서 재생해 응답 지연 방지.
fn play_sound(kind: &str) {
    if MUTED.load(Ordering::Relaxed) {
        return;
    }
    let approval = kind == "approval";
    std::thread::spawn(move || unsafe {
        if approval {
            let _ = Beep(880, 120);
            let _ = Beep(660, 160);
        } else {
            let _ = Beep(660, 110);
            let _ = Beep(880, 150);
        }
    });
}

/// 문자열을 공백 정리 후 n자로 자름 (watcher와 공용)
pub(crate) fn short(s: &str, n: usize) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = joined.chars().collect();
    if chars.len() > n {
        format!("{}…", chars[..n].iter().collect::<String>())
    } else {
        joined
    }
}

/// 알림 발송: 소리 재생 + 프론트로 emit + 창 표시 + 디스코드 전송 (HTTP 서버와 watcher가 공용)
pub(crate) fn dispatch_notification(app: &tauri::AppHandle, done: TaskDone) {
    play_sound(&done.kind);
    send_discord(&done);
    let _ = app.emit("task-done", done);
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_always_on_top(true);
    }
}

/// 디스코드 웹훅 URL 설정 (프론트에서 저장값 동기화)
#[tauri::command]
fn set_discord_webhook(url: String) {
    if let Ok(mut w) = DISCORD_WEBHOOK.lock() {
        *w = url.trim().to_string();
    }
}

/// 알림 임베드 본문 생성
fn discord_body(done: &TaskDone) -> String {
    let icon = match done.source.as_str() {
        "claude" => "🟠",
        "codex" => "🟢",
        _ => "🔔",
    };
    let (label, color) = if done.kind == "approval" {
        ("승인 필요 🔔", 16_234_325) // 주황
    } else {
        ("작업 완료 ✅", 4_113_588) // 청록
    };
    let title = format!(
        "{} {} · {}",
        icon,
        if done.message.is_empty() {
            "작업"
        } else {
            &done.message
        },
        label
    );
    serde_json::json!({
        "username": "DevPet 🐾",
        "embeds": [{ "title": title, "description": done.detail, "color": color }]
    })
    .to_string()
}

/// 알림을 디스코드 웹훅으로 전송 (별도 스레드, 실패 무시)
fn send_discord(done: &TaskDone) {
    let url = match DISCORD_WEBHOOK.lock() {
        Ok(w) => w.clone(),
        Err(_) => return,
    };
    if !url.starts_with("http") {
        return;
    }
    let body = discord_body(done);
    std::thread::spawn(move || {
        let _ = ureq::post(&url)
            .set("Content-Type", "application/json")
            .timeout(Duration::from_secs(6))
            .send_string(&body);
    });
}

/// 디스코드 연동 테스트 전송 (결과 문자열 반환)
#[tauri::command]
fn test_discord() -> String {
    let url = match DISCORD_WEBHOOK.lock() {
        Ok(w) => w.clone(),
        Err(_) => return "오류".into(),
    };
    if !url.starts_with("http") {
        return "URL을 먼저 입력하세요".into();
    }
    let body = serde_json::json!({
        "username": "DevPet 🐾",
        "content": "✅ DevPet 디스코드 연동 테스트! 앞으로 여기로 알림이 옵니다."
    })
    .to_string();
    match ureq::post(&url)
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(8))
        .send_string(&body)
    {
        Ok(_) => "전송 성공".into(),
        Err(e) => format!("실패: {}", e),
    }
}

fn disk_usage_pct() -> f32 {
    let disks = Disks::new_with_refreshed_list();
    let mut total: u64 = 0;
    let mut available: u64 = 0;
    for disk in disks.list() {
        total += disk.total_space();
        available += disk.available_space();
    }
    if total == 0 {
        return 0.0;
    }
    let used = total.saturating_sub(available);
    (used as f64 / total as f64 * 100.0) as f32
}

/// 1초마다 CPU/메모리/디스크를 측정해서 프론트로 emit
fn spawn_metrics_loop(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let mut sys = System::new_all();
        loop {
            // CPU 사용률은 두 번의 갱신 사이 간격이 필요함
            sys.refresh_cpu_all();
            std::thread::sleep(Duration::from_millis(1000));
            sys.refresh_cpu_all();
            sys.refresh_memory();

            let cpu = sys.global_cpu_usage();
            let mem_total = sys.total_memory();
            let mem_used = sys.used_memory();
            let mem_pct = if mem_total > 0 {
                (mem_used as f64 / mem_total as f64 * 100.0) as f32
            } else {
                0.0
            };
            let disk_pct = disk_usage_pct();

            let _ = app.emit(
                "metrics",
                Metrics {
                    cpu,
                    mem_used,
                    mem_total,
                    mem_pct,
                    disk_pct,
                },
            );
        }
    });
}

/// 로컬 HTTP 서버: Claude Code / Codex 훅이 작업 완료를 알려주는 엔드포인트
/// POST http://127.0.0.1:37651/notify   body: {"source","message","detail"}
fn spawn_notify_server(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let server = match tiny_http::Server::http("127.0.0.1:37651") {
            Ok(s) => s,
            Err(e) => {
                eprintln!("notify 서버 시작 실패: {e}");
                return;
            }
        };
        for mut req in server.incoming_requests() {
            let mut body = String::new();
            let _ = req.as_reader().read_to_string(&mut body);

            let done: TaskDone = serde_json::from_str(&body).unwrap_or(TaskDone {
                source: "unknown".to_string(),
                kind: "completed".to_string(),
                message: if body.trim().is_empty() {
                    "작업 완료".to_string()
                } else {
                    body.clone()
                },
                detail: String::new(),
                hwnd: 0,
            });

            dispatch_notification(&app, done);

            let resp = tiny_http::Response::from_string("ok");
            let _ = req.respond(resp);
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // 단일 인스턴스: 앱은 항상 1개만. 이미 실행 중이면 두 번째 실행은 기존 창만 보여주고 종료.
        // (가장 먼저 등록되어야 함)
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
            }
        }))
        .setup(|app| {
            let handle = app.handle().clone();
            spawn_metrics_loop(handle.clone());
            spawn_notify_server(handle.clone());
            // Claude Code(CLI+데스크탑) / Codex 대화 기록을 감시해 완료 감지 (훅 불필요)
            watcher::spawn(handle);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            open_url,
            set_mute,
            focus_window,
            set_discord_webhook,
            test_discord
        ])
        .run(tauri::generate_context!())
        .expect("DevPet 실행 중 오류");
}
