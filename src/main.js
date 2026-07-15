import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow, currentMonitor, availableMonitors } from "@tauri-apps/api/window";
import { LogicalSize, PhysicalPosition } from "@tauri-apps/api/dpi";

const $ = (id) => document.getElementById(id);
const win = getCurrentWindow();

const SIZE = {
  collapsed: [260, 176],
  panel: [260, 320],
  list: [260, 456],
  listDiscord: [260, 580], // 디스코드 설정 폼 펼쳤을 때
  closeMenu: [260, 360], // × 종료 선택 메뉴. 너비는 다른 뷰와 동일해야(260) 펫·×버튼이 안 밀림. 높이는 패널 top:170 + 내용(~180)이 안 잘리게.
};

function getVar(name) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

/* ─────────────── 시스템 메트릭 ─────────────── */
function levelColor(pct, dangerAt = 85, warnAt = 60) {
  if (pct >= dangerAt) return getVar("--danger");
  if (pct >= warnAt) return getVar("--warn");
  return getVar("--ok");
}
function setMetric(prefix, pct, dangerAt, warnAt) {
  const p = Math.max(0, Math.min(100, Math.round(pct)));
  $(`${prefix}-val`).textContent = `${p}%`;
  const bar = $(`${prefix}-bar`);
  bar.style.width = `${p}%`;
  bar.style.background = levelColor(p, dangerAt, warnAt);
}
function reactBodyColor(cpu) {
  const body = $("body");
  if (body) body.setAttribute("fill", levelColor(cpu));
}
const MOUTH = {
  happy: "M50 78 Q60 86 70 78",
  neutral: "M51 81 L69 81",
  sad: "M50 84 Q60 77 70 84",
};
function reactFace(mem) {
  const mouth = $("mouth");
  const svg = $("pet-svg");
  if (mem >= 85) {
    mouth.setAttribute("d", MOUTH.sad);
    svg.classList.add("worried");
  } else if (mem >= 60) {
    mouth.setAttribute("d", MOUTH.neutral);
    svg.classList.remove("worried");
  } else {
    mouth.setAttribute("d", MOUTH.happy);
    svg.classList.remove("worried");
  }
}

/* ─────────────── 말풍선 ─────────────── */
let bubbleTimer = null;
let bubbleHwnd = 0;
function showBubble(text, ms = 5000, tone = "", hwnd = 0) {
  bubbleHwnd = hwnd;
  const bubble = $("bubble");
  $("bubble-text").textContent = text;
  bubble.classList.remove("hidden", "pop", "warn", "danger");
  if (tone) bubble.classList.add(tone);
  void bubble.offsetWidth;
  bubble.classList.add("pop");
  const pet = $("pet");
  pet.classList.remove("happy");
  void pet.offsetWidth;
  pet.classList.add("happy");
  if (bubbleTimer) clearTimeout(bubbleTimer);
  if (ms > 0) bubbleTimer = setTimeout(hideBubble, ms);
}
function hideBubble() {
  $("bubble").classList.add("hidden");
  if (bubbleTimer) clearTimeout(bubbleTimer);
}
$("bubble").addEventListener("click", () => {
  if (bubbleHwnd) invoke("focus_window", { hwnd: bubbleHwnd });
  hideBubble();
});

