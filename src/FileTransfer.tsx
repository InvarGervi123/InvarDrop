import { useRef, useState } from "react";
import type { SecureConnection } from "./useSecurity";

type Operate = (command: string, args: Record<string, unknown>) => Promise<boolean>;
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "Unknown size";
  const units = ["B", "KiB", "MiB", "GiB"];
  const index = bytes ? Math.min(3, Math.floor(Math.log(bytes) / Math.log(1024))) : 0;
  return `${new Intl.NumberFormat("en", { maximumFractionDigits: 1 }).format(bytes / 1024 ** index)} ${units[index]}`;
}
const labels: Record<string, string> = {
  offering: "Waiting for the receiver…", preparing: "Preparing file…",
  sending: "Accepted · sending file…", receiving: "Accepted · receiving file…",
  finalizing: "Verifying and saving…", complete: "Completed · BLAKE3 verified",
  rejected: "Declined", expired: "Request expired", error: "Transfer failed",
};

export function FileTransfer({ connection, operate }: { connection: SecureConnection; operate: Operate }) {
  const busy = useRef(false);
  const [pending, setPending] = useState(false);
  const [confirmedId, setConfirmedId] = useState<string | null>(null);
  const [respondedId, setRespondedId] = useState<string | null>(null);
  const transfer = connection.transfer;
  const incoming = transfer?.direction === "incoming";
  const waiting = transfer?.status === "awaiting_approval";
  const active = transfer && !["complete", "rejected", "expired", "error"].includes(transfer.status);
  const answered = transfer && respondedId === transfer.id;
  const run = async (accept?: boolean) => {
    if (busy.current || (accept !== undefined && (!transfer || answered || !waiting || (accept && confirmedId !== transfer.id)))) return;
    busy.current = true; setPending(true);
    // Lock this request immediately, including the interval before the backend event.
    const request = transfer?.id;
    if (accept !== undefined && request) setRespondedId(request);
    try {
      const ok = accept === undefined
        ? await operate("file_choose", { id: connection.id })
        : await operate("file_respond", { id: connection.id, transferId: request, accept });
      if (!ok && accept !== undefined) setRespondedId(null);
    } catch { setRespondedId(null); }
    finally { busy.current = false; setPending(false); }
  };
  const status = waiting ? incoming ? "Waiting for your decision" : "Waiting for the receiver…"
    : transfer?.status === "finalizing" && !incoming ? "Waiting for verification…"
    : labels[transfer?.status ?? ""] ?? "Processing file…";
  return <div className="file-transfer">
    {connection.phase === "connected" && !active && <button type="button" disabled={pending || Boolean(active)} onClick={() => void run()}>Choose a file to send</button>}
    {transfer && <section className="file-request" aria-label={incoming ? "Incoming file request" : "Outgoing file"}>
      {incoming && <p className="sender-name">From <strong dir="auto">{transfer.senderName ?? "Paired device"}</strong><small>Device-reported name · verified key shown above</small></p>}
      <p className="file-name" dir="auto">{transfer.name}</p>
      <p className="file-size" title={`${transfer.size.toLocaleString()} bytes`}>{formatBytes(transfer.size)} · {incoming ? "Incoming" : "Outgoing"}</p>
      <p className={`request-status request-${transfer.status}`} role="status">{status}</p>
      {incoming && waiting && connection.phase === "connected" && <>
        <label className="comparison-check"><input type="checkbox" checked={confirmedId === transfer.id} disabled={pending || Boolean(answered)} onChange={event => setConfirmedId(event.target.checked ? transfer.id : null)} />I want to save this file</label>
        <div className="request-actions">
          <button type="button" className="secondary" disabled={pending || Boolean(answered)} onClick={() => void run(false)}>Decline file</button>
          <button type="button" disabled={pending || Boolean(answered) || confirmedId !== transfer.id} onClick={() => void run(true)}>Accept file</button>
        </div>
        <p className="file-hint">{answered ? "Decision sent…" : "Nothing is saved until you accept. The request expires after 90 seconds."}</p>
      </>}
      {incoming && ["rejected", "expired"].includes(transfer.status) && <p className="file-hint">No file was saved.</p>}
      {transfer.path && <p className="saved-path">Saved to: <span dir="auto">{transfer.path}</span></p>}
      {transfer.error && transfer.status !== "expired" && <p className="error-text" role="alert">{transfer.error}</p>}
    </section>}
  </div>;
}
