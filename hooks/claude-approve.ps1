# Claude Code Notification 훅용 — 승인/입력이 필요할 때 펫에게 알림
# stdin JSON({message, transcript_path, ...}) 수신
#   message 예: "Claude needs your permission to use Bash"
$ErrorActionPreference = 'SilentlyContinue'

$raw = [Console]::In.ReadToEnd()

# 알림 클릭 시 포커스할 창 = 지금 활성 창
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

$customTitle = ""                     # 사용자가 지정한 대화창 이름
$aiTitle = ""                         # Claude 자동 생성 제목
$lastUser = ""                        # 마지막 사용자 요청 (대체용)
$notice = "승인 또는 입력이 필요해요"  # 무엇을 승인해야 하는지

try {
    $j = $raw | ConvertFrom-Json
    if ($j.message) { $notice = $j.message }
    # 제목은 대화창 이름 우선 (없으면 마지막 사용자 요청)
    if ($j.transcript_path -and (Test-Path $j.transcript_path)) {
        foreach ($line in (Get-Content $j.transcript_path -Encoding UTF8)) {
            try {
                $o = $line | ConvertFrom-Json
                if ($o.type -eq 'custom-title' -and $o.customTitle) { $customTitle = $o.customTitle }
                elseif ($o.type -eq 'ai-title' -and $o.aiTitle) { $aiTitle = $o.aiTitle }
                elseif ($o.message.role -eq 'user') {
                    $t = Get-MsgText $o.message.content
                    if ($t) { $lastUser = $t }
                }
            } catch {}
        }
    }
} catch {}

$title = if ($customTitle) { $customTitle } elseif ($aiTitle) { $aiTitle } elseif ($lastUser) { $lastUser } else { "작업" }

$message = Short $title 30    # 대화창 이름
$detail = Short $notice 55    # 승인 대상(간략)

$json = @{ source = "claude"; kind = "approval"; message = $message; detail = $detail; hwnd = $hwnd } | ConvertTo-Json -Compress
$bytes = [System.Text.Encoding]::UTF8.GetBytes($json)
try {
    Invoke-RestMethod -Uri "http://127.0.0.1:37651/notify" -Method Post -Body $bytes -ContentType "application/json; charset=utf-8" -TimeoutSec 2
} catch {}
