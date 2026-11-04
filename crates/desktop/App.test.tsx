import { expect, open, test } from "nsh/test";

test("the TSX shell reads the Rust snapshot", async () => {
  const app = await open("./App.tsx", { width: 1280, height: 840, platform: "macos" });
  await app.waitFor("text:Tact");
  expect(app.texts().length).toBeGreaterThan(0);
});
