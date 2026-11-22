import { useEffect, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Device } from "./useDiscovery";

export interface BluetoothSnapshot {
  supported?: boolean;
  revision: number;
  status: "disabled" | "starting" | "scanning" | "ready" | "paused" | "unavailable";
  error: string | null;
  devices: Device[];
}
export function useBluetooth() {
  const [snapshot, setSnapshot] = useState<BluetoothSnapshot | null>(null);
  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    const accept = (incoming: BluetoothSnapshot) => {
      if (!disposed && incoming && typeof incoming.revision === "number") {
        setSnapshot(previous => !previous || incoming.revision > previous.revision ? incoming : previous);
      }
    };
    const connect = async () => {
      if (!isTauri()) return;
      try {
        const stop = await listen<BluetoothSnapshot>("bluetooth-updated", event => accept(event.payload));
        if (disposed) { stop(); return; }
        unlisten = stop;
        accept(await invoke<BluetoothSnapshot>("bluetooth_snapshot"));
      } catch { /* The existing app bridge reports connection errors. */ }
    };
    void connect();
    return () => { disposed = true; unlisten?.(); };
  }, []);
  return snapshot;
}

export function nearbyDevices(lan: Device[], bluetooth: Device[]): Device[] {
  const devices = new Map(lan.map(device => [device.id, { ...device }]));
  for (const device of bluetooth) {
    const previous = devices.get(device.id);
    devices.set(device.id, previous ? { ...previous, canConnect: previous.canConnect || device.canConnect } : device);
  }
  return [...devices.values()];
}

const SCAN_INTERVAL = 90_000;

export function useBluetoothDiscovery(snapshot: BluetoothSnapshot | null, allowed: boolean, idle: boolean, hasLan: boolean) {
  const enabled = snapshot?.status === "ready" || snapshot?.status === "scanning" || snapshot?.status === "paused" || snapshot?.status === "starting";
  useEffect(() => {
    if (!allowed || snapshot?.supported !== true) return;
    const start = () => {
      if (document.visibilityState === "visible") void invoke("bluetooth_start").catch(() => {});
    };
    start();
    document.addEventListener("visibilitychange", start);
    return () => document.removeEventListener("visibilitychange", start);
  }, [allowed, snapshot?.supported]);

  useEffect(() => {
    if (!allowed || !enabled || !idle || hasLan) return;
    let timer: ReturnType<typeof setTimeout>;
    const scan = () => {
      if (document.visibilityState !== "visible") return;
      void invoke("bluetooth_scan").catch(() => {});
      timer = setTimeout(scan, SCAN_INTERVAL);
    };
    const onVisibility = () => {
      clearTimeout(timer);
      if (document.visibilityState === "visible") scan();
    };
    timer = setTimeout(scan, SCAN_INTERVAL);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [allowed, enabled, idle, hasLan]);
}
