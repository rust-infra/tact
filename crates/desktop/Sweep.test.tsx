import { expect, open, test } from "nsh/test";

const options = { width: 1280, height: 840, platform: "macos" } as const;

test("title bar entry points open", async () => {
  const app = await open("./App.tsx", options);
  await app.waitFor("text:File");
  await app.press("text:File");
  await app.waitFor("text:New Session");
  await app.waitFor("text:Command Center");
  await app.press("text:File");
  await app.press("text:View");
  await app.waitFor("text:Toggle Sessions");
  await app.waitFor("text:Toggle Inspector");
  await app.waitFor("text:Back to Bottom");
  await app.press("text:View");
  await app.press("text:Session");
  await app.waitFor("text:Stop");
  await app.press("text:Session");
  await app.press("label:Hide sessions");
  await app.wait(60);
  expect(app.shows("text:SESSIONS")).toBe(false);
  await app.press("label:Show sessions");
  await app.waitFor("text:SESSIONS");
  await app.press("label:Hide inspector");
  await app.wait(60);
  await app.press("label:Show inspector");
  await app.waitFor("text:Run");
});

test("app menus close when the background is pressed", async () => {
  const app = await open("./App.tsx", options);
  await app.press("text:File");
  await app.waitFor("text:Extensions");
  await app.press("id:app-menu-scrim");
  await app.wait(80);
  expect(app.texts()).not.toContain("Extensions");
});

test("rail entry points respond", async () => {
  const app = await open("./App.tsx", options);
  await app.waitFor("text:SESSIONS");
  await app.press("text:Archived");
  await app.wait(80);
  await app.press("text:All");
  await app.wait(80);
  await app.press("text:Active");
  await app.wait(80);
  await app.type("id:session-search", "a");
  await app.wait(80);
  await app.clear("id:session-search");
  await app.press("label:Switch project");
  await app.waitFor("text:Projects");
  await app.press("label:Close");
});

test("run room tabs and inspector tabs respond", async () => {
  const app = await open("./App.tsx", options);
  await app.waitFor("text:Chat");
  await app.press("text:Trajectory");
  await app.waitFor("text:Select a record");
  await app.press("text:Chat");
  await app.waitFor("text:Chat");
  for (const tab of ["Goals", "Subagents", "Background", "Context", "Run"]) {
    await app.press(`text:${tab}`);
    await app.wait(60);
  }
});

test("every overlay opens and closes", async () => {
  const app = await open("./App.tsx", options);
  await app.press("label:Open command center");
  await app.waitFor("text:Command center");
  await app.press("label:Close");
  await app.press("label:Open settings");
  await app.waitFor("text:Provider");
  await app.press("label:Close");
  await app.press("label:Manage sessions");
  await app.waitFor("text:Open");
  await app.press("label:Close");
  await app.press("label:Switch project");
  await app.waitFor("text:Projects");
  await app.press("label:Close");
});

test("the overlay scrim dismisses a dialog", async () => {
  const app = await open("./App.tsx", options);
  await app.press("label:Open settings");
  await app.waitFor("text:Provider");
  await app.press("id:overlay-scrim");
  await app.wait(80);
  expect(app.texts()).not.toContain("Provider");
});

test("settings rows are present", async () => {
  const app = await open("./App.tsx", options);
  await app.press("label:Open settings");
  for (const label of [
    "Theme",
    "Language",
    "Model",
    "Subagent model",
    "Permission",
    "Reasoning effort",
    "Thinking budget",
    "Notifications",
    "Hook output",
    "Provider",
    "Base URL",
    "API key",
    "Max tokens",
    "Context window",
    "Snapshot max items",
    "Micro compact",
    "Tokio console",
    "Skill body inject",
    "Config path",
  ]) {
    await app.waitFor(`text:${label}`);
  }
});

