import { useRef, useState } from "react";
import type { SecureConnection } from "./useSecurity";
import { Icon } from "./Icon";

type Operate = (command: string, args: Record<string, unknown>) => Promise<boolean>;

export function PairCard({ connection, operate, recipient, cancel }: {
  connection: SecureConnection; operate: Operate; recipient?: string; cancel?: () => void;
}) {
  const { id, code, phase, localApproved } = connection;
  const challenge = id + ":" + code;
  const [compared, setCompared] = useState<string | null>(null);
  const [submitted, setSubmitted] = useState<{ challenge: string; approve: boolean } | null>(null);
  const [pending, setPending] = useState(false);
  const lock = useRef(false);
  const answered = submitted?.challenge === challenge;
  const respond = async (approve: boolean) => {
    if (lock.current || answered || localApproved || phase !== "verifying" || !code || (approve && compared !== challenge)) return;
    lock.current = true;
    setPending(true);
    try {
      if (await operate("secure_respond", { id, approve, code: approve ? code : "" })) setSubmitted({ challenge, approve });
    } finally {
      lock.current = false;
      setPending(false);
    }
  };
  if (phase !== "verifying" || !code) return null;
  return <section className="activity-card pairing-card" aria-label="Pairing verification">
    <div className="activity-heading"><span className="tile-icon"><Icon name="shield" /></span>
      <div><h2>Pair devices</h2>{recipient && <p dir="auto">{recipient}</p>}</div></div>
    <p>Compare every group with the code shown on the other device. Pair only if all groups match.</p>
    <code className="verification-code" aria-label="Verification code" dir="ltr">{code}</code>
    {localApproved || answered ? <>
      <p role="status">{answered && !submitted.approve ? "Declining pairing…" : "Waiting for the other device to confirm…"}</p>
      {(localApproved || submitted?.approve) && <button type="button" className="secondary" onClick={cancel ?? (() => void operate("secure_cancel", { id }))}>Cancel pairing</button>}
    </> : <>
      <label className="comparison-check"><input type="checkbox" checked={compared === challenge} disabled={pending} onChange={event => setCompared(event.target.checked ? challenge : null)} />I compared the full code on both devices</label>
      <div className="request-actions">
        <button type="button" className="secondary" disabled={pending} onClick={() => void respond(false)}>Reject</button>
        <button type="button" disabled={compared !== challenge || pending} onClick={() => void respond(true)}>Codes match — pair</button>
      </div>
    </>}
  </section>;
}
