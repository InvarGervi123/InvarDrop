import { Icon } from "./Icon";

export function recoveryHint(message: string): string | null {
  if (/Local network permission was not granted/i.test(message)) return "Allow local network access in Android app permissions, then restart InvarDrop. Your identity and trusted devices are kept.";
  if (/Saving to Downloads failed/i.test(message)) return "Check Downloads access and free space. Keep the private profile; reconnect and retry recovery.";
  if (/profile is already in use/i.test(message)) return "Close the other InvarDrop instance using this profile, then restart.";
  if (/storage is unavailable/i.test(message)) return "Unlock your system credential store and check available disk space, then restart InvarDrop.";
  if (/identity or trust data is corrupt/i.test(message)) return "Keep the existing profile and credential. Do not delete them independently.";
  if (/revoked/i.test(message)) return "Review the key in Device identity & trusted peers. New pairing requires your explicit approval.";
  if (/source.*changed|verification failed; file/i.test(message)) return "Select the source again for a new transfer. Unverified data is never published.";
  if (/no supported LAN endpoint/i.test(message)) return "Wait for fresh discovery information on the same LAN. A device name does not establish trust.";
  if (/INVARDROP_LISTEN_PORT|TCP listener could not bind/i.test(message)) return "Choose an available TCP port for INVARDROP_LISTEN_PORT, then restart. Do not run two instances on the same fixed port.";
  if (/TCP access denied/i.test(message)) return "Check InvarDrop local-network permission and VPN policy on this device. Keep platform security protections enabled.";
  if (/TCP connection/i.test(message)) return "Check the peer's current TCP port and the receiving interface/source in firewall rules. Working mDNS discovery does not prove TCP access.";
  if (/TLS identity verification failed/i.test(message)) return "Verify the intended device and saved peer key. Do not bypass certificate checks or delete the existing identity.";
  if (/TLS handshake/i.test(message)) return "TCP reached the peer. Check that both devices run InvarDrop with compatible TLS and protocol versions.";
  if (/peer authentication timed out/i.test(message)) return "Check the peer application and saved trust on both devices. No pairing or transfer is approved automatically.";
  if (/pairing expired|pairing was rejected/i.test(message)) return "Start fresh pairing and compare every code group on both devices. Approve only if all match.";
  if (/failed or disconnected|connection failed/i.test(message)) return "Check that both devices are available on the same network, then reconnect. Saved checkpoints appear in Transfer recovery.";
  if (/request expired|stale approval/i.test(message)) return "Reconnect and request a new transfer or pairing. A fresh approval is required.";
  if (/limit reached|storage is full|queue.*full/i.test(message)) return "Finish an active operation or review older entries before trying again.";
  if (/file could not be read or saved/i.test(message)) return "Check the source, available disk space, and file permissions before retrying.";
  return null;
}
export function ErrorNotice({ message }: { message: string }) {
  const hint = recoveryHint(message);
  return <div className="error-text" role="alert"><Icon name="alert" /><div><p>{message}</p>{hint && <p className="error-hint">{hint}</p>}</div></div>;
}
