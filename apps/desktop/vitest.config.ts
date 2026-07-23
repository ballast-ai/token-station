import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    clearMocks: true,
    restoreMocks: true,
    // @lobehub/icons brand logos add a transitive dependency on emoji-mart .json files. Let Vite inline them.
    // transpile it. Otherwise, native Node ESM fails to load without the `with { type: "json" }` assertion,
    // silently skip the App and AddProviderPage render smoke tests.
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
