import { Icon } from "./Icon";
import type { Device } from "./useDiscovery";

export function deviceIcon(name: string) {
  return /^Android phone\b/i.test(name) ? "phone" : "device";
}

interface Props {
  devices: Device[];
  selected: Device | null;
  disabled: boolean;
  starting: boolean;
  unavailable: boolean;
  bluetoothAction: boolean;
  onSelect: (device: Device) => void;
  findMore: () => void;
}

export function NearbyDevices({ devices, selected, disabled, starting, unavailable, bluetoothAction, onSelect, findMore }: Props) {
  const visible = selected && !devices.some(device => device.id === selected.id)
    ? [...devices, { ...selected, canConnect: false }] : devices;
  return <section className="panel nearby-panel" aria-labelledby="nearby-title">
    <header className="panel-header">
      <h2 id="nearby-title">Nearby devices</h2>
      <Icon name="nearby" />
    </header>
    {visible.length ? <ul className="device-list">
      {visible.map(device => {
        const active = device.id === selected?.id;
        return <li key={device.id}><button type="button" className="device-row"
          aria-pressed={active} disabled={disabled || (!active && !device.canConnect)} onClick={() => onSelect(device)}>
          <span className="tile-icon"><Icon name={deviceIcon(device.name)} /></span>
          <span className="tile-copy"><strong dir="auto">{device.name}</strong>
            <small>{!device.canConnect ? "Unavailable" : active ? "Selected" : "Available"}</small></span>
          <span className="selection-mark" aria-hidden="true">{active && <Icon name="check" />}</span>
        </button></li>;
      })}
    </ul> : <div className="nearby-empty">
      <span className="tile-icon empty-icon"><Icon name="nearby" /></span>
      <p role="status">{starting ? "Looking for devices…" : unavailable ? "Nearby sharing is limited" : "No devices nearby yet"}</p>
      <small>{unavailable ? "Check local network access and keep both devices nearby." : "Open InvarDrop on the other device. It will appear here when available."}</small>
    </div>}
    {bluetoothAction && <button type="button" className="quiet find-button" disabled={disabled} onClick={findMore}>Find more devices</button>}
  </section>;
}
