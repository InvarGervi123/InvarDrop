import { useEffect, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface Device { id: string; name: string; canConnect: boolean }
interface Snapshot {
  revision: number;
  localDevice: Device;
  status: "starting" | "searching" | "unavailable" | "stopped";
  devices: Device[];
}

export function useDiscovery() {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [bridgeError, setBridgeError] = useState(false);
  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    const accept = (incoming: Snapshot) => {
      if (!disposed) {
        setSnapshot(previous =>
          !previous || incoming.revision > previous.revision ? incoming : previous);
      }
    };
    const connect = async () => {
      try {
        if (!isTauri()) throw new Error("Native discovery requires the desktop app");
        // Subscribe first; revisions prevent a delayed initial response overwriting an event.
        const stop = await listen<Snapshot>("discovery-updated", event => accept(event.payload));
        if (disposed) { stop(); return; }
        unlisten = stop;
        accept(await invoke<Snapshot>("discovery_snapshot"));
      } catch {
        if (!disposed) setBridgeError(true);
        unlisten?.();
        unlisten = undefined;
      }
    };
    void connect();
    return () => { disposed = true; unlisten?.(); };
  }, []);
  return { snapshot, bridgeError };
}
