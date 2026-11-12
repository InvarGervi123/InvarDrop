import "./App.css";
import { useDiscovery } from "./useDiscovery";

function App() {
  const { snapshot, bridgeError } = useDiscovery();
  const devices = snapshot?.devices ?? [];
  const unavailable = bridgeError || snapshot?.status === "unavailable";
  const searching = snapshot?.status === "searching";
  return (
    <main className="app">
      <header>
        <span className="mark" aria-hidden="true">↓</span>
        <h1>InvarDrop</h1>
        <p>File sharing. Close to home.</p>
      </header>
      <section aria-labelledby="devices-title" className="empty-state">
        <span className="status" role="status">
          {unavailable ? "Discovery unavailable" : searching ? "Looking nearby" : "Starting discovery"}
        </span>
        <h2 id="devices-title">Nearby devices</h2>
        {unavailable ? (
          <p className="discovery-message" role="alert">
            {bridgeError ? "Open the desktop app to discover devices." : "Local discovery is unavailable. Retrying automatically…"}
          </p>
        ) : devices.length ? (
          <ul className="device-list" aria-label="Nearby InvarDrop devices">
            {devices.map(device => (
              <li key={device.id}>
                <span className="device-name" dir="auto">{device.name}</span>
                <span className="device-trust">Not paired</span>
              </li>
            ))}
          </ul>
        ) : (
          <p className="discovery-message">Open InvarDrop on another device on the same network.</p>
        )}
        <p className="local-name" title={snapshot?.localDevice.name}>
          {snapshot ? `This device: ${snapshot.localDevice.name}` : "Preparing local discovery…"}
        </p>
      </section>
      <footer>
        <p>Discovery only · File transfers coming later</p>
        <p>Free &amp; open source · By Invar Gervi</p>
      </footer>
    </main>
  );
}

export default App;
