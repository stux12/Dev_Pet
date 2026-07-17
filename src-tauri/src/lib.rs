mod watcher;

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use sysinfo::{Disks, System};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager};
use tauri_winrt_notification::{Duration as ToastDuration, Toast};
use windows::core::w;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Diagnostics::Debug::Beep;
use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
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
    /// 사용자 프롬프트 ~ 완료까지 걸린 초. 0 이면 표시하지 않음.
    #[serde(default)]
    pub elapsed_secs: u64,
    /// 이번 작업에 쓴 토큰(입력+캐시생성+출력, 캐시 읽기 제외). 0 이면 표시하지 않음.
    #[serde(default)]
    pub tokens: u64,
}

fn default_kind() -> String {
    "completed".to_string()
}

fn unknown_source() -> String {
    "unknown".to_string()
}

// 부팅 시 자동 실행: HKCU Run 키에 현재 exe 경로를 등록/해제한다.
// (시작프로그램 폴더 .lnk 는 COM 이 필요해서 레지스트리 방식을 쓴다)
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "DevPet";

/// 부팅 시 자동 실행이 켜져 있는지
#[tauri::command]
fn get_autostart() -> bool {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(RUN_KEY)
        .and_then(|k| k.get_value::<String, _>(RUN_VALUE))
        .is_ok()
}

/// 부팅 시 자동 실행 켜기/끄기. 켜면 지금 실행 중인 exe 경로가 등록된다.
#[tauri::command]
fn set_autostart(enabled: bool) -> Result<(), String> {
    let (key, _) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .create_subkey(RUN_KEY)
        .map_err(|e| e.to_string())?;
    if enabled {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        // 경로에 공백이 있어도 되도록 따옴표로 감싼다
        let val = format!("\"{}\"", exe.display());
        key.set_value(RUN_VALUE, &val).map_err(|e| e.to_string())?;
    } else {
        let _ = key.delete_value(RUN_VALUE); // 없으면 그냥 무시
    }
    Ok(())
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

/// 앱을 완전히 종료 (백그라운드 유지가 아니라 프로세스 자체를 끔)
#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    unregister_claude_hook(); // 알림도 중지되므로 훅도 함께 해제(다음 실행 시 재등록)
    app.exit(0);
}