test("extensions tabs and model picker respond", async () => {
  const app = await open("./App.tsx", options);
  await app.press("text:File");
  await app.press("text:Extensions");
  await app.waitFor("text:MCP");
  await app.press("text:Skills");
  await app.waitFor("id:extensions-search");
  await app.type("id:extensions-search", "x");
  expect((app.value("id:extensions-search") ?? "").endsWith("x")).toBe(true);
  await app.clear("id:extensions-search");
  await app.press("text:Plugins");
  await app.waitFor("id:extensions-search");
  await app.press("text:Hooks");
  await app.waitFor("text:Load");
  await app.press("label:Close");
});

test("composer controls respond", async () => {
  const app = await open("./App.tsx", options);
  await app.waitFor("id:composer");
  await app.press("label:Choose model");
  await app.wait(60);
  if (app.shows("label:Close")) await app.press("label:Close");
  await app.press("label:Cycle permission mode");
  await app.wait(80);
  await app.press("label:Cycle permission mode");
  await app.wait(80);
  await app.type("id:composer", "/");
  await app.waitFor("text:theme");
  await app.clear("id:composer");
});

test("help opens the reader and escape closes it", async () => {
  const app = await open("./App.tsx", options);
  await app.type("id:composer", "/help");
  await app.submit("id:composer");
  await app.waitFor("id:reader");
  await app.keyboard.press("escape");
  await app.wait(80);
  expect(app.shows("id:reader")).toBe(false);
});

test("the reader scrim dismisses the reader", async () => {
  const app = await open("./App.tsx", options);
  await app.type("id:composer", "/help");
  await app.submit("id:composer");
  await app.waitFor("id:reader");
  await app.press("id:reader-scrim");
  await app.wait(80);
  expect(app.shows("id:reader")).toBe(false);
});

test("shortcuts open their surfaces", async () => {
  const app = await open("./App.tsx", options);
  await app.keyboard.press("mod+k");
  await app.waitFor("text:Command center");
  await app.keyboard.press("escape");
  await app.keyboard.press("mod+,");
  await app.waitFor("text:Provider");
  await app.keyboard.press("escape");
  await app.keyboard.press("mod+b");
  await app.wait(60);
  expect(app.shows("text:SESSIONS")).toBe(false);
  await app.keyboard.press("mod+b");
  await app.waitFor("text:SESSIONS");
  await app.keyboard.press("mod+alt+b");
  await app.wait(60);
  await app.keyboard.press("mod+alt+b");
  await app.wait(60);
});

test("narrow windows turn the side panels into drawers", async () => {
  const app = await open("./App.tsx", options);
  await app.setViewport(820, 840);
  await app.wait(80);
  expect(app.shows("id:rail")).toBe(false);
  expect(app.shows("id:inspector-panel")).toBe(false);
  expect(app.shows("id:titlebar-project")).toBe(false);
  await app.press("label:Show sessions");
  await app.waitFor("id:rail");
  await app.press("id:rail-drawer-scrim");
  await app.wait(80);
  expect(app.shows("id:rail")).toBe(false);
  await app.press("label:Show inspector");
  await app.waitFor("id:inspector-panel");
  await app.keyboard.press("escape");
  await app.wait(80);
  expect(app.shows("id:inspector-panel")).toBe(false);
});

test("settings inputs accept typing", async () => {
  const app = await open("./App.tsx", options);
  await app.press("label:Open settings");
  await app.waitFor("id:setting-provider");
  await app.type("id:setting-provider", "x");
  expect((app.value("id:setting-provider") ?? "").endsWith("x")).toBe(true);
  await app.clear("id:setting-provider");
});

test("session manager rows accept typing", async () => {
  const app = await open("./App.tsx", options);
  await app.press("label:Manage sessions");
  await app.waitFor("id:sessions-panel");
  await app.type("id:session-manager-search", "a");
  expect((app.value("id:session-manager-search") ?? "").endsWith("a")).toBe(true);
  await app.clear("id:session-manager-search");
  if (app.shows("id:session-title")) {
    await app.type("id:session-title >> nth=0", "x");
    expect((app.value("id:session-title >> nth=0") ?? "").endsWith("x")).toBe(true);
  }
});

