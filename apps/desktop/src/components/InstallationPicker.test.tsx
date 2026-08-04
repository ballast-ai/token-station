import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { AgentInstallationView } from "../api";
import InstallationPicker, { installationLabels } from "./InstallationPicker";

function installation(path: string, version: string | null): AgentInstallationView {
  return {
    managed: false,
    connected: false,
    adapter_ready: true,
    discovery: {
      agent_id: "claude-code",
      executable_path: path,
      canonical_path: path,
      binary_source: "path",
      modified_at_ms: null,
      binary_sha256: null,
      upgrade_command: null,
      version_raw: version,
      version_normalized: version,
      environment: "macos",
      evidence: [],
      is_path_default: false,
      runnable: true,
      config_candidates: [],
      config_fingerprint: null,
      conflict_group: null,
      diagnostics: [],
      scanned_at_ms: 1,
    },
    compatibility: {
      agent_id: "claude-code",
      installation_path: path,
      status: "DETECTED_VERIFIED",
      reason_code: "DefaultAdmission",
      message: "ok",
      matched_catalog_version: "fixture",
      connector_id: "claude-code-v1",
      allowed_actions: ["preview_connect"],
    },
  };
}

describe("InstallationPicker", () => {
  it("does not render for a single installation", () => {
    render(
      <InstallationPicker
        agentName="Claude Code"
        installations={[installation("/Users/x/bin/claude", "1.2.3")]}
        selectedPath="/Users/x/bin/claude"
        onSelect={vi.fn()}
      />,
    );
    expect(screen.queryByRole("button", { name: /选择安装/ })).toBeNull();
  });

  it("shows only short names, versions and stable duplicate indexes", () => {
    const labels = installationLabels([
      installation("/Users/x/bin/claude.exe", "1.2.3"),
      installation("C:\\Tools\\claude.exe", "1.2.3"),
      installation("/opt/claude", null),
    ]);
    expect(labels.map((item) => item.label)).toEqual([
      "claude.exe · v1.2.3 · 1/2",
      "claude.exe · v1.2.3 · 2/2",
      "claude",
    ]);
    expect(labels.map((item) => item.path)).toEqual([
      "/Users/x/bin/claude.exe",
      "C:\\Tools\\claude.exe",
      "/opt/claude",
    ]);
  });

  it("labels WorkBuddy variants by their verified app bundle path", () => {
    const labels = installationLabels(
      [
        installation(
          "/Applications/WorkBuddy.app/Contents/Resources/app.asar.unpacked/cli/bin/codebuddy",
          "2.115.0",
        ),
        installation(
          "/Volumes/WorkBuddy/WorkBuddy AI.app/Contents/Resources/app.asar.unpacked/cli/bin/codebuddy",
          "2.106.4",
        ),
      ],
      { china: "WorkBuddy 中国版", global: "WorkBuddy 海外版" },
    );

    expect(labels.map((item) => item.label)).toEqual([
      "WorkBuddy 中国版 · v2.115.0",
      "WorkBuddy 海外版 · v2.106.4",
    ]);
  });

  it("returns the exact visible canonical path and closes after selection", async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    const installations = [
      installation("/Users/x/bin/claude", "1.2.3"),
      installation("C:\\Tools\\claude.exe", "2.0.0"),
    ];
    render(
      <InstallationPicker
        agentName="Claude Code"
        installations={installations}
        selectedPath={installations[0].discovery.canonical_path}
        onSelect={onSelect}
      />,
    );

    expect(screen.queryByText("/Users/x/bin/claude")).toBeNull();
    await user.click(screen.getByRole("button", { name: /选择安装/ }));
    expect(screen.getByRole("option", { name: "claude · v1.2.3" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByText("C:\\Tools\\claude.exe")).toBeInTheDocument();
    await user.click(screen.getByRole("option", { name: "claude.exe · v2.0.0" }));
    expect(onSelect).toHaveBeenCalledWith("C:\\Tools\\claude.exe");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("lists the exact source and immutable binary facts for every installation", async () => {
    const user = userEvent.setup();
    const pathDefault = installation("/opt/homebrew/bin/claude", "2.0.0");
    Object.assign(pathDefault.discovery, {
      binary_source: "homebrew",
      modified_at_ms: 1_784_700_000_000,
      binary_sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
      upgrade_command: "brew upgrade claude-code",
      is_path_default: true,
    });
    const npm = installation("/Users/x/.npm/bin/claude", "1.2.3");
    Object.assign(npm.discovery, {
      binary_source: "npm_global",
      modified_at_ms: 1_784_600_000_000,
      binary_sha256: "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
      upgrade_command: "npm install --global @anthropic-ai/claude-code@latest",
    });
    const store = installation("C:\\Program Files\\WindowsApps\\OpenAI.Codex\\codex.exe", "0.146.0");
    Object.assign(store.discovery, {
      binary_source: "microsoft_store",
      environment: "windows",
    });

    render(
      <InstallationPicker
        agentName="Claude Code"
        installations={[pathDefault, npm, store]}
        selectedPath={pathDefault.discovery.canonical_path}
        onSelect={vi.fn()}
      />,
    );

    await user.click(screen.getByRole("button", { name: /选择安装/ }));
    expect(screen.getByText("/opt/homebrew/bin/claude")).toBeInTheDocument();
    expect(screen.getByText("/Users/x/.npm/bin/claude")).toBeInTheDocument();
    expect(screen.getByText(/Homebrew · 当前生效/)).toBeInTheDocument();
    expect(screen.getByText(/npm 全局/)).toBeInTheDocument();
    expect(screen.getByText(/Microsoft Store/)).toBeInTheDocument();
    expect(screen.getByText(/SHA-256 0123456789ab/)).toBeInTheDocument();
    expect(screen.getByText(/SHA-256 abcdef012345/)).toBeInTheDocument();
  });
});
