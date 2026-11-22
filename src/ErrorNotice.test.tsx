import { describe, expect, it } from "vitest";
import { recoveryHint } from "./ErrorNotice";

describe("connection-stage recovery guidance", () => {
  it.each([
    ["TCP connection timed out; check the advertised port, network and firewall rules", "receiving interface/source"],
    ["TCP access denied by this device; check local-network permissions or VPN policy", "local-network permission"],
    ["TLS handshake failed; no authenticated connection was established", "transport reached the peer"],
    ["TLS identity verification failed", "Do not bypass certificate checks"],
    ["Pairing expired; compare fresh codes on both devices before approving", "every code group"],
  ])("explains %s without suggesting insecure fallback", (message, hint) => {
    expect(recoveryHint(message)).toContain(hint);
  });
});

it("explains local network permission denial without asking to reset identity", () => {
  const hint = recoveryHint("Local network permission was not granted; discovery and secure connections are disabled");
  expect(hint).toContain("Android app permissions");
  expect(hint).toContain("identity and trusted devices are kept");
});