test("extension load buttons run their commands", async () => {
  const app = await open("./App.tsx", options);
  await app.press("text:File");
  await app.press("text:Extensions");
  await app.waitFor("text:MCP");
  await app.press("text:Load");
  await app.wait(400);
  await app.press("text:Hooks");
  await app.press("text:Load");
  await app.wait(400);
});

test("trajectory records open their detail tabs", async () => {
  const app = await open("./App.tsx", options);
  await app.press("text:Trajectory");
  await app.waitFor("text:Select a record");
  for (const tab of ["Payload", "Result", "Schema", "Timing", "Summary"]) {
    await app.press(`label:${tab} detail`);
    await app.wait(60);
  }
  const record = app.texts().find((text) => /^#\d/.test(text));
  if (record) {
    await app.press(`text:${record}`);
    await app.wait(120);
    expect(app.texts()).not.toContain("Select a record");
    await app.press("label:Payload detail");
    await app.wait(60);
  }
});

test("command center runs read-only commands", async () => {
  const app = await open("./App.tsx", options);
  await app.press("label:Open command center");
  await app.type("text:Search commands", "stats");
  await app.waitFor("text:stats");
  await app.press("text:stats");
  await app.wait(300);
});

test("command center supports arrow selection and enter", async () => {
  const app = await open("./App.tsx", options);
  await app.press("label:Open command center");
  await app.waitFor("text:theme");
  await app.type("text:Search commands", " ");
  await app.keyboard.press("Down");
  await app.keyboard.press("Return");
  await app.wait(80);
  expect(app.value("id:composer")).toBe("/model ");
});

test("commands that take arguments prefill the composer", async () => {
  const app = await open("./App.tsx", options);
  await app.press("label:Open command center");
  await app.type("text:Search commands", "goal create");
  await app.waitFor("text:goal create");
  await app.press("label:goal create");
  await app.wait(80);
  expect(app.value("id:composer")).toBe("/goal create ");
});

test("read-only commands run without failing", async () => {
  const app = await open("./App.tsx", options);
  for (const command of [
    "stats",
    "history",
    "tasks-dag",
    "background",
    "mcp list",
    "hooks list",
    "goal list",
    "help",
  ]) {
    await app.press("label:Open command center");
    await app.type("text:Search commands", command);
    await app.waitFor(`text:${command}`);
    await app.press(`label:${command} >> nth=0`);
    await app.wait(250);
    await app.keyboard.press("escape");
    await app.wait(80);
  }
});

test("attachments are written and shown as chips", async () => {
  const app = await open("./App.tsx", options);
  app.onPick(() => [
    { name: "shot.png", type: "image/png", data: new Uint8Array([137, 80, 78, 71]) },
  ]);
  await app.press("label:Attach image");
  await app.wait(200);
  expect(app.texts()).toContain("shot.png");
  await app.press("text:×");
  await app.wait(100);
  expect(app.texts()).not.toContain("shot.png");
});

test("the voice control does not fail when disabled", async () => {
  const app = await open("./App.tsx", options);
  await app.press("label:Voice input");
  await app.wait(100);
});

test("the project path input accepts typing", async () => {
  const app = await open("./App.tsx", options);
  await app.press("label:Switch project");
  await app.waitFor("id:project-path");
  await app.type("id:project-path", "/tmp");
  expect((app.value("id:project-path") ?? "").includes("/tmp")).toBe(true);
});

test("mod+enter submits the composer", async () => {
  const app = await open("./App.tsx", options);
  await app.type("id:composer", "/help");
  await app.keyboard.press("mod+enter");
  await app.waitFor("id:reader");
});
