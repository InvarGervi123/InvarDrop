import { FileTransfer } from "./FileTransfer";
import { PairCard } from "./PairCard";
import { Icon } from "./Icon";
import type { SecureConnection } from "./useSecurity";

type Operate = (command: string, args: Record<string, unknown>) => Promise<boolean>;

export function transferActive(transfer: SecureConnection["transfer"]) {
  return Boolean(transfer && !["complete", "rejected", "expired", "cancelled", "error"].includes(transfer.status));
}

type Props = {
  connections: SecureConnection[];
  recipients: Record<string, string>;
  operate: Operate;
};

function awaitingApproval(connection: SecureConnection) {
  return connection.transfer?.direction === "incoming" && connection.transfer.status === "awaiting_approval";
}

export function Attention({ connections, recipients, pendingId, cancel, operate }: Props & {
  pendingId?: string;
  cancel: () => void;
}) {
  const pairing = connections.filter(connection => connection.phase === "verifying");
  const awaiting = connections.filter(awaitingApproval);
  if (!pairing.length && !awaiting.length) return null;
  return <div className="attention">
    <p className="visually-hidden" role="status">{pairing.length > 0 ? "Pairing needs your attention." : "New incoming transfer request."}</p>
    {pairing.map(connection => <PairCard key={connection.id} connection={connection} operate={operate}
      recipient={recipients[connection.id]} cancel={connection.id === pendingId ? cancel : undefined} />)}
    {awaiting.map(connection => <FileTransfer key={connection.id} connection={connection} operate={operate} />)}
  </div>;
}

export function TransferActivity({ connections, recipients, operate }: Props) {
  const transfers = connections.filter(connection => connection.transfer && !awaitingApproval(connection));
  const ongoing = transfers.filter(connection => transferActive(connection.transfer));
  const recent = transfers.filter(connection => !transferActive(connection.transfer)).slice(-4).reverse();
  return <section className="activity-section" aria-labelledby="activity-title">
    <header className="activity-label"><Icon name="inbox" /><h2 id="activity-title">Activity</h2></header>
    {transfers.length > 0 ? <div className="activity-list">{[...ongoing, ...recent].map(connection =>
      <FileTransfer key={connection.id} connection={connection} operate={operate} recipient={recipients[connection.id]} />)}</div>
      : <p className="activity-empty">Incoming requests and transfers appear here.</p>}
  </section>;
}
