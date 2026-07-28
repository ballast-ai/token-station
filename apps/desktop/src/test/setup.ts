import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, beforeEach } from "vitest";

// Existing component-level suites exercise the explicitly selected Chinese UI.
// App and LanguageProvider default-language suites clear this preference themselves.
beforeEach(() => {
  window.localStorage.setItem("token-station-language", "zh-CN");
});

afterEach(() => {
  cleanup();
});
