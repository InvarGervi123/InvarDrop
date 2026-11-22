import { Icon } from "./Icon";
import type { useShare } from "./useShare";

type Share = ReturnType<typeof useShare>;

export function ShareComposer({ share, ready, folders }: { share: Share; ready: boolean; folders: boolean }) {
  const { draft, picking, intent } = share;
  const disabled = Boolean(picking || intent);
  return <section className="panel composer-panel" aria-labelledby="composer-title">
    <header className="panel-header">
      <h2 id="composer-title">Files to share</h2>
      <Icon name="file" />
    </header>
    {draft ? <div className="file-tile">
      <span className="tile-icon"><Icon name={draft.folder ? "folder" : "file"} /></span>
      <span className="tile-copy"><strong dir="auto">{draft.name}</strong>
        <small>{draft.folder ? "Folder selected" : draft.count === 1 ? "File selected" : "Files selected"}</small></span>
      <button type="button" className="icon-button quiet" aria-label="Remove selected files" disabled={disabled} onClick={() => void share.clear()}><Icon name="close" /></button>
    </div> : <button type="button" className="picker-tile" disabled={disabled || !ready} onClick={() => void share.pick(false)}>
      <span className="tile-icon"><Icon name="add" /></span>
      <span className="tile-copy"><strong>{picking ? "Choosing files…" : "Choose files"}</strong><small>Browse this device</small></span>
    </button>}
    <div className="file-options">
      {draft && <button type="button" className="quiet" disabled={disabled} onClick={() => void share.pick(false)}>{picking ? "Choosing…" : "Change files"}</button>}
      {folders && <button type="button" className="quiet" disabled={disabled || !ready} onClick={() => void share.pick(true)}><Icon name="folder" /> Choose a folder</button>}
    </div>
  </section>;
}

export function SendAction({ share, available, ready, offerConversion }: {
  share: Share; available: boolean; ready: boolean; offerConversion: boolean;
}) {
  const { draft, picking, intent, connection, device } = share;
  const sending = intent?.stage === "submitting" ? "Preparing to send…" : connection?.phase === "verifying" ? "Pair to continue" : "Connecting…";
  const hint = intent ? "With " : !device ? "Choose a nearby device to send to." : "To ";
  return <div className="composer-footer">
    <p className="recipient" role="status">{intent || device ? <>{hint}<strong dir="auto">{intent?.device.name ?? device?.name}</strong>{!intent && !available ? " · Unavailable" : ""}</> : hint}</p>
    <button type="button" className="send-button" disabled={!ready || !draft || !available || Boolean(picking || intent)}
      onClick={() => void share.send(offerConversion)}>
      <Icon name="send" />{intent ? sending : draft?.folder ? "Send folder" : "Send files"}
    </button>
    {connection && intent?.stage === "connecting" && connection.phase === "connecting" &&
      <button type="button" className="quiet" disabled={share.cancelling} onClick={() => void share.cancel()}>Cancel connection</button>}
  </div>;
}
