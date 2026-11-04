import { expect, open, test } from "nsh/test";

test("the shell shows the rail, the run room and the title bar", async () => {
  const app = await open("./App.tsx", { width: 1280, height: 840, platform: "macos" });
  await app.waitFor("text:Chat");
  await app.waitFor("text:Trajectory");
  expect(app.texts()).toContain("SESSIONS");
  expect(app.texts()).toContain("Commands");
});

test("the command center opens from the title bar", async () => {
  const app = await open("./App.tsx", { width: 1280, height: 840, platform: "macos" });
  await app.press("label:Open command center");
  await app.waitFor("text:Command center");
  expect(app.texts()).toContain("theme");
});

test("the composer accepts a draft", async () => {
  const app = await open("./App.tsx", { width: 1280, height: 840, platform: "macos" });
  await app.waitFor("text:Chat");
  await app.type("id:composer", "hello");
  expect(app.value("id:composer")).toBe("hello");
});

test("the command center closes with its close button", async () => {
  const app = await open("./App.tsx", { width: 1280, height: 840, platform: "macos" });
  await app.press("label:Open command center");
  await app.waitFor("text:Command center");
  await app.press("label:Close");
  await app.wait(100);
  expect(app.texts()).not.toContain("Command center");
});

test("the trajectory page shows the ledger", async () => {
  const app = await open("./App.tsx", { width: 1280, height: 840, platform: "macos" });
  await app.waitFor("text:Trajectory");
  await app.press("text:Trajectory");
  await app.waitFor("text:Select a record");
});

test("a command opens the reader", async () => {
  const app = await open("./App.tsx", { width: 1280, height: 840, platform: "macos" });
  await app.press("label:Open command center");
  await app.waitFor("text:help");
  await app.press("text:help");
  await app.waitFor("id:reader");
});

test("slash opens the command suggestions", async () => {
  const app = await open("./App.tsx", { width: 1280, height: 840, platform: "macos" });
  await app.waitFor("id:composer");
  await app.type("id:composer", "/");
  await app.waitFor("text:theme");
});

test("the settings overlay lists the settings", async () => {
  const app = await open("./App.tsx", { width: 1280, height: 840, platform: "macos" });
  await app.press("label:Open settings");
  await app.waitFor("text:Provider");
  await app.waitFor("text:Reasoning effort");
});

test("the app menu opens and escape closes it", async () => {
  const app = await open("./App.tsx", { width: 1280, height: 840, platform: "macos" });
  await app.waitFor("text:File");
  await app.press("text:File");
  await app.waitFor("text:Command Center");
  await app.keyboard.press("escape");
  await app.wait(100);
  expect(app.texts()).not.toContain("Command Center");
});

test("submitting a slash command runs it", async () => {
  const app = await open("./App.tsx", { width: 1280, height: 840, platform: "macos" });
  await app.waitFor("id:composer");
  await app.type("id:composer", "/help");
  await app.submit("id:composer");
  await app.waitFor("id:reader");
});

test("the command center shortcut works", async () => {
  const app = await open("./App.tsx", { width: 1280, height: 840, platform: "macos" });
  await app.waitFor("text:Chat");
  await app.keyboard.press("mod+k");
  await app.waitFor("text:Command center");
});
