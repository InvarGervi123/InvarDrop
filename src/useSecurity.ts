import { useEffect, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
export interface SecureConnection {
  id: string; phase: string; peer: string | null; code: string | null;
  localApproved: boolean; canReconnect: boolean; error: string | null;
  transfer: FileTransfer | null;
}
export interface FileTransfer {
  id: string; direction: "outgoing" | "incoming"; name: string; size: number;
  senderName: string | null; status: string; path: string | null; error: string | null;
}
export interface SecuritySnapshot {
  revision: number; status: string; identity: string | null;
  trusted: string[]; revoked: string[]; connections: SecureConnection[]; error: string | null;
  receiveDirectory: string;
}
export function useSecurity() {
  const [snapshot, setSnapshot] = useState<SecuritySnapshot | null>(null);
  const [bridgeError, setBridgeError] = useState(false);
  useEffect(() => {
    let disposed = false; let unlisten: UnlistenFn | undefined;
    const accept = (incoming: SecuritySnapshot) => {
      if (!disposed) setSnapshot(previous => !previous || incoming.revision > previous.revision ? incoming : previous);
    };
    const connect = async () => {
      try {
        if (!isTauri()) throw new Error("Native app required");
        const stop = await listen<SecuritySnapshot>("security-updated", event => accept(event.payload));
        if (disposed) { stop(); return; } unlisten = stop;
        accept(await invoke<SecuritySnapshot>("security_snapshot"));
      } catch { if (!disposed) setBridgeError(true); unlisten?.(); unlisten = undefined; }
    };
    void connect(); return () => { disposed = true; unlisten?.(); };
  }, []);
  return { snapshot, bridgeError };
}
