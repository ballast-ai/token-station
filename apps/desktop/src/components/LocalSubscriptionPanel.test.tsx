import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import LocalSubscriptionPanel from "./LocalSubscriptionPanel";
import { LanguageProvider } from "./LanguageProvider";
import { connectSubscription, discoverSubscription, listSubscriptions } from "../localSubscriptionsApi";

vi.mock("../localSubscriptionsApi", () => ({
  connectSubscription: vi.fn(), discoverSubscription: vi.fn(), listSubscriptions: vi.fn(),
  getSubscriptionModels: vi.fn(), disconnectSubscription: vi.fn(),
}));
beforeEach(() => {
  vi.mocked(listSubscriptions).mockResolvedValue([]);
  vi.mocked(discoverSubscription).mockResolvedValue({ id:"local1",label:"Local Copilot",email:null,state:"ready" });
  vi.mocked(connectSubscription).mockResolvedValue("provider1");
});
it("blocks a missing local login and explains how to recover", async () => {
  vi.mocked(discoverSubscription).mockResolvedValue({ id:"",label:"Copilot",email:null,state:"missing" });
  render(<LanguageProvider><LocalSubscriptionPanel kind="copilot" onChanged={vi.fn()} /></LanguageProvider>);
  expect(await screen.findByText("请先在本机登录 Copilot，然后重新检查。")).toBeInTheDocument();
  expect(screen.getByRole("button", { name:"使用本机登录接入" })).toBeDisabled();
  expect(connectSubscription).not.toHaveBeenCalled();
});
it("connects a ready login and refreshes App state before closing", async () => {
  const user=userEvent.setup(); const changed=vi.fn().mockResolvedValue(undefined); const connected=vi.fn();
  render(<LanguageProvider><LocalSubscriptionPanel kind="copilot" onChanged={changed} onConnected={connected} /></LanguageProvider>);
  const button=screen.getByRole("button", { name:"使用本机登录接入" });
  await waitFor(() => expect(button).toBeEnabled());
  await user.click(button);
  expect(connectSubscription).toHaveBeenCalledWith("copilot",undefined);
  expect(changed).toHaveBeenCalledOnce(); expect(connected).toHaveBeenCalledOnce();
  expect(changed.mock.invocationCallOrder[0]).toBeLessThan(connected.mock.invocationCallOrder[0]);
});
it("keeps a failed connection open without rendering raw backend data", async () => {
  vi.mocked(connectSubscription).mockRejectedValue(new Error("fixture-secret"));
  const user=userEvent.setup(); const connected=vi.fn();
  render(<LanguageProvider><LocalSubscriptionPanel kind="copilot" onChanged={vi.fn()} onConnected={connected} /></LanguageProvider>);
  const button=screen.getByRole("button", { name:"使用本机登录接入" });
  await waitFor(() => expect(button).toBeEnabled()); await user.click(button);
  expect(await screen.findByRole("alert")).toHaveTextContent("无法连接 Copilot");
  expect(screen.queryByText(/fixture-secret/)).toBeNull(); expect(connected).not.toHaveBeenCalled();
});
