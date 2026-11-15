import { createHash } from "node:crypto";
import { readFile, writeFile, mkdir } from "node:fs/promises";
import { fileURLToPath, pathToFileURL } from "node:url";
import path from "node:path";

export const version = "2.12.4";
export const sourceUrl = `https://raw.githubusercontent.com/tauri-apps/tauri/tauri-cli-v${version}/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi`;
export const sourceHash = "d00089d3b1a2c4a30d7b6e098079100cb8f25e980f84d4515aca4597b95e967d";
const digest = value => createHash("sha256").update(value).digest("hex");

export function preserveUserData(source, expectedHash = sourceHash) {
  if (digest(source) !== expectedHash) throw new Error("NSIS template integrity check failed");
  const remove = (text, start, end) => {
    const first = text.indexOf(start), last = text.indexOf(end, first);
    if (first < 0 || last <= first || text.indexOf(start, first + 1) >= 0) throw new Error("Unexpected NSIS template structure");
    return text.slice(0, first) + "; InvarDrop keeps received files, identity and recovery data on uninstall.\n" + text.slice(last);
  };
  let result = remove(source, "Var DeleteAppDataCheckbox\n", "!define MUI_PAGE_CUSTOMFUNCTION_PRE un.SkipIfPassive");
  result = remove(result, "  ; Delete app data if the checkbox is selected\n", "  !ifmacrodef NSIS_HOOK_POSTUNINSTALL");
  if (/DeleteAppDataCheckbox|RmDir[^\n]*\$(?:APPDATA|LOCALAPPDATA)/i.test(result)) throw new Error("Unsafe profile deletion remains in NSIS template");
  return `; Adapted from Tauri ${version}, Apache-2.0 OR MIT. Copyright Tauri Contributors.\n; Source and SHA-256 are pinned in scripts/prepare-nsis.mjs.\n${result}`;
}

export async function prepare() {
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
  const installed = JSON.parse(await readFile(path.join(root, "node_modules/@tauri-apps/cli/package.json"), "utf8"));
  if (installed.version !== version) throw new Error("Review and repin the NSIS template before upgrading Tauri CLI");
  const output = path.join(root, "src-tauri/target/packaging");
  await mkdir(output, { recursive: true });
  const cache = path.join(output, "upstream-installer.nsi");
  let source;
  try { source = await readFile(cache, "utf8"); }
  catch (error) {
    if (error.code !== "ENOENT") throw error;
    const response = await fetch(sourceUrl, { signal: AbortSignal.timeout(60_000) });
    if (!response.ok) throw new Error(`NSIS template download failed: HTTP ${response.status}`);
    source = await response.text();
    if (digest(source) !== sourceHash) throw new Error("Downloaded NSIS template integrity check failed");
    await writeFile(cache, source, { flag: "wx" });
  }
  await writeFile(path.join(output, "installer.nsi"), preserveUserData(source));
  console.log(`Prepared Tauri ${version} NSIS template with profile deletion removed.`);
}
if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) await prepare();
