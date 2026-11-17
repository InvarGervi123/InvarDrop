import { useRef, useState } from "react";
import type { SecureConnection } from "./useSecurity";
import { FileTransfer } from "./FileTransfer";
import { ErrorNotice } from "./ErrorNotice";
import { Icon } from "./Icon";

type Operate = (command: string, args: Record<string, unknown>) => Promise<boolean>;
export function PairCard({ connection: c, operate }: { connection: SecureConnection; operate: Operate }) {
  const incoming = c.transfer?.direction === "incoming";
  const challenge = `${c.id}:${c.code}`;
  const [compared, setCompared] = useState<string | null>(null);
  const [submitted, setSubmitted] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const lock = useRef(false);
  const respond = async (approve: boolean) => {
    if (lock.current || submitted === challenge || c.localApproved || c.phase !== "verifying" || !c.code || (approve && compared !== challenge)) return;
    lock.current = true; setPending(true);
    try { if (await operate("secure_respond", { id: c.id, approve, code: approve ? c.code : "" })) setSubmitted(challenge); }
    catch { /* The app reports IPC errors; allow a deliberate retry. */ }
    finally { lock.current = false; setPending(false); }
  };
  const title = c.phase === "connected" ? incoming && c.transfer?.status === "awaiting_approval" ? c.transfer?.isFolder ? "Incoming folder request" : "Incoming file request" : "Secure connection" : c.phase === "verifying" ? "Compare on both devices" : c.phase === "closed" ? "Connection closed" : c.phase === "committing" ? "Saving approved trust…" : "Connecting securely…";
  return <section className="pair-card" aria-label={c.phase === "verifying" ? "Pairing verification" : "Connection status"}>
    <div className="section-heading"><h2><Icon name="shield" />{title}</h2><span className="status">{c.phase === "connected" ? "Trusted key" : c.phase === "verifying" ? "Not trusted yet" : c.phase === "closed" ? "Disconnected" : "Verifying"}</span></div>
    {c.peer && <p className="peer-key">Key: <code dir="ltr">{c.peer.slice(0, 16)}</code></p>}
    {c.phase === "verifying" && c.code && <>
      <p>Compare every group on both devices. Pair only if all match. A matching device name is not proof of identity.</p>
      <code className="verification-code" aria-label="Verification code" dir="ltr">{c.code}</code>
      {c.localApproved || submitted === challenge ? <p role="status">Waiting for the pairing decision… Cancellation does not revoke saved trust.</p> : <>
        <label className="comparison-check"><input type="checkbox" checked={compared === challenge} disabled={pending} onChange={e => setCompared(e.target.checked ? challenge : null)} />I compared the full code on both devices</label>
        <div className="request-actions"><button type="button" className="secondary" disabled={pending} onClick={() => void respond(false)}>Reject</button><button type="button" disabled={compared !== challenge || pending} onClick={() => void respond(true)}>Codes match — pair</button></div>
      </>}
    </>}
    {c.phase === "connected" && !incoming && <p className="file-hint">Peer key verified over TLS 1.3. Choose {c.supportsBundles ? "files or a folder" : "a file"}; the receiver approves each transfer.</p>}
    <FileTransfer connection={c} operate={operate} />
    {c.error && c.error !== c.transfer?.error && <ErrorNotice message={c.error} />}
    {c.phase !== "closed" && <button type="button" className="secondary connection-action" onClick={() => void operate("secure_cancel", { id: c.id })}>{c.phase === "connected" ? "Disconnect" : "Cancel connection"}</button>}
    {c.phase === "closed" && c.canReconnect && <button type="button" className="secondary" onClick={() => void operate("secure_reconnect", { id: c.id })}>Reconnect with pinned key</button>}
  </section>;
}
