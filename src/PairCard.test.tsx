// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { PairCard } from "./PairCard";
import type { SecureConnection } from "./useSecurity";
afterEach(cleanup);
const connection = (): SecureConnection => ({ id: "pair", phase: "verifying", peer: "a".repeat(64), code: "0000 1111 2222 3333 4444 5555 6666 7777", supportsResume: true, supportsBundles: true, localApproved: false, canReconnect: false, error: null, transfer: null });
it("marks pairing as untrusted and requires a full-code confirmation", () => {
  const operate = vi.fn(); render(<PairCard connection={connection()} operate={operate} />);
  expect(screen.getByText("Not trusted yet")).toBeTruthy();
  const approve = screen.getByRole("button", { name: "Codes match — pair" }) as HTMLButtonElement;
  expect(approve.disabled).toBe(true); expect(screen.queryByRole("button", { name: /send/ })).toBeNull();
  expect(document.activeElement).not.toBe(approve); fireEvent.click(approve); expect(operate).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("checkbox")); expect(operate).not.toHaveBeenCalled();
});
it("never reuses confirmation after the comparison code changes", () => {
  const c = connection(); const operate = vi.fn(); const view = render(<PairCard connection={c} operate={operate} />);
  fireEvent.click(screen.getByRole("checkbox"));
  view.rerender(<PairCard connection={{ ...c, code: "8888 1111 2222 3333 4444 5555 6666 7777" }} operate={operate} />);
  expect((screen.getByRole("checkbox") as HTMLInputElement).checked).toBe(false);
  fireEvent.click(screen.getByRole("button", { name: "Codes match — pair" })); expect(operate).not.toHaveBeenCalled();
});
it("blocks duplicate and opposite decisions until the backend changes phase", async () => {
  const operate = vi.fn().mockResolvedValue(true); const c = connection();
  render(<PairCard connection={c} operate={operate} />);
  fireEvent.click(screen.getByRole("checkbox"));
  fireEvent.click(screen.getByRole("button", { name: "Codes match — pair" }));
  fireEvent.click(screen.getByRole("button", { name: "Reject" }));
  await waitFor(() => expect(screen.queryByRole("button", { name: "Reject" })).toBeNull());
  expect(operate).toHaveBeenCalledExactlyOnceWith("secure_respond", { id: c.id, approve: true, code: c.code });
});
it("allows pairing retry after an IPC failure without assuming approval", async () => {
  const operate = vi.fn().mockResolvedValueOnce(false).mockResolvedValue(true);
  render(<PairCard connection={connection()} operate={operate} />);
  fireEvent.click(screen.getByRole("checkbox")); const approve = screen.getByRole("button", { name: "Codes match — pair" }) as HTMLButtonElement;
  fireEvent.click(approve); await waitFor(() => expect(approve.disabled).toBe(false)); fireEvent.click(approve);
  await waitFor(() => expect(operate).toHaveBeenCalledTimes(2));
});
it("shows trusted status only after the authenticated connection is established", () => {
  render(<PairCard connection={{ ...connection(), phase: "connected", code: null, localApproved: true }} operate={vi.fn()} />);
  expect(screen.getByText("Trusted key")).toBeTruthy(); expect(screen.getByText(/Peer key verified over TLS 1.3/)).toBeTruthy();
  expect(screen.queryByRole("checkbox")).toBeNull(); expect(screen.getByRole("button", { name: "Choose files to send" })).toBeTruthy();
});
it("offers pinned reconnect and actionable guidance after disconnection", () => {
  render(<PairCard connection={{ ...connection(), phase: "closed", code: null, canReconnect: true, error: "Connection failed or disconnected" }} operate={vi.fn()} />);
  expect(screen.getByRole("alert").textContent).toContain("same network");
  expect(screen.getByRole("button", { name: "Reconnect with pinned key" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Choose files to send" })).toBeNull();
});
