import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { HarnessModelTarget, ProviderView } from "../api";
import HarnessModelMapping from "./HarnessModelMapping";
import { LanguageProvider } from "./LanguageProvider";

const providers: ProviderView[] = [
  {
    name: "deepseek",
    brand_id: "deepseek",
    provider: "openai-compatible",
    base_url: "https://api.deepseek.com/v1",
    models: ["deepseek-v4-pro", "deepseek-v4-flash"],
    has_auth: true,
  },
  {
    name: "openai",
    brand_id: "openai",
    provider: "openai",
    base_url: "https://api.openai.com/v1",
    models: ["gpt-5.5", "gpt-5.5-mini"],
    has_auth: true,
  },
];

const routes: Record<string, HarnessModelTarget> = {
  auto: { upstream: "openai", model: "gpt-5.5-mini" },
  fast: { upstream: "deepseek", model: "deepseek-v4-flash" },
  balanced: { upstream: "openai", model: "gpt-5.5" },
  power: { upstream: "deepseek", model: "deepseek-v4-pro" },
  "claude-fable-5-1": { upstream: "openai", model: "gpt-5.5-mini" },
};

function renderMapping(element: React.ReactElement) {
  return render(<LanguageProvider>{element}</LanguageProvider>);
}

describe("HarnessModelMapping", () => {
  it("defaults populated legacy mappings to off and requires explicit opt-in", async () => {
    const user = userEvent.setup();
    const onEnabledChange = vi.fn();
    renderMapping(<HarnessModelMapping agentId="claude-code" providers={providers} routes={routes} onEnabledChange={onEnabledChange} />);
    const toggle = screen.getByRole("switch", { name: "启用模型映射" });
    expect(toggle).not.toBeChecked();
    expect(screen.getByRole("combobox", { name: "Sonnet 供应商" })).toBeDisabled();
    await user.click(toggle);
    expect(onEnabledChange).toHaveBeenCalledWith(true);
  });

  it("shows Claude roles plus the exact Fable model and follows read-only inheritance", () => {
    renderMapping(
      <HarnessModelMapping
        agentId="claude-code"
        providers={providers}
        routes={routes}
        readOnly
      />,
    );

    expect(screen.getByText("claude-fable-5-1")).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: /Fable 供应商/i })).toBeDisabled();
    expect(screen.getByRole("combobox", { name: /Fable 模型/i })).toBeDisabled();
    expect(screen.getByRole("combobox", { name: /Fable 供应商/i })).toHaveTextContent("openai");
    expect(screen.getByRole("combobox", { name: /Fable 模型/i })).toHaveTextContent("gpt-5.5-mini");
  });

  it("lets every OpenCode request select an independent provider and model", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    renderMapping(
      <HarnessModelMapping
        agentId="opencode"
        enabled
        providers={providers}
        routes={routes}
        onChange={onChange}
      />,
    );

    await user.click(screen.getByRole("combobox", { name: /fast 供应商/i }));
    await user.click(screen.getByRole("option", { name: "openai" }));
    expect(onChange).toHaveBeenCalledWith("fast", { upstream: "openai", model: "gpt-5.5" });

    await user.click(screen.getByRole("combobox", { name: /power 模型/i }));
    await user.click(screen.getByRole("option", { name: "deepseek-v4-flash" }));
    expect(onChange).toHaveBeenCalledWith("power", {
      upstream: "deepseek",
      model: "deepseek-v4-flash",
    });
  });
});
