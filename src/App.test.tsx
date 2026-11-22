// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { useDiscovery } from "./useDiscovery";
import { useSecurity, type SecureConnection, type SecuritySnapshot } from "./useSecurity";
import { useBluetooth } from "./useBluetooth";
import App from "./App";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("./useDiscovery", () => ({ useDiscovery: vi.fn() }));
vi.mock("./useSecurity", () => ({ useSecurity: vi.fn() }));
vi.mock("./useBluetooth", async original => ({ ...await original<typeof import("./useBluetooth")>(), useBluetooth: vi.fn() }));

const call = vi.mocked(invoke);
const key = "b".repeat(64);
const draft = { id: "draft", name: "photo.png", count: 1, folder: false };
const local = { id: "local", name: "Linux computer ABCD", canConnect: true };
const peer = { id: "peer", name: "Android phone 1234", canConnect: true };
const security: SecuritySnapshot = { revision: 1, status: "ready", identity: "a".repeat(64), trusted: [key], revoked: [], connections: [], error: null, receiveDirectory: "/profile/received", resumable: [] };
const connection = (changes: Partial<SecureConnection> = {}): SecureConnection => ({
  id: "session", phase: "connected", peer: key, code: null, localApproved: true, canReconnect: true,
  supportsBundles: true, supportsResume: true, error: null, transfer: null, ...changes,
});
const state = (connections: SecureConnection[]) => {
  vi.mocked(useSecurity).mockReturnValue({ bridgeError: false, snapshot: { ...security, connections } });
};
const discovery = (devices = [peer], status: "searching" | "unavailable" = "searching") => {
  vi.mocked(useDiscovery).mockReturnValue({ bridgeError: false, snapshot: { revision: 1, localDevice: local, supportsFolderPicker: true, status, devices } });
};
const calls = (command: string) => call.mock.calls.filter(([name]) => name === command);
const sendButton = () => screen.getByRole<HTMLButtonElement>("button", { name: "Send files" });
const openSettings = () => fireEvent.click(screen.getByRole("button", { name: "Settings" }));
const choose = async () => {
  fireEvent.click(screen.getByRole("button", { name: /Choose files/ }));
  await screen.findByText(draft.name);
};
const select = () => fireEvent.click(screen.getByRole("button", { name: /Android phone/ }));

beforeEach(() => {
  localStorage.clear();
  vi.mocked(useBluetooth).mockReturnValue(null);
  discovery();
  state([]);
  call.mockImplementation(async command => {
    if (command === "processing_snapshot") return [];
    if (command === "processing_converter_status") return false;
    if (command === "share_pick") return draft;
    if (command === "secure_connect") return "session";
  });
});
afterEach(() => { cleanup(); vi.clearAllMocks(); });

it.each(["files", "device"])("selects %s first and initiates a trusted send with one explicit action", async order => {
  const app = render(<App />);
  if (order === "files") { await choose(); expect(sendButton().disabled).toBe(true); select(); }
  else { select(); expect(sendButton().disabled).toBe(true); await choose(); }
  expect(calls("secure_connect")).toHaveLength(0);
  expect(screen.getByRole("button", { name: /Android phone/ }).getAttribute("aria-pressed")).toBe("true");
  fireEvent.click(sendButton());
  expect(calls("secure_connect")).toEqual([["secure_connect", { id: "peer" }]]);
  await screen.findByRole("button", { name: "Connecting…" });
  expect(calls("share_send")).toHaveLength(0);
  state([connection()]);
  app.rerender(<App />);
  await waitFor(() => expect(calls("share_send")).toEqual([["share_send", { id: "session", draftId: "draft", offerConversion: false }]]));
  await waitFor(() => expect(screen.queryByText(draft.name)).toBeNull());
});

it("keeps the draft local until both devices finish first-time pairing", async () => {
  const app = render(<App />);
  await choose(); select(); fireEvent.click(sendButton());
  state([connection({ phase: "verifying", code: "0000 1111 2222 3333 4444 5555 6666 7777", localApproved: false })]);
  app.rerender(<App />);
  const approval = screen.getByRole<HTMLButtonElement>("button", { name: "Codes match — pair" });
  expect(approval.disabled).toBe(true);
  fireEvent.click(screen.getByRole("checkbox"));
  fireEvent.click(approval);
  await screen.findByText("Waiting for the other device to confirm…");
  expect(calls("share_send")).toHaveLength(0);
  state([connection()]);
  app.rerender(<App />);
  await waitFor(() => expect(calls("share_send")).toHaveLength(1));
});

it("offers a Bluetooth-only peer through the same send flow", async () => {
  discovery([], "unavailable");
  vi.mocked(useBluetooth).mockReturnValue({ supported: true, revision: 1, status: "scanning", error: null, devices: [peer] });
  render(<App />);
  await choose(); select(); fireEvent.click(sendButton());
  expect(calls("secure_connect")).toEqual([["secure_connect", { id: "peer" }]]);
});

