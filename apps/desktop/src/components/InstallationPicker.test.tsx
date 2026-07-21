import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { AgentInstallationView } from "../api";
import InstallationPicker, { installationLabels } from "./InstallationPicker";

function installation(path: string, version: string | null): AgentInstallationView {
  return {
    connected: false,
    discovery: {
      agent_id: "claude-code",
      executable_path: path,
      canonical_path: path,
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
      reason_code: "VerifiedRangeMatch",
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

  it("returns the hidden canonical path and closes after selection", async () => {
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
    expect(screen.queryByText("C:\\Tools\\claude.exe")).toBeNull();
    await user.click(screen.getByRole("option", { name: "claude.exe · v2.0.0" }));
    expect(onSelect).toHaveBeenCalledWith("C:\\Tools\\claude.exe");
    expect(screen.queryByRole("listbox")).toBeNull();
  });
});
