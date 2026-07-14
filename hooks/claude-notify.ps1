# Claude Code Stop 훅용 — 작업이 끝나면 "무슨 작업이 끝났는지"를 펫에게 알림
# stdin으로 JSON({session_id, transcript_path, ...})을 받음
$ErrorActionPreference = 'SilentlyContinue'

$raw = [Console]::In.ReadToEnd()

# 알림 클릭 시 포커스할 창 = 지금 활성 창(작업하던 터미널/앱)
$hwnd = 0
try {
    if (-not ("Native.FgWin" -as [type])) {
        Add-Type -Namespace Native -Name FgWin -MemberDefinition '[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();'
    }
    $hwnd = [Native.FgWin]::GetForegroundWindow().ToInt64()
} catch {}

function Get-MsgText($content) {
    if ($null -eq $content) { return "" }
    if ($content -is [string]) { return $content }
    $parts = @()
    foreach ($c in $content) {
        if ($c.type -eq 'text' -and $c.text) { $parts += $c.text }
    }
    return ($parts -join ' ')
}
function Short($s, $n) {
    if (-not $s) { return "" }
    $s = ($s -replace '\s+', ' ').Trim()
    if ($s.Length -gt $n) { return $s.Substring(0, $n) + "…" }
    return $s
}

$customTitle = ""    # 사용자가 지정한 대화창 이름
$aiTitle = ""        # Claude가 자동 생성한 대화창 제목
$lastUser = ""       # 마지막 사용자 요청 (제목 대체용)
$summary = ""        # 마지막 어시스턴트 메시지 (요약)

try {
    $j = $raw | ConvertFrom-Json
    if ($j.transcript_path -and (Test-Path $j.transcript_path)) {
        $lines = Get-Content $j.transcript_path -Encoding UTF8
        foreach ($line in $lines) {
            try {
                $o = $line | ConvertFrom-Json
                if ($o.type -eq 'custom-title' -and $o.customTitle) { $customTitle = $o.customTitle }
                elseif ($o.type -eq 'ai-title' -and $o.aiTitle) { $aiTitle = $o.aiTitle }
                else {
                    $role = $o.message.role
                    $text = Get-MsgText $o.message.content
                    if ($text) {
                        if ($role -eq 'user') { $lastUser = $text }
                        elseif ($role -eq 'assistant') { $summary = $text }
                    }
                }
            } catch {}
        }
    }
} catch {}

# 대화창 이름 우선: 사용자 지정 > AI 자동 > 마지막 요청
$title = if ($customTitle) { $customTitle } elseif ($aiTitle) { $aiTitle } elseif ($lastUser) { $lastUser } else { "작업" }

$message = Short $title 30   # 대화창 이름(간결하게)
$detail = Short $summary 55  # 세부 내용(간략 요약)

$json = @{ source = "claude"; kind = "completed"; message = $message; detail = $detail; hwnd = $hwnd } | ConvertTo-Json -Compress
$bytes = [System.Text.Encoding]::UTF8.GetBytes($json)   # 한글 깨짐 방지: UTF-8 바이트 전송
try {
    Invoke-RestMethod -Uri "http://127.0.0.1:37651/notify" -Method Post -Body $bytes -ContentType "application/json; charset=utf-8" -TimeoutSec 2
} catch {}
