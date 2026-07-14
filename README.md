# 🐾 DevPet

> 데스크탑 위에 떠 있는 귀여운 펫이 **시스템 상태**를 보여주고, **Claude Code / Codex 작업이 끝나거나 승인이 필요할 때** 알려주는 Windows 데스크탑 위젯.

CPU·메모리·디스크 사용률을 펫의 색과 표정으로 표현하고, AI 코딩 도구(Claude Code CLI·데스크탑 앱, Codex)의 작업 완료/승인 요청을 감지해 **펫 말풍선 + 알림 소리 + (선택) 디스코드**로 알려줍니다.

---

## ✨ 주요 기능

| 기능 | 설명 |
|------|------|
| 🖥️ **시스템 모니터링** | CPU/메모리/디스크 실시간 표시. CPU→몸통 색, 메모리→표정, 위험 시 말풍선 경고 |
| 🔔 **작업 완료 알림** | Claude Code·Codex 작업이 끝나면 "무슨 작업이 완료됐는지" 펫이 알림 (설정 불필요) |
| ⚠️ **승인 필요 알림** | 명령 실행 등 허용이 필요할 때 알림 (best-effort) |
| 📋 **알림 리스트** | 종(🔔) 아이콘에 최근 50건 보관, 안 읽은 개수 배지 |
| 🔊 **알림 소리** | 완료/승인 구분음, 음소거 토글 |
| 💬 **디스코드 연동** | 웹훅 URL만 넣으면 모든 알림을 디스코드로도 전송 |
| 🎮 **탱탱볼 모드** | 펫이 중력의 영향을 받는 공이 되어 통통 튀는 재미 요소 |
| 📌 **항상 위 / 투명 / 드래그 이동** | 작업표시줄에 안 뜨는 가벼운 위젯 (~40MB) |

---

## 🚀 설치 및 실행

### 방법 A — 설치 파일로 실행 (권장, 가장 간단)

1. `DevPet_0.1.0_x64_en-US.msi` 를 실행해 설치 (또는 릴리스에서 다운로드)
2. 시작 메뉴에서 **DevPet** 실행

### 방법 B — 소스에서 빌드

