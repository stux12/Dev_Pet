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

/* ─────────────── 위험 경고 ─────────────── */
const warned = { cpu: false, mem: false, disk: false };
const warnedAt = { cpu: 0, mem: 0, disk: 0 };
const REWARN_MS = 3 * 60 * 1000;
function checkWarnings(m) {
  const now = Date.now();
  const checks = [
    ["cpu", m.cpu >= 85, `앗, CPU가 ${Math.round(m.cpu)}%까지 치솟았어요! 🔥\n무거운 작업이나 멈춘 프로세스가 있는지 살펴볼까요?`],
    ["mem", m.mem_pct >= 85, `메모리가 ${Math.round(m.mem_pct)}%예요 😥\n안 쓰는 앱이나 브라우저 탭을 좀 닫아주면 한결 가벼워질 거예요.`],
    ["disk", m.disk_pct >= 90, `디스크가 ${Math.round(m.disk_pct)}%나 찼어요 🧹\n곧 저장 공간이 부족해질 수 있어요. 정리가 필요해요!`],
  ];
  for (const [key, isDanger, msg] of checks) {
    if (isDanger) {
      if (!warned[key] || now - warnedAt[key] > REWARN_MS) {
        showBubble(msg, 9000, "danger");
        warned[key] = true;
        warnedAt[key] = now;
        break;
      }
    } else {
      warned[key] = false;
    }
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
let unread = 0;

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
    if (n.detail) {
      const d = document.createElement("div");
      d.className = "n-detail";
      d.textContent = n.detail;
      el.appendChild(d);
    }
    el.addEventListener("click", () => {
      if (n.hwnd) invoke("focus_window", { hwnd: n.hwnd });
    });
    box.appendChild(el);
  }
}
function addNotif(d) {
  notifs.push({
    source: d.source || "unknown",
    kind: d.kind || "completed",
    title: (d.message || "작업").trim(),
    detail: (d.detail || "").trim(),
    hwnd: d.hwnd || 0,
    ts: Date.now(),
  });
  saveNotifs();
  if (view !== "list") {
    unread++;
    updateBadge();
  }
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
  if (d.detail && d.detail.trim() && d.detail.trim() !== title) {
    text += `\n${d.detail.trim()}`;
  }
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
  $("panel").classList.toggle("hidden", v !== "panel");
  $("notif-list").classList.toggle("hidden", v !== "list");
  const [w, h] = SIZE[v] || SIZE.collapsed;
  await win.setSize(new LogicalSize(w, h));
  if (v === "list") {
    unread = 0;
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
  renderList();
});
$("close-btn").addEventListener("click", async (ev) => {
  ev.stopPropagation();
  await win.hide();
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
setTimeout(() => showBubble("클릭하면 상태를, 🔔로 알림을 볼 수 있어요! 🐾", 4500), 600);
