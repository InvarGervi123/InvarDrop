import { Icon } from "./Icon";

export function recoveryHint(message: string): string | null {
  if (/Bluetooth|BlueZ|RFCOMM|BLE /i.test(message)) return "Enable Bluetooth sharing on both devices, allow Nearby devices permission on Android, and check OS bonding prompts. Scan again with both apps open. A discovered name does not establish identity.";
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
  if (/TLS handshake/i.test(message)) return "The transport reached the peer. Check that both devices run InvarDrop with compatible TLS and protocol versions.";
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

export function friendlyError(message: string): string {
  if (/choose one file|folder selection is unavailable|finish the current file preparation/i.test(message)) return message;
  if (/permission.*bluetooth|bluetooth permission/i.test(message)) return "Allow nearby device access in system settings, then try again.";
  if (/local network permission|TCP access denied/i.test(message)) return "Allow local network access in system settings, then reopen InvarDrop.";
  if (/pairing.*expired|stale approval/i.test(message)) return "This request expired. Ask the other device to try again.";
  if (/pairing.*rejected/i.test(message)) return "Pairing was declined. Check with the other person before trying again.";
  if (/identity verification|identity has been revoked/i.test(message)) return "This device could not be verified. Check that you selected the intended device and review trusted devices in Settings.";
  if (/secure storage|security profile|identity or trust data/i.test(message)) return "InvarDrop could not open your saved device identity. Unlock your system credential store or reopen InvarDrop.";
  if (/saving to downloads/i.test(message)) return "The file was verified, but could not be saved to Downloads. Free some storage and review Transfer recovery in Settings.";
  if (/too many files/i.test(message)) return "Choose fewer files and try again.";
  if (/file name|filenames/i.test(message)) return "Rename the selected files with simple, distinct names and try again.";
  if (/source file changed/i.test(message)) return "The original file changed. Choose it again to send the latest copy.";
  if (/BLAKE3|verification failed/i.test(message)) return "The received data could not be verified. Ask the sender to try again.";
  if (/storage.*full|queue.*full|limit reached/i.test(message)) return "Finish a current transfer or free some storage before trying again.";
  if (/file could not be read|selected files could not be prepared|picker|private storage/i.test(message)) return "Check that the files can be opened and that this device has free storage, then try again.";
  if (/bluetooth.*(?:adapter|unsupported|unavailable|power)/i.test(message)) return "Check that Bluetooth is on and keep InvarDrop open on both devices.";
  if (/connection|handshake|endpoint|authentication timed out|TLS|protocol/i.test(message)) return "Check that InvarDrop is open on the other device, then try again.";
  if (/no longer available/i.test(message)) return "That device is out of reach. Open InvarDrop on the other device and try again.";
  if (/not ready|selection is already in use/i.test(message)) return "The device is busy or still starting. Wait a moment and try again.";
  if (/choose files again/i.test(message)) return "Choose your files again, then retry.";
  return "Try again. More information is available in Settings.";
}
