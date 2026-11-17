import { useRef, useState } from "react";
import { ErrorNotice } from "./ErrorNotice";
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
  sending: "Transferring · sending file…", receiving: "Transferring · receiving file…",
  cancelling: "Cancelling…", cancelled: "Cancelled", committing: "Saving verified file…",
  finalizing: "Verifying and saving…", complete: "Completed · BLAKE3 verified",
  rejected: "Declined", expired: "Request expired", error: "Transfer failed",
};

export function FileTransfer({ connection, operate }: { connection: SecureConnection; operate: Operate }) {
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
  const run = async (accept?: boolean, folder = false) => {
    if (busy.current || (accept !== undefined && (!transfer || answered || !waiting || (accept && confirmedId !== transfer.id)))) return;
    busy.current = true; setPending(true);
    // Lock this request immediately, including the interval before the backend event.
    const request = transfer?.id;
    if (accept !== undefined && request) setRespondedId(request);
    try {
      const ok = accept === undefined
        ? await operate(folder ? "folder_choose" : "file_choose", { id: connection.id })
        : await operate("file_respond", { id: connection.id, transferId: request, accept });
      if (!ok && accept !== undefined) setRespondedId(null);
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
  return <div className="file-transfer">
    <div className="send-actions">
    {connection.phase === "connected" && !active && <button type="button" disabled={pending || Boolean(active)} onClick={() => void run()}><Icon name="send" />Choose {connection.supportsBundles ? "files" : "a file"} to send</button>}
    {connection.phase === "connected" && connection.supportsBundles && !active && <button type="button" className="secondary" disabled={pending} onClick={() => void run(undefined, true)}><Icon name="folder" />Choose a folder to send</button>}
    </div>
    {transfer && <section className="file-request" aria-label={incoming ? transfer.isFolder ? "Incoming folder request" : "Incoming file request" : "Outgoing file"}>
      {incoming && <p className="sender-name">From <strong dir="auto">{transfer.senderName ?? "Paired device"}</strong><small>Device-reported name · verified key shown above</small></p>}
      <p className="file-name" dir="auto">{transfer.name}</p>
      <p className="file-size" title={`${transfer.size.toLocaleString("en")} bytes`}>{formatBytes(transfer.size)}{transfer.isFolder && <> · {transfer.fileCount} files · folder</>} · {incoming ? "Incoming" : "Outgoing"}</p>
      <p className={`request-status request-${transfer.status}`} role="status">{status}</p>
      <div className="transfer-metrics" aria-label="Transfer statistics">
        <progress aria-label="File data progress" aria-valuetext={`${progressPercent(transfer)}% · ${formatBytes(transfer.transferred)} of ${formatBytes(transfer.size)}`} max={100} value={progressPercent(transfer)} />
        <p>{progressPercent(transfer)}% · {formatBytes(transfer.transferred)} / {formatBytes(transfer.size)}</p>
        {transfer.resuming && <p>Resuming verified data · {formatBytes(Math.max(0, transfer.size - transfer.transferred))} remaining</p>}
        <p>{!measuring || transfer.bytesPerSecond === null ? "Speed —" : `${(transfer.bytesPerSecond / 1024 ** 2).toFixed(2)} MiB/s`}
          {measuring && transfer.etaSeconds !== null && <> · ETA ~{formatEta(transfer.etaSeconds)}</>}</p>
      </div>
      {connection.phase === "connected" && active && !(incoming && waiting) && transfer.status !== "committing" && <button type="button" className="secondary" disabled={pending || cancelId === transfer.id || transfer.status === "cancelling"} onClick={() => void cancelTransfer()}>Cancel transfer</button>}
      {connection.supportsResume && active && <p className="file-hint">Cancel stops this operation and removes incomplete received data. Discard removes a saved recovery record.</p>}
      {transfer.status === "cancelled" && <p className="file-hint">Transfer stopped. Reconnect to send another file.</p>}
      {incoming && waiting && connection.phase === "connected" && <>
        <label className="comparison-check"><input type="checkbox" checked={confirmedId === transfer.id} disabled={pending || Boolean(answered)} onChange={event => setConfirmedId(event.target.checked ? transfer.id : null)} />I want to {transfer.resuming ? "resume" : "save"} this {transfer.isFolder ? "folder" : "file"}</label>
        <div className="request-actions">
          <button type="button" className="secondary" disabled={pending || Boolean(answered)} onClick={() => void run(false)}>Decline {transfer.resuming ? "resume" : transfer.isFolder ? "folder" : "file"}</button>
          <button type="button" disabled={pending || Boolean(answered) || confirmedId !== transfer.id} onClick={() => void run(true)}>Accept {transfer.resuming ? "resume" : transfer.isFolder ? "folder" : "file"}</button>
        </div>
        <p className="file-hint">{answered ? "Decision sent…" : transfer.resuming ? "Saved data stays isolated. Resume starts only after you accept; declining keeps the recovery data. The request expires after 90 seconds." : "Nothing is saved until you accept. The request expires after 90 seconds."}</p>
      </>}
      {incoming && ["rejected", "expired"].includes(transfer.status) && <p className="file-hint">{transfer.resuming ? "Resume did not start. Saved recovery data is kept." : "No file was saved."}</p>}
      {incoming && transfer.status === "complete" && !transfer.isFolder && transfer.name.toLowerCase().endsWith(".zip") && <div className="zip-extraction">
        <h3>ZIP extraction (optional)</h3>
        <p className="file-hint">The verified ZIP stays saved. Extract a separate copy only if you want to; extracted content is never opened or run.</p>
        <label className="comparison-check"><input type="checkbox" checked={extractConfirmed === extractionKey} disabled={extracted === extractionKey} onChange={event => setExtractConfirmed(event.target.checked ? extractionKey : null)} />I want to extract this ZIP</label>
        <button type="button" disabled={extractConfirmed !== extractionKey || extracted === extractionKey} onClick={() => void extract()}>{extracted === extractionKey ? "Extraction requested" : "Extract ZIP"}</button>
        {extracted === extractionKey && <p className="file-hint">Check File preparation &amp; extraction below for the result. Your original ZIP is kept.</p>}
      </div>}
      {(transfer.downloadPath || transfer.path) && <p className="saved-path">Saved to: <span dir="auto">{transfer.downloadPath || transfer.path}</span></p>}
      {transfer.error && transfer.status !== "expired" && <ErrorNotice message={transfer.error} />}
    </section>}
  </div>;
}
