// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { useDiscovery } from "./useDiscovery";
import { useSecurity } from "./useSecurity";
import App from "./App";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
vi.mock("./useDiscovery", () => ({ useDiscovery: vi.fn() }));
vi.mock("./useSecurity", () => ({ useSecurity: vi.fn() }));
const call = vi.mocked(invoke);
const peer = "b".repeat(64);
beforeEach(() => {
  call.mockImplementation(async command => command === "processing_snapshot" ? [] : command === "processing_converter_status" ? false : undefined);
  vi.mocked(useDiscovery).mockReturnValue({ bridgeError: false, snapshot: { revision: 1, localDevice: { id: "local", name: "InvarDrop local", canConnect: true }, status: "searching", devices: [{ id: "peer", name: "InvarDrop nearby", canConnect: true }] } });
  vi.mocked(useSecurity).mockReturnValue({ bridgeError: false, snapshot: { revision: 1, status: "ready", identity: "a".repeat(64), trusted: [peer], revoked: [], connections: [], error: null, receiveDirectory: "/profile/received", resumable: [] } });
});
afterEach(() => { cleanup(); vi.resetAllMocks(); });
it("never labels a discovered device as trusted based on its name", async () => {
  render(<App />); expect(screen.getByText("Unverified name")).toBeTruthy();
  const connect = screen.getByRole("button", { name: "Connect to InvarDrop nearby" });
  fireEvent.click(connect); await waitFor(() => expect(call).toHaveBeenCalledWith("secure_connect", { id: "peer" }));
});
it("explains discovery failure and offers automatic retry guidance", () => {
  vi.mocked(useDiscovery).mockReturnValue({ bridgeError: false, snapshot: { revision: 1, localDevice: { id: "local", name: "local", canConnect: false }, status: "unavailable", devices: [] } });
  render(<App />); expect(screen.getByText(/Retrying automatically/)).toBeTruthy(); expect(screen.queryByRole("button", { name: /Connect to/ })).toBeNull();
});
it("shows a useful empty state and keyboard-focusable scrolling region", () => {
  vi.mocked(useDiscovery).mockReturnValue({ bridgeError: false, snapshot: { revision: 1, localDevice: { id: "local", name: "local", canConnect: true }, status: "searching", devices: [] } });
  render(<App />); expect(screen.getByText("No devices found yet")).toBeTruthy();
  const region = screen.getByRole("region", { name: "Devices and transfers" }); expect(region.tabIndex).toBe(0);
  expect(screen.getByText(/Devices appear automatically/)).toBeTruthy();
});
it("requires confirmation before revoking trust and explains recovery loss", async () => {
  render(<App />); fireEvent.click(screen.getByText("Device identity & trusted peers"));
  fireEvent.click(screen.getByRole("button", { name: "Revoke trust" }));
  expect(call.mock.calls.some(([command]) => command === "secure_revoke")).toBe(false);
  expect(screen.getByText(/invalidates all saved transfer recovery records/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Keep trust" }));
  expect(screen.queryByRole("button", { name: "Confirm revoke" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Revoke trust" }));
  fireEvent.click(screen.getByRole("button", { name: "Confirm revoke" }));
  await waitFor(() => expect(call).toHaveBeenCalledWith("secure_revoke", { id: peer }));
});
it("keeps an unsuccessful trust revocation available for deliberate retry", async () => {
  call.mockImplementation(async command => { if (command === "secure_revoke") throw "Secure storage is unavailable; no insecure fallback is used"; return command === "processing_snapshot" ? [] : false; });
  render(<App />); fireEvent.click(screen.getByText("Device identity & trusted peers")); fireEvent.click(screen.getByRole("button", { name: "Revoke trust" }));
  fireEvent.click(screen.getByRole("button", { name: "Confirm revoke" }));
  expect(await screen.findByRole("alert")).toBeTruthy();
  await waitFor(() => expect((screen.getByRole("button", { name: "Confirm revoke" }) as HTMLButtonElement).disabled).toBe(false));
  expect(screen.getByText(/Unlock your system credential store/)).toBeTruthy();
});
it("does not issue duplicate Connect requests while IPC is pending", () => {
  call.mockImplementation(command => command === "secure_connect" ? new Promise(() => {}) : Promise.resolve(command === "processing_snapshot" ? [] : false));
  render(<App />); const connect = screen.getByRole("button", { name: "Connect to InvarDrop nearby" }); fireEvent.click(connect); fireEvent.click(connect);
  expect(call.mock.calls.filter(([command]) => command === "secure_connect")).toHaveLength(1);
});

it("focuses the safe trust action and restores focus after keeping trust", () => {
  render(<App />); fireEvent.click(screen.getByText("Device identity & trusted peers"));
  fireEvent.click(screen.getByRole("button", { name: "Revoke trust" }));
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "Keep trust" }));
  fireEvent.click(screen.getByRole("button", { name: "Keep trust" }));
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "Revoke trust" }));
});
it("distinguishes startup from a completed discovery search with no peers", () => {
  vi.mocked(useDiscovery).mockReturnValue({ bridgeError: false, snapshot: null });
  render(<App />); expect(screen.getByText("Starting local discovery…")).toBeTruthy();
  expect(screen.queryByText("No devices found yet")).toBeNull();
});
