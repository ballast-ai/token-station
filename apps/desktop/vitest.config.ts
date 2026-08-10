import react from "@vitejs/plugin-react";
import path from "node:path";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    clearMocks: true,
    restoreMocks: true,
    // @lobehub/icons brand logos depend transitively on emoji-mart JSON. Let Vite
    // inline and transform it; otherwise native Node ESM lacks the
    // `with { type: "json" }` assertion and silently skips the App and
    // AddProviderPage rendering smoke tests.
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
