import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, beforeEach } from "vitest";
import { AGENT_VISIBILITY_STORAGE_KEY } from "../components/AgentVisibilityPreferences";

// Radix primitives use browser layout and pointer-capture APIs that jsdom does
// not implement. Keep the public interaction tests browser-shaped.
if (!HTMLElement.prototype.scrollIntoView) {
  HTMLElement.prototype.scrollIntoView = () => {};
}
if (!HTMLElement.prototype.hasPointerCapture) {
  HTMLElement.prototype.hasPointerCapture = () => false;
}
if (!HTMLElement.prototype.setPointerCapture) {
  HTMLElement.prototype.setPointerCapture = () => {};
}
if (!HTMLElement.prototype.releasePointerCapture) {
  HTMLElement.prototype.releasePointerCapture = () => {};
}

// Existing component-level suites exercise the explicitly selected Chinese UI.
// App and LanguageProvider default-language suites clear this preference themselves.
beforeEach(() => {
  window.localStorage.setItem("token-station-language", "zh-CN");
  window.localStorage.removeItem(AGENT_VISIBILITY_STORAGE_KEY);
});

afterEach(() => {
  cleanup();
});
