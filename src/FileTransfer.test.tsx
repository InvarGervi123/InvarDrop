// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { FileTransfer, formatBytes } from "./FileTransfer";
import type { SecureConnection } from "./useSecurity";
afterEach(cleanup);
const connection = (status = "awaiting_approval", id = "one"): SecureConnection => ({
  id: "connection", phase: "connected", peer: "abc", code: null, localApproved: true, canReconnect: false, error: null,
  transfer: { id, name: "photo.png", senderName: "InvarDrop sender", size: 1536, direction: "incoming", status, path: null, error: null },
});
it("renders sender, filename and readable size as text", () => {
  render(<FileTransfer connection={connection()} operate={vi.fn()} />);
  expect(screen.getByText("InvarDrop sender")).toBeTruthy();
  expect(screen.getByText("photo.png")).toBeTruthy();
  expect(screen.getByText("1.5 KiB · Incoming")).toBeTruthy();
// Cold DOM/ICU initialization can exceed the default 5 s on hosted Windows.
}, 15_000);
it("requires explicit confirmation and never approves on checkbox alone", async () => {
  const operate = vi.fn().mockResolvedValue(true);
  render(<FileTransfer connection={connection()} operate={operate} />);
  const accept = screen.getByRole("button", { name: "Accept file" }) as HTMLButtonElement;
  expect(accept.disabled).toBe(true); fireEvent.click(accept); expect(operate).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("checkbox")); expect(operate).not.toHaveBeenCalled();
  fireEvent.click(accept); await waitFor(() => expect(operate).toHaveBeenCalledWith("file_respond", { id: "connection", transferId: "one", accept: true }));
  fireEvent.click(accept); fireEvent.click(screen.getByText("Decline file")); expect(operate).toHaveBeenCalledTimes(1);
});
it("declines without checking confirmation", async () => {
  const operate = vi.fn().mockResolvedValue(true);
  render(<FileTransfer connection={connection()} operate={operate} />);
  fireEvent.click(screen.getByText("Decline file"));
  await waitFor(() => expect(operate).toHaveBeenCalledWith("file_respond", { id: "connection", transferId: "one", accept: false }));
});
it("blocks duplicate pending actions and allows retry after failed IPC", async () => {
  let resolve!: (ok: boolean) => void;
  const operate = vi.fn().mockImplementation(() => new Promise<boolean>(done => { resolve = done; }));
  render(<FileTransfer connection={connection()} operate={operate} />);
  fireEvent.click(screen.getByText("Decline file")); fireEvent.click(screen.getByText("Decline file"));
  expect(operate).toHaveBeenCalledTimes(1); resolve(false);
  await waitFor(() => expect((screen.getByText("Decline file") as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByText("Decline file")); expect(operate).toHaveBeenCalledTimes(2);
});
it("does not reuse confirmation for a new request", () => {
  const operate = vi.fn(); const view = render(<FileTransfer connection={connection()} operate={operate} />);
  fireEvent.click(screen.getByRole("checkbox"));
  view.rerender(<FileTransfer connection={connection("awaiting_approval", "two")} operate={operate} />);
  expect((screen.getByRole("checkbox") as HTMLInputElement).checked).toBe(false);
  fireEvent.click(screen.getByText("Accept file")); expect(operate).not.toHaveBeenCalled();
});
describe.each([["receiving", "Accepted · receiving file…"], ["rejected", "Declined"], ["expired", "Request expired"], ["complete", "Completed · BLAKE3 verified"], ["error", "Transfer failed"]])("state %s", (status, label) => {
  it("shows state and removes approval controls", () => {
    render(<FileTransfer connection={connection(status)} operate={vi.fn()} />);
    expect(screen.getByRole("status").textContent).toBe(label);
    expect(screen.queryByText("Accept file")).toBeNull();
  });
});
it("formats empty and large files", () => { expect(formatBytes(0)).toBe("0 B"); expect(formatBytes(4 * 1024 ** 3)).toBe("4 GiB"); });
it("renders a device-reported name as text, never HTML", () => {
  const c = connection(); c.transfer!.senderName = "<img src=x onerror=alert(1)>";
  const view = render(<FileTransfer connection={c} operate={vi.fn()} />);
  expect(screen.getByText("<img src=x onerror=alert(1)>")).toBeTruthy();
  expect(view.container.querySelector("img")).toBeNull();
});
it("never offers acceptance on a closed connection", () => {
  const c = connection(); c.phase = "closed";
  render(<FileTransfer connection={c} operate={vi.fn()} />);
  expect(screen.queryByText("Accept file")).toBeNull();
  expect(screen.queryByRole("checkbox")).toBeNull();
});