/// 숨겨진 펫 창을 다시 보이게 (트레이 클릭/메뉴에서 호출)
fn show_pet(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// 시스템 트레이 아이콘 구성: 좌클릭=펫 보이기, 우클릭 메뉴=보이기/완전 종료
fn setup_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let show_i = MenuItem::with_id(app, "tray_show", "펫 보이기", true, None::<&str>)?;
    let quit_i = MenuItem::with_id(app, "tray_quit", "완전 종료", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_i, &quit_i])?;

    let mut builder = TrayIconBuilder::with_id("devpet-tray")
        .tooltip("DevPet — 클릭하면 펫이 나타나요")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "tray_show" => show_pet(app),
            "tray_quit" => {
                unregister_claude_hook();
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_pet(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
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

/// 토스트 알림이 'DevPet' 이름으로 뜨도록 AUMID를 등록(HKCU)하고 프로세스에 지정한다.
/// (미등록 AUMID면 `show()`가 에러 없이 조용히 표시 안 됨 → 토스트가 안 뜨는 원인)
fn register_aumid() {
    if let Ok((key, _)) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .create_subkey("Software\\Classes\\AppUserModelId\\com.devpet.app")
    {
        let _ = key.set_value("DisplayName", &"DevPet");
    }
    unsafe {
        let _ = SetCurrentProcessExplicitAppUserModelID(w!("com.devpet.app"));
    }
}

/// Claude Code CLI 승인 대기(permission_prompt) 시 DevPet 로 알림을 보내는 PowerShell 훅.
/// 승인 대기 중엔 transcript 에 tool_use 가 아직 기록되지 않아(승인 후에야 기록됨) 파일
/// 감시로는 감지 불가 → CLI 의 Notification 훅으로 처리한다. 완료 알림은 파일 감시 유지.
const APPROVAL_HOOK_PS1: &str = r#"# DevPet 승인/입력대기 알림 훅 (Claude Code CLI Notification 이벤트) — DevPet 앱이 자동 생성/갱신함
$ErrorActionPreference = "SilentlyContinue"
$raw = [Console]::In.ReadToEnd()
try { $d = $raw | ConvertFrom-Json } catch { exit 0 }
$nt = $d.notification_type
# permission_prompt = 권한 확인 대기 / agent_needs_input = 질문 등 사용자 입력 대기
# (idle_prompt 는 완료 알림과 중복되어 제외)
if ($nt -ne "permission_prompt" -and $nt -ne "agent_needs_input") { exit 0 }
# transcript 에서 제목 추출: custom-title > ai-title > 마지막 사용자 프롬프트
$title = ""
$tp = $d.transcript_path
if ($tp -and (Test-Path $tp)) {
    $custom = ""; $ai = ""; $lastUser = ""
    # 제목 메타(custom-title/ai-title)는 턴마다 기록되므로 파일 꼬리만 보면 충분.
    # 전체 파싱은 큰 세션(수 MB)에서 느리고, Get-Content -Tail 은 대용량에서 매우 느리므로
    # (13MB 기준 6초+) FileStream 으로 끝 512KB 만 직접 읽는다(같은 파일 기준 ~60ms).
    $tail = @()
    try {
        $fs = [System.IO.File]::Open($tp, 'Open', 'Read', 'ReadWrite')
        $flen = $fs.Length
        $take = [Math]::Min($flen, 524288)
        [void]$fs.Seek(-$take, 'End')
        $buf = New-Object byte[] $take
        [void]$fs.Read($buf, 0, $take)
        $fs.Close()
        $parts = [System.Text.Encoding]::UTF8.GetString($buf) -split "`n"
        # 앞이 잘린 첫 줄은 버린다(파일 중간부터 읽었을 때만).
        if ($flen -gt $take -and $parts.Count -gt 1) { $parts = $parts[1..($parts.Count - 1)] }
        $tail = $parts
    } catch { $tail = @() }
    for ($i = $tail.Count - 1; $i -ge 0; $i--) {
        if ($custom -and $ai -and $lastUser) { break }
        $line = $tail[$i]
        if (-not $line) { continue }
        # JSON 파싱 전 문자열 프리필터 — assistant/도구 줄(대다수)은 파싱 없이 건너뛴다.
        if ($line -notmatch '"type":"(custom-title|ai-title|user)"') { continue }
        try { $o = $line | ConvertFrom-Json } catch { continue }
        # 역순이라 먼저 만난 것이 최신 값 → 이미 채워졌으면 건너뛴다.
        switch ($o.type) {
            "custom-title" { if (-not $custom -and $o.customTitle) { $custom = $o.customTitle } }
            "ai-title"     { if (-not $ai -and $o.aiTitle)         { $ai = $o.aiTitle } }
            "user" {
                if (-not $lastUser) {
                    $c = $o.message.content
                    if ($c -is [string]) { if ($c.Trim()) { $lastUser = $c } }
                    elseif ($c) { foreach ($p in $c) { if ($p.type -eq "text" -and $p.text) { $lastUser = $p.text } } }
                }
            }
        }
    }
    if     ($custom)   { $title = $custom }
    elseif ($ai)       { $title = $ai }
    elseif ($lastUser) { $title = $lastUser }
}
if (-not $title) { $title = Split-Path $d.cwd -Leaf }
$title = ($title -replace "\s+", " ").Trim()
if ($title.Length -gt 30) { $title = $title.Substring(0, 30) }
$detail = if ($nt -eq "permission_prompt") { "확인이 필요해요 🔔" } else { "입력을 기다리고 있어요 ✋" }
$payload = @{ source = "claude"; kind = "approval"; message = $title; detail = $detail }
$json  = $payload | ConvertTo-Json -Compress
$bytes = [System.Text.Encoding]::UTF8.GetBytes($json)
try { Invoke-RestMethod -Uri "http://127.0.0.1:37651/notify" -Method Post -Body $bytes -ContentType "application/json" -TimeoutSec 3 | Out-Null } catch {}
exit 0
"#;

/// 위 훅 스크립트를 ~/.claude 에 쓰고, settings.json 의 hooks.Notification 에 등록한다.
/// 매 시작 시 멱등적으로 갱신(스크립트 최신화 + 중복 방지). .claude 가 없으면(Claude Code
/// 미설치) 조용히 스킵. 사용자의 기존 설정은 파싱 후 보존한다.
fn register_claude_hook() {
    let home = match std::env::var("USERPROFILE") {
        Ok(h) => h,
        Err(_) => return,
    };
    let claude_dir = std::path::Path::new(&home).join(".claude");
    if !claude_dir.is_dir() {
        return;
    }
    let script_path = claude_dir.join("devpet-approval-hook.ps1");
    // UTF-8 BOM 을 붙여 Windows PowerShell 5.1 이 한글을 UTF-8 로 읽게 한다.
    let mut content = String::from("\u{feff}");
    content.push_str(APPROVAL_HOOK_PS1);
    if std::fs::write(&script_path, content).is_err() {
        return;
    }
    let settings_path = claude_dir.join("settings.json");
    let mut root: serde_json::Value = std::fs::read_to_string(&settings_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    let cmd = format!(
        "powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{}\"",
        script_path.display()
    );
    let obj = root.as_object_mut().unwrap();
    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    let notif = match hooks.as_object_mut() {
        Some(h) => h.entry("Notification").or_insert_with(|| serde_json::json!([])),
        None => return,
    };
    let arr = match notif.as_array_mut() {
        Some(a) => a,
        None => return,
    };
    // 기존 DevPet 훅(스크립트 이름으로 식별)은 제거 후 재삽입 → 경로/내용 갱신 반영, 중복 방지.
    arr.retain(|e| !e.to_string().contains("devpet-approval-hook.ps1"));
    // permission_prompt = 권한 확인 대기, agent_needs_input = 질문 등 입력 대기.
    // matcher 가 정규식인지 리터럴인지 보장되지 않아 타입별로 따로 등록한다.
    for matcher in ["permission_prompt", "agent_needs_input"] {
        arr.push(serde_json::json!({
            "matcher": matcher,
            "hooks": [{ "type": "command", "command": cmd, "timeout": 10 }]
        }));
    }
    if let Ok(s) = serde_json::to_string_pretty(&root) {
        let _ = std::fs::write(&settings_path, s);
    }
}

/// 완전 종료 시 훅 해제: 스크립트 삭제 + settings.json 의 DevPet 훅 제거(빈 컨테이너도 정리).
/// 앱이 꺼져 있으면 알림을 받을 수 없으니 훅이 헛돌 이유가 없고, 앱을 지운 뒤 설정만 남는
/// 것도 막는다. 다음 실행 때 register_claude_hook 이 다시 등록하므로 재설정은 불필요.
fn unregister_claude_hook() {
    let home = match std::env::var("USERPROFILE") {
        Ok(h) => h,
        Err(_) => return,
    };
    let claude_dir = std::path::Path::new(&home).join(".claude");
    if !claude_dir.is_dir() {
        return;
    }
    let _ = std::fs::remove_file(claude_dir.join("devpet-approval-hook.ps1"));
    let settings_path = claude_dir.join("settings.json");
    let mut root: serde_json::Value = match std::fs::read_to_string(&settings_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .filter(|v| v.is_object())
    {
        Some(v) => v,
        None => return,
    };
    let obj = root.as_object_mut().unwrap();
    if let Some(hooks) = obj.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        let mut notif_empty = false;
        if let Some(arr) = hooks.get_mut("Notification").and_then(|n| n.as_array_mut()) {
            arr.retain(|e| !e.to_string().contains("devpet-approval-hook.ps1"));
            notif_empty = arr.is_empty();
        }
        if notif_empty {
            hooks.remove("Notification");
        }
    }
    // 우리가 만든 빈 컨테이너는 남기지 않는다(사용자가 원래 쓰던 다른 훅은 그대로 보존).
    let hooks_empty = obj
        .get("hooks")
        .and_then(|h| h.as_object())
        .map(|h| h.is_empty())
        .unwrap_or(false);
    if hooks_empty {
        obj.remove("hooks");
    }
    if let Ok(s) = serde_json::to_string_pretty(&root) {
        let _ = std::fs::write(&settings_path, s);
    }
}

/// Windows 토스트 알림 표시. 클릭하면 숨겨진 펫이 다시 나타난다(실행 중 프로세스가 콜백 수신).
/// 백그라운드(창 숨김) 상태에서만 호출된다.
fn show_toast(app: &tauri::AppHandle, done: &TaskDone) {
    let icon = match done.source.as_str() {
        "claude" => "🟠",
        "codex" => "🟢",
        _ => "🔔",
    };
    let name = if done.message.is_empty() {
        "작업"
    } else {
        &done.message
    };
    let label = if done.kind == "approval" {
        "승인 필요 🔔"
    } else {
        "작업 완료 ✅"
    };
    let title = format!("{} {}", icon, name);
    let body = format!("{}\n클릭하면 펫이 나타나요", label);

    let build = |app_id: &str, handle: tauri::AppHandle| {
        Toast::new(app_id)
            .title(&title)
            .text1(&body)
            .duration(ToastDuration::Short)
            .on_activated(move |_action| {
                show_pet(&handle); // 토스트 클릭 → 펫 등장
                let _ = handle.emit("open-notif-list", ()); // + 알림 리스트 열기
                Ok(())
            })
            .show()
    };
    // 설치본은 앱 AUMID로 'DevPet'으로 표시됨. 실패하면 PowerShell 앱ID로 폴백(표시 보장).
    if build("com.devpet.app", app.clone()).is_err() {
        let _ = build(Toast::POWERSHELL_APP_ID, app.clone());
    }
}

/// 알림 발송: 소리 재생 + 디스코드 + 프론트 emit. (HTTP 서버와 watcher가 공용)
/// 창이 보이는 중이면 펫을 앞으로, 백그라운드(숨김)면 펫을 띄우지 않고 토스트만 쌓는다.
pub(crate) fn dispatch_notification(app: &tauri::AppHandle, done: TaskDone) {
    play_sound(&done.kind);
    send_discord(&done);
    let hidden = app
        .get_webview_window("main")
        .map(|w| !w.is_visible().unwrap_or(true))
        .unwrap_or(false);
    if hidden {
        // 백그라운드: 펫을 띄우지 않고 윈도우 토스트만 표시
        show_toast(app, &done);
    }
    // 리스트/배지 누적(펫이 다시 보일 때 확인). 보이는 상태면 말풍선도 표시됨.
    let _ = app.emit("task-done", &done);
    if !hidden {
        if let Some(win) = app.get_webview_window("main") {
            let _ = win.show();
            let _ = win.set_always_on_top(true);
        }
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
        "{} {} · {}{}",
        icon,
        if done.message.is_empty() {
            "작업"
        } else {
            &done.message
        },
        label,
        {
            let mut meta: Vec<String> = Vec::new();
            if let Some(e) = fmt_elapsed(done.elapsed_secs) {
                meta.push(e);
            }
            if let Some(t) = fmt_tokens(done.tokens) {
                meta.push(format!("{} 토큰", t));
            }
            if meta.is_empty() {
                String::new()
            } else {
                format!(" ({})", meta.join(" · "))
            }
        }
    );
    // 상세 내용은 넣지 않는다(제목 + 상태 + 시간/토큰만 간결하게).
    serde_json::json!({
        "username": "DevPet 🐾",
        "embeds": [{ "title": title, "color": color }]
    })
    .to_string()
}

/// 토큰 수 → "12k" 같은 짧은 표시. 0 이면 None(표시 안 함).
fn fmt_tokens(n: u64) -> Option<String> {
    if n < 1 {
        return None;
    }
    if n < 1_000 {
        return Some(n.to_string());
    }
    if n < 1_000_000 {
        let k = n as f64 / 1000.0;
        return Some(if n < 10_000 {
            format!("{:.1}k", k)
        } else {
            format!("{:.0}k", k)
        });
    }
    Some(format!("{:.1}M", n as f64 / 1_000_000.0))
}

/// 소요 시간(초) → "2분 30초" 같은 표시. 0/1초 미만이면 None(표시 안 함).
fn fmt_elapsed(secs: u64) -> Option<String> {
    if secs < 1 {
        return None;
    }
    if secs < 60 {
        return Some(format!("{}초", secs));
    }
    let m = secs / 60;
    if m < 60 {
        let r = secs % 60;
        return Some(if r > 0 {
            format!("{}분 {}초", m, r)
        } else {
            format!("{}분", m)
        });
    }
    let h = m / 60;
    let rm = m % 60;
    Some(if rm > 0 {
        format!("{}시간 {}분", h, rm)
    } else {
        format!("{}시간", h)
    })
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
                elapsed_secs: 0,
                tokens: 0,
            });

            dispatch_notification(&app, done);

            let resp = tiny_http::Response::from_string("ok");
            let _ = req.respond(resp);
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    register_aumid(); // 토스트 알림 표시를 위한 AUMID 등록/지정 (창 생성 전에)
    register_claude_hook(); // CLI 승인 대기 알림용 Notification 훅 설치/등록
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
            setup_tray(&handle)?; // 시스템 트레이(백그라운드에서 다시 열기)
            spawn_metrics_loop(handle.clone());
            spawn_notify_server(handle.clone());
            // Claude Code(CLI+데스크탑) / Codex 대화 기록을 감시해 완료 감지 (훅 불필요)
            watcher::spawn(handle);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            open_url,
            set_mute,
            quit_app,
            focus_window,
            set_discord_webhook,
            test_discord,
            get_autostart,
            set_autostart
        ])
        .run(tauri::generate_context!())
        .expect("DevPet 실행 중 오류");
}
