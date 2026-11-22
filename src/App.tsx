import { useCallback, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";
import { useDiscovery } from "./useDiscovery";
import { useSecurity } from "./useSecurity";
import { nearbyDevices, useBluetooth, useBluetoothDiscovery } from "./useBluetooth";
import { NearbyDevices, deviceIcon } from "./NearbyDevices";
import { SendAction, ShareComposer } from "./ShareComposer";
import { Attention, TransferActivity, transferActive } from "./Activity";
import { ProcessingPanel } from "./ProcessingPanel";
import { Settings } from "./Settings";
import { friendlyError } from "./ErrorNotice";
import { Icon } from "./Icon";
import { failureText, useShare } from "./useShare";

function App() {
  const { snapshot, bridgeError } = useDiscovery();
  const { snapshot: security, bridgeError: securityBridgeError } = useSecurity();
  const bluetooth = useBluetooth();
  const [settings, setSettings] = useState(false);
  const [bluetoothAllowed, setBluetoothAllowed] = useState(() => localStorage.getItem("bluetooth-sharing") !== "off");
  const [offerConversion, setOfferConversion] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [lastError, setLastError] = useState<string | null>(null);
  const [pending, setPending] = useState(0);
  const operations = useRef(new Set<string>());
  const report = useCallback((message: string | null) => {
    setError(message);
    if (message) setLastError(message);
  }, []);
  const connections = security?.connections ?? [];
  const share = useShare(connections, report);
  const devices = nearbyDevices(snapshot?.devices ?? [], bluetooth?.devices ?? []);
  const canFindMore = bluetooth?.supported === true && bluetoothAllowed;
  const bluetoothActive = bluetoothAllowed && ["starting", "ready", "scanning", "paused"].includes(bluetooth?.status ?? "");
  const lanUnavailable = bridgeError || snapshot?.status === "unavailable" || snapshot?.status === "stopped";
  const unavailable = lanUnavailable && !bluetoothActive;
  const ready = security?.status === "ready" && !securityBridgeError;
  const canReceive = snapshot?.status === "searching" || ["ready", "scanning"].includes(bluetooth?.status ?? "");
  const folders = snapshot?.supportsFolderPicker ?? false;
  const idle = !share.intent && !share.picking && !connections.some(connection =>
    ["connecting", "verifying", "committing"].includes(connection.phase) || transferActive(connection.transfer));
  useBluetoothDiscovery(bluetooth, bluetoothAllowed, idle, Boolean(snapshot?.devices.some(device => device.canConnect)));

  const operate = async (command: string, args: Record<string, unknown>, done?: (value: unknown) => void) => {
    const key = [command, args.id ?? "", args.transferId ?? ""].join(":");
    if (operations.current.has(key)) return false;
    operations.current.add(key);
    setPending(count => count + 1);
    report(null);
    try { const result = await invoke(command, args); done?.(result); return true; }
    catch (failure) { report(failureText(failure)); return false; }
    finally { operations.current.delete(key); setPending(count => count - 1); }
  };
  const setBluetooth = (enabled: boolean) => {
    setBluetoothAllowed(enabled);
    localStorage.setItem("bluetooth-sharing", enabled ? "on" : "off");
    void operate(enabled ? "bluetooth_enable" : "bluetooth_disable", {});
  };
  const openSettings = () => {
    setSettings(show => !show);
    document.documentElement.scrollTop = 0;
  };
  const coreError = securityBridgeError ? "Open the installed InvarDrop app to share files." :
    security?.status === "unavailable" && security.error ? friendlyError(security.error) : null;

  return <main className="app">
    <header className="app-header">
      <div className="brand"><span className="brand-mark"><Icon name="brand" /></span><span>InvarDrop</span></div>
      <button className="header-button quiet" type="button" onClick={openSettings}><Icon name={settings ? "back" : "settings"} />{settings ? "Back to sharing" : "Settings"}</button>
    </header>
    <div className="page">
      <header className="intro">
        {settings ? <><h1>Settings</h1><p>Discovery, trusted devices, and transfer recovery.</p></> :
          <><h1>Share nearby</h1><p>Pick files and a nearby device to send them to.</p></>}
      </header>
      {(coreError || error) && <div className="operation-error" role="alert">
        <Icon name="alert" /><p>{error ? friendlyError(error) : coreError}</p>
        {error && <button type="button" className="icon-button quiet" aria-label="Dismiss message" onClick={() => report(null)}><Icon name="close" /></button>}
      </div>}
      <Attention connections={connections} recipients={share.recipients} pendingId={share.intent?.connectionId} cancel={() => void share.cancel()} operate={operate} />
      {settings ? <Settings security={security} devices={devices} bluetooth={bluetooth} bluetoothAllowed={bluetoothAllowed} fileOptions={folders}
        offerConversion={offerConversion} setOfferConversion={setOfferConversion} lan={snapshot?.status ?? "unknown"}
        lastError={lastError} busy={pending > 0} operate={operate} onBluetooth={setBluetooth} /> : <div className="share-grid">
        <ShareComposer share={share} ready={ready} folders={folders} />
        <NearbyDevices devices={devices} selected={share.device} disabled={Boolean(share.intent) || pending > 0}
          starting={!bridgeError && (!snapshot || snapshot.status === "starting") && devices.length === 0}
          unavailable={Boolean(unavailable)} bluetoothAction={canFindMore && (!bluetoothActive || devices.length === 0) && bluetooth?.status !== "scanning"}
          onSelect={share.selectDevice} findMore={() => bluetoothActive ? void operate("bluetooth_scan", {}) : setBluetooth(true)} />
        <SendAction share={share} ready={ready} offerConversion={offerConversion}
          available={devices.some(device => device.id === share.device?.id && device.canConnect)} />
      </div>}
      <TransferActivity connections={connections} recipients={share.recipients} operate={operate} />
      <ProcessingPanel settingsOpen={settings && folders} />
      <footer className="app-footer">
        <div className="local-device"><Icon name={deviceIcon(snapshot?.localDevice.name ?? "")} /><div><span>This device</span><strong dir="auto">{snapshot?.localDevice.name ?? "Getting ready…"}</strong></div></div>
        <p><Icon name="shield" />{ready && canReceive ? "Ready to receive" : "Local sharing"}</p>
      </footer>
    </div>
  </main>;
}
export default App;
