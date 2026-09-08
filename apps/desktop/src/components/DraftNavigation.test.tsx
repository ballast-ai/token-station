import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { DraftNavigationBoundary, useDraftGuard, useDraftNavigation } from "./DraftNavigation";
function Editor({ leave }: { leave: () => void }) {
  useDraftGuard(true);
  const navigate = useDraftNavigation();
  return <button onClick={() => navigate(leave)}>leave</button>;
}
it("preserves a dirty editor on cancel and restores navigation focus", async () => {
  const user = userEvent.setup(); const leave = vi.fn();
  render(<DraftNavigationBoundary><Editor leave={leave} /></DraftNavigationBoundary>);
  await user.click(screen.getByRole("button", { name: "leave" }));
  expect(leave).not.toHaveBeenCalled();
  await user.click(screen.getByRole("button", { name: "继续编辑" }));
  expect(leave).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "leave" })).toHaveFocus();
  await user.click(screen.getByRole("button", { name: "leave" }));
  await user.click(screen.getByRole("button", { name: "放弃更改并离开" }));
  expect(leave).toHaveBeenCalledTimes(1);
});
