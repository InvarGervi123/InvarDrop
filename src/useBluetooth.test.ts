import { expect, it } from "vitest";
import { nearbyDevices } from "./useBluetooth";

it("combines LAN and Bluetooth candidates without inventing trust", () => {
  const lan = [{ id: "same", name: "LAN name", canConnect: false }];
  const bluetooth = [{ id: "same", name: "Bluetooth name", canConnect: true }, { id: "other", name: "Nearby", canConnect: true }];
  expect(nearbyDevices(lan, bluetooth)).toEqual([{ id: "same", name: "LAN name", canConnect: true }, bluetooth[1]]);
  expect(lan[0].canConnect).toBe(false);
});
