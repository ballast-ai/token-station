import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import type { FreeProviderPresetView, StateView } from "../api";
import FreeProviderConfigPage from "./FreeProviderConfigPage";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

const invokeMock = vi.mocked(invoke);
const openUrlMock = vi.mocked(openUrl);

const preset: FreeProviderPresetView = {
  id: "nvidia",
  upstream_name: "nvidia_free",
  label: "NVIDIA API Catalog",
  short_label: "NV",
  base_url: "https://integrate.api.nvidia.com/v1",
  offer_kind: "recurring",
  region: "global",
  tags: ["长期免费", "全球平台", "开发用途"],
  free_note: "build.nvidia.com 托管 API 的免费开发额度",
  key_instruction: "登录 build.nvidia.com，打开模型页面并点击 Get API Key。",
  application_url: "https://build.nvidia.com/",
  docs_url: "https://docs.example.com/nvidia",
  verified_at: "2026-07-27",
  overage_policy: "rate_limited",
  models: [
    {
      id: "openai/gpt-oss-120b",
      label: "GPT-OSS 120B",
      tool: "declared",
      vision: "unknown",
      json_schema: "declared",
      context_window: 131072,
    },
    {
      id: "openai/gpt-oss-20b",
      label: "GPT-OSS 20B",
      tool: "declared",
      vision: "unknown",
      json_schema: "declared",
      context_window: 131072,
    },
  ],
};

beforeEach(() => {
  invokeMock.mockReset();
  openUrlMock.mockReset();
});

it("shows the one-line key instruction and opens the official application page externally", async () => {
  const user = userEvent.setup();
  render(<FreeProviderConfigPage preset={preset} onBack={vi.fn()} onAdded={vi.fn()} onBusyChange={vi.fn()} />);

  expect(screen.getByText(preset.key_instruction)).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: /申请免费 API Key/ }));
  expect(openUrlMock).toHaveBeenCalledWith(preset.application_url);
});

it("selects all reviewed models by default and submits only the current selection", async () => {
  const user = userEvent.setup();
  const onAdded = vi.fn();
  const onBusyChange = vi.fn();
  invokeMock.mockResolvedValue({ providers: [] } as unknown as StateView);
  render(<FreeProviderConfigPage preset={preset} onBack={vi.fn()} onAdded={onAdded} onBusyChange={onBusyChange} />);

  expect(screen.getByRole("checkbox", { name: /GPT-OSS 120B/ })).toBeChecked();
  expect(screen.getByRole("checkbox", { name: /GPT-OSS 20B/ })).toBeChecked();
  await user.click(screen.getByRole("checkbox", { name: /GPT-OSS 20B/ }));
  await user.type(screen.getByLabelText("API Key"), "nvapi-test");
  await user.click(screen.getByRole("button", { name: "验证并添加免费供应商" }));

  expect(invokeMock).toHaveBeenCalledWith("add_free_provider", {
    presetId: "nvidia",
    selectedModels: ["openai/gpt-oss-120b"],
    apiKey: "nvapi-test",
    guardConfirmed: false,
  });
  expect(onAdded).toHaveBeenCalledOnce();
  expect(onBusyChange).toHaveBeenNthCalledWith(1, true);
  expect(onBusyChange).toHaveBeenLastCalledWith(false);
});
