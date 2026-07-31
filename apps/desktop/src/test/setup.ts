import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, beforeEach } from "vitest";
import { AGENT_VISIBILITY_STORAGE_KEY } from "../components/AgentVisibilityPreferences";

// Existing component-level suites exercise the explicitly selected Chinese UI.
// App and LanguageProvider default-language suites clear this preference themselves.
beforeEach(() => {
  window.localStorage.setItem("token-station-language", "zh-CN");
  window.localStorage.removeItem(AGENT_VISIBILITY_STORAGE_KEY);
});

afterEach(() => {
  cleanup();
});
