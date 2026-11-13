import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";
import { useDiscovery } from "./useDiscovery";
import { useSecurity } from "./useSecurity";
import { PairCard } from "./PairCard";

function App() {
  const { snapshot, bridgeError } = useDiscovery();
  const { snapshot: security, bridgeError: securityBridgeError } = useSecurity();
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const devices = snapshot?.devices ?? [];
  const unavailable = bridgeError || snapshot?.status === "unavailable";
  const ready = security?.status === "ready" && !securityBridgeError;
  const active = security?.connections.filter(c => c.phase !== "closed") ?? [];
  const visibleConnections = active.length ? active.slice().reverse() : security?.connections.slice(-1) ?? [];
  const operate = async (command: string, args: Record<string, unknown>) => {
    setError(null); setBusy(true);
    try { await invoke(command, args); return true; }
    catch (failure) { setError(typeof failure === "string" ? failure : "Operation failed. Try again."); return false; }
    finally { setBusy(false); }
  };
  return (
    <main className="app">
      <header><span className="mark" aria-hidden="true">↓</span><h1>InvarDrop</h1><p>File sharing. Close to home.</p></header>
      <div className="workspace">
        <section aria-labelledby="devices-title" className="empty-state">
          <span className="status" role="status">{unavailable ? "Discovery unavailable" : snapshot?.status === "searching" ? "Looking nearby" : "Starting discovery"}</span>
          <h2 id="devices-title">Nearby devices</h2>
          {unavailable ? <p className="discovery-message">{bridgeError ? "Open the desktop app to discover devices." : "Local discovery is unavailable. Retrying automatically…"}</p> : devices.length ? (
            <ul className="device-list" aria-label="Nearby InvarDrop devices">{devices.map(device => <li key={device.id}>
              <span className="device-name" dir="auto">{device.name}</span>
              <button type="button" disabled={!ready || !device.canConnect || busy} onClick={() => void operate("secure_connect", { id: device.id })}>{device.canConnect ? "Connect" : "Discovery only"}</button>
            </li>)}</ul>
          ) : <p className="discovery-message">Open InvarDrop on another device on the same network.</p>}
          <p className="local-name">{snapshot ? `This device: ${snapshot.localDevice.name}` : "Preparing local discovery…"}</p>
        </section>
        {security?.status === "starting" && <p className="service-status" role="status">Opening protected device identity…</p>}
        {(securityBridgeError || security?.error) && <p role="alert" className="error-text">{security?.error ?? "Open the desktop app to use secure connections."}</p>}
        {error && <p role="alert" className="error-text">{error}</p>}
        {visibleConnections.map(c => <PairCard key={c.id} connection={c} operate={operate} />)}
        {security?.identity && <details className="trust-settings"><summary>Device identity &amp; trusted peers</summary>
          <p>Your public key fingerprint:</p><code className="fingerprint" dir="ltr">{security.identity}</code>
          <p>Discovery names are unverified. Only the key and full comparison code establish trust.</p>
          <ul>{security.trusted.map(id => <li key={id}><span>Paired key {id.slice(0, 16)}</span><button type="button" className="secondary" disabled={busy} onClick={() => void operate("secure_revoke", { id })}>Revoke trust</button></li>)}</ul>
          {security.revoked.length > 0 && <><p>Revoked keys. Allowing pairing again does not restore trust.</p><ul>{security.revoked.map(id => <li key={id}><span>{id.slice(0, 16)}</span><button type="button" className="secondary" disabled={busy} onClick={() => void operate("secure_unblock", { id })}>Allow new pairing</button></li>)}</ul></>}
        </details>}
      </div>
      <footer><p>Secure connections only · No file transfer yet</p><p>Free &amp; open source · By Invar Gervi</p></footer>
    </main>
  );
}
export default App;
