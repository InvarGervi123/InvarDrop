import { Icon } from "./Icon";

export function recoveryHint(message: string): string | null {
  if (/profile is already in use/i.test(message)) return "Close the other InvarDrop instance using this profile, then restart.";
  if (/storage is unavailable/i.test(message)) return "Unlock your system credential store and check available disk space, then restart InvarDrop.";
  if (/identity or trust data is corrupt/i.test(message)) return "Keep the existing profile and credential. Do not delete them independently.";
  if (/revoked/i.test(message)) return "Review the key in Device identity & trusted peers. New pairing requires your explicit approval.";
  if (/source.*changed|verification failed; file/i.test(message)) return "Select the source again for a new transfer. Unverified data is never published.";
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
