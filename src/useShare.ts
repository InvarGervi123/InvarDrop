import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Device } from "./useDiscovery";
import type { SecureConnection } from "./useSecurity";

export interface Draft { id: string; name: string; count: number; folder: boolean }
interface Intent {
  draft: Draft;
  device: Device;
  connectionId?: string;
  stage: "connecting" | "submitting";
  offerConversion: boolean;
}
export type Report = (message: string | null) => void;

export function failureText(failure: unknown) {
  return typeof failure === "string" ? failure : "Operation failed. Try again.";
}

export function useShare(connections: SecureConnection[], report: Report) {
  const [draft, setDraft] = useState<Draft | null>(null);
  const [device, setDevice] = useState<Device | null>(null);
  const [picking, setPicking] = useState(false);
  const [intent, setIntent] = useState<Intent | null>(null);
  const [recipients, setRecipients] = useState<Record<string, string>>({});
  const locked = useRef(false);
  const submitted = useRef<Intent | null>(null);
  const cancelling = useRef(false);
  const [cancelPending, setCancelPending] = useState(false);
  const connection = connections.find(item => item.id === intent?.connectionId);

  const selectDevice = (next: Device) => {
    if (locked.current) return;
    report(null);
    setDevice(previous => previous?.id === next.id ? null : next);
  };
  const pick = async (folder: boolean) => {
    if (locked.current) return;
    locked.current = true;
    report(null);
    setPicking(true);
    try {
      const next = await invoke<Draft | null>("share_pick", { folder });
      if (next) setDraft(next);
    } catch (failure) { report(failureText(failure)); }
    finally { setPicking(false); locked.current = false; }
  };
  const clear = async () => {
    if (!draft || locked.current) return;
    locked.current = true;
    try {
      await invoke("share_clear", { id: draft.id });
      setDraft(null);
      report(null);
    } catch (failure) { report(failureText(failure)); }
    finally { locked.current = false; }
  };
  const send = async (offerConversion: boolean) => {
    if (!draft || !device || locked.current) return;
    locked.current = true;
    report(null);
    const request: Intent = { draft, device, stage: "connecting", offerConversion };
    setIntent(request);
    try {
      const id = await invoke<string>("secure_connect", { id: device.id });
      setRecipients(previous => Object.fromEntries([...Object.entries(previous).filter(([key]) => key !== id).slice(-31), [id, device.name]]));
      setIntent({ ...request, connectionId: id });
    } catch (failure) {
      report(failureText(failure));
      setIntent(null);
      locked.current = false;
    }
  };

  useEffect(() => {
    if (!intent || !connection || intent.stage !== "connecting" || submitted.current === intent) return;
    if (connection.phase === "closed") {
      if (connection.error !== "Connection cancelled") report(connection.error ?? "Device is no longer available");
      setIntent(null);
      locked.current = false;
      cancelling.current = false;
      setCancelPending(false);
    } else if (connection.phase === "connected" && !cancelling.current) {
      submitted.current = intent;
      setIntent({ ...intent, stage: "submitting" });
      void invoke("share_send", { id: connection.id, draftId: intent.draft.id, offerConversion: intent.offerConversion })
        .then(() => setDraft(null))
        .catch(failure => report(failureText(failure)))
        .finally(() => { setIntent(null); locked.current = false; });
    }
  }, [connection, intent, report, cancelPending]);

  const cancel = async () => {
    if (!connection || intent?.stage !== "connecting" || cancelling.current) return;
    cancelling.current = true;
    setCancelPending(true);
    try { await invoke("secure_cancel", { id: connection.id }); }
    catch (failure) {
      report(failureText(failure));
      cancelling.current = false;
      setCancelPending(false);
    }
  };

  return { draft, device, selectDevice, pick, clear, send, intent, picking, connection, cancel, cancelling: cancelPending, recipients };
}
