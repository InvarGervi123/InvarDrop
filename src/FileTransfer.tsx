import { useState } from "react";
import type { SecureConnection } from "./useSecurity";

type Operate = (command: string, args: Record<string, unknown>) => Promise<boolean>;
const statusText: Record<string, string> = {
  offering: "Waiting for the receiver to accept…",
  preparing: "Preparing the file…",
  sending: "Sending file…",
  receiving: "Receiving file…",
  finalizing: "Verifying and saving file…",
  awaiting_confirmation: "Waiting for the receiver to verify the file…",
  complete: "Transfer complete · BLAKE3 verified",
  rejected: "File declined. You can choose another file.",
  error: "Transfer failed. Reconnect before trying again.",
};

export function FileTransfer({ connection, operate }: { connection: SecureConnection; operate: Operate }) {
  const [pending, setPending] = useState(false);
  const transfer = connection.transfer;
  const active = transfer && !["complete", "rejected", "error"].includes(transfer.status);
  const run = async (command: string, args: Record<string, unknown>) => {
    if (pending) return;
    setPending(true);
    try { await operate(command, args); }
    finally { setPending(false); }
  };
  return <div className="file-transfer">
    {connection.phase === "connected" && <button type="button" disabled={pending || Boolean(active)}
      onClick={() => void run("file_choose", { id: connection.id })}>Choose a file to send</button>}
    {transfer && <section aria-label="File transfer">
      <p className="file-name" dir="auto">{transfer.name}</p>
      <p>{transfer.size.toLocaleString()} bytes · {transfer.direction === "incoming" ? "Incoming" : "Outgoing"}</p>
      <p role="status">{transfer.status === "awaiting_approval"
        ? transfer.direction === "incoming" ? "Incoming file — accept only if you want to save it." : "Waiting for the receiver to accept…"
        : transfer.status === "finalizing" && transfer.direction === "outgoing" ? "Waiting for the receiver to verify the file…"
        : statusText[transfer.status] ?? "Processing file…"}</p>
      {transfer.direction === "incoming" && transfer.status === "awaiting_approval" && connection.phase === "connected" && <>
        <button type="button" disabled={pending} onClick={() => void run("file_respond", { id: connection.id, transferId: transfer.id, accept: true })}>Accept file</button>
        <button type="button" className="secondary" disabled={pending} onClick={() => void run("file_respond", { id: connection.id, transferId: transfer.id, accept: false })}>Decline file</button>
      </>}
      {transfer.path && <p className="saved-path">Saved to: <span dir="auto">{transfer.path}</span></p>}
      {transfer.error && <p className="error-text" role="alert">{transfer.error}</p>}
    </section>}
  </div>;
}
