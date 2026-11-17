import { useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";
import { useDiscovery } from "./useDiscovery";
import { useSecurity } from "./useSecurity";
import { ResumeTransfers } from "./ResumeTransfers";
import { PairCard } from "./PairCard";
import { ProcessingPanel } from "./ProcessingPanel";
import { TrustSettings } from "./TrustSettings";
import { ErrorNotice } from "./ErrorNotice";
import { Icon } from "./Icon";

function App() {
  const { snapshot, bridgeError } = useDiscovery();
  const { snapshot: security, bridgeError: securityBridgeError } = useSecurity();
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(0);
  const operations = useRef(new Set<string>());
  const devices = snapshot?.devices ?? [];
  const unavailable = bridgeError || snapshot?.status === "unavailable" || snapshot?.status === "stopped";
  const ready = security?.status === "ready" && !securityBridgeError;
  const active = security?.connections.filter(c => c.phase !== "closed") ?? [];
  const visibleConnections = active.length ? active.slice().reverse() : security?.connections.slice(-1) ?? [];
  const operate = async (command: string, args: Record<string, unknown>) => {
    const key = `${command}:${args.id ?? args.transferId ?? args.connectionId ?? ""}`;
    if (operations.current.has(key)) return false;
    operations.current.add(key); setPending(count => count + 1); setError(null);
    try { await invoke(command, args); return true; }
    catch (failure) { setError(typeof failure === "string" ? failure : "Operation failed. Try again."); return false; }
    finally { operations.current.delete(key); setPending(count => count - 1); }
  };
  return <main className="app">
    <header className="app-header"><span className="mark" aria-hidden="true">↓</span><div><h1>InvarDrop</h1><p>Private sharing, nearby.</p></div><span className="offline-label">LAN only</span></header>
    <div className="workspace" tabIndex={0} role="region" aria-label="Devices and transfers">
      {security?.status === "starting" && <p className="service-status" role="status">Opening protected device identity…</p>}
      {(securityBridgeError || security?.error) && <ErrorNotice message={security?.error ?? "Open the desktop app to use secure connections."} />}
      {error && <ErrorNotice message={error} />}
      {visibleConnections.map(c => <PairCard key={c.id} connection={c} operate={operate} />)}
      {security && <ResumeTransfers snapshot={security} operate={operate} />}
      <section aria-labelledby="devices-title" className="empty-state">
        <div className="section-heading"><h2 id="devices-title"><Icon name="device" />Nearby devices</h2><span className="status" role="status">{unavailable ? "Unavailable" : snapshot?.status === "searching" ? "Discovering" : "Starting…"}</span></div>
        {unavailable ? <p className="discovery-message">{bridgeError ? "Open the desktop app to discover devices." : "Local discovery is unavailable. Retrying automatically. Check your local network and firewall if it persists."}</p> : devices.length ? <>
          <ul className="device-list" aria-label="Nearby InvarDrop devices">{devices.map(device => <li key={device.id}>
            <Icon name="device" /><div className="device-copy"><span className="device-name" dir="auto">{device.name}</span><span className="device-trust">Unverified name</span></div>
            <button type="button" aria-label={`${device.canConnect ? "Connect to" : "Discovery only:"} ${device.name}`} disabled={!ready || !device.canConnect || pending > 0} onClick={() => void operate("secure_connect", { id: device.id })}>{device.canConnect ? "Connect" : "Discovery only"}</button>
          </li>)}</ul><p className="file-hint">Names are unverified. Saved keys are checked when you connect.</p>
        </> : !snapshot || snapshot.status === "starting" ? <div className="discovery-empty"><p>Starting local discovery…</p><p className="file-hint">Preparing the nearby device list.</p></div> : <div className="discovery-empty"><p>No devices found yet</p><p className="file-hint">Open InvarDrop on another device on the same local network. Devices appear automatically.</p></div>}
        <p className="local-name">{snapshot ? <>This device: <span dir="auto">{snapshot.localDevice.name}</span></> : "Preparing local discovery…"}</p>
      </section>
      <ProcessingPanel />
      {security?.receiveDirectory && <p className="receive-directory">Destination: Downloads/InvarDrop · Saved only after approval and verification</p>}
      {security && <TrustSettings snapshot={security} operate={operate} busy={pending > 0} />}
    </div>
    <footer>Offline &amp; account-free · By Invar Gervi</footer>
  </main>;
}
export default App;
