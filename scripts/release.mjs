#!/usr/bin/env node
// DevPet 릴리스 자동화: 서명 빌드 → latest.json → GitHub 릴리스(MSI + 매니페스트) 업로드.
// 자동 업데이트가 깨지지 않으려면 매 릴리스에 서명(.sig)과 latest.json 이 반드시 함께 올라가야 한다.
//
// 사용법:  node scripts/release.mjs [릴리스노트파일.md]
//   - 버전은 package.json 의 현재 값을 사용한다(미리 bump + 커밋/푸시해 둘 것).
//   - 릴리스노트 파일을 주면 그 내용을, 없으면 기본 문구를 릴리스 본문으로 쓴다.
//   - private key: ~/.tauri/devpet_updater.key (없으면 중단), GitHub 토큰: git credential.

import { execSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";

const REPO = "stux12/Dev_Pet";
const KEY_PATH = path.join(os.homedir(), ".tauri", "devpet_updater.key");

function die(msg) {
  console.error(`[release] ✖ ${msg}`);
  process.exit(1);
}
function step(msg) {
  console.log(`\n[release] ${msg}`);
}

// ── 0. 사전 점검 ──────────────────────────────────────────────
if (!fs.existsSync(KEY_PATH)) {
  die(`서명 키가 없습니다: ${KEY_PATH}\n  분실했다면 자동 업데이트를 이어갈 수 없습니다(사용자 재설치 필요).`);
}
const pkg = JSON.parse(fs.readFileSync("package.json", "utf8"));
const version = pkg.version;
const tag = `v${version}`;
const msiName = `DevPet_${version}_x64_en-US.msi`;
const msiPath = path.join("src-tauri", "target", "release", "bundle", "msi", msiName);

// 버전 3곳 일치 확인(불일치 시 빌드 산출물 이름이 어긋난다)
const cargoVer = (fs.readFileSync("src-tauri/Cargo.toml", "utf8").match(/^version = "(.+)"/m) || [])[1];
const confVer = JSON.parse(fs.readFileSync("src-tauri/tauri.conf.json", "utf8")).version;
if (cargoVer !== version || confVer !== version) {
  die(`버전 불일치: package.json=${version}, Cargo.toml=${cargoVer}, tauri.conf.json=${confVer}\n  세 곳을 같은 버전으로 맞춘 뒤 다시 실행하세요.`);
}

const notesArg = process.argv[2];
const notes =
  notesArg && fs.existsSync(notesArg)
    ? fs.readFileSync(notesArg, "utf8")
    : `v${version} 릴리스`;

console.log(`[release] 대상: ${tag}  (MSI: ${msiName})`);

// ── 1. 서명 빌드 ──────────────────────────────────────────────
step("서명 빌드 중… (실행 중인 앱이 있으면 종료)");
try {
  execSync("taskkill /F /IM dev-pet.exe", { stdio: "ignore" });
} catch {
  /* 실행 중이 아니면 무시 */
}
const env = {
  ...process.env,
  TAURI_SIGNING_PRIVATE_KEY: fs.readFileSync(KEY_PATH, "utf8").trim(),
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "",
};
execSync("npm run tauri build", { stdio: "inherit", env });

if (!fs.existsSync(msiPath)) die(`MSI 가 생성되지 않았습니다: ${msiPath}`);
const sigPath = msiPath + ".sig";
if (!fs.existsSync(sigPath)) {
  die(`서명(.sig)이 없습니다. tauri.conf.json 의 bundle.createUpdaterArtifacts 가 true 인지 확인하세요.`);
}
const signature = fs.readFileSync(sigPath, "utf8").trim();

// ── 2. latest.json (업데이트 매니페스트) ──────────────────────
step("latest.json 생성");
const latest = {
  version,
  notes: notes.split("\n").find((l) => l.trim()) || `v${version}`,
  pub_date: new Date().toISOString().replace(/\.\d+Z$/, "Z"),
  platforms: {
    "windows-x86_64": {
      signature,
      url: `https://github.com/${REPO}/releases/download/${tag}/${msiName}`,
    },
  },
};
const latestPath = "latest.json";
fs.writeFileSync(latestPath, JSON.stringify(latest, null, 2));

// ── 3. GitHub 토큰 (git credential) ───────────────────────────
step("GitHub 토큰 조회");
let token;
try {
  const cred = execSync("git credential fill", {
    input: "protocol=https\nhost=github.com\n\n",
  }).toString();
  token = (cred.match(/^password=(.+)$/m) || [])[1]?.trim();
} catch (e) {
  die(`git credential 실패: ${e.message}`);
}
if (!token) die("GitHub 토큰을 가져오지 못했습니다.");

const gh = (url, opt = {}) =>
  fetch(url, {
    ...opt,
    headers: {
      Authorization: `token ${token}`,
      "User-Agent": "devpet-release",
      Accept: "application/vnd.github+json",
      ...(opt.headers || {}),
    },
  });

// ── 4. 릴리스 생성 + 에셋 업로드 ──────────────────────────────
step(`GitHub 릴리스 ${tag} 생성`);
const relRes = await gh(`https://api.github.com/repos/${REPO}/releases`, {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify({ tag_name: tag, name: tag, body: notes, draft: false, prerelease: false }),
});
if (!relRes.ok) die(`릴리스 생성 실패 (${relRes.status}): ${await relRes.text()}`);
const rel = await relRes.json();
const uploadBase = rel.upload_url.split("{")[0];

for (const [file, name, ctype] of [
  [msiPath, msiName, "application/octet-stream"],
  [latestPath, "latest.json", "application/json"],
]) {
  step(`업로드: ${name}`);
  const res = await gh(`${uploadBase}?name=${encodeURIComponent(name)}`, {
    method: "POST",
    headers: { "Content-Type": ctype },
    body: fs.readFileSync(file),
  });
  if (!res.ok) die(`업로드 실패 ${name} (${res.status}): ${await res.text()}`);
  const a = await res.json();
  console.log(`[release]   ✔ ${a.name} (${a.size} bytes)`);
}

fs.rmSync(latestPath, { force: true });

// ── 5. 매니페스트 검증 ────────────────────────────────────────
step("매니페스트 검증");
const check = await fetch(
  `https://github.com/${REPO}/releases/latest/download/latest.json`,
).then((r) => r.json());
if (check.version === version) {
  console.log(`[release]   ✔ latest.json 이 v${version} 로 서빙됨`);
} else {
  console.log(`[release]   ⚠ latest.json version=${check.version} (기대 ${version}) — 잠시 후 반영될 수 있음`);
}

console.log(`\n[release] ✅ 완료: ${rel.html_url}`);