/* ─────────────── 최초 감지(로딩) 화면 ─────────────── */
// 앱을 켤 때마다 시작 시 한 번, 필요한 대화 기록 파일을 감지할 때까지
// 생각하는 표정 + "감지 중" 문구를 띄우고, 스캔이 끝나면 준비 완료 문구로 전환.
let scanning = true;
let scanDone = false;
const SCAN_MIN_MS = 2200; // 너무 빨리 끝나도 최소 이만큼은 생각하는 로딩을 보여줌
const scanStart = Date.now();
function beginScan() {
  $("pet-svg").classList.add("thinking");
  showBubble("필요한 파일을 감지하고 있어요… 🔍", 0); // 0 = 자동으로 안 사라짐
}
function finishScan(info) {
  if (scanDone) return;
  scanDone = true;
  const wait = Math.max(0, SCAN_MIN_MS - (Date.now() - scanStart));
  setTimeout(() => {
    scanning = false;
    $("pet-svg").classList.remove("thinking");
    const found = [];
    if (info && info.claude) found.push("Claude");
    if (info && info.codex) found.push("Codex");
    const who = found.length ? `(${found.join(" · ")} 기록 감지) ` : "";
    showBubble(`감지 완료! ${who}이제 정상적으로 사용할 수 있어요 ✅`, 3800);
    // 스캔이 끝난 뒤에야 사용 소개를 보여준다 (실행 직후엔 안 나오도록)
    setTimeout(() => {
      if (view === "collapsed" && !closeMenuOpen) {
        showBubble("클릭하면 상태를, 🔔로 알림을 볼 수 있어요! 🐾", 4500);
      }
    }, 4200);
  }, wait);
}
beginScan();
listen("scan-ready", (e) => finishScan(e.payload));
// 백그라운드 토스트 클릭 → 펫이 뜨면서 알림 리스트까지 열기
listen("open-notif-list", () => { setView("list"); });
// 안전장치: 이벤트가 안 오더라도 8초 뒤엔 로딩 해제
setTimeout(() => finishScan(null), 8000);

/* ─────────────── 위험 경고 ─────────────── */
const warned = { cpu: false, mem: false, disk: false };
const warnedAt = { cpu: 0, mem: 0, disk: 0 };
const REWARN_MS = 3 * 60 * 1000;
function checkWarnings(m) {
  if (scanning) return; // 최초 감지 중엔 로딩 말풍선을 유지
  const now = Date.now();
  const checks = [
    ["cpu", m.cpu >= 85, `앗, CPU가 ${Math.round(m.cpu)}%까지 치솟았어요! 🔥\n무거운 작업이나 멈춘 프로세스가 있는지 살펴볼까요?`],
    ["mem", m.mem_pct >= 85, `메모리가 ${Math.round(m.mem_pct)}%예요 😥\n안 쓰는 앱이나 브라우저 탭을 좀 닫아주면 한결 가벼워질 거예요.`],
    ["disk", m.disk_pct >= 90, `디스크가 ${Math.round(m.disk_pct)}%나 찼어요 🧹\n곧 저장 공간이 부족해질 수 있어요. 정리가 필요해요!`],
  ];
  // 위험이 해제된 항목은 경고 상태 리셋(회복 후 다시 위험해지면 재경고)
  for (const [key, isDanger] of checks) {
    if (!isDanger) warned[key] = false;
  }
  // 경고가 필요한(위험 + 쿨다운 경과) 항목 중, 가장 오래전에 알린 것부터 표시.
  // (CPU가 먼저라고 항상 선점해 메모리·디스크 경고가 밀리던 문제 방지)
  const due = checks
    .filter(([key, isDanger]) => isDanger && (!warned[key] || now - warnedAt[key] > REWARN_MS))
    .sort((a, b) => warnedAt[a[0]] - warnedAt[b[0]]);
  if (due.length) {
    const [key, , msg] = due[0];
    showBubble(msg, 9000, "danger");
    warned[key] = true;
    warnedAt[key] = now;
  }
}

listen("metrics", (e) => {
  const m = e.payload;
  setMetric("cpu", m.cpu, 85, 60);
  setMetric("mem", m.mem_pct, 85, 60);
  setMetric("disk", m.disk_pct, 90, 90);
  reactBodyColor(m.cpu);
  reactFace(m.mem_pct);
  checkWarnings(m);
});

/* ─────────────── 알림 저장/리스트 ─────────────── */
const LS_KEY = "devpet_notifs";
const MAX_NOTIFS = 50;
let notifs = [];
try {
  notifs = JSON.parse(localStorage.getItem(LS_KEY) || "[]");
} catch (e) {
  notifs = [];
}
// 재시작 시엔 기존 알림을 모두 읽음 처리(배지는 앱 실행 후 새로 온 것만 셈)
notifs.forEach((n) => { n.read = true; });

