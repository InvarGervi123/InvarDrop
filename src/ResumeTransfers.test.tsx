// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { ResumeTransfers } from "./ResumeTransfers";
import type { SecuritySnapshot } from "./useSecurity";
afterEach(cleanup);
const peer = "a".repeat(64);
const snapshot = (): SecuritySnapshot => ({
  revision: 1, status: "ready", identity: "self", trusted: [peer], revoked: [], error: null, receiveDirectory: "/received",
  connections: [{ id: "connection", phase: "connected", peer, code: null, localApproved: true, canReconnect: true, supportsBundles: true, supportsResume: true, error: null, transfer: null }],
  resumable: [{ id: "saved", peer, name: "Holiday", size: 4096, verified: 1536, fileCount: 2, isFolder: true, direction: "incoming", requested: false }],
});
it("renders saved verified bytes, remaining bytes and total progress without starting work", () => {
  const operate = vi.fn(); render(<ResumeTransfers snapshot={snapshot()} operate={operate} />);
  expect(screen.getByText("Holiday")).toBeTruthy(); expect(screen.getByText("Receiving · 2 files · folder · 4 KiB")).toBeTruthy();
  expect(screen.getByText("1.5 KiB verified · 2.5 KiB remaining · 37.5%")).toBeTruthy();
  expect((screen.getByRole("progressbar") as HTMLProgressElement).value).toBe(37.5);
  expect(screen.getByText("Speed — · waiting to resume")).toBeTruthy(); expect(operate).not.toHaveBeenCalled();
}, 15_000);
it("requires explicit per-transfer approval and blocks duplicate pending resume actions", async () => {
  let finish!: (ok: boolean) => void;
  const operate = vi.fn().mockImplementation(() => new Promise<boolean>(resolve => { finish = resolve; }));
  render(<ResumeTransfers snapshot={snapshot()} operate={operate} />);
  fireEvent.click(screen.getByText("Resume")); expect(operate).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("checkbox")); expect(operate).not.toHaveBeenCalled();
  fireEvent.click(screen.getByText("Resume")); fireEvent.click(screen.getByText("Resume"));fireEvent.click(screen.getByText("Discard"));
  expect(operate).toHaveBeenCalledExactlyOnceWith("resume_transfer", { id: "connection", transferId: "saved" });
  expect((screen.getByText("Discard") as HTMLButtonElement).disabled).toBe(true); finish(true);
  await waitFor(() => expect(screen.getByText("Resume requested · waiting for the sender")).toBeTruthy());
  expect((screen.getByText("Resume") as HTMLButtonElement).disabled).toBe(true);
});
it("requires a second decision for Discard and allows keeping verified data", async () => {
  let finish!: (ok: boolean) => void;
  const operate = vi.fn().mockImplementation(() => new Promise<boolean>(resolve => { finish = resolve; }));
  render(<ResumeTransfers snapshot={snapshot()} operate={operate} />);
  fireEvent.click(screen.getByText("Discard")); expect(operate).not.toHaveBeenCalled();
  expect(screen.queryByText("Resume")).toBeNull(); fireEvent.click(screen.getByText("Keep")); expect(operate).not.toHaveBeenCalled();
  fireEvent.click(screen.getByText("Discard")); fireEvent.click(screen.getByText("Discard saved data"));fireEvent.click(screen.getByText("Discard saved data"));
  expect(operate).toHaveBeenCalledExactlyOnceWith("resume_discard", { transferId: "saved" });finish(true);
  await waitFor(() => expect(screen.getByText("Recovery record discarded")).toBeTruthy());
});
it("keeps offline recovery visible, permits Discard and never resumes an offline or legacy peer", () => {
  const state = snapshot(); state.connections = []; const operate = vi.fn();const view = render(<ResumeTransfers snapshot={state} operate={operate} />);
  expect(screen.getByText("Waiting for trusted device to reconnect")).toBeTruthy();expect((screen.getByText("Resume") as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByText("Resume"));expect(operate).not.toHaveBeenCalled();expect((screen.getByText("Discard") as HTMLButtonElement).disabled).toBe(false);
  const legacy=snapshot();legacy.connections[0].supportsResume=false;view.rerender(<ResumeTransfers snapshot={legacy} operate={operate}/>);
  expect(screen.getByText("Peer does not support resume")).toBeTruthy();expect((screen.getByText("Resume") as HTMLButtonElement).disabled).toBe(true);
});
it("hides records for revoked or no longer trusted keys, even on a stale connected snapshot", () => {
  const state=snapshot();state.revoked=[peer];const operate=vi.fn();const view=render(<ResumeTransfers snapshot={state} operate={operate}/>);
  expect(screen.queryByText("Holiday")).toBeNull();state.revoked=[];state.trusted=[];view.rerender(<ResumeTransfers snapshot={state} operate={operate}/>);expect(screen.queryByText("Holiday")).toBeNull();expect(operate).not.toHaveBeenCalled();
});
it("does not reuse approval after a checkpoint changes or a device reconnects", () => {
  const state=snapshot();const operate=vi.fn();const view=render(<ResumeTransfers snapshot={state} operate={operate}/>);
  fireEvent.click(screen.getByRole("checkbox"));state.resumable[0].verified=2048;view.rerender(<ResumeTransfers snapshot={state} operate={operate}/>);
  expect((screen.getByRole("checkbox") as HTMLInputElement).checked).toBe(false);fireEvent.click(screen.getByText("Resume"));expect(operate).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("checkbox"));state.connections[0].id="reconnected";view.rerender(<ResumeTransfers snapshot={state} operate={operate}/>);
  expect((screen.getByRole("checkbox") as HTMLInputElement).checked).toBe(false);fireEvent.click(screen.getByText("Resume"));expect(operate).not.toHaveBeenCalled();
});
it("allows retry after an IPC failure without accepting automatically", async () => {
  const operate=vi.fn().mockResolvedValueOnce(false).mockResolvedValue(true);render(<ResumeTransfers snapshot={snapshot()} operate={operate}/>);
  fireEvent.click(screen.getByRole("checkbox"));fireEvent.click(screen.getByText("Resume"));await waitFor(()=>expect((screen.getByText("Resume") as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByText("Resume"));await waitFor(()=>expect(operate).toHaveBeenCalledTimes(2));
});
it("blocks Resume and Discard for an active recovery operation", () => {
  const state=snapshot();state.connections[0].transfer={id:"saved",direction:"incoming",name:"Holiday",size:4096,fileCount:2,isFolder:true,resuming:true,senderName:"Sender",transferred:2048,bytesPerSecond:1024,etaSeconds:null,status:"receiving",path:null,error:null};
  const operate=vi.fn();render(<ResumeTransfers snapshot={state} operate={operate}/>);
  expect(screen.getByText("Transfer active · use Cancel to stop")).toBeTruthy();fireEvent.click(screen.getByText("Resume"));fireEvent.click(screen.getByText("Discard"));expect(operate).not.toHaveBeenCalled();
});
it("shows a peer resume request as a request, never as consent", () => {
  const state=snapshot();state.resumable[0].direction="outgoing";state.resumable[0].requested=true;const operate=vi.fn();render(<ResumeTransfers snapshot={state} operate={operate}/>);
  expect(screen.getByText("Peer requests resume")).toBeTruthy();expect((screen.getByText("Resume") as HTMLButtonElement).disabled).toBe(true);expect(operate).not.toHaveBeenCalled();
});
it("handles zero-byte operations and renders names as text", () => {
  const state=snapshot();state.resumable[0].name="<img src=x onerror=alert(1)>";state.resumable[0].size=0;state.resumable[0].verified=0;
  const view=render(<ResumeTransfers snapshot={state} operate={vi.fn()}/>);expect(screen.getByText("0 B verified · 0 B remaining · 0%")).toBeTruthy();expect(view.container.querySelector("img")).toBeNull();
});

it("requires reconnect before offering the same transfer again after a declined attempt", () => {
  const state=snapshot();state.connections[0].transfer={id:"saved",direction:"incoming",name:"Holiday",size:4096,fileCount:2,isFolder:true,resuming:true,senderName:"Sender",transferred:1536,bytesPerSecond:null,etaSeconds:null,status:"rejected",path:null,error:null};
  const operate=vi.fn();const view=render(<ResumeTransfers snapshot={state} operate={operate}/>);expect(screen.getByText("Reconnect to retry this transfer")).toBeTruthy();
  fireEvent.click(screen.getByText("Resume"));expect(operate).not.toHaveBeenCalled();state.connections[0].id="new";state.connections[0].transfer=null;view.rerender(<ResumeTransfers snapshot={state} operate={operate}/>);
  expect((screen.getByRole("checkbox") as HTMLInputElement).checked).toBe(false);expect((screen.getByText("Resume") as HTMLButtonElement).disabled).toBe(true);
});

it("shows completed acknowledgement records without Resume and Discard keeps published files", async () => {
  const state=snapshot();state.resumable[0].completed=true;const operate=vi.fn().mockResolvedValue(true);render(<ResumeTransfers snapshot={state} operate={operate}/>);
  expect(screen.getByText("Completed · awaiting acknowledgement")).toBeTruthy();expect(screen.queryByText("Resume")).toBeNull();expect(screen.queryByRole("checkbox")).toBeNull();expect((screen.getByRole("progressbar") as HTMLProgressElement).value).toBe(100);
  expect(operate).not.toHaveBeenCalled();fireEvent.click(screen.getByText("Discard"));expect(operate).not.toHaveBeenCalled();expect(screen.getByText("Remove only the saved acknowledgement record? Your completed files stay saved.")).toBeTruthy();
  fireEvent.click(screen.getByText("Discard acknowledgement"));fireEvent.click(screen.getByText("Discard acknowledgement"));await waitFor(()=>expect(operate).toHaveBeenCalledExactlyOnceWith("resume_discard",{transferId:"saved"}));
});
it("does not automatically remove a completed zero-byte journal when a trusted peer reconnects", () => {
  const state=snapshot();state.resumable[0].completed=true;state.resumable[0].size=0;state.resumable[0].verified=0;state.connections=[];const operate=vi.fn();const view=render(<ResumeTransfers snapshot={state} operate={operate}/>);
  expect(screen.getByText("0 B verified · 0 B remaining · 100%")).toBeTruthy();const connected=snapshot();connected.resumable=state.resumable;view.rerender(<ResumeTransfers snapshot={connected} operate={operate}/>);
  expect(screen.getByText("Completed · awaiting acknowledgement")).toBeTruthy();expect(screen.queryByText("Resume")).toBeNull();expect(operate).not.toHaveBeenCalled();
});
