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