function saveNotifs() {
  notifs = notifs.slice(-MAX_NOTIFS);
  localStorage.setItem(LS_KEY, JSON.stringify(notifs));
}
function relTime(ts) {
  const s = Math.floor((Date.now() - ts) / 1000);
  if (s < 60) return "방금";
  if (s < 3600) return `${Math.floor(s / 60)}분 전`;
  if (s < 86400) return `${Math.floor(s / 3600)}시간 전`;
  return `${Math.floor(s / 86400)}일 전`;
}
function iconOf(source) {
  return source === "claude" ? "🟠" : source === "codex" ? "🟢" : "🔔";
}
function updateBadge() {
  // 배지 = 안 읽은 알림 개수 (리스트 항목 기준이라 채팅별 dedup과 항상 일치)
  const unread = notifs.filter((n) => !n.read).length;
  const badge = $("bell-badge");
  const bell = $("bell-btn");
  if (unread > 0) {
    badge.textContent = unread > 99 ? "99+" : String(unread);
    badge.classList.remove("hidden");
    bell.classList.add("has-unread");
  } else {
    badge.classList.add("hidden");
    bell.classList.remove("has-unread");
  }
}
function renderList() {
  const box = $("notif-items");
  const wrap = $("notif-list");
  box.innerHTML = "";
  wrap.classList.toggle("has-items", notifs.length > 0);
  for (const n of [...notifs].reverse()) {
    const el = document.createElement("div");
    el.className = "notif-item" + (n.kind === "approval" ? " approval" : "");
    const kindLabel = n.kind === "approval" ? "승인 필요" : "완료";
    const title = document.createElement("div");
    title.className = "n-title";
    const left = document.createElement("span");
    left.textContent = `${iconOf(n.source)} ${n.title} · ${kindLabel}`;
    const time = document.createElement("span");
    time.className = "n-time";
    time.textContent = relTime(n.ts);
    title.append(left, time);
    el.appendChild(title);
    // 세부내용은 표시하지 않음 (제목 + 상태만 간단히)
    el.addEventListener("click", () => {
      if (n.hwnd) invoke("focus_window", { hwnd: n.hwnd });
    });
    box.appendChild(el);
  }
}
function addNotif(d) {
  const source = d.source || "unknown";
  const title = (d.message || "작업").trim();
  // 같은 채팅(동일 source+제목)에서 온 기존 알림은 리스트에서 제거 → 채팅당 최신 1건만 유지
  notifs = notifs.filter((n) => !(n.source === source && n.title === title));
  notifs.push({
    source,
    kind: d.kind || "completed",
    title,
    detail: (d.detail || "").trim(),
    hwnd: d.hwnd || 0,
    ts: Date.now(),
    read: view === "list", // 리스트를 보고 있으면 바로 읽음 처리
  });
  saveNotifs();
  updateBadge(); // 배지는 안 읽은 항목 수로 자동 계산(중복 카운트 없음)
  renderList();
}

/* ─────────────── 작업 완료 / 승인 알림 ─────────────── */
listen("task-done", (e) => {
  const d = e.payload;
  const icon = iconOf(d.source);
  const title = d.message && d.message.trim() ? d.message.trim() : "작업";
  let text;
  let ms;
  let tone;
  if (d.kind === "approval") {
    text = `${icon} ${title} 승인 필요 🔔`;
    ms = 20000;
    tone = "warn";
  } else {
    text = `${icon} ${title} 작업 완료 ✅`;
    ms = 10000;
    tone = "";
  }
  // 세부내용은 표시하지 않음 (제목 + 상태만 간단히)
  showBubble(text, ms, tone, d.hwnd || 0);
  addNotif(d);
});

/* ─────────────── 뷰 전환 (접힘/패널/리스트) ─────────────── */
let view = "collapsed";
async function setView(v) {
  view = v;
  // 뷰 바뀌면 디스코드 설정 폼 닫기
  discordOpen = false;
  $("discord-settings").classList.add("hidden");
  // 종료 메뉴가 열려 있었으면 함께 닫기(다른 뷰로 전환 시 겹쳐 보이는 문제 방지)
  closeMenuOpen = false;
  $("close-menu").classList.add("hidden");
  $("panel").classList.toggle("hidden", v !== "panel");
  $("notif-list").classList.toggle("hidden", v !== "list");
  const [w, h] = SIZE[v] || SIZE.collapsed;
  await win.setSize(new LogicalSize(w, h));
  if (v === "list") {
    notifs.forEach((n) => { n.read = true; }); // 리스트 열면 모두 읽음
    saveNotifs();
    updateBadge();
    renderList();
  }
}

