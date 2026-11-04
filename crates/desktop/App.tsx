import { mount, signal } from "nsh";
import { Controller } from "tact-desktop/native";
import { Shell } from "./tsx/Shell";
import type { Snapshot } from "./tsx/types";

const controller = new Controller();

function App() {
  const snapshot = signal<Snapshot>(JSON.parse(controller.snapshot()));
  const reader = signal<{ title: string; markdown: string } | null>(null);
  const toast = signal<{ title: string; body: string } | null>(null);

  controller.subscribe((message: string) => {
    const parsed = JSON.parse(message);
    if (parsed.type === "reader") {
      reader.set({ title: parsed.title, markdown: parsed.markdown });
      return;
    }
    if (parsed.type === "toast") {
      toast.set({ title: parsed.title, body: parsed.body });
      return;
    }
    snapshot.set(parsed as Snapshot);
  });

  return <Shell snapshot={snapshot} controller={controller} reader={reader} toast={toast} />;
}

mount(App);
