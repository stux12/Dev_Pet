// 외부 의존성 없이 귀여운 펫 아이콘(512x512 PNG)을 생성합니다.
// 실행: node gen-icon.mjs  →  icon.png 생성
// 이후 `npx tauri icon icon.png` 로 모든 플랫폼 아이콘을 만듭니다.
import { deflateSync } from "node:zlib";
import { writeFileSync } from "node:fs";

const W = 512, H = 512;
const buf = new Uint8Array(W * H * 4); // RGBA, 투명 배경

function px(x, y, r, g, b, a = 1) {
  if (x < 0 || y < 0 || x >= W || y >= H) return;
  const i = (y * W + x) * 4;
  const da = buf[i + 3] / 255;
  const outA = a + da * (1 - a);
  if (outA <= 0) return;
  buf[i]     = (r * a + buf[i]     * da * (1 - a)) / outA;
  buf[i + 1] = (g * a + buf[i + 1] * da * (1 - a)) / outA;
  buf[i + 2] = (b * a + buf[i + 2] * da * (1 - a)) / outA;
  buf[i + 3] = outA * 255;
}

function ellipse(cx, cy, rx, ry, r, g, b, a = 1) {
  for (let y = Math.floor(cy - ry); y <= cy + ry; y++) {
    for (let x = Math.floor(cx - rx); x <= cx + rx; x++) {
      const dx = (x - cx) / rx, dy = (y - cy) / ry;
      if (dx * dx + dy * dy <= 1) px(x, y, r, g, b, a);
    }
  }
}

// 몸통 (teal)
ellipse(256, 292, 190, 180, 62, 198, 180);
// 볼 (pink)
ellipse(158, 330, 32, 30, 255, 157, 176, 0.75);
ellipse(354, 330, 32, 30, 255, 157, 176, 0.75);
// 눈 (dark)
ellipse(198, 250, 26, 34, 27, 43, 58);
ellipse(314, 250, 26, 34, 27, 43, 58);
// 눈 하이라이트
ellipse(206, 236, 9, 9, 255, 255, 255);
ellipse(322, 236, 9, 9, 255, 255, 255);
// 입 (아래로 볼록한 미소) — 두 점 사이 이차곡선 근사
for (let t = 0; t <= 1; t += 0.005) {
  const x = 218 + t * (294 - 218);
  const y = 322 + Math.sin(t * Math.PI) * 26;
  ellipse(x, y, 4, 4, 27, 43, 58);
}

// ── PNG 인코딩 ──
const crcTable = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();
function crc32(bytes) {
  let c = 0xffffffff;
  for (let i = 0; i < bytes.length; i++) c = crcTable[(c ^ bytes[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const t = Buffer.from(type, "ascii");
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const cd = Buffer.concat([t, data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(cd), 0);
  return Buffer.concat([len, cd, crc]);
}

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(W, 0);
ihdr.writeUInt32BE(H, 4);
ihdr[8] = 8;   // bit depth
ihdr[9] = 6;   // color type RGBA
ihdr[10] = 0;  // compression
ihdr[11] = 0;  // filter
ihdr[12] = 0;  // interlace

// 스캔라인마다 필터 바이트(0) 추가
const raw = Buffer.alloc((W * 4 + 1) * H);
for (let y = 0; y < H; y++) {
  raw[y * (W * 4 + 1)] = 0;
  buf.subarray(y * W * 4, (y + 1) * W * 4).forEach((v, i) => {
    raw[y * (W * 4 + 1) + 1 + i] = v;
  });
}
const idat = deflateSync(raw, { level: 9 });

const sig = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
const png = Buffer.concat([
  sig,
  chunk("IHDR", ihdr),
  chunk("IDAT", idat),
  chunk("IEND", Buffer.alloc(0)),
]);

writeFileSync(new URL("./icon.png", import.meta.url), png);
console.log("icon.png 생성 완료 (512x512)");
