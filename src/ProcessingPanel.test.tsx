// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { ProcessingPanel } from "./ProcessingPanel";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
let onChange: () => void;
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async (_name: string, callback: () => void) => {
  onChange = callback;
  return () => {};
}) }));
const call = vi.mocked(invoke);
const job = (state = "choice") => ({ id: "job", name: "song.ogg", state, done: 0, total: 1, error: null, path: null });
beforeEach(() => { call.mockImplementation(async command => command === "processing_snapshot" ? [job()] : command === "processing_converter_status" ? true : undefined); });
afterEach(() => { cleanup(); vi.resetAllMocks(); vi.useRealTimers(); });
it("keeps originals as default and never converts on detection", async () => {
  render(<ProcessingPanel />); await screen.findByText("song.ogg");
  expect(call.mock.calls.every(([c]) => ["processing_snapshot", "processing_converter_status"].includes(c))).toBe(true);
  expect(document.querySelector("[autofocus]")).toBeNull();
  fireEvent.click(screen.getByText("Keep originals"));
  await waitFor(() => expect(call).toHaveBeenCalledWith("processing_decide", { id: "job", convert: false }));
}, 15_000);
it("converts only after explicit action and blocks duplicate pending decisions", async () => {
  call.mockImplementation(command => command === "processing_snapshot" ? Promise.resolve([job()]) : command === "processing_converter_status" ? Promise.resolve(true) : new Promise(() => {}));
  render(<ProcessingPanel />); await screen.findByText("song.ogg");
  const convert = screen.getByText("Convert OGG to MP3") as HTMLButtonElement;
  await waitFor(() => expect(convert.disabled).toBe(false)); fireEvent.click(convert); fireEvent.click(convert);
  expect(call.mock.calls.filter(([c]) => c === "processing_decide")).toEqual([["processing_decide", { id: "job", convert: true }]]);
});
it("sends cancellation by id and waits for actual backend cancellation", async () => {
  call.mockImplementation(async command => command === "processing_snapshot" ? [job("converting")] : command === "processing_converter_status" ? true : undefined);
  render(<ProcessingPanel />); await screen.findByText("song.ogg"); fireEvent.click(screen.getByText("Cancel processing"));
  await screen.findByText("Cancelling processing…"); fireEvent.click(screen.getByText("Cancel processing"));
  expect(call.mock.calls.filter(([c]) => c === "processing_cancel")).toEqual([["processing_cancel", { id: "job" }]]);
  expect(screen.queryByText("Processing cancelled")).toBeNull();
});
it("shows empty completed extraction as finished rather than running", async () => {
  call.mockImplementation(async command => command === "processing_snapshot" ? [{ ...job("complete"), name: "empty.zip", total: 0 }] : false);
  render(<ProcessingPanel />); await screen.findByText("empty.zip");
  expect((screen.getByRole("progressbar") as HTMLProgressElement).value).toBe(100);
  expect(screen.getByText("Extraction completed")).toBeTruthy(); expect(screen.queryByText("Cancel processing")).toBeNull();
});
it("clearly explains missing FFmpeg while allowing original-format sending", async () => {
  call.mockImplementation(async command => command === "processing_snapshot" ? [job()] : false);
  render(<ProcessingPanel />); await screen.findByText("song.ogg");
  expect((screen.getByText("Convert OGG to MP3") as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByText("FFmpeg not selected")).toBeTruthy();
  fireEvent.click(screen.getByText("Keep originals"));
  await waitFor(() => expect(call).toHaveBeenCalledWith("processing_decide", { id: "job", convert: false }));
});
it("enables conversion only after a native FFmpeg selection is confirmed", async () => {
  let selected = false;
  call.mockImplementation(async command => { if (command === "processing_converter") selected = true; return command === "processing_snapshot" ? [job()] : command === "processing_converter_status" ? selected : undefined; });
  render(<ProcessingPanel />); await screen.findByText("song.ogg");
  const convert = screen.getByText("Convert OGG to MP3") as HTMLButtonElement; expect(convert.disabled).toBe(true);
  fireEvent.click(screen.getByText("Optional audio conversion")); fireEvent.click(screen.getByRole("button", { name: "Choose FFmpeg executable" }));
  await waitFor(() => expect(convert.disabled).toBe(false)); fireEvent.click(convert);
  await waitFor(() => expect(call).toHaveBeenCalledWith("processing_decide", { id: "job", convert: true }));
});
it("keeps an acknowledged decision disabled until the backend snapshot advances", async () => {
  render(<ProcessingPanel />); await screen.findByText("song.ogg"); fireEvent.click(screen.getByText("Keep originals"));
  await waitFor(() => expect((screen.getByText("Keep originals") as HTMLButtonElement).disabled).toBe(true));
  fireEvent.click(screen.getByText("Keep originals")); fireEvent.click(screen.getByText("Convert OGG to MP3"));
  expect(call.mock.calls.filter(([c]) => c === "processing_decide")).toHaveLength(1);
});
it("reports an unavailable processing snapshot", async () => {
  call.mockImplementation(async command => { if (command === "processing_snapshot") throw new Error("Offline"); return false; });
  render(<ProcessingPanel />); expect((await screen.findByRole("alert")).textContent).toContain("Could not update file preparation status");
});
it("coalesces updates while a snapshot request is unresolved", async () => {
  vi.useFakeTimers(); let resolve!: (value: never[]) => void;
  call.mockImplementation(command => command === "processing_snapshot" ? new Promise(done => { resolve = done; }) : Promise.resolve(false));
  render(<ProcessingPanel />); await act(async () => { vi.advanceTimersByTime(5000); });
  await act(async () => { onChange(); onChange(); });
  expect(call.mock.calls.filter(([c]) => c === "processing_snapshot")).toHaveLength(1);
  await act(async () => { resolve([]); });
  expect(call.mock.calls.filter(([c]) => c === "processing_snapshot")).toHaveLength(2);
});
it("dismisses only a finished result and explains that saved data stays", async () => {
  call.mockImplementation(async command => command === "processing_snapshot" ? [job("prepared")] : false);
  render(<ProcessingPanel />); await screen.findByText("song.ogg"); expect(screen.getByText(/prepared resume data are kept/)).toBeTruthy();
  fireEvent.click(screen.getByText("Dismiss")); await waitFor(() => expect(call).toHaveBeenCalledWith("processing_dismiss", { id: "job" }));
  await waitFor(() => expect(screen.queryByText("song.ogg")).toBeNull());
});
it("reveals a newly requested processing stage without stealing focus or scrolling every update", async () => {
  vi.useFakeTimers(); let jobs: ReturnType<typeof job>[] = [];
  call.mockImplementation(async command => command === "processing_snapshot" ? jobs : false);
  render(<ProcessingPanel />); await act(async () => {});
  const panel = screen.getByRole("region", { name: "Optional file processing" });
  panel.scrollIntoView = vi.fn(); const originalFocus = document.activeElement;
  jobs = [job()]; await act(async () => { onChange(); });
  expect(panel.scrollIntoView).toHaveBeenCalledExactlyOnceWith({ block: "nearest" });
  expect(document.activeElement).toBe(originalFocus);
  jobs = [{ ...job("converting"), done: 1 }]; await act(async () => { onChange(); });
  expect(panel.scrollIntoView).toHaveBeenCalledTimes(1);
});