it("automatically starts discovery without requesting a new Bluetooth grant", async () => {
  discovery([]);
  vi.mocked(useBluetooth).mockReturnValue({ supported: true, revision: 1, status: "disabled", error: null, devices: [] });
  render(<App />);
  await waitFor(() => expect(call).toHaveBeenCalledWith("bluetooth_start"));
  expect(calls("bluetooth_enable")).toHaveLength(0);
  fireEvent.click(screen.getByRole("button", { name: "Find more devices" }));
  expect(calls("bluetooth_enable")).toEqual([["bluetooth_enable", {}]]);
});

it("keeps the files selected after a connection failure and hides technical details from sharing", async () => {
  const app = render(<App />);
  await choose(); select(); fireEvent.click(sendButton());
  state([connection({ phase: "closed", error: "TCP connection refused" })]);
  app.rerender(<App />);
  expect((await screen.findByRole("alert")).textContent).toContain("Check that InvarDrop is open");
  expect(screen.queryByText("TCP connection refused")).toBeNull();
  expect(screen.getByText(draft.name)).toBeTruthy();
  expect(sendButton().disabled).toBe(false);
  openSettings();
  fireEvent.click(screen.getByText("Connection diagnostics"));
  expect(screen.getAllByText("TCP connection refused").length).toBeGreaterThan(0);
});

it("keeps a completed transfer visible after a later connection error", () => {
  const transfer: NonNullable<SecureConnection["transfer"]> = {
    id: "receipt", direction: "incoming", senderName: peer.name, name: "receipt.png", size: 110, fileCount: 1, isFolder: false,
    resuming: false, transferred: 110, bytesPerSecond: null, etaSeconds: null, status: "complete", path: "Downloads/InvarDrop/receipt.png", error: null,
  };
  state([connection({ transfer })]);
  const app = render(<App />);
  expect(screen.getByText("Completed")).toBeTruthy();
  state([connection({ phase: "closed", error: "Connection failed or disconnected", transfer })]);
  app.rerender(<App />);
  expect(screen.getByText("Completed")).toBeTruthy();
  expect(screen.getByText("Downloads/InvarDrop/receipt.png")).toBeTruthy();
  expect(screen.queryByRole("alert")).toBeNull();
});

it("shows an actionable empty state and updates it when a peer arrives", () => {
  discovery([]);
  const app = render(<App />);
  expect(screen.getByText("No devices nearby yet")).toBeTruthy();
  expect(screen.getByText(/Open InvarDrop on the other device/)).toBeTruthy();
  discovery();
  app.rerender(<App />);
  expect(screen.getByRole("button", { name: /Android phone.*Available/ })).toBeTruthy();
});

it("keeps an Android file draft when a replacement picker is cancelled", async () => {
  vi.mocked(useDiscovery).mockReturnValue({ bridgeError: false, snapshot: { revision: 1, localDevice: peer, supportsFolderPicker: false, status: "searching", devices: [] } });
  render(<App />);
  await choose();
  expect(screen.queryByRole("button", { name: "Choose a folder" })).toBeNull();
  call.mockResolvedValueOnce(null);
  fireEvent.click(screen.getByRole("button", { name: "Change files" }));
  await screen.findByRole("button", { name: "Change files" });
  expect(screen.getByText(draft.name)).toBeTruthy();
});

it("selects a folder without requiring a peer and blocks duplicate picker actions", async () => {
  let finish!: (value: unknown) => void;
  call.mockImplementation(command => command === "share_pick" ? new Promise(resolve => { finish = resolve; }) : Promise.resolve(command === "processing_snapshot" ? [] : false));
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "Choose a folder" }));
  fireEvent.click(screen.getByRole("button", { name: "Choose a folder" }));
  expect(calls("share_pick")).toEqual([["share_pick", { folder: true }]]);
  finish({ ...draft, name: "Holiday", folder: true });
  await screen.findByText("Holiday");
  expect(calls("secure_connect")).toHaveLength(0);
  expect(screen.getByRole<HTMLButtonElement>("button", { name: "Send folder" }).disabled).toBe(true);
});

it("requires confirmation before revoking trust and focuses the safe action first", async () => {
  render(<App />); openSettings(); fireEvent.click(screen.getByText("Device identity & trusted peers"));
  fireEvent.click(screen.getByRole("button", { name: "Revoke trust" }));
  expect(calls("secure_revoke")).toHaveLength(0);
  expect(screen.getByText(/invalidates all saved transfer recovery records/)).toBeTruthy();
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "Keep trust" }));
  fireEvent.click(screen.getByRole("button", { name: "Keep trust" }));
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "Revoke trust" }));
  fireEvent.click(screen.getByRole("button", { name: "Revoke trust" }));
  fireEvent.click(screen.getByRole("button", { name: "Confirm revoke" }));
  await waitFor(() => expect(call).toHaveBeenCalledWith("secure_revoke", { id: key }));
});

it("distinguishes startup from an empty nearby list", () => {
  vi.mocked(useDiscovery).mockReturnValue({ bridgeError: false, snapshot: null });
  render(<App />);
  expect(screen.getByText("Looking for devices…")).toBeTruthy();
  expect(screen.queryByText("No devices nearby yet")).toBeNull();
});
