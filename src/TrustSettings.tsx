import { useEffect, useRef, useState } from "react";
import type { SecuritySnapshot } from "./useSecurity";

type Operate = (command: string, args: Record<string, unknown>) => Promise<boolean>;
function TrustedKey({ id, operate, busy }: { id: string; operate: Operate; busy: boolean }) {
  const [confirming, setConfirming] = useState(false);
  const [pending, setPending] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const lock = useRef(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const keep = useRef<HTMLButtonElement>(null);
  const wasConfirming = useRef(false);
  useEffect(() => {
    if (confirming) keep.current?.focus();
    else if (wasConfirming.current) trigger.current?.focus();
    wasConfirming.current = confirming;
  }, [confirming]);
  const revoke = async () => {
    if (!confirming || lock.current || submitted) return;
    lock.current = true; setPending(true);
    try { if (await operate("secure_revoke", { id })) setSubmitted(true); }
    catch { /* The app reports IPC errors; allow a deliberate retry. */ }
    finally { lock.current = false; setPending(false); }
  };
  return <li className="trusted-key">
    <div className="key-heading"><span className="status">Trusted key</span><code dir="ltr">{id.slice(0, 16)}</code></div>
    {confirming ? <div role="group" aria-label={`Confirm trust revocation for ${id.slice(0, 16)}`}>
      <p>Revoke trust for this key? This disconnects the device and invalidates all saved transfer recovery records on this device. Completed files are kept.</p>
      <div className="request-actions"><button type="button" ref={keep} className="secondary" disabled={pending || submitted} onClick={() => setConfirming(false)}>Keep trust</button><button type="button" className="danger" disabled={busy || pending || submitted} onClick={() => void revoke()}>{submitted ? "Revoking…" : "Confirm revoke"}</button></div>
    </div> : <button ref={trigger} type="button" className="secondary" disabled={busy} onClick={() => setConfirming(true)}>Revoke trust</button>}
  </li>;
}
export function TrustSettings({ snapshot, operate, busy }: { snapshot: SecuritySnapshot; operate: Operate; busy: boolean }) {
  if (!snapshot.identity) return null;
  return <details className="trust-settings"><summary>Device identity &amp; trusted peers<span className="summary-detail">{snapshot.trusted.length} trusted</span></summary>
    <p>Your public key fingerprint:</p><code className="fingerprint" dir="ltr">{snapshot.identity}</code>
    <p>Discovery names are unverified. Only the key and full comparison code establish trust.</p>
    {snapshot.trusted.length ? <ul>{snapshot.trusted.map(id => <TrustedKey key={id} id={id} operate={operate} busy={busy} />)}</ul> : <p>No trusted devices yet. Connect to a nearby device and compare the full code on both screens.</p>}
    {snapshot.revoked.length > 0 && <><p>Revoked keys. Allowing pairing again does not restore trust or old checkpoints.</p><ul>{snapshot.revoked.map(id => <li key={id}><code dir="ltr">{id.slice(0, 16)}</code><button type="button" className="secondary" disabled={busy} onClick={() => void operate("secure_unblock", { id })}>Allow new pairing</button></li>)}</ul></>}
  </details>;
}
