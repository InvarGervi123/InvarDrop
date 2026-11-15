import { useEffect, useRef, useState } from "react";
import { formatBytes } from "./FileTransfer";
import type { FileTransfer, ResumeView, SecuritySnapshot } from "./useSecurity";

type Operate = (command: string, args: Record<string, unknown>) => Promise<boolean>;
const active = (transfer: FileTransfer | null) => transfer && !["complete", "rejected", "expired", "cancelled", "error"].includes(transfer.status);

function ResumeRow({ job, snapshot, operate }: { job: ResumeView; snapshot: SecuritySnapshot; operate: Operate }) {
  const busy = useRef(false);
  const [pending, setPending] = useState(false);
  const [confirmed, setConfirmed] = useState<string | null>(null);
  const [submitted, setSubmitted] = useState<string | null>(null);
  const [discarding, setDiscarding] = useState<string | null>(null);
  const [discarded, setDiscarded] = useState(false);
  const keepButton = useRef<HTMLButtonElement>(null);
  const discardButton = useRef<HTMLButtonElement>(null);
  const wasDiscardOpen = useRef(false);
  const peers = snapshot.connections.filter(connection => connection.peer === job.peer && connection.phase === "connected");
  const connection = peers.find(connection => connection.supportsResume && !active(connection.transfer) && connection.transfer?.id !== job.id);
  const retryNeedsReconnect = !connection && peers.some(connection => connection.transfer?.id === job.id);
  const jobActive = snapshot.connections.some(connection => connection.transfer?.id === job.id && active(connection.transfer));
  // A new connection or checkpoint requires a fresh, specific user decision.
  const decision = `${job.id}:${job.peer}:${job.name}:${job.size}:${job.fileCount}:${job.isFolder}:${job.verified}:${connection?.id ?? "offline"}`;
  const verified = job.completed ? job.size : Math.min(job.size, Math.max(0, job.verified));
  const remaining = Math.max(0, job.size - verified);
  const percentage = job.completed ? 100 : job.size > 0 ? Math.floor(verified / job.size * 1000) / 10 : 0;
  const answered = submitted === decision;
  const discardOpen = discarding === decision;
  useEffect(() => {
    if (discardOpen) keepButton.current?.focus();
    else if (wasDiscardOpen.current) discardButton.current?.focus();
    wasDiscardOpen.current = discardOpen;
  }, [discardOpen]);
  const resume = async () => {
    if (busy.current || !connection || jobActive || confirmed !== decision || answered || discardOpen || discarded || job.completed) return;
    busy.current = true; setPending(true);
    try {
      if (await operate("resume_transfer", { id: connection.id, transferId: job.id })) {
        setSubmitted(decision); setConfirmed(null);
      }
    } catch { /* The app reports IPC failures; leave the action available for retry. */ }
    finally { busy.current = false; setPending(false); }
  };
  const discard = async () => {
    if (busy.current || jobActive || !discardOpen || discarded) return;
    busy.current = true; setPending(true);
    try {
      if (await operate("resume_discard", { transferId: job.id })) {
        setDiscarded(true); setDiscarding(null); setConfirmed(null);
      }
    } catch { /* The app reports IPC failures; leave the action available for retry. */ }
    finally { busy.current = false; setPending(false); }
  };
  const status = discarded ? "Recovery record discarded"
    : job.completed ? "Completed · awaiting acknowledgement"
    : jobActive ? "Transfer active · use Cancel to stop"
    : retryNeedsReconnect ? "Reconnect to retry this transfer"
    : answered ? job.direction === "incoming" ? "Resume requested · waiting for the sender" : "Waiting for receiver approval…"
    : job.requested && job.direction === "outgoing" ? "Peer requests resume"
    : connection ? "Ready to resume with your approval"
    : peers.some(peer => peer.supportsResume) ? "Connection busy with another transfer"
    : peers.length ? "Peer does not support resume"
    : "Waiting for trusted device to reconnect";
  return <li className="resume-card file-request" aria-label={`${job.name} resumable transfer`}>
    <p className="file-name" dir="auto">{job.name}</p>
    <p className="file-size">{job.direction === "incoming" ? "Receiving" : "Sending"}{job.isFolder ? ` · ${job.fileCount} files · folder` : " · file"} · {formatBytes(job.size)}</p>
    <p className="peer-key">Paired key <span dir="ltr">{job.peer.slice(0, 16)}</span></p>
    <div className="transfer-metrics">
      <progress aria-label="Previously verified data" aria-valuetext={`${percentage}% · ${formatBytes(verified)} verified · ${formatBytes(remaining)} remaining`} max={100} value={percentage} />
      <p>{formatBytes(verified)} verified · {formatBytes(remaining)} remaining · {percentage}%</p>
      <p>Speed — · {job.completed ? "completed" : "waiting to resume"}</p>
    </div>
    <p className="request-status" role="status">{status}</p>
    {discardOpen ? <div role="group" aria-label={`Confirm discard for ${job.name}`}>
      <p className="file-hint">{job.completed ? "Remove only the saved acknowledgement record? Your completed files stay saved." : "Discard saved recovery data for this transfer? You will need to send it again from the beginning."}</p>
      <div className="request-actions">
        <button ref={keepButton} type="button" className="secondary" disabled={pending} onClick={() => setDiscarding(null)}>Keep</button>
        <button type="button" className="danger" disabled={pending || jobActive || discarded} onClick={() => void discard()}>{job.completed ? "Discard acknowledgement" : "Discard saved data"}</button>
      </div>
    </div> : <>
      {!job.completed && <label className="comparison-check"><input type="checkbox" checked={confirmed === decision} disabled={pending || !connection || Boolean(jobActive) || answered || discarded} onChange={event => setConfirmed(event.target.checked ? decision : null)} />I want to resume this transfer</label>}
      <div className="request-actions">
        {!job.completed && <button type="button" disabled={pending || !connection || Boolean(jobActive) || confirmed !== decision || answered || discarded} onClick={() => void resume()}>Resume</button>}
        <button ref={discardButton} type="button" className="secondary" disabled={pending || Boolean(jobActive) || discarded} onClick={() => { setDiscarding(decision); setConfirmed(null); }}>Discard</button>
      </div>
    </>}
  </li>;
}

export function ResumeTransfers({ snapshot, operate }: { snapshot: SecuritySnapshot; operate: Operate }) {
  const jobs = snapshot.resumable.filter(job => snapshot.trusted.includes(job.peer) && !snapshot.revoked.includes(job.peer));
  if (!jobs.length) return null;
  return <section className="pair-card resume-transfers" aria-labelledby="resume-title">
    <h2 id="resume-title">Transfer recovery</h2>
    <p className="file-hint">Verified data is kept for recovery. Resume requires approval; Discard removes the saved recovery record.</p>
    <ul className="resume-list">{jobs.map(job => <ResumeRow key={job.id} job={job} snapshot={snapshot} operate={operate} />)}</ul>
  </section>;
}
