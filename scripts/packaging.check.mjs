import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { preserveUserData } from "./prepare-nsis.mjs";
const fixture = `; App-owned uninstall actions stay intact.
Delete "$INSTDIR\\invardrop.exe"
Var DeleteAppDataCheckbox
Function un.ConfirmShow
  ; deletion checkbox
FunctionEnd
!define MUI_PAGE_CUSTOMFUNCTION_PRE un.SkipIfPassive
!insertmacro MUI_UNPAGE_CONFIRM
  ; Delete app data if the checkbox is selected
  RmDir /r "$APPDATA\\profile"
  !ifmacrodef NSIS_HOOK_POSTUNINSTALL
  !endif
`;
const hash = value => createHash("sha256").update(value).digest("hex");
test("removes profile deletion and its UI while retaining app-owned uninstall actions", () => {
  const result = preserveUserData(fixture, hash(fixture));
  assert.ok(result.includes('Delete "$INSTDIR\\invardrop.exe"'));
  assert.ok(result.includes("!insertmacro MUI_UNPAGE_CONFIRM"));
  assert.ok(!result.includes("DeleteAppDataCheckbox"));
  assert.ok(!result.includes('RmDir /r "$APPDATA'));
});
test("rejects altered upstream input before applying changes", () => {
  assert.throws(() => preserveUserData(fixture + "tampered", hash(fixture)), /integrity/);
});
test("rejects missing removal boundaries instead of producing a partial installer", () => {
  const changed = fixture.replace("!define MUI_PAGE_CUSTOMFUNCTION_PRE un.SkipIfPassive", "changed upstream");
  assert.throws(() => preserveUserData(changed, hash(changed)), /structure/);
});
test("rejects unexpected profile deletion outside the reviewed upstream blocks", () => {
  const changed = fixture + 'RmDir /r "$LOCALAPPDATA\\profile"\n';
  assert.throws(() => preserveUserData(changed, hash(changed)), /Unsafe/);
});