/* ─────────────── 디스코드 웹훅 ─────────────── */
const WEBHOOK_KEY = "devpet_discord_webhook";
let discordOpen = false;
function setDsStatus(msg, cls) {
  const el = $("discord-status");
  el.textContent = msg;
  el.className = "ds-status" + (cls ? " " + cls : "");
}
// 시작 시 저장된 웹훅 복원 → Rust에 동기화
const savedHook = localStorage.getItem(WEBHOOK_KEY) || "";
$("discord-url").value = savedHook;
if (savedHook) invoke("set_discord_webhook", { url: savedHook });

$("discord-btn").addEventListener("click", async (ev) => {
  ev.stopPropagation();
  discordOpen = !discordOpen;
  $("discord-settings").classList.toggle("hidden", !discordOpen);
  const [w, h] = discordOpen ? SIZE.listDiscord : SIZE.list;
  await win.setSize(new LogicalSize(w, h));
});
$("discord-save").addEventListener("click", (ev) => {
  ev.stopPropagation();
  const url = $("discord-url").value.trim();
  localStorage.setItem(WEBHOOK_KEY, url);
  invoke("set_discord_webhook", { url });
  setDsStatus(url ? "저장됐어요 ✓" : "URL을 비웠어요", url ? "ok" : "");
});
$("discord-test").addEventListener("click", async (ev) => {
  ev.stopPropagation();
  const url = $("discord-url").value.trim();
  localStorage.setItem(WEBHOOK_KEY, url);
  await invoke("set_discord_webhook", { url });
  setDsStatus("전송 중…", "");
  try {
    const res = await invoke("test_discord");
    setDsStatus(res, res.includes("성공") ? "ok" : "err");
  } catch (e) {
    setDsStatus("오류: " + e, "err");
  }
});

/* ─────────────── 사용량 링크 ─────────────── */
document.querySelectorAll(".link-btn").forEach((btn) => {
  btn.addEventListener("click", (ev) => {
    ev.stopPropagation();
    invoke("open_url", { url: btn.dataset.url });
  });
});

/* ─────────────── 종 / 음소거 / 지우기 / 닫기 ─────────────── */
$("bell-btn").addEventListener("click", (ev) => {
  ev.stopPropagation();
  if (physicsOn) { setPhysics(false); return; } // 물리 중이면 종 클릭으로 종료
  setView(view === "list" ? "collapsed" : "list");
});

let muted = localStorage.getItem("devpet_muted") === "1";
function applyMute() {
  $("mute-btn").textContent = muted ? "🔇" : "🔊";
  invoke("set_mute", { muted });
}
$("mute-btn").addEventListener("click", (ev) => {
  ev.stopPropagation();
  muted = !muted;
  localStorage.setItem("devpet_muted", muted ? "1" : "0");
  applyMute();
});
$("clear-btn").addEventListener("click", (ev) => {
  ev.stopPropagation();
  notifs = [];
  saveNotifs();
  updateBadge();
  renderList();
});
/* ─────────────── × 종료 선택 (백그라운드 유지 / 완전 종료) ─────────────── */
let closeMenuOpen = false;
async function openCloseMenu() {
  closeMenuOpen = true;
  hideBubble();
  // 다른 뷰는 닫고 메뉴만 표시
  $("panel").classList.add("hidden");
  $("notif-list").classList.add("hidden");
  $("discord-settings").classList.add("hidden");
  $("close-menu").classList.remove("hidden");
  await win.setSize(new LogicalSize(...SIZE.closeMenu));
}
async function closeCloseMenu() {
  closeMenuOpen = false;
  $("close-menu").classList.add("hidden");
  view = "collapsed";
  await win.setSize(new LogicalSize(...SIZE.collapsed));
}
$("close-btn").addEventListener("click", (ev) => {
  ev.stopPropagation();
  if (closeMenuOpen) { closeCloseMenu(); return; }
  openCloseMenu();
});
$("cm-cancel").addEventListener("click", (ev) => {
  ev.stopPropagation();
  closeCloseMenu();
});
$("cm-bg").addEventListener("click", async (ev) => {
  ev.stopPropagation();
  // 백그라운드 유지: 창만 숨김(프로세스·알림 감시는 계속). 다음에 다시 보이면 접힌 상태로.
  $("close-menu").classList.add("hidden");
  closeMenuOpen = false;
  view = "collapsed";
  await win.setSize(new LogicalSize(...SIZE.collapsed));
  await win.hide();
});
$("cm-quit").addEventListener("click", async (ev) => {
  ev.stopPropagation();
  await invoke("quit_app"); // 프로세스 완전 종료
});

