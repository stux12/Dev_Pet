// Codex notify 포워더 (네이티브 exe)
// Codex가 `codex-forward.exe <event-json>` 로 호출 → argv가 shell 인용 없이 깔끔하게 전달됨.
//   1) 기존 OpenAI computer-use 알림(turn-ended)을 그대로 전달 (원본 기능 보존)
//   2) 펫에게 완료/승인 알림 POST
use std::io::Write;
use std::net::TcpStream;
use std::process::Command;
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

const ORIG_EXE: &str = r"C:\Users\Dev\AppData\Local\OpenAI\Codex\runtimes\cua_node\ecfc0d9aa02807e3\bin\node_modules\@oai\sky\bin\windows\codex-computer-use.exe";

fn short(s: &str, n: usize) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = joined.chars().collect();
    if chars.len() > n {
        format!("{}…", chars[..n].iter().collect::<String>())
    } else {
        joined
    }
}

fn post(body: &str) {
    if let Ok(mut stream) = TcpStream::connect("127.0.0.1:37651") {
        let req = format!(
            "POST /notify HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.as_bytes().len(),
            body
        );
        let _ = stream.write_all(req.as_bytes());
        let _ = stream.flush();
    }
}

fn main() {
    // 알림 클릭 시 포커스할 창 = 지금 활성 창(작업하던 Codex/터미널)
    let hwnd: i64 = unsafe { GetForegroundWindow().0 as usize as i64 };

    let args: Vec<String> = std::env::args().skip(1).collect();
    let json = args.join(" ");

    // 1) 원본 computer-use 알림 유지 (경로 없으면 조용히 건너뜀)
    if std::path::Path::new(ORIG_EXE).exists() {
        let _ = Command::new(ORIG_EXE).arg("turn-ended").arg(&json).spawn();
    }

    // 2) 펫 알림
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) {
        let typ = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
        let kind = if typ.contains("approval") || typ.contains("approve") || typ.contains("request") {
            "approval"
        } else {
            "completed"
        };

        let title = v
            .get("input-messages")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "작업".to_string());

        let summary = v
            .get("last-assistant-message")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();

        let detail = if kind == "approval" && summary.is_empty() {
            "승인 또는 입력이 필요해요".to_string()
        } else {
            short(&summary, 55)
        };

        let body = serde_json::json!({
            "source": "codex",
            "kind": kind,
            "message": short(&title, 30),
            "detail": detail,
            "hwnd": hwnd,
        })
        .to_string();

        post(&body);
    }
}