**사전 요구사항 (Windows)**
- [Node.js](https://nodejs.org) 18+
- [Rust](https://rustup.rs) (stable-msvc)
- **Microsoft C++ Build Tools** (Visual Studio Installer → "C++ 빌드 도구" 워크로드)
- WebView2 런타임 (Windows 11엔 기본 포함)

**빌드**
```bash
git clone https://github.com/stux12/Dev_Pet.git
cd Dev_Pet
npm install
npm run tauri build
```

**빌드 결과물 위치** (프로젝트 폴더 기준)

| 파일 | 경로 | 용도 |
|------|------|------|
| 실행 파일 | `src-tauri/target/release/dev-pet.exe` | 설치 없이 **바로 실행** |
| 설치 파일(MSI) | `src-tauri/target/release/bundle/msi/DevPet_0.1.0_x64_en-US.msi` | 정식 설치 / **다른 PC 배포** |

- 예시 전체 경로: `C:\...\Dev_Pet\src-tauri\target\release\bundle\msi\DevPet_0.1.0_x64_en-US.msi`
- 파일 탐색기 주소창에 `src-tauri\target\release\bundle\msi` 를 붙여넣으면 해당 폴더가 열립니다.
- ⚠️ `target/` 폴더는 `.gitignore`로 **저장소에는 포함되지 않습니다.** 각자 `npm run tauri build`로 생성하세요.
- 다른 PC에 배포하려면 **`.msi` 파일 하나만** 넘겨주면 됩니다.

**개발 모드로 실행**
```bash
npm run tauri dev
```

> 아이콘을 새로 만들려면(선택): `node gen-icon.mjs && npx tauri icon icon.png`

---

## 📖 사용법

### 1. 펫 조작
- **클릭** → 시스템 상태 패널 열기/닫기
- **드래그** → 원하는 위치로 이동
- **마우스 올리기** → 우상단 `×`(숨기기), 좌상단 🔔(알림 목록) 나타남

### 2. 시스템 모니터링 (상태 패널)
- **CPU** → 펫 몸통 색: 초록(정상) → 주황(60%↑) → 빨강(85%↑)
- **메모리** → 펫 표정: 웃음(정상) → 무표정(60%↑) → 걱정+땀방울(85%↑)
- **디스크** → 90% 초과 시에만 경고색
- **위험 수준**이면 펫이 말풍선으로 상황을 친근하게 설명
- **Claude 사용량 / Codex 사용량** 버튼 → 각 사용량 페이지 열기

### 3. 작업 완료 / 승인 알림 — **설정 불필요!**
펫 앱을 켜두기만 하면, Claude Code(CLI·데스크탑 앱)와 Codex의 작업을 자동 감지합니다.
- 작업이 끝나면 → 🟠/🟢 `{작업 제목} 작업 완료 ✅` + 요약
- 승인/허용이 필요하면 → `{작업 제목} 승인 필요 🔔` + 대기 중인 명령
- 말풍선은 항상 **최신 1건**만, 지난 알림은 🔔 목록에서 확인

### 4. 알림 목록 · 소리
- 🔔 클릭 → 알림 목록 (최근 50건, 재시작해도 유지)
- 목록 헤더: 🔊 음소거 · 🎮 탱탱볼 · 🗑 전체 삭제
- 안 읽은 알림 개수는 🔔에 빨간 배지로 표시

### 5. 💬 디스코드로도 알림 받기
1. 디스코드 → **서버 설정 → 연동 → 웹후크 → 새 웹후크** 만들고 **URL 복사**
2. 펫 → 🔔 → **디스코드 버튼**(블러플 로고) 클릭
3. URL 붙여넣기 → **저장** → **테스트**(초록 "전송 성공" 확인)
4. 이후 모든 알림이 펫 + 디스코드 양쪽으로! (완료=청록, 승인=주황 임베드)

### 6. 🎮 탱탱볼 모드
🔔 → 🎮 버튼 → 펫이 중력의 영향을 받는 공이 됩니다. 마우스로 들어 높은 곳에서 놓으면 통통 튀다가 바닥에 안착. **종료**: 펫 더블클릭 / 🎮 다시 클릭 / 🔔 클릭.

### 7. 부팅 시 자동 실행
시작프로그램 폴더에 실행 파일 바로가기를 넣으면 됩니다 (경로는 본인 환경에 맞게):
```powershell
$lnk = Join-Path ([Environment]::GetFolderPath('Startup')) "DevPet.lnk"
$sc  = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk)
$sc.TargetPath = "<설치경로>\dev-pet.exe"   # 예: ...\src-tauri\target\release\dev-pet.exe
$sc.Save()
```
해제: 위 `DevPet.lnk` 삭제 또는 **작업 관리자 > 시작 프로그램**에서 "사용 안 함".

---

## ⚙️ 알림은 어떻게 감지하나요? (동작 원리)

펫 앱이 AI 도구의 **대화 기록 파일을 직접 감시**합니다. 훅 설정이 필요 없고, CLI·데스크탑 앱·Codex를 모두 커버합니다.

- **Claude**: `%USERPROFILE%\.claude\projects\*\*.jsonl` — 마지막이 '도구호출 없는 assistant 텍스트'면 완료. 제목은 대화창 이름.
- **Codex**: `%USERPROFILE%\.codex\sessions\**\*.jsonl` — `task_complete` 이벤트 감지.
- **승인 필요(best-effort)**: 마지막이 도구호출(tool_use)이고 결과 없이 ~5초 조용하면 추정 알림.
- 앱 시작 이후의 완료만 알림(과거 것 무시). 감지까지 약 1~2초.

> **왜 훅이 아니라 감시?** Claude Code **데스크탑 앱은 command 훅을 실행하지 않기** 때문입니다(CLI만 실행). 파일 감시는 둘 다 커버합니다.

**한계**: 승인 감지는 파일에 명시적 마커가 없어 추정 방식이라, **자동 승인된 긴 작업(빌드 등)을 승인 대기로 오인**할 수 있습니다. Codex는 `sandbox=elevated`면 자동 승인이라 승인 요청이 없습니다.

수동 테스트용 로컬 엔드포인트도 있습니다: `POST http://127.0.0.1:37651/notify` — body `{source,kind,message,detail}`.

---

## 🧩 (선택) 훅 방식 — CLI에서 즉시 알림

CLI만 쓰고 즉각적인 알림을 원하면 훅을 쓸 수 있습니다. **단, 파일 감시와 같이 켜면 CLI에서 이중 알림**이 됩니다.

- Claude: `~/.claude/settings.json` 의 `hooks`에 `Stop`→`hooks/claude-notify.ps1`, `Notification`→`hooks/claude-approve.ps1` 등록 (경로는 본인 환경에 맞게, 절대경로).
- Codex: notify 슬롯이 OpenAI 런타임에 점유돼 있어 네이티브 포워더 `hooks/codex-forward.exe`로 원본 호출 + 펫 알림을 함께 처리. 원본 exe 경로는 `src-tauri/src/bin/codex-forward.rs` 의 `ORIG_EXE` 상수(환경마다 다름) → 수정 후 `cargo build --release --bin codex-forward`.

> ⚠️ 한글이 든 `.ps1` 은 **UTF-8 with BOM**으로 저장해야 PowerShell 5.1에서 안 깨집니다. transcript 읽을 땐 `Get-Content -Encoding UTF8`.

---

## 🏗️ 프로젝트 구조

```
pet-app/
├─ index.html                 # 펫 UI (마크업)
├─ src/
│  ├─ main.js                 # UI 로직(알림·물리·디스코드·뷰 전환)
│  └─ styles.css              # 스타일
├─ src-tauri/
│  ├─ src/lib.rs              # 메트릭·알림 HTTP 서버·소리·창 포커스·디스코드 전송
│  ├─ src/watcher.rs          # 대화 기록 파일 감시(완료/승인 감지)
│  ├─ src/bin/codex-forward.rs# (선택) Codex notify 포워더
│  ├─ tauri.conf.json         # 투명/always-on-top 창 설정
│  └─ capabilities/           # Tauri 권한
├─ hooks/                     # (선택) 훅 스크립트/바이너리
└─ gen-icon.mjs               # 아이콘 생성기
```

**기술 스택**: [Tauri v2](https://tauri.app) (Rust 백엔드) + Vite + Vanilla JS. HTTP는 `ureq`, 시스템 정보는 `sysinfo`, 창 제어/소리는 `windows` 크레이트.

---

## 📝 참고 / 한계

- **Windows 전용** (WebView2 + `windows` 크레이트 사용).
- 토큰 잔량 API가 없어 사용량은 **페이지 링크**로 대체합니다.
- 승인 알림은 best-effort(위 "동작 원리" 참고).
- 디스코드 전송은 CORS 회피를 위해 Rust에서 처리합니다.

---

## 📄 License

MIT

---

## 🗒️ 업데이트 이력

> 커밋이 있을 때마다 무엇을 바꿨는지 여기에 간략히 기록합니다. (최신순)

### 2026-07-14
- **Codex 알림 제목 정확도 개선** — 사용자가 설정한 스레드 제목(`session_index.jsonl`의 `thread_name`)을 표시하도록 변경. 긴 지시문/시스템 프롬프트 대신 실제 작업 제목("포트폴리오 사이트 구축" 등)이 나옵니다.
- **알림 간소화** — 말풍선·리스트에서 세부내용을 빼고 `제목 + 작업 완료/승인 필요`만 깔끔하게 표시.
- **Codex 오탐 알림 수정** — 승인 판정용 JSON(`{"outcome":"allow"}` 등)·시스템 프롬프트가 완료 알림으로 잘못 뜨던 문제 제외 처리.
- **승인 감지 안정화** — 승인 필요 감지 대기 시간을 약 14초로 상향해 AI 처리/생각 중 오탐 방지.
- **문서** — README에 빌드 결과물(exe/MSI) 위치 안내 추가.
- **최초 릴리스** — 데스크탑 시스템 모니터(CPU·메모리·디스크) + Claude Code/Codex 작업 완료·승인 알림 + 알림 리스트/소리 + 디스코드 연동 + 탱탱볼 모드.