/* ─────────────── 탱탱볼 물리 모드 ─────────────── */
const pet = $("pet");
let physicsOn = false;
let rafId = null;
let ball = null; // {x,y,vx,vy,sf,limits...}

async function initBall() {
  const pos = await win.outerPosition();
  const size = await win.outerSize();
  // 창 중심이 속한 모니터를 직접 찾음 (다중 모니터에서 현재 화면에 적용되도록)
  const cx = pos.x + size.width / 2;
  const cy = pos.y + size.height / 2;
  let mon = null;
  try {
    const mons = await availableMonitors();
    mon =
      mons.find(
        (m) =>
          cx >= m.position.x &&
          cx < m.position.x + m.size.width &&
          cy >= m.position.y &&
          cy < m.position.y + m.size.height
      ) || null;
  } catch (_) {}
  if (!mon) {
    try { mon = await currentMonitor(); } catch (_) {}
  }
  const sf = mon ? mon.scaleFactor : window.devicePixelRatio || 1;
  const mL = mon ? mon.position.x : 0;
  const mT = mon ? mon.position.y : 0;
  const mW = mon ? mon.size.width : window.screen.width * sf;
  const mH = mon ? mon.size.height : window.screen.height * sf;
  // 펫(공)의 화면상 실제 테두리 여백(논리px→물리px). 펫 svg: 창 안에서 x[70,190], y[44,164]
  const offL = 70 * sf, offR = 190 * sf, offT = 44 * sf, offB = 164 * sf;
  return {
    x: pos.x, y: pos.y, vx: 0, vy: 0, sf,
    left: mL - offL,
    right: mL + mW - offR,
    top: mT - offT,
    floor: mT + mH - offB,
  };
}
function squash() {
  const svg = $("pet-svg");
  svg.classList.remove("squash");
  void svg.offsetWidth;
  svg.classList.add("squash");
}
let winceT = null;
function wince() {
  // 충돌 시 두 눈 찡끗 (안착하면 해제되어 원래 표정)
  const svg = $("pet-svg");
  svg.classList.add("wince");
  if (winceT) clearTimeout(winceT);
  winceT = setTimeout(() => svg.classList.remove("wince"), 260);
}
function step() {
  if (!physicsOn || !ball) return;
  const DT = 0.4; // 시간 배속 (낮을수록 느림, 슬로모션)
  const g = 1.9 * ball.sf;
  const REST = 0.72, WALL = 0.7, AIR = 0.999;
  const IMPACT = 2 * ball.sf; // 이 속도 이상 충돌이면 반응(찡끗/찌그러짐)
  ball.vy = (ball.vy + g * DT) * AIR;
  ball.vx *= AIR;
  ball.x += ball.vx * DT;
  ball.y += ball.vy * DT;

  let hitFloor = false, impact = false;
  if (ball.y >= ball.floor) {
    ball.y = ball.floor;
    if (Math.abs(ball.vy) > IMPACT) { impact = true; squash(); }
    ball.vy = -ball.vy * REST;
    ball.vx *= 0.9;
    hitFloor = true;
  }
  if (ball.y <= ball.top) { ball.y = ball.top; if (Math.abs(ball.vy) > IMPACT) impact = true; ball.vy = -ball.vy * WALL; }
  if (ball.x <= ball.left) { ball.x = ball.left; if (Math.abs(ball.vx) > IMPACT) impact = true; ball.vx = -ball.vx * WALL; }
  if (ball.x >= ball.right) { ball.x = ball.right; if (Math.abs(ball.vx) > IMPACT) impact = true; ball.vx = -ball.vx * WALL; }
  if (impact) wince();

  win.setPosition(new PhysicalPosition(Math.round(ball.x), Math.round(ball.y)));

  const slow = Math.abs(ball.vy) < 1.4 * ball.sf && Math.abs(ball.vx) < 0.6 * ball.sf;
  if (hitFloor && slow) {
    // 바닥에 안착 → 정지 (안착 시엔 찡끗 안 함)
    ball.vy = 0; ball.vx = 0;
    rafId = null;
    return;
  }
  rafId = requestAnimationFrame(step);
}
async function setPhysics(on) {
  if (on) {
    await setView("collapsed");
    document.body.classList.add("physics");
    physicsOn = true;
    ball = await initBall();
    showBubble("탱탱볼 모드! 나를 들어서 던져봐요 🏀\n(더블클릭하면 종료)", 4500);
    if (!rafId) rafId = requestAnimationFrame(step);
  } else {
    physicsOn = false;
    if (rafId) { cancelAnimationFrame(rafId); rafId = null; }
    document.body.classList.remove("physics");
    $("pet-svg").classList.remove("wince");
    showBubble("돌아왔어요! 👋", 2500);
  }
}

