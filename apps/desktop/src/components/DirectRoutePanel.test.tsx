import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderView } from "../api";
import DirectRoutePanel, { DIRECT_PROVIDER_ORDER_STORAGE_KEY } from "./DirectRoutePanel";
import { ErrorToastProvider } from "./ErrorToast";

const providers: ProviderView[] = [
  {
    name: "deepseek-account",
    brand_id: "deepseek",
    provider: "openai-compatible",
    base_url: "https://api.deepseek.com/v1",
    models: ["deepseek-chat", "deepseek-reasoner"],
    has_auth: true,
  },
  {
    name: "openai-account",
    brand_id: "openai",
    provider: "openai",
    base_url: "https://api.openai.com/v1",
    models: ["gpt-5.6", "gpt-5.6-mini"],
    has_auth: true,
  },
  {
    name: "empty-account",
    brand_id: null,
    provider: "openai-compatible",
    base_url: "https://example.com/v1",
    models: [],
    has_auth: false,
  },
];

beforeEach(() => {
  window.localStorage.removeItem(DIRECT_PROVIDER_ORDER_STORAGE_KEY);
});

describe("DirectRoutePanel", () => {
  it("places the primary apply action in the card header", () => {
    render(
      <DirectRoutePanel
        providers={providers}
        target={{ upstream: "deepseek-account", model: "deepseek-chat" }}
        busy={false}
        applying={false}
        onApply={vi.fn()}
      />,
    );

    expect(screen.getByRole("button", { name: "应用" }).closest(".panel-head"))
      .toBeInTheDocument();
  });

  it("keeps the applied badge to one unambiguous state label", () => {
    render(
      <DirectRoutePanel
        providers={providers}
        target={{ upstream: "deepseek-account", model: "deepseek-chat" }}
        busy={false}
        applying={false}
        onApply={vi.fn()}
      />,
    );

    const applied = document.querySelector(".direct-applied-target");
    expect(applied).toHaveTextContent(/^已应用$/);
    expect(applied).not.toHaveTextContent("deepseek-account");
    const selectedRow = screen.getByRole("radio", { name: /deepseek-account/ }).closest(".direct-provider-row");
    expect(selectedRow).toHaveTextContent("deepseek-account");
    expect(selectedRow).toHaveTextContent("deepseek-chat");
  });

  it("keeps the applied route visible while a different target is only a draft", async () => {
    const user = userEvent.setup();
    render(
      <DirectRoutePanel
        providers={providers}
        target={{ upstream: "deepseek-account", model: "deepseek-chat" }}
        busy={false}
        applying={false}
        onApply={vi.fn()}
      />,
    );

    await user.click(screen.getByRole("radio", { name: /openai-account/ }));

    expect(document.querySelector(".direct-applied-target")).toHaveTextContent("更改未应用");
    expect(document.querySelector(".direct-applied-target")).toHaveClass("is-draft");
    expect(screen.getByText("当前已应用：deepseek-account / deepseek-chat"))
      .toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /openai-account/ })).toBeChecked();
  });

  it("lists every provider once with its brand and only its managed models", async () => {
    const user = userEvent.setup();
    render(
      <DirectRoutePanel
        providers={providers}
        target={null}
        busy={false}
        applying={false}
        onApply={vi.fn()}
      />,
    );

    const rows = screen.getAllByRole("radio");
    expect(rows).toHaveLength(3);
    expect(document.querySelector('[data-provider-brand="deepseek"]')).toBeInTheDocument();
    expect(document.querySelector('[data-provider-brand="deepseek"]')).toHaveAttribute("aria-hidden", "true");
    expect(document.querySelector('[data-provider-brand="openai"]')).toBeInTheDocument();
    expect(document.querySelector('[data-provider-brand="empty-account"]')).toBeNull();
    const emptyRow = screen.getByRole("radio", { name: /empty-account/ }).closest(".direct-provider-row");
    if (!emptyRow) throw new Error("empty provider row missing");
    expect(within(emptyRow as HTMLElement).getByText("E", { selector: ".brand-fallback" }))
      .toBeInTheDocument();
    expect(emptyRow.querySelector('[data-provider-artwork="fallback"]')).toBeInTheDocument();

    const openAiRadio = screen.getByRole("radio", { name: /openai-account/ });
    expect(openAiRadio).toHaveClass("direct-provider-radio");
    const openAiRow = openAiRadio.closest(".direct-provider-row");
    if (!openAiRow) throw new Error("openai direct row missing");
    await user.click(within(openAiRow as HTMLElement).getByText("openai-account"));
    expect(openAiRadio).toBeChecked();
    expect(openAiRow).toHaveClass("selected");
    expect(openAiRow?.querySelectorAll(".direct-selected-mark")).toHaveLength(1);

    await user.click(screen.getByRole("combobox", { name: "openai-account 模型" }));
    expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual([
      "gpt-5.6",
      "gpt-5.6-mini",
    ]);
    expect(screen.queryByRole("option", { name: "deepseek-chat" })).toBeNull();
  });

  it("shows provider identity before the model selector without manual reorder controls", () => {
    const { container } = render(
      <DirectRoutePanel
        providers={providers}
        target={null}
        busy={false}
        applying={false}
        onApply={vi.fn()}
      />,
    );

    const row = screen.getByRole("radio", { name: /openai-account/ }).closest(".direct-provider-row");
    if (!row) throw new Error("openai direct row missing");
    const provider = row.querySelector(".direct-provider-copy");
    const model = within(row as HTMLElement).getByRole("combobox", { name: "openai-account 模型" });
    if (!provider) throw new Error("openai provider identity missing");

    expect(provider.compareDocumentPosition(model) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(container.querySelector(".direct-drag-handle")).toBeNull();
    expect(container.querySelector("[aria-roledescription]")).toBeNull();
  });

  it("applies the selected provider and model", async () => {
    const onApply = vi.fn();
    const user = userEvent.setup();
    render(
      <DirectRoutePanel
        providers={providers}
        target={null}
        busy={false}
        applying={false}
        onApply={onApply}
      />,
    );

    await user.click(screen.getByRole("radio", { name: /openai-account/ }));
    await user.click(screen.getByRole("combobox", { name: "openai-account 模型" }));
    await user.click(screen.getByRole("option", { name: "gpt-5.6-mini" }));
    expect(onApply).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "应用" }));
    expect(onApply).toHaveBeenCalledOnce();
    expect(onApply).toHaveBeenCalledWith("openai-account", "gpt-5.6-mini");
  });

  it("moves a successfully applied provider to the top and persists the new order", async () => {
    const user = userEvent.setup();
    const onApply = vi.fn().mockResolvedValue(true);
    render(
      <DirectRoutePanel
        providers={providers}
        target={{ upstream: "deepseek-account", model: "deepseek-chat" }}
        busy={false}
        applying={false}
        onApply={onApply}
      />,
    );

    await user.click(screen.getByRole("radio", { name: /openai-account/ }));
    await user.click(screen.getByRole("button", { name: "应用" }));

    await waitFor(() => expect(screen.getAllByRole("radio")[0]).toHaveAccessibleName(/openai-account/));
    expect(onApply).toHaveBeenCalledWith("openai-account", "gpt-5.6");
    expect(JSON.parse(window.localStorage.getItem(DIRECT_PROVIDER_ORDER_STORAGE_KEY) ?? "[]"))
      .toEqual(["openai-account", "deepseek-account", "empty-account"]);
  });

  it("animates changed row positions after a successful apply", async () => {
    const user = userEvent.setup();
    const rect = (top: number) => ({
      x: 0,
      y: top,
      top,
      right: 600,
      bottom: top + 64,
      left: 0,
      width: 600,
      height: 64,
      toJSON: () => ({}),
    } as DOMRect);
    let nextFrame: FrameRequestCallback | null = null;
    const requestFrame = vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => {
      nextFrame = callback;
      return 1;
    });
    const cancelFrame = vi.spyOn(window, "cancelAnimationFrame").mockImplementation(() => {});

    try {
      const { container } = render(
        <DirectRoutePanel
          providers={providers}
          target={{ upstream: "deepseek-account", model: "deepseek-chat" }}
          busy={false}
          applying={false}
          onApply={vi.fn().mockResolvedValue(true)}
        />,
      );
      const items = Array.from(container.querySelectorAll<HTMLElement>(".direct-provider-item"));
      const [deepseekItem, openAiItem, emptyItem] = items;
      let deepseekReads = 0;
      let openAiReads = 0;
      vi.spyOn(deepseekItem, "getBoundingClientRect")
        .mockImplementation(() => rect(deepseekReads++ === 0 ? 0 : 72));
      vi.spyOn(openAiItem, "getBoundingClientRect")
        .mockImplementation(() => rect(openAiReads++ === 0 ? 72 : 0));
      vi.spyOn(emptyItem, "getBoundingClientRect").mockImplementation(() => rect(144));

      await user.click(screen.getByRole("radio", { name: /openai-account/ }));
      await user.click(screen.getByRole("button", { name: "应用" }));

      expect(openAiItem.style.transform).toBe("translate(0px, 72px)");
      expect(deepseekItem.style.transform).toBe("translate(0px, -72px)");
      expect(nextFrame).not.toBeNull();

      act(() => nextFrame?.(performance.now()));
      expect(openAiItem.style.transform).toBe("");
      expect(deepseekItem.style.transform).toBe("");
    } finally {
      requestFrame.mockRestore();
      cancelFrame.mockRestore();
    }
  });

  it("keeps the provider order when apply fails", async () => {
    const user = userEvent.setup();
    const onApply = vi.fn().mockResolvedValue(false);
    render(
      <DirectRoutePanel
        providers={providers}
        target={{ upstream: "deepseek-account", model: "deepseek-chat" }}
        busy={false}
        applying={false}
        onApply={onApply}
      />,
    );

    await user.click(screen.getByRole("radio", { name: /openai-account/ }));
    await user.click(screen.getByRole("button", { name: "应用" }));

    await waitFor(() => expect(onApply).toHaveBeenCalledOnce());
    expect(screen.getAllByRole("radio")[0]).toHaveAccessibleName(/deepseek-account/);
    expect(JSON.parse(window.localStorage.getItem(DIRECT_PROVIDER_ORDER_STORAGE_KEY) ?? "[]"))
      .toEqual(["deepseek-account", "openai-account", "empty-account"]);
  });

  it("reports whether the selected direct target is still unapplied", async () => {
    const user = userEvent.setup();
    const onDraftChange = vi.fn();
    render(
      <DirectRoutePanel
        providers={providers}
        target={{ upstream: "deepseek-account", model: "deepseek-chat" }}
        busy={false}
        applying={false}
        onApply={vi.fn()}
        onDraftChange={onDraftChange}
      />,
    );

    expect(onDraftChange).toHaveBeenLastCalledWith(false);
    await user.click(screen.getByRole("combobox", { name: "deepseek-account 模型" }));
    await user.click(screen.getByRole("option", { name: "deepseek-reasoner" }));
    expect(onDraftChange).toHaveBeenLastCalledWith(true);
  });

  it("keeps an un-applied model selection across an equivalent provider refresh", async () => {
    const user = userEvent.setup();
    const { rerender } = render(
      <DirectRoutePanel
        providers={providers}
        target={{ upstream: "openai-account", model: "gpt-5.6" }}
        busy={false}
        applying={false}
        onApply={vi.fn()}
      />,
    );

    await user.click(screen.getByRole("combobox", { name: "openai-account 模型" }));
    await user.click(screen.getByRole("option", { name: "gpt-5.6-mini" }));
    expect(screen.getByRole("combobox", { name: "openai-account 模型" }))
      .toHaveTextContent("gpt-5.6-mini");

    rerender(
      <DirectRoutePanel
        providers={structuredClone(providers)}
        target={{ upstream: "openai-account", model: "gpt-5.6" }}
        busy={false}
        applying={false}
        onApply={vi.fn()}
      />,
    );

    expect(screen.getByRole("combobox", { name: "openai-account 模型" }))
      .toHaveTextContent("gpt-5.6-mini");

    rerender(
      <DirectRoutePanel
        providers={providers.map((provider) => provider.name === "openai-account"
          ? { ...provider, models: [...provider.models, "gpt-5.7"] }
          : provider)}
        target={{ upstream: "openai-account", model: "gpt-5.6" }}
        busy={false}
        applying={false}
        onApply={vi.fn()}
      />,
    );

    expect(screen.getByRole("combobox", { name: "openai-account 模型" }))
      .toHaveTextContent("gpt-5.6-mini");
  });

  it("disables every routing control while busy", () => {
    const onApply = vi.fn();
    render(
      <DirectRoutePanel
        providers={providers}
        target={{ upstream: "deepseek-account", model: "deepseek-chat" }}
        busy
        applying={false}
        onApply={onApply}
      />,
    );
    const row = screen.getByRole("radio", { name: /deepseek-account/ }).closest(".direct-provider-row");
    if (!row) throw new Error("deepseek direct row missing");

    expect(within(row as HTMLElement).getByRole("radio")).toBeDisabled();
    expect(within(row as HTMLElement).getByRole("combobox", { name: "deepseek-account 模型" }))
      .toBeDisabled();
    expect(screen.getByRole("button", { name: "应用" })).toBeDisabled();

    fireEvent.click(row);

    expect(onApply).not.toHaveBeenCalled();
  });

  it("restores the successfully promoted provider order after remounting", async () => {
    const user = userEvent.setup();
    const firstRender = render(
      <DirectRoutePanel
        providers={providers}
        target={{ upstream: "deepseek-account", model: "deepseek-chat" }}
        busy={false}
        applying={false}
        onApply={vi.fn().mockResolvedValue(true)}
      />,
    );

    await user.click(screen.getByRole("radio", { name: /openai-account/ }));
    await user.click(screen.getByRole("button", { name: "应用" }));
    await waitFor(() => expect(
      JSON.parse(window.localStorage.getItem(DIRECT_PROVIDER_ORDER_STORAGE_KEY) ?? "[]"),
    ).toEqual(["openai-account", "deepseek-account", "empty-account"]));

    firstRender.unmount();
    render(
      <DirectRoutePanel
        providers={providers}
        target={null}
        busy={false}
        applying={false}
        onApply={vi.fn()}
      />,
    );

    expect(screen.getAllByRole("radio")[0]).toHaveAccessibleName(/openai-account/);
    expect(screen.getAllByRole("radio")[1]).toHaveAccessibleName(/deepseek-account/);
  });

  it("keeps routing usable when the browser rejects provider-order storage", async () => {
    const setItem = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new DOMException("storage unavailable", "QuotaExceededError");
    });
    const onApply = vi.fn();

    try {
      render(
        <ErrorToastProvider>
          <DirectRoutePanel
            providers={providers}
            target={null}
            busy={false}
            applying={false}
            onApply={onApply}
          />
        </ErrorToastProvider>,
      );

      expect(screen.getByRole("alert")).toHaveTextContent("无法保存供应商显示顺序");

      const user = userEvent.setup();
      await user.click(screen.getByRole("radio", { name: /openai-account/ }));
      await user.click(screen.getByRole("button", { name: "应用" }));
      expect(onApply).toHaveBeenCalledWith("openai-account", "gpt-5.6");
      await waitFor(() => expect(screen.getAllByRole("radio")[0]).toHaveAccessibleName(/openai-account/));
    } finally {
      setItem.mockRestore();
    }
  });

  it("reports provider-order storage failure in Japanese", () => {
    window.localStorage.setItem("token-station-language", "ja");
    const setItem = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new DOMException("storage unavailable", "QuotaExceededError");
    });

    try {
      render(
        <ErrorToastProvider>
          <DirectRoutePanel
            providers={providers}
            target={null}
            busy={false}
            applying={false}
            onApply={vi.fn()}
          />
        </ErrorToastProvider>,
      );

      expect(screen.getByRole("alert")).toHaveTextContent("表示順を保存できませんでした");
    } finally {
      setItem.mockRestore();
    }
  });

  it("preserves a known provider without auto-selecting a replacement model", () => {
    render(
      <DirectRoutePanel
        providers={providers}
        target={{ upstream: "openai-account", model: null }}
        busy={false}
        applying={false}
        onApply={vi.fn()}
      />,
    );

    expect(screen.getByRole("radio", { name: /openai-account/ })).toBeChecked();
    expect(screen.getByRole("combobox", { name: "openai-account 模型" }))
      .toHaveTextContent("请选择");
    expect(screen.getByRole("button", { name: "应用" })).toBeDisabled();
    expect(screen.getByText("配置未完成", { selector: ".direct-applied-target" })).toBeInTheDocument();
    expect(screen.getByText("已保留供应商 openai-account；请选择模型后再应用。")).toBeInTheDocument();
  });

  it("does not infer a route from the first provider", () => {
    render(
      <DirectRoutePanel
        providers={providers}
        target={null}
        busy={false}
        applying={false}
        onApply={vi.fn()}
      />,
    );

    expect(screen.getByRole("button", { name: "应用" })).toBeDisabled();
    expect(screen.getByText("请选择一个有可用模型的供应商，再点击应用。")).toBeInTheDocument();
  });

  it("自定义供应商即使名为 openai 也只显示首字母", () => {
    render(
      <DirectRoutePanel
        providers={[{
          name: "openai",
          brand_id: null,
          provider: "openai-compatible",
          base_url: "https://custom.example/v1",
          models: ["custom-model"],
          has_auth: true,
        }]}
        target={null}
        busy={false}
        applying={false}
        onApply={vi.fn()}
      />,
    );

    const row = screen.getByRole("radio", { name: /openai/ }).closest(".direct-provider-row");
    if (!row) throw new Error("custom provider row missing");
    expect(row.querySelector('[data-provider-brand="openai"]')).toBeNull();
    expect(within(row as HTMLElement).getByText("O", { selector: ".brand-fallback" }))
      .toBeInTheDocument();
  });
});
