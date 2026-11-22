import { useRef, useState } from "react";
import { friendlyError } from "./ErrorNotice";
import { Icon } from "./Icon";
import type { SecureConnection, FileTransfer as Transfer } from "./useSecurity";

const byteFormatter = new Intl.NumberFormat("en", { maximumFractionDigits: 1 });

type Operate = (command: string, args: Record<string, unknown>) => Promise<boolean>;
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "Unknown size";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  const index = bytes ? Math.max(0, Math.min(4, Math.floor(Math.log(bytes) / Math.log(1024)))) : 0;
  return `${byteFormatter.format(bytes / 1024 ** index)} ${units[index]}`;
}
export function formatEta(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "";
  const value = Math.ceil(seconds);
  if (value < 60) return `${value} s`;
  if (value < 3600) return `${Math.floor(value / 60)} min ${value % 60} s`;
  return `${Math.floor(value / 3600)} h ${Math.floor(value % 3600 / 60)} min`;
}
export function progressPercent(transfer: Transfer): number {
  if (transfer.size === 0) return transfer.status === "complete" ? 100 : 0;
  return Math.floor(Math.min(100, Math.max(0, transfer.transferred / transfer.size * 100)) * 10) / 10;
}
const labels: Record<string, string> = {
  offering: "Waiting for the receiver…", preparing: "Preparing transfer…",
  sending: "Sending…", receiving: "Receiving…",
  cancelling: "Cancelling…", cancelled: "Cancelled", committing: "Saving verified file…",
  finalizing: "Verifying and saving…", complete: "Completed",
  rejected: "Declined", expired: "Request expired", error: "Transfer failed",
};