// 물리 모드 드래그(던지기): 포인터 캡처로 창 밖에서도 추적
let pDrag = false, grabX = 0, grabY = 0, pvx = 0, pvy = 0, lastDown = 0;
pet.addEventListener("pointerdown", async (e) => {
  if (!physicsOn || e.button !== 0) return;
  // 빠른 두 번 누름 = 더블클릭 → 물리 종료 (preventDefault가 dblclick을 막으므로 직접 감지)
  const now = performance.now();
  if (now - lastDown < 400) {
    lastDown = 0;
    setPhysics(false);
    return;
  }
  lastDown = now;
  e.preventDefault();
  pet.setPointerCapture(e.pointerId);
  if (rafId) { cancelAnimationFrame(rafId); rafId = null; }
  pDrag = true;
  const sf = window.devicePixelRatio || (ball ? ball.sf : 1);
  const pos = await win.outerPosition();
  grabX = e.screenX * sf - pos.x;
  grabY = e.screenY * sf - pos.y;
  pvx = 0; pvy = 0;
  ball.vx = 0; ball.vy = 0;
});
pet.addEventListener("pointermove", (e) => {
  if (!physicsOn || !pDrag) return;
  const sf = window.devicePixelRatio || ball.sf;
  const nx = e.screenX * sf - grabX;
  const ny = e.screenY * sf - grabY;
  pvx = nx - ball.x; // 프레임당 이동량 ≈ 속도
  pvy = ny - ball.y;
  ball.x = nx; ball.y = ny;
  win.setPosition(new PhysicalPosition(Math.round(nx), Math.round(ny)));
});
pet.addEventListener("pointerup", (e) => {
  if (!physicsOn || !pDrag) return;
  pDrag = false;
  try { pet.releasePointerCapture(e.pointerId); } catch (_) {}
  const cap = 45 * (ball ? ball.sf : 1);
  ball.vx = Math.max(-cap, Math.min(cap, pvx));
  ball.vy = Math.max(-cap, Math.min(cap, pvy));
  if (!rafId) rafId = requestAnimationFrame(step);
});

/* ─────────────── 펫: 클릭=패널 토글, 드래그=이동 (일반 모드) ─────────────── */
let downX = 0;
let downY = 0;
let dragging = false;
pet.addEventListener("mousedown", (e) => {
  if (physicsOn || e.button !== 0) return;
  downX = e.screenX;
  downY = e.screenY;
  dragging = false;
});
pet.addEventListener("mousemove", (e) => {
  if (physicsOn || e.buttons !== 1 || dragging) return;
  if (Math.abs(e.screenX - downX) > 4 || Math.abs(e.screenY - downY) > 4) {
    dragging = true;
    win.startDragging();
  }
});
pet.addEventListener("click", () => {
  if (physicsOn || dragging) return;
  setView(view === "panel" ? "collapsed" : "panel");
});
// 물리 모드 종료: 펫 더블클릭
pet.addEventListener("dblclick", () => {
  if (physicsOn) setPhysics(false);
});
// 게임 버튼(리스트 헤더): 탱탱볼 모드 토글
$("game-btn").addEventListener("click", (ev) => {
  ev.stopPropagation();
  setPhysics(!physicsOn);
});

/* ─────────────── 초기화 ─────────────── */
view = "collapsed";
applyMute();
updateBadge();
renderList();
// 사용 소개 말풍선은 스캔 완료 후에 표시됨 (finishScan 참고)
