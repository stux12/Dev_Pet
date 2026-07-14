import { defineConfig } from "vite";

// Tauri 개발 서버 설정
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // Rust 빌드 산출물(실행 중인 exe 포함)을 감시하지 않도록 제외 → EBUSY 방지
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  build: {
    target: "esnext",
  },
});
