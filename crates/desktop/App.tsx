import { mount, signal } from "nsh";
import { Controller } from "tact-desktop/native";
import { Shell } from "./tsx/Shell";
import type { Snapshot } from "./tsx/types";

const controller = new Controller();

function App() {
  const snapshot = signal<Snapshot>(JSON.parse(controller.snapshot()));
  const reader = signal<{ title: string; markdown: string } | null>(null);
  const toast = signal<{ title: string; body: string } | null>(null);
  const draft = signal("");
  const attachments = signal<string[]>([]);
  const voice = signal<{ phase: string; error?: string }>({ phase: "idle" });

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
    if (parsed.type === "voice") {
      if (parsed.event === "transcript") {
        draft.update((current) => (current ? `${current} ${parsed.text}` : parsed.text));
        voice.set({ phase: "idle" });
      } else if (parsed.event === "error") {
        voice.set({ phase: "idle", error: parsed.text });
      } else if (parsed.event === "recording_started") {
        voice.set({ phase: "recording" });
      } else if (parsed.event === "recording_stopped" || parsed.event === "transcribing") {
        voice.set({ phase: "transcribing" });
      } else {
        voice.set({ phase: "idle" });
      }
      return;
    }
    snapshot.set(parsed as Snapshot);
  });

  return (
    <Shell
      snapshot={snapshot}
      controller={controller}
      reader={reader}
      toast={toast}
      draft={draft}
      attachments={attachments}
      voice={voice}
    />
  );
}

mount(App);
