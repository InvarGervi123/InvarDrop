// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { FileTransfer, formatBytes, progressPercent } from "./FileTransfer";
import type { SecureConnection } from "./useSecurity";
afterEach(cleanup);
const connection = (status = "awaiting_approval", id = "one"): SecureConnection => ({
  id: "connection", phase: "connected", peer: "abc", code: null, localApproved: true, canReconnect: false, error: null,
  transfer: { id, name: "photo.png", senderName: "InvarDrop sender", size: 1536, direction: "incoming", transferred: 0, bytesPerSecond: null, etaSeconds: null, status, path: null, error: null },
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
describe.each([["receiving", "Transferring · receiving file…"], ["rejected", "Declined"], ["expired", "Request expired"], ["complete", "Completed · BLAKE3 verified"], ["error", "Transfer failed"]])("state %s", (status, label) => {
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

it("shows measured progress, speed and reliable ETA", () => {
  const c=connection("receiving"); c.transfer!.transferred=768; c.transfer!.bytesPerSecond=2*1024**2; c.transfer!.etaSeconds=4;
  render(<FileTransfer connection={c} operate={vi.fn()} />);
  expect((screen.getByRole("progressbar") as HTMLProgressElement).value).toBe(50);
  expect(screen.getByText("50% · 768 B / 1.5 KiB")).toBeTruthy();
  expect(screen.getByText(/2.00 MiB\/s/)).toBeTruthy(); expect(screen.getByText(/ETA ~4 s/)).toBeTruthy();
});
it("handles zero-byte progress and does not invent an ETA", () => {
  const c=connection("receiving"); c.transfer!.size=0;
  expect(progressPercent(c.transfer!)).toBe(0); c.transfer!.status="complete"; expect(progressPercent(c.transfer!)).toBe(100);
  render(<FileTransfer connection={c} operate={vi.fn()} />); expect(screen.queryByText(/ETA/)).toBeNull();
});
it.each(["incoming","outgoing"] as const)("cancels %s transfer only once", async direction => {
  const c=connection("receiving"); c.transfer!.direction=direction;
  const operate=vi.fn().mockResolvedValue(true);
  render(<FileTransfer connection={c} operate={operate} />);
  fireEvent.click(screen.getByText("Cancel transfer"));
  await waitFor(()=>expect(operate).toHaveBeenCalledWith("file_cancel",{id:"connection",transferId:"one"}));
  fireEvent.click(screen.getByText("Cancel transfer")); expect(operate).toHaveBeenCalledTimes(1);
});
it.each(["cancelling","cancelled","committing","complete","error"])("does not offer another cancellation in %s", status=>{
  render(<FileTransfer connection={connection(status)} operate={vi.fn()} />);
  const button=screen.queryByText("Cancel transfer") as HTMLButtonElement|null;
  expect(button===null || button.disabled).toBe(true);
});
it("progress is monotonic across actual byte samples",()=>{
  const t=connection().transfer!; let previous=0;
  for(const n of [0,1,512,1024,1536]) {t.transferred=n;const p=progressPercent(t);expect(p).toBeGreaterThanOrEqual(previous);expect(p).toBeLessThanOrEqual(100);previous=p;}
  expect(previous).toBe(100);
});
