import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import ChatGPTPlanPanel from "./ChatGPTPlanPanel";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("./LanguageProvider", () => ({ useLocalizedCopy: () => ({ copy: (en: string) => en }) }));
const account = { id: "registration-a", email: "person@example.test", label: "Personal", state: "ready", expires_at: null };
const attempt = { id: "attempt-a", authorization_url: "https://auth.openai.com/api/accounts/authorize?state=fixture" };
const models = [{ id: "model-a", name: "Model A", context_window: null, supports_images: null }];

function mockCommands(overrides: Record<string, unknown> = {}) {
  vi.mocked(invoke).mockImplementation(async (command) => {
    const values: Record<string, unknown> = {
      chatgpt_accounts_list: [], chatgpt_login_start: attempt,
      chatgpt_login_status: { id: attempt.id, state: "pending", account_id: null, error: null },
      chatgpt_login_cancel: null, chatgpt_models: models,
      chatgpt_connect_provider: "chatgpt-plan-a", chatgpt_sign_out: { remote_revoked: true, warning: null },
      ...overrides,
    };
    return values[command] as never;
  });
}

describe("ChatGPT plan connection", () => {
  beforeEach(() => { vi.mocked(invoke).mockReset(); vi.mocked(openUrl).mockReset(); mockCommands(); });

  it("does not start authorization or inference when rendered", async () => {
    render(<ChatGPTPlanPanel />);
    await waitFor(() => expect(screen.getByRole("button", { name: "Continue with ChatGPT" })).toBeEnabled());
    expect(openUrl).not.toHaveBeenCalled();
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(screen.getByText(/share your ChatGPT plan allowance/)).toBeInTheDocument();
  });

  it("opens the official authorization page and cancels with focus restored", async () => {
    render(<ChatGPTPlanPanel />);
    const signIn = screen.getByRole("button", { name: "Continue with ChatGPT" });
    await waitFor(() => expect(signIn).toBeEnabled());
    fireEvent.click(signIn);
    const cancel = await screen.findByRole("button", { name: "Cancel sign-in" });
    await waitFor(() => expect(openUrl).toHaveBeenCalledWith(attempt.authorization_url));
    fireEvent.click(cancel);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("chatgpt_login_cancel", { attemptId: attempt.id }));
    await waitFor(() => expect(signIn).toHaveFocus());
  });

  it("refreshes accounts after completed authorization", async () => {
    mockCommands({ chatgpt_login_status: { id: attempt.id, state: "complete", account_id: account.id, error: null } });
    render(<ChatGPTPlanPanel />);
    await waitFor(() => expect(screen.getByRole("button", { name: "Continue with ChatGPT" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Continue with ChatGPT" }));
    expect(await screen.findByText("ChatGPT sign-in completed.")).toBeInTheDocument();
    expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "chatgpt_accounts_list")).toHaveLength(2);
  });

  it("keeps completion visible when refreshing accounts is asynchronous", async () => {
    let finish!: (value: unknown) => void;
    let reads = 0;
    vi.mocked(invoke).mockImplementation(async command => {
      if (command === "chatgpt_accounts_list") {
        reads += 1;
        return (reads === 1 ? [] : new Promise(resolve => { finish = resolve; })) as never;
      }
      if (command === "chatgpt_login_start") return attempt as never;
      return { id: attempt.id, state: "complete", account_id: account.id, error: null } as never;
    });
    render(<ChatGPTPlanPanel />);
    await waitFor(() => expect(screen.getByRole("button", { name: "Continue with ChatGPT" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Continue with ChatGPT" }));
    await waitFor(() => expect(reads).toBe(2));
    await act(async () => finish([account]));
    expect(await screen.findByText("ChatGPT sign-in completed.")).toBeInTheDocument();
    expect(screen.getByText("person@example.test")).toBeInTheDocument();
  });

  it("shows a safe failed-login message without rendering backend credentials", async () => {
    mockCommands({ chatgpt_login_status: { id: attempt.id, state: "failed", error: "access_token=secret-fixture" } });
    render(<ChatGPTPlanPanel />);
    await waitFor(() => expect(screen.getByRole("button", { name: "Continue with ChatGPT" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Continue with ChatGPT" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Sign-in failed. Try again.");
    expect(document.body.textContent).not.toContain("secret-fixture");
  });

  it("rejects an authorization URL outside the fixed issuer", async () => {
    mockCommands({ chatgpt_login_start: { ...attempt, authorization_url: "https://auth.openai.com.evil.test/authorize" } });
    render(<ChatGPTPlanPanel />);
    await waitFor(() => expect(screen.getByRole("button", { name: "Continue with ChatGPT" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Continue with ChatGPT" }));
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(openUrl).not.toHaveBeenCalled();
    expect(invoke).toHaveBeenCalledWith("chatgpt_login_cancel", { attemptId: attempt.id });
  });

  it("connects an existing registration without another browser authorization", async () => {
    mockCommands({ chatgpt_accounts_list: [account] });
    const changed = vi.fn();
    render(<ChatGPTPlanPanel onChanged={changed} />);
    expect(await screen.findByText("Personal")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Refresh models" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Disconnect connection" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Use this account" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("chatgpt_connect_provider", { accountId: account.id }));
    await waitFor(() => expect(changed).toHaveBeenCalledTimes(1));
    expect(await screen.findByText(/Select this provider on the Agents page/)).toBeInTheDocument();
    expect(openUrl).not.toHaveBeenCalled();
  });

  it("disables inference for identity-only grants and reuses the registration for sign-in", async () => {
    mockCommands({ chatgpt_accounts_list: [{ ...account, state: "identity_only" }] });
    render(<ChatGPTPlanPanel />);
    expect(await screen.findByRole("button", { name: "Use this account" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Sign in again" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("chatgpt_login_start", { accountId: account.id }));
  });

  it("distinguishes local sign-out from unconfirmed remote revocation", async () => {
    mockCommands({ chatgpt_accounts_list: [account], chatgpt_sign_out: { remote_revoked: false, warning: "secret-fixture" } });
    render(<ChatGPTPlanPanel provider={{ name: "chatgpt_registration-a", provider: "chatgpt-plan", credential_reference: account.id, base_url: "https://api.openai.com/v1", has_auth: true, models: ["model-a"] }} />);
    fireEvent.click(await screen.findByRole("button", { name: "Disconnect connection" }));
    expect(invoke).not.toHaveBeenCalledWith("chatgpt_sign_out", expect.anything());
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Disconnect connection" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Remote revocation is unconfirmed");
    expect(document.body.textContent).not.toContain("secret-fixture");
  });

  it("cancels a pending listener on unmount", async () => {
    const { unmount } = render(<ChatGPTPlanPanel />);
    await waitFor(() => expect(screen.getByRole("button", { name: "Continue with ChatGPT" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Continue with ChatGPT" }));
    await screen.findByRole("button", { name: "Cancel sign-in" });
    await act(async () => unmount());
    expect(invoke).toHaveBeenCalledWith("chatgpt_login_cancel", { attemptId: attempt.id });
  });
});


it("manages only the exact provider registration when account emails match", async () => {
  mockCommands({ chatgpt_accounts_list: [account, { ...account, id: "registration-b", label: "Second workspace" }] });
  render(<ChatGPTPlanPanel provider={{ name: "custom-plan", provider: "chatgpt-plan", credential_source: "chatgpt_account", credential_reference: "registration-b", base_url: "https://api.openai.com/v1", has_auth: true, models: ["managed-model"] }} />);
  expect(await screen.findByText("Second workspace")).toBeInTheDocument();
  expect(screen.queryByText("Personal")).toBeNull();
  expect(screen.getByText("managed-model")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Use this account" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Refresh models" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("chatgpt_models", { accountId: "registration-b" }));
  expect(invoke).not.toHaveBeenCalledWith("chatgpt_connect_provider", expect.anything());
});

it("connects the exact completed registration before returning to the model collection", async () => {
  mockCommands({ chatgpt_login_status: { id: attempt.id, state: "complete", account_id: account.id, error: null } });
  const changed = vi.fn().mockResolvedValue(undefined);
  const connected = vi.fn();
  render(<ChatGPTPlanPanel onChanged={changed} onConnected={connected} />);
  await waitFor(() => expect(screen.getByRole("button", { name: "Continue with ChatGPT" })).toBeEnabled());
  fireEvent.click(screen.getByRole("button", { name: "Continue with ChatGPT" }));
  await waitFor(() => expect(connected).toHaveBeenCalledOnce());
  expect(invoke).toHaveBeenCalledWith("chatgpt_connect_provider", { accountId: "registration-a" });
  expect(changed).toHaveBeenCalledOnce();
  expect(changed.mock.invocationCallOrder[0]).toBeLessThan(connected.mock.invocationCallOrder[0]);
});

it("does not leave setup when provider connection fails after sign-in", async () => {
  mockCommands({ chatgpt_login_status: { id: attempt.id, state: "complete", account_id: account.id, error: null } });
  const fallback = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "chatgpt_connect_provider") throw new Error("access_token=SECRET_FIXTURE");
    return fallback(command, args);
  });
  const connected = vi.fn();
  render(<ChatGPTPlanPanel onConnected={connected} />);
  await waitFor(() => expect(screen.getByRole("button", { name: "Continue with ChatGPT" })).toBeEnabled());
  fireEvent.click(screen.getByRole("button", { name: "Continue with ChatGPT" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Connect the account again");
  expect(connected).not.toHaveBeenCalled();
  expect(document.body.textContent).not.toContain("SECRET_FIXTURE");
});

it("refuses an authorization result for another managed registration", async () => {
  mockCommands({
    chatgpt_accounts_list: [{ ...account, state: "reauthentication_required" }],
    chatgpt_login_status: { id: attempt.id, state: "complete", account_id: "another-registration", error: null },
  });
  render(<ChatGPTPlanPanel provider={{ name: "chatgpt_registration-a", provider: "chatgpt-plan", credential_reference: account.id, base_url: "https://api.openai.com/v1", has_auth: true, models: ["model-a"] }} />);
  fireEvent.click(await screen.findByRole("button", { name: "Sign in again" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("expected account");
  expect(invoke).not.toHaveBeenCalledWith("chatgpt_connect_provider", expect.anything());
});
