import { useEffect, useRef, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { formatBytes } from "./FileTransfer";
import { ErrorNotice } from "./ErrorNotice";

interface Job { id: string; name: string; state: string; done: number; total: number; error: string | null; path: string | null }
const labels: Record<string, string> = {
  choice: "Waiting for your choice", converting: "Converting audio…", sending: "Preparing transfer offer…",
  prepared: "Preparation complete", extracting: "Extracting ZIP…", complete: "Extraction completed",
  failed: "Processing failed", cancelled: "Processing cancelled",
};
const terminal = (state: string) => ["prepared", "complete", "failed", "cancelled"].includes(state);
export function ProcessingPanel() {
  const [jobs, setJobs] = useState<Job[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [pollError, setPollError] = useState(false);
  const [converter, setConverter] = useState<boolean | null>(null);
  const [pending, setPending] = useState(new Set<string>());
  const [submitted, setSubmitted] = useState(new Set<string>());
  const busy = useRef(new Set<string>());
  const acknowledged = useRef(new Set<string>());
  const dismissed = useRef(new Set<string>());
  const panel = useRef<HTMLElement>(null);
  const seenJobs = useRef(new Set<string>());
  const native = isTauri();
  useEffect(() => {
    // Reveal a newly requested processing stage without focusing an approval button.
    if (jobs.some(job => !seenJobs.current.has(job.id))) panel.current?.scrollIntoView?.({ block: "nearest" });
    seenJobs.current = new Set(jobs.map(job => job.id));
  }, [jobs]);
  useEffect(() => {
    if (!native) return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    // Schedule only after the previous request settles; slow IPC cannot build a queue.
    const refresh = async () => {
      try {
        const next = await invoke<Job[]>("processing_snapshot");
        if (!disposed) {
          const visible = next.filter(job => !dismissed.current.has(job.id));
          const keys = ["id", "name", "state", "done", "total", "error", "path"] as const;
          setJobs(previous => previous.length === visible.length && previous.every((job, i) => keys.every(key => job[key] === visible[i][key])) ? previous : visible);
          setPollError(false);
          const ids = new Set(visible.map(job => job.id));
          let changed = false;
          for (const key of acknowledged.current) {
            if (!ids.has(key.slice(key.indexOf(":") + 1))) { acknowledged.current.delete(key); changed = true; }
          }
          if (changed) setSubmitted(new Set(acknowledged.current));
        }
      } catch { if (!disposed) setPollError(true); }
      if (!disposed) timer = setTimeout(() => void refresh(), 500);
    };
    void refresh();
    void invoke<boolean>("processing_converter_status").then(value => { if (!disposed) setConverter(value); }).catch(() => { if (!disposed) setError("Could not check FFmpeg selection. Choose a trusted executable to configure conversion."); });
    return () => { disposed = true; clearTimeout(timer); };
  }, [native]);
  const run = async (command: string, args: Record<string, unknown> = {}, id = command) => {
    const key = `${command}:${id}`;
    if (busy.current.has(key) || acknowledged.current.has(key)) return;
    busy.current.add(key); setPending(new Set(busy.current)); setError(null);
    try {
      await invoke(command, args);
      if (command === "processing_converter") setConverter(await invoke<boolean>("processing_converter_status"));
      if (command === "processing_decide" || command === "processing_cancel") {
        // Keep the action disabled until the next backend state, even after fast IPC.
        acknowledged.current.add(key); setSubmitted(new Set(acknowledged.current));
      }
      if (command === "processing_dismiss") { dismissed.current.add(id); setJobs(previous => previous.filter(job => job.id !== id)); }
    } catch (failure) { setError(typeof failure === "string" ? failure : "Processing action failed. Original files are kept; try again."); }
    finally { busy.current.delete(key); setPending(new Set(busy.current)); }
  };
  const settings = <details className="trust-settings processing-settings"><summary>Optional audio conversion<span className="summary-detail">{!native ? "Available in the desktop app" : converter === null ? "Checking FFmpeg selection…" : converter ? "FFmpeg selected" : "FFmpeg not selected"}</span></summary>
    <p>Keep originals is the default. OGG conversion is offered after choosing files; ZIP extraction is offered only after a verified transfer.</p>
    <p>{converter ? "FFmpeg is selected for this session. Codec support is checked during conversion." : "FFmpeg is not bundled or downloaded. Sending originals and extracting ZIP files do not require it."}</p>
    <button type="button" className="secondary" disabled={!native || pending.has("processing_converter:processing_converter")} onClick={() => void run("processing_converter")}>{pending.has("processing_converter:processing_converter") ? "Choosing executable…" : "Choose FFmpeg executable"}</button>
    <p>Choose only a trusted local FFmpeg executable. Your selection is cleared when the app closes.</p>
  </details>;
  return <section ref={panel} className={jobs.length ? "pair-card processing-panel" : "processing-panel"} aria-label="Optional file processing">
    {jobs.length > 0 && <><h2>File preparation &amp; extraction</h2><p className="file-hint">Separate from transfer. Original files stay unchanged.</p></>}
    {pollError && <ErrorNotice message="Could not refresh processing status. Retrying automatically." />}
    {error && <ErrorNotice message={error} />}
    {jobs.map(job => {
      const deciding = pending.has(`processing_decide:${job.id}`) || submitted.has(`processing_decide:${job.id}`);
      const cancelling = pending.has(`processing_cancel:${job.id}`) || submitted.has(`processing_cancel:${job.id}`);
      const finished = terminal(job.state);
      const extraction = ["extracting", "complete"].includes(job.state);
      return <section key={job.id} className="file-request processing-job" aria-label={`${job.name} processing`}>
        <p className="file-name" dir="auto">{job.name}</p>
        <p className={`request-status request-${job.state}`} role="status">{cancelling && !finished ? "Cancelling processing…" : labels[job.state] ?? "Processing…"}</p>
        {job.state === "converting" || job.state === "sending" || (job.total === 0 && !finished) ? <progress aria-label="Processing progress" /> : <progress aria-label="Processing progress" max={finished && job.total === 0 ? 100 : Math.max(1, job.total)} value={finished && job.total === 0 ? ["complete", "prepared"].includes(job.state) ? 100 : 0 : job.done} />}
        {job.total > 0 && !["failed", "cancelled"].includes(job.state) && <p className="file-hint">{extraction ? `${formatBytes(job.done)} / ${formatBytes(job.total)} extracted` : `${job.done} / ${job.total} audio files prepared`}</p>}
        {job.state === "choice" && <>
          <p>OGG audio found. Keep its original format, or convert it to MP3 before offering the transfer. The receiver will see the prepared names and sizes.</p>
          {converter !== true && <p className="file-hint">Conversion needs a selected FFmpeg executable. You can keep originals now or choose FFmpeg in Optional audio conversion below.</p>}
          <div className="request-actions"><button type="button" disabled={deciding || cancelling} onClick={() => void run("processing_decide", { id: job.id, convert: false }, job.id)}>Keep originals</button><button type="button" className="secondary" disabled={converter !== true || deciding || cancelling} onClick={() => void run("processing_decide", { id: job.id, convert: true }, job.id)}>Convert OGG to MP3</button></div>
        </>}
        {["choice", "converting", "extracting"].includes(job.state) && <button type="button" className="secondary connection-action" disabled={cancelling || (job.state === "choice" && deciding)} onClick={() => void run("processing_cancel", { id: job.id }, job.id)}>Cancel processing</button>}
        {finished && <button type="button" className="secondary" disabled={pending.has(`processing_dismiss:${job.id}`)} onClick={() => void run("processing_dismiss", { id: job.id }, job.id)}>Dismiss</button>}
        {job.state === "prepared" && <p className="file-hint">Preparation finished. Check the transfer above for receiver approval and delivery.</p>}
        {finished && <p className="file-hint">Dismiss hides this result. Originals, received files, and prepared resume data are kept.</p>}
        {job.error && <ErrorNotice message={job.error} />}
        {job.path && <p className="saved-path">Extracted to: <span dir="auto">{job.path}</span></p>}
      </section>;
    })}
    {settings}
  </section>;
}