export function FileTransfer({ connection, operate, recipient }: { connection: SecureConnection; operate: Operate; recipient?: string }) {
  const busy = useRef(false);
  const [extractConfirmed, setExtractConfirmed] = useState<string | null>(null);
  const [extracted, setExtracted] = useState<string | null>(null);
  const extractionRequests = useRef(new Set<string>());
  const [pending, setPending] = useState(false);
  const [confirmedId, setConfirmedId] = useState<string | null>(null);
  const [respondedId, setRespondedId] = useState<string | null>(null);
  const [cancelId, setCancelId] = useState<string | null>(null);
  const transfer = connection.transfer;
  const measuring = transfer && ["sending", "receiving"].includes(transfer.status);
  const extractionKey = `${connection.id}:${transfer?.id}`;
  const incoming = transfer?.direction === "incoming";
  const waiting = transfer?.status === "awaiting_approval";
  const active = transfer && !["complete", "rejected", "expired", "cancelled", "error"].includes(transfer.status);
  const answered = transfer && respondedId === transfer.id;
  const respond = async (accept: boolean) => {
    if (busy.current || !transfer || answered || !waiting || (accept && confirmedId !== transfer.id)) return;
    busy.current = true;
    setPending(true);
    setRespondedId(transfer.id);
    try {
      if (!await operate("file_respond", { id: connection.id, transferId: transfer.id, accept })) setRespondedId(null);
    } catch { setRespondedId(null); }
    finally { busy.current = false; setPending(false); }
  };
  const cancelTransfer = async () => {
    if (busy.current || !transfer || !active || cancelId === transfer.id || ["cancelling", "committing"].includes(transfer.status)) return;
    busy.current = true; setPending(true); setCancelId(transfer.id);
    try { if (!await operate("file_cancel", { id: connection.id, transferId: transfer.id })) setCancelId(null); }
    catch { setCancelId(null); }
    finally { busy.current = false; setPending(false); }
  };
  const extract = async () => {
    if (!transfer || transfer.status !== "complete" || !incoming || transfer.isFolder || !transfer.name.toLowerCase().endsWith(".zip") || extractConfirmed !== extractionKey || extractionRequests.current.has(extractionKey)) return;
    extractionRequests.current.add(extractionKey); setExtracted(extractionKey);
    try {
      if (!await operate("processing_extract", { connectionId: connection.id, transferId: transfer.id })) {
        extractionRequests.current.delete(extractionKey); setExtracted(current => current === extractionKey ? null : current);
      }
    } catch {
      extractionRequests.current.delete(extractionKey); setExtracted(current => current === extractionKey ? null : current);
    }
  };
  const statusText = waiting ? transfer?.resuming
    ? incoming ? "Waiting for your approval to resume" : "Waiting for receiver approval to resume…"
    : incoming ? "Waiting for your decision" : "Waiting for the receiver…"
    : transfer?.status === "finalizing" && !incoming ? "Waiting for verification…"
    : labels[transfer?.status ?? ""] ?? "Processing file…";
  const status = transfer?.isFolder ? statusText.replace(/file/g, "folder") : statusText;
  if (!transfer) return null;
  const kind = transfer.resuming ? "resume" : transfer.isFolder ? "folder" : "file";
  const title = waiting && incoming ? (transfer.resuming ? "Resume request" : transfer.isFolder ? "Incoming folder" : "Incoming file")
    : transfer.status === "complete" ? (incoming ? "Received" : "Sent")
    : incoming ? "Incoming transfer" : "Outgoing transfer";
  return <section className="activity-card file-request" aria-label={title}>
      <div className="activity-heading">
        <span className={transfer.status === "complete" ? "tile-icon success-icon" : "tile-icon"}><Icon name={transfer.status === "complete" ? "check" : incoming ? "inbox" : "send"} /></span>
        <div><h3>{title}</h3>
        {incoming ? <p>From <strong dir="auto">{transfer.senderName ?? "Paired device"}</strong></p>
          : recipient && <p>To <strong dir="auto">{recipient}</strong></p>}</div>
      </div>
      <p className="file-name" dir="auto">{transfer.name}</p>
      <p className="file-size" title={transfer.size.toLocaleString("en") + " bytes"}>{formatBytes(transfer.size)}{transfer.isFolder && <> · {transfer.fileCount} files · folder</>} · {incoming ? "Incoming" : "Outgoing"}</p>
      <p className={`request-status request-${transfer.status}`} role="status">{answered && waiting ? "Decision sent…" : status}</p>
      {active && (!waiting || transfer.resuming) && <div className="transfer-metrics" aria-label="Transfer statistics">
        <progress aria-label="File data progress" aria-valuetext={`${progressPercent(transfer)}% · ${formatBytes(transfer.transferred)} of ${formatBytes(transfer.size)}`} max={100} value={progressPercent(transfer)} />
        <p>{progressPercent(transfer)}% · {formatBytes(transfer.transferred)} / {formatBytes(transfer.size)}</p>
        {transfer.resuming && <p>Resuming verified data · {formatBytes(Math.max(0, transfer.size - transfer.transferred))} remaining</p>}
        {measuring && transfer.bytesPerSecond !== null && <p>{(transfer.bytesPerSecond / 1024 ** 2).toFixed(2)} MiB/s
          {transfer.etaSeconds !== null && <> · ETA ~{formatEta(transfer.etaSeconds)}</>}</p>}
      </div>}
      {connection.phase === "connected" && active && !(incoming && waiting) && transfer.status !== "committing" && <button type="button" className="secondary" disabled={pending || cancelId === transfer.id || transfer.status === "cancelling"} onClick={() => void cancelTransfer()}>Cancel transfer</button>}
      {transfer.status === "cancelled" && <p className="file-hint">Choose files to start a new transfer.</p>}
      {incoming && waiting && connection.phase === "connected" && <>
        <label className="comparison-check"><input type="checkbox" checked={confirmedId === transfer.id} disabled={pending || Boolean(answered)} onChange={event => setConfirmedId(event.target.checked ? transfer.id : null)} />I want to {transfer.resuming ? "resume" : "save"} this {transfer.isFolder ? "folder" : "file"}</label>
        <div className="request-actions">
          <button type="button" className="secondary" disabled={pending || Boolean(answered)} onClick={() => void respond(false)}>Decline {kind}</button>
          <button type="button" disabled={pending || Boolean(answered) || confirmedId !== transfer.id} onClick={() => void respond(true)}>Accept {kind}</button>
        </div>
        <p className="file-hint">{transfer.resuming ? "Declining keeps the verified recovery data." : "Accepted files are saved after verification."}</p>
      </>}
      {incoming && ["rejected", "expired"].includes(transfer.status) && <p className="file-hint">{transfer.resuming ? "Resume did not start. Saved recovery data is kept." : "No file was saved."}</p>}
      {incoming && transfer.status === "complete" && !transfer.isFolder && transfer.name.toLowerCase().endsWith(".zip") && <details className="zip-extraction"><summary>Extract archive</summary>
        <p className="file-hint">The verified ZIP stays saved. Extract a separate copy only if you want to; extracted content is never opened or run.</p>
        <label className="comparison-check"><input type="checkbox" checked={extractConfirmed === extractionKey} disabled={extracted === extractionKey} onChange={event => setExtractConfirmed(event.target.checked ? extractionKey : null)} />I want to extract this ZIP</label>
        <button type="button" disabled={extractConfirmed !== extractionKey || extracted === extractionKey} onClick={() => void extract()}>{extracted === extractionKey ? "Extraction requested" : "Extract ZIP"}</button>
        {extracted === extractionKey && <p className="file-hint">Check File preparation &amp; extraction below for the result. Your original ZIP is kept.</p>}
      </details>}
      {(transfer.downloadPath || transfer.path) && <p className="saved-path">Saved to: <span dir="auto">{transfer.downloadPath || transfer.path}</span></p>}
      {transfer.status === "error" && <p className="error-text" role="alert">{transfer.error ? friendlyError(transfer.error) : "Ask the other device to try again."}</p>}
    </section>;
}
