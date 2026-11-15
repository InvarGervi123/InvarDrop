import { lstat, rm } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { prepare } from "./prepare-nsis.mjs";

// Tauri can retain removed resources in both generated AppImage staging trees.
for (const relative of [
  "../src-tauri/target/release/bundle/appimage/InvarDrop.AppDir",
  "../src-tauri/target/release/bundle/appimage_deb",
]) {
  const staging = fileURLToPath(new URL(relative, import.meta.url));
  const previous = await lstat(staging).catch(error => { if (error.code !== "ENOENT") throw error; });
  if (previous && (!previous.isDirectory() || previous.isSymbolicLink())) throw new Error("Refusing to clean an unexpected AppImage staging path");
  await rm(staging, { recursive: true, force: true });
}
if (process.platform === "win32") await prepare();
