import { useState } from "react";
import { ErrorNotice, friendlyError } from "./ErrorNotice";
import { ResumeTransfers } from "./ResumeTransfers";
import { TrustSettings } from "./TrustSettings";
import { transferActive } from "./Activity";
import type { BluetoothSnapshot } from "./useBluetooth";
import type { Device } from "./useDiscovery";
import type { SecuritySnapshot } from "./useSecurity";

type Operate = (command: string, args: Record<string, unknown>, done?: (result: unknown) => void) => Promise<boolean>;

export function Settings({ security, bluetooth, devices, bluetoothAllowed, fileOptions, offerConversion, setOfferConversion,
  lan, lastError, busy, operate, onBluetooth }: {
  security: SecuritySnapshot | null;
  bluetooth: BluetoothSnapshot | null;
  devices: Device[];
  bluetoothAllowed: boolean;
  fileOptions: boolean;
  offerConversion: boolean;
  setOfferConversion: (value: boolean) => void;
  lan: string;
  lastError: string | null;
  busy: boolean;
  operate: Operate;
  onBluetooth: (enabled: boolean) => void;
}) {
  const [recovery, setRecovery] = useState<string | null>(null);
  const dial = (command: string, id: string) => void operate(command, { id }, result => {
    if (typeof result === "string") setRecovery(result);
  });
  const recoveryConnection = security?.connections.find(connection => connection.id === recovery);
  const recoverable = security?.resumable.filter(job =>
    !job.completed && security.trusted.includes(job.peer) && !security.revoked.includes(job.peer)
    && !security.connections.some(connection => connection.peer === job.peer && connection.phase === "connected"
      && connection.supportsResume && !transferActive(connection.transfer) && connection.transfer?.id !== job.id)) ?? [];
  const pinned = security?.connections.slice().reverse().find(connection =>
    connection.phase === "closed" && connection.canReconnect
    && recoverable.some(job => job.peer === connection.peer));
  const connecting = recovery !== null && (!recoveryConnection || ["connecting", "verifying", "committing"].includes(recoveryConnection.phase));
  const errors = [...new Set([lastError, security?.error, bluetooth?.error].filter((error): error is string => Boolean(error)))];
  return <div className="settings-options">
    {bluetooth?.supported !== false && <section className="settings-section">
      <h2>Nearby sharing</h2>
      <p>Use Bluetooth to share when the local network is out of reach. Turning it off interrupts Bluetooth transfers.</p>
      <div className="request-actions">
        {bluetoothAllowed && ["starting", "ready", "scanning", "paused"].includes(bluetooth?.status ?? "")
          ? <><button type="button" className="secondary" disabled={busy} onClick={() => onBluetooth(false)}>Turn off Bluetooth sharing</button>
            <button type="button" className="quiet" disabled={busy} onClick={() => void operate("bluetooth_scan", {})}>Scan now</button></>
          : <button type="button" className="secondary" disabled={busy} onClick={() => onBluetooth(true)}>Enable Bluetooth sharing</button>}
        {bluetoothAllowed && bluetooth?.status === "unavailable" &&
          <button type="button" className="quiet" disabled={busy} onClick={() => onBluetooth(false)}>Keep Bluetooth sharing off</button>}
      </div>
    </section>}
    {fileOptions && <section className="settings-section">
      <h2>File options</h2>
      <label className="comparison-check"><input type="checkbox" checked={offerConversion} onChange={event => setOfferConversion(event.target.checked)} />Offer to convert OGG audio before sending</label>
    </section>}
    {security && <TrustSettings snapshot={security} operate={operate} busy={busy} />}
    {security && security.resumable.length > 0 && <section className="settings-section">
      <ResumeTransfers snapshot={security} operate={operate} />
      {recoverable.length > 0 && <div className="recovery-connect">
        <p>Open InvarDrop on the other device to continue an interrupted transfer.</p>
        <div className="request-actions">
          {pinned && <button type="button" className="secondary" disabled={busy || connecting} onClick={() => dial("secure_reconnect", pinned.id)}>Reconnect to last device</button>}
          {devices.filter(device => device.canConnect).map(device =>
            <button key={device.id} type="button" className="secondary" disabled={busy || connecting} onClick={() => dial("secure_connect", device.id)}>Reconnect to <bdi>{device.name}</bdi></button>)}
          {connecting && recoveryConnection && <button type="button" className="quiet" onClick={() => void operate("secure_cancel", { id: recoveryConnection.id })}>Cancel connection</button>}
        </div>
        {connecting && <p role="status">Connecting for recovery…</p>}
        {recoveryConnection?.phase === "closed" && recoveryConnection.error && recoveryConnection.error !== "Connection cancelled" &&
          <p role="alert" className="error-text">{friendlyError(recoveryConnection.error)}</p>}
      </div>}
    </section>}
    <details className="trust-settings diagnostics">
      <summary>Connection diagnostics</summary>
      <p>Local network: {lan}. Bluetooth: {bluetooth?.status ?? "unknown"}.</p>
      {errors.map(error => <ErrorNotice key={error} message={error} />)}
      {security?.connections.slice(-8).reverse().map(connection => <div className="diagnostic-row" key={connection.id}>
        <p>{connection.transport === "bluetooth" ? "Bluetooth RFCOMM" : "LAN"} · {connection.phase}{connection.peer && <> · <code>{connection.peer.slice(0, 16)}</code></>}</p>
        {connection.error && <ErrorNotice message={connection.error} />}
        {connection.transfer?.error && connection.transfer.error !== connection.error && <ErrorNotice message={connection.transfer.error} />}
        {connection.phase === "connected" && <button type="button" className="secondary" onClick={() => void operate("secure_cancel", { id: connection.id })}>Disconnect</button>}
      </div>)}
    </details>
  </div>;
}
