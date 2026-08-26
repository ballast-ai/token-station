import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
    expect(within(openAiRow as HTMLElement).getByRole("button", { name: /调整 openai-account 顺序/ }))
      .toHaveAttribute("aria-roledescription", "可排序项");

    await user.click(screen.getByRole("combobox", { name: "openai-account 模型" }));
    expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual([
      "gpt-5.6",
      "gpt-5.6-mini",
    ]);
    expect(screen.queryByRole("option", { name: "deepseek-chat" })).toBeNull();
  });

  it("applies the selected provider and model even after keyboard reordering", async () => {
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

    const handle = screen.getByRole("button", { name: /调整 openai-account 顺序/ });
    await user.click(handle);
    await user.keyboard("{ArrowUp}");
    expect(screen.getAllByRole("radio")[0]).toHaveAccessibleName(/openai-account/);
    expect(onApply).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "应用" }));
    expect(onApply).toHaveBeenCalledOnce();
    expect(onApply).toHaveBeenCalledWith("openai-account", "gpt-5.6-mini");
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

  it("uses one accessible sortable handle and keeps the provider model bound while moving", async () => {
    const onApply = vi.fn();
    const user = userEvent.setup();
    render(
      <DirectRoutePanel
        providers={providers}
        target={{ upstream: "deepseek-account", model: "deepseek-chat" }}
        busy={false}
        applying={false}
        onApply={onApply}
      />,
    );

    const handle = screen.getByRole("button", { name: /调整 deepseek-account 顺序/ });
    expect(handle).toHaveAttribute("aria-roledescription", "可排序项");
    expect(handle).not.toHaveAttribute("draggable");

    await user.click(screen.getByRole("combobox", { name: "deepseek-account 模型" }));
    await user.click(screen.getByRole("option", { name: "deepseek-reasoner" }));
    await user.click(handle);
    await user.keyboard("{ArrowDown}");

    const movedRow = screen.getAllByRole("radio")[1].closest(".direct-provider-row");
    if (!movedRow) throw new Error("moved deepseek row missing");
    expect(within(movedRow as HTMLElement).getByRole("radio", { name: /deepseek-account/ })).toBeChecked();
    expect(within(movedRow as HTMLElement).getByRole("combobox", { name: "deepseek-account 模型" }))
      .toHaveTextContent("deepseek-reasoner");
    expect(movedRow.querySelector('[data-provider-brand="deepseek"]')).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /调整 deepseek-account 顺序/ })).toHaveFocus();
    expect(screen.getByText("已将 deepseek-account 移到第 2 项，共 3 项。"))
      .toHaveAttribute("role", "status");
    expect(onApply).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "应用" }));
    expect(onApply).toHaveBeenCalledWith("deepseek-account", "deepseek-reasoner");
  });

  it("does not enter pointer dragging before the activation distance and cancels cleanly", async () => {
    render(
      <DirectRoutePanel
        providers={providers}
        target={null}
        busy={false}
        applying={false}
        onApply={vi.fn()}
      />,
    );

    const handle = screen.getByRole("button", { name: /调整 deepseek-account 顺序/ });
    const row = screen.getByRole("radio", { name: /deepseek-account/ }).closest(".direct-provider-row");
    if (!row) throw new Error("deepseek direct row missing");
    const initialOrder = screen.getAllByRole("radio").map((radio) => radio.getAttribute("aria-label"));

    fireEvent.pointerDown(handle, {
      pointerId: 1,
      button: 0,
      isPrimary: true,
      clientX: 20,
      clientY: 20,
    });
    fireEvent.pointerMove(document, { pointerId: 1, clientX: 20, clientY: 24 });
    expect(row).not.toHaveClass("dragging");

    fireEvent.pointerMove(document, { pointerId: 1, clientX: 20, clientY: 30 });
    await waitFor(() => expect(row).toHaveClass("dragging"));

    fireEvent.keyDown(document, { key: "Escape", code: "Escape" });
    await waitFor(() => expect(row).not.toHaveClass("dragging"));
    expect(screen.getAllByRole("radio").map((radio) => radio.getAttribute("aria-label")))
      .toEqual(initialOrder);
    expect(JSON.parse(window.localStorage.getItem(DIRECT_PROVIDER_ORDER_STORAGE_KEY) ?? "[]"))
      .toEqual(["deepseek-account", "openai-account", "empty-account"]);
  });

  it("starts pointer sorting only from the dedicated drag handle", () => {
    const { container } = render(
      <DirectRoutePanel
        providers={providers}
        target={null}
        busy={false}
        applying={false}
        onApply={vi.fn()}
      />,
    );
    const row = screen.getByRole("radio", { name: /deepseek-account/ }).closest(".direct-provider-row");
    if (!row) throw new Error("deepseek direct row missing");
    const targets = [
      row,
      within(row as HTMLElement).getByRole("radio"),
      row.querySelector<HTMLElement>(".direct-provider-brand"),
      within(row as HTMLElement).getByRole("combobox", { name: "deepseek-account 模型" }),
    ].filter((target): target is HTMLElement => Boolean(target));

    targets.forEach((target, index) => {
      fireEvent.pointerDown(target, {
        pointerId: index + 1,
        button: 0,
        isPrimary: true,
        clientX: 20,
        clientY: 20,
      });
      fireEvent.pointerMove(document, { pointerId: index + 1, clientX: 20, clientY: 40 });
      expect(row).not.toHaveClass("dragging");
      fireEvent.pointerUp(document, { pointerId: index + 1, clientX: 20, clientY: 40 });
    });
    expect(container.querySelector(".direct-provider-row.dragging")).toBeNull();
  });

  it("reorders rows through the pointer sensor without changing or applying the selected target", async () => {
    const onApply = vi.fn();
    const { container } = render(
      <DirectRoutePanel
        providers={providers}
        target={{ upstream: "openai-account", model: "gpt-5.6" }}
        busy={false}
        applying={false}
        onApply={onApply}
      />,
    );
    const wrappers = Array.from(container.querySelectorAll<HTMLElement>(".direct-provider-sortable"));
    wrappers.forEach((wrapper, index) => {
      const top = index * 72;
      vi.spyOn(wrapper, "getBoundingClientRect").mockReturnValue({
        x: 0,
        y: top,
        top,
        right: 600,
        bottom: top + 64,
        left: 0,
        width: 600,
        height: 64,
        toJSON: () => ({}),
      });
    });

    const handle = screen.getByRole("button", { name: /调整 deepseek-account 顺序/ });
    fireEvent.pointerDown(handle, {
      pointerId: 1,
      button: 0,
      isPrimary: true,
      clientX: 20,
      clientY: 32,
    });
    fireEvent.pointerMove(document, { pointerId: 1, clientX: 20, clientY: 42 });
    await waitFor(() => expect(handle.closest(".direct-provider-row")).toHaveClass("dragging"));
    fireEvent.pointerMove(document, { pointerId: 1, clientX: 20, clientY: 104 });
    await waitFor(() => expect(wrappers[0].style.transform).not.toBe(""));
    expect(wrappers[1].style.transform).toMatch(/-72px/);
    fireEvent.pointerUp(document, { pointerId: 1, clientX: 20, clientY: 104 });

    await waitFor(() => expect(screen.getAllByRole("radio")[1]).toHaveAccessibleName(/deepseek-account/));
    expect(Array.from(container.querySelectorAll<HTMLElement>(".direct-provider-sortable"))
      .every((wrapper) => wrapper.style.transform === "")).toBe(true);
    expect(screen.getByRole("radio", { name: /openai-account/ })).toBeChecked();
    expect(onApply).not.toHaveBeenCalled();
    expect(JSON.parse(window.localStorage.getItem(DIRECT_PROVIDER_ORDER_STORAGE_KEY) ?? "[]"))
      .toEqual(["openai-account", "deepseek-account", "empty-account"]);
  });

  it("keeps the order when pointer dragging ends outside every provider row", async () => {
    const { container } = render(
      <DirectRoutePanel
        providers={providers}
        target={null}
        busy={false}
        applying={false}
        onApply={vi.fn()}
      />,
    );
    Array.from(container.querySelectorAll<HTMLElement>(".direct-provider-sortable"))
      .forEach((wrapper, index) => {
        const top = index * 72;
        vi.spyOn(wrapper, "getBoundingClientRect").mockReturnValue({
          x: 0,
          y: top,
          top,
          right: 600,
          bottom: top + 64,
          left: 0,
          width: 600,
          height: 64,
          toJSON: () => ({}),
        });
      });
    const initialOrder = screen.getAllByRole("radio").map((radio) => radio.getAttribute("aria-label"));
    const handle = screen.getByRole("button", { name: /调整 deepseek-account 顺序/ });

    fireEvent.pointerDown(handle, {
      pointerId: 1,
      button: 0,
      isPrimary: true,
      clientX: 20,
      clientY: 32,
    });
    fireEvent.pointerMove(document, { pointerId: 1, clientX: 20, clientY: 42 });
    await waitFor(() => expect(handle.closest(".direct-provider-row")).toHaveClass("dragging"));
    fireEvent.pointerMove(document, { pointerId: 1, clientX: 700, clientY: 500 });
    fireEvent.pointerUp(document, { pointerId: 1, clientX: 700, clientY: 500 });

    await waitFor(() => expect(handle.closest(".direct-provider-row")).not.toHaveClass("dragging"));
    expect(screen.getAllByRole("radio").map((radio) => radio.getAttribute("aria-label")))
      .toEqual(initialOrder);
    expect(JSON.parse(window.localStorage.getItem(DIRECT_PROVIDER_ORDER_STORAGE_KEY) ?? "[]"))
      .toEqual(["deepseek-account", "openai-account", "empty-account"]);
  });

  it("supports standard keyboard pickup, movement, drop, and focus restoration", async () => {
    const { container } = render(
      <DirectRoutePanel
        providers={providers}
        target={null}
        busy={false}
        applying={false}
        onApply={vi.fn()}
      />,
    );
    Array.from(container.querySelectorAll<HTMLElement>(".direct-provider-sortable"))
      .forEach((wrapper, index) => {
        const top = index * 72;
        vi.spyOn(wrapper, "getBoundingClientRect").mockReturnValue({
          x: 0,
          y: top,
          top,
          right: 600,
          bottom: top + 64,
          left: 0,
          width: 600,
          height: 64,
          toJSON: () => ({}),
        });
      });

    const handle = screen.getByRole("button", { name: /调整 deepseek-account 顺序/ });
    handle.focus();
    fireEvent.keyDown(handle, { key: " ", code: "Space" });
    await waitFor(() => expect(handle.closest(".direct-provider-row")).toHaveClass("dragging"));
    expect(document.querySelector('[id^="DndLiveRegion"]'))
      .toHaveTextContent("deepseek-account 当前位于第 1 项，共 3 项。");
    fireEvent.keyDown(document, { key: "ArrowDown", code: "ArrowDown" });
    fireEvent.keyDown(document, { key: " ", code: "Space" });

    await waitFor(() => expect(screen.getAllByRole("radio")[1]).toHaveAccessibleName(/deepseek-account/));
    expect(document.querySelector('[id^="DndLiveRegion"]'))
      .toHaveTextContent("已将 deepseek-account 放到第 2 项，共 3 项。");
    expect(screen.getByRole("button", { name: /调整 deepseek-account 顺序/ })).toHaveFocus();
  });

  it("supports Enter pickup and drop with focus restoration", async () => {
    const { container } = render(
      <DirectRoutePanel
        providers={providers}
        target={null}
        busy={false}
        applying={false}
        onApply={vi.fn()}
      />,
    );
    Array.from(container.querySelectorAll<HTMLElement>(".direct-provider-sortable"))
      .forEach((wrapper, index) => {
        const top = index * 72;
        vi.spyOn(wrapper, "getBoundingClientRect").mockReturnValue({
          x: 0,
          y: top,
          top,
          right: 600,
          bottom: top + 64,
          left: 0,
          width: 600,
          height: 64,
          toJSON: () => ({}),
        });
      });
    const handle = screen.getByRole("button", { name: /调整 deepseek-account 顺序/ });
    handle.focus();

    fireEvent.keyDown(handle, { key: "Enter", code: "Enter" });
    await waitFor(() => expect(handle.closest(".direct-provider-row")).toHaveClass("dragging"));
    fireEvent.keyDown(document, { key: "ArrowDown", code: "ArrowDown" });
    fireEvent.keyDown(document, { key: "Enter", code: "Enter" });

    await waitFor(() => expect(screen.getAllByRole("radio")[1]).toHaveAccessibleName(/deepseek-account/));
    expect(screen.getByRole("button", { name: /调整 deepseek-account 顺序/ })).toHaveFocus();
  });

  it("supports Enter pickup and Escape cancellation with localized instructions", async () => {
    const { container } = render(
      <DirectRoutePanel
        providers={providers}
        target={null}
        busy={false}
        applying={false}
        onApply={vi.fn()}
      />,
    );
    Array.from(container.querySelectorAll<HTMLElement>(".direct-provider-sortable"))
      .forEach((wrapper, index) => {
        const top = index * 72;
        vi.spyOn(wrapper, "getBoundingClientRect").mockReturnValue({
          x: 0,
          y: top,
          top,
          right: 600,
          bottom: top + 64,
          left: 0,
          width: 600,
          height: 64,
          toJSON: () => ({}),
        });
      });
    const handle = screen.getByRole("button", { name: /调整 deepseek-account 顺序/ });
    expect(handle).toHaveAccessibleDescription(/按空格或回车拾取该供应商/);
    handle.focus();

    fireEvent.keyDown(handle, { key: "Enter", code: "Enter" });
    await waitFor(() => expect(handle.closest(".direct-provider-row")).toHaveClass("dragging"));
    fireEvent.keyDown(document, { key: "ArrowDown", code: "ArrowDown" });
    fireEvent.keyDown(document, { key: "Escape", code: "Escape" });

    await waitFor(() => expect(handle.closest(".direct-provider-row")).not.toHaveClass("dragging"));
    expect(screen.getAllByRole("radio")[0]).toHaveAccessibleName(/deepseek-account/);
    expect(handle).toHaveFocus();
  });

  it("does not wrap keyboard reordering at either end of the list", async () => {
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

    const initialOrder = screen.getAllByRole("radio").map((radio) => radio.getAttribute("aria-label"));
    await user.click(screen.getByRole("button", { name: /调整 deepseek-account 顺序/ }));
    await user.keyboard("{ArrowUp}");
    await user.click(screen.getByRole("button", { name: /调整 empty-account 顺序/ }));
    await user.keyboard("{ArrowDown}");

    expect(screen.getAllByRole("radio").map((radio) => radio.getAttribute("aria-label")))
      .toEqual(initialOrder);
  });

  it("disables every routing and sorting control while busy", () => {
    const onApply = vi.fn();
    const { container } = render(
      <DirectRoutePanel
        providers={providers}
        target={{ upstream: "deepseek-account", model: "deepseek-chat" }}
        busy
        applying={false}
        onApply={onApply}
      />,
    );
    const handle = screen.getByRole("button", { name: /调整 deepseek-account 顺序/ });
    const row = screen.getByRole("radio", { name: /deepseek-account/ }).closest(".direct-provider-row");
    if (!row) throw new Error("deepseek direct row missing");

    expect(handle).toBeDisabled();
    expect(within(row as HTMLElement).getByRole("radio")).toBeDisabled();
    expect(within(row as HTMLElement).getByRole("combobox", { name: "deepseek-account 模型" }))
      .toBeDisabled();
    expect(screen.getByRole("button", { name: "应用" })).toBeDisabled();

    fireEvent.pointerDown(handle, {
      pointerId: 1,
      button: 0,
      isPrimary: true,
      clientX: 20,
      clientY: 20,
    });
    fireEvent.pointerMove(document, { pointerId: 1, clientX: 20, clientY: 40 });
    fireEvent.click(row);

    expect(container.querySelector(".direct-provider-row.dragging")).toBeNull();
    expect(onApply).not.toHaveBeenCalled();
  });

  it("restores the persisted provider order after remounting", async () => {
    const user = userEvent.setup();
    const firstRender = render(
      <DirectRoutePanel
        providers={providers}
        target={null}
        busy={false}
        applying={false}
        onApply={vi.fn()}
      />,
    );

    await user.click(screen.getByRole("button", { name: /调整 openai-account 顺序/ }));
    await user.keyboard("{ArrowUp}");
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
      const handle = screen.getByRole("button", { name: /调整 openai-account 顺序/ });
      await user.click(handle);
      await user.keyboard("{ArrowUp}");
      expect(screen.getAllByRole("radio")[0]).toHaveAccessibleName(/openai-account/);

      await user.click(screen.getByRole("radio", { name: /openai-account/ }));
      await user.click(screen.getByRole("button", { name: "应用" }));
      expect(onApply).toHaveBeenCalledWith("openai-account", "gpt-5.6");
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
