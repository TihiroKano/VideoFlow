import { fileURLToPath, URL } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 期望一个固定端口的开发服务器，且不需要 Vite 清屏。
export default defineConfig({
  plugins: [react()],
  resolve: {
    // 与 tsconfig.json 的 paths 保持一致
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // Rust 与测试目录不参与前端热更新
      ignored: ["**/src-tauri/**", "**/test/**", "**/bin/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    // Tauri 在 Windows 上使用 WebView2，支持现代语法
    target: "chrome110",
    sourcemap: false,
  },
});