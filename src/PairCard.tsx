import { useState } from "react";
import type { SecureConnection } from "./useSecurity";
import { FileTransfer } from "./FileTransfer";

type Operate = (command: string, args: Record<string, unknown>) => Promise<boolean>;
export function PairCard({ connection: c, operate }: { connection: SecureConnection; operate: Operate }) {
  const [compared, setCompared] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const approve = async () => {
    if (!compared || submitted || !c.code) return;
    setSubmitted(true);
    if (!await operate("secure_respond", { id: c.id, approve: true, code: c.code })) setSubmitted(false);
  };
  return (
    <section className="pair-card" aria-label={c.phase === "verifying" ? "Pairing verification" : "Connection status"}>
      <h3>{c.phase === "connected" ? "Secure connection" : c.phase === "verifying" ? "Compare on both devices" : c.phase === "closed" ? "Connection closed" : c.phase === "committing" ? "Saving approved trust…" : "Connecting securely…"}</h3>
      {c.peer && <p className="peer-key">Key: <span dir="ltr">{c.peer.slice(0, 16)}</span></p>}
      {c.phase === "verifying" && c.code && <>
        <p>Compare every group on both devices. Pair only if all match.</p>
        <code className="verification-code" aria-label="Verification code" dir="ltr">{c.code}</code>
        {c.localApproved || submitted ? <p role="status">Waiting for approval on the other device… Cancellation does not revoke saved trust.</p> : <>
          <label className="comparison-check"><input type="checkbox" checked={compared} onChange={e => setCompared(e.target.checked)} />I compared the full code on both devices</label>
          <button type="button" disabled={!compared || submitted} onClick={() => void approve()}>Codes match — pair</button>
        </>}
        {!c.localApproved && !submitted && <button type="button" className="secondary" onClick={() => void operate("secure_respond", { id: c.id, approve: false, code: "" })}>Reject</button>}
      </>}
      {c.phase === "connected" && <p>Peer key verified. Single files up to 4 GiB. The receiver approves each file.</p>}
      <FileTransfer connection={c} operate={operate} />
      {c.error && <p className="error-text">{c.error}</p>}
      {c.phase !== "closed" && <button type="button" className="secondary" onClick={() => void operate("secure_cancel", { id: c.id })}>{c.phase === "connected" ? "Disconnect" : "Cancel connection"}</button>}
      {c.phase === "closed" && c.canReconnect && <button type="button" className="secondary" onClick={() => void operate("secure_reconnect", { id: c.id })}>Reconnect with pinned key</button>}
    </section>
  );
}
