#!/usr/bin/env node
// Render the Tact Desktop HTML prototype at the same logical size as the GPUI
// capture (1280x840 @2x) so the two can be compared pixel by pixel.
//
// Usage: node scripts/proto-shot.mjs [out.png] [theme]

import { spawn } from "node:child_process";
import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const CHROME =
  process.env.CHROME_PATH ??
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const PORT = 9333;
const OUT = process.argv[2] ?? "target/ui-proto/prototype.png";
const THEME = process.argv[3] ?? "light";
const STATE = process.argv[4] ?? "";
const WIDTH = 1280;
const HEIGHT = 840;

const chrome = spawn(
  CHROME,
  [
    "--headless=new",
    `--remote-debugging-port=${PORT}`,
    `--user-data-dir=/tmp/tact-proto-${process.pid}`,
    "--no-first-run",
    "--disable-gpu",
    "--hide-scrollbars",
    "--allow-file-access-from-files",
    "about:blank",
  ],
  { stdio: "ignore" },
);

chrome.on("error", (error) => {
  console.error(`proto-shot.mjs could not start Chrome (${CHROME}): ${error.message}`);
  process.exit(2);
});

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function waitForDevtools() {
  for (let i = 0; i < 100; i += 1) {
    try {
      const response = await fetch(`http://127.0.0.1:${PORT}/json/version`);
      if (response.ok) return;
    } catch {}
    await sleep(100);
  }
  throw new Error("Chrome DevTools endpoint did not start");
}

async function openPage() {
  const response = await fetch(
    `http://127.0.0.1:${PORT}/json/new?${encodeURIComponent("about:blank")}`,
    { method: "PUT" },
  );
  if (!response.ok) throw new Error(`json/new failed: ${response.status}`);
  return response.json();
}

function connect(wsUrl) {
  const ws = new WebSocket(wsUrl);
  let nextId = 1;
  const pending = new Map();
  const events = new Map();
  ws.addEventListener("message", (event) => {
    const message = JSON.parse(event.data);
    if (message.id) {
      const waiter = pending.get(message.id);
      if (waiter) {
        pending.delete(message.id);
        if (message.error) waiter.reject(new Error(JSON.stringify(message.error)));
        else waiter.resolve(message.result);
      }
    } else if (message.method) {
      for (const handler of events.get(message.method) ?? []) handler(message.params);
    }
  });
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const id = nextId++;
      pending.set(id, { resolve, reject });
      ws.send(JSON.stringify({ id, method, params }));
    });
  const once = (method) =>
    new Promise((resolve) => {
      const handler = (params) => {
        events.get(method).delete(handler);
        resolve(params);
      };
      if (!events.has(method)) events.set(method, new Set());
      events.get(method).add(handler);
    });
  return new Promise((resolve, reject) => {
    ws.addEventListener("open", () => resolve({ ws, send, once }));
    ws.addEventListener("error", reject);
  });
}

const main = async () => {
  await waitForDevtools();
  const page = await openPage();
  const { ws, send, once } = await connect(page.webSocketDebuggerUrl);

  await send("Page.enable");
  await send("Emulation.setDeviceMetricsOverride", {
    width: WIDTH,
    height: HEIGHT,
    deviceScaleFactor: 2,
    mobile: false,
  });

  const loaded = once("Page.loadEventFired");
  const hash = ["palette", "sessions", "extensions", "hooks", "subagent"].includes(STATE)
    ? `#${STATE}`
    : "";
  const url = `file://${ROOT}/docs/design/tact-desktop/prototype.html?theme=${THEME}${hash}`;
  await send("Page.navigate", { url });
  await loaded;
  await sleep(600);

  await send("Runtime.evaluate", {
    awaitPromise: true,
    expression: `
      (async () => {
        await document.fonts.ready;
        document.body.style.padding = "0";
        document.body.style.margin = "0";
        document.body.style.overflow = "hidden";
        document.querySelector(".page-grain")?.remove();
        const frame = document.querySelector(".app-frame");
        if (frame) {
          frame.style.height = "${HEIGHT}px";
          frame.style.width = "${WIDTH}px";
          frame.style.borderRadius = "0";
          frame.style.border = "0";
          frame.style.boxShadow = "none";
        }
        document.querySelector("#workbench")?.classList.remove("is-initializing");
        if ("${STATE}" === "inspector") {
          document.querySelector("#workbench")?.classList.remove("inspector-hidden");
          document.querySelector("#inspector-toggle")?.setAttribute("aria-pressed", "true");
        }
        return true;
      })()
    `,
  });
  await sleep(1200);

  const metrics = await send("Runtime.evaluate", {
    returnByValue: true,
    expression: `(() => {
      const selectors = [
        ".app-frame", ".titlebar", ".brand-mark", ".workspace-switcher",
        ".titlebar-actions", ".command-trigger", ".avatar-button",
        ".workbench", ".rail", ".project-switcher", ".new-run", ".rail-search",
        ".run-item", ".composer-zone", ".composer", ".sticky-host", ".run-room",
        "#palette-modal", "#session-manager", "#extensions-manager", "#hooks-sheet",
      ];
      const out = {};
      for (const selector of selectors) {
        const node = document.querySelector(selector);
        if (!node) continue;
        const rect = node.getBoundingClientRect();
        const style = getComputedStyle(node);
        out[selector] = {
          x: Math.round(rect.x * 100) / 100,
          y: Math.round(rect.y * 100) / 100,
          width: Math.round(rect.width * 100) / 100,
          height: Math.round(rect.height * 100) / 100,
          padding: style.padding,
          gap: style.gap,
          fontSize: style.fontSize,
          borderRadius: style.borderRadius,
        };
      }
      return out;
    })()`,
  });
  const metricsPath = resolve(ROOT, OUT).replace(/\.png$/, ".json");
  writeFileSync(metricsPath, JSON.stringify(metrics.result.value, null, 2));
  console.log(metricsPath);

  const shot = await send("Page.captureScreenshot", { format: "png" });
  mkdirSync(dirname(resolve(ROOT, OUT)), { recursive: true });
  writeFileSync(resolve(ROOT, OUT), Buffer.from(shot.data, "base64"));
  console.log(resolve(ROOT, OUT));

  await send("Target.closeTarget", { targetId: page.id }).catch(() => {});
  ws.close();
  chrome.kill("SIGKILL");
};

main().catch((error) => {
  console.error(error);
  chrome.kill("SIGKILL");
  process.exit(1);
});
