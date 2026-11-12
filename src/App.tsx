import "./App.css";

function App() {
  return (
    <main className="app">
      <header>
        <span className="mark" aria-hidden="true">↓</span>
        <h1>InvarDrop</h1>
        <p>File sharing. Close to home.</p>
      </header>
      <section aria-labelledby="status-title" className="empty-state">
        <span className="status">In development</span>
        <h2 id="status-title">A simple start.</h2>
        <p>Sharing with nearby devices is coming next.</p>
        <p className="detail">Device discovery and file transfers are not available in this version.</p>
      </section>
      <footer>Free &amp; open source · By Invar Gervi</footer>
    </main>
  );
}

export default App;
