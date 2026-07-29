import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    clearMocks: true,
    restoreMocks: true,
    // @lobehub/icons(品牌 logo)会传递依赖到 emoji-mart 的 .json;交给 vite 内联
    // 转译,否则 Node 原生 ESM 会因缺 `with { type: "json" }` 断言而整套加载失败,
    // 静默跳过 App / AddProviderPage 的渲染冒烟测试。
    server: {
      deps: {
        inline: [/@lobehub/, /emoji-mart/],
      },
    },
    coverage: {
      provider: "v8",
      reporter: ["text", "json-summary", "lcov"],
      reportsDirectory: "./coverage",
      reportOnFailure: true,
      include: ["src/**/*.{ts,tsx}"],
      exclude: [
        "src/main.tsx",
        "src/vite-env.d.ts",
        "src/**/*.test.{ts,tsx}",
        "src/test/**",
      ],
      thresholds: {
        lines: 80,
      },
    },
  },
});
