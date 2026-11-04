import {
  List,
  Picker,
  Scroll,
  Text,
  View,
  computed,
  effect,
  keymap,
  signal,
  token,
} from "nsh";
import plexSans from "../assets/fonts/IBMPlexSans-Regular.ttf";
import lilex from "../assets/fonts/Lilex-Regular.ttf";
import { DiffView, Input, TextView } from "nsh/base";
import { AnsiBlock } from "./ansi";
import { MermaidDiagram, splitMermaid } from "./mermaid";
import type {
  Body,
  Gate,
  Goal,
  Row,
  Snapshot,
  Subagent,
  Task,
  Tool,
  ToolItem,
  TrajectoryDetail,
  TrajectoryState,
} from "./types";

type Controller = {
  dispatch(command: string): void;
  reader(kind: string, id: number | null): string;
  trajectory(): string;
  trajectoryRecord(index: number): string;
  attachment(name: string, dataBase64: string): string;
};

type TranscriptList = {
  scrollToEnd(options?: { animated?: boolean }): void;
};

type Overlay =
  | { kind: "command_center" }
  | { kind: "settings" }
  | { kind: "sessions" }
  | { kind: "extensions" }
  | { kind: "models" }
  | { kind: "projects" }
  | { kind: "trajectory" };

interface ShellProps {
  snapshot: ReturnType<typeof signal<Snapshot>>;
  controller: Controller;
  reader: ReturnType<typeof signal<{ title: string; markdown: string } | null>>;
  toast: ReturnType<typeof signal<{ title: string; body: string } | null>>;
  draft: ReturnType<typeof signal<string>>;
  attachments: ReturnType<typeof signal<string[]>>;
  voice: ReturnType<typeof signal<{ phase: string; error?: string }>>;
}

const sans = plexSans;
const mono = lilex;

function color(name: string): string {
  return token(name as never) as string;
}


export function Shell(props: ShellProps) {
  const { snapshot, controller, reader, toast, draft, attachments, voice } = props;
  const overlay = signal<Overlay | null>(null);
  const historyCursor = signal<number | null>(null);
  const expandedThinking = signal<number[]>([]);
  const runTab = signal<"chat" | "trajectory">("chat");
  const railOpen = signal(true);
  const inspectorOpen = signal(true);
  const viewportWidth = signal(0);
  const lastNarrow = signal<boolean | null>(null);
  const transcriptRef = signal<TranscriptList | null>(null);
  const followingTail = signal(snapshot().tail_following);
  let scrollTimer: ReturnType<typeof setTimeout> | null = null;
  const slashCursor = signal(0);
  const menu = signal<string | null>(null);
  const narrow = computed(() => viewportWidth() > 0 && viewportWidth() < 1080);

  const send = (command: object) => controller.dispatch(JSON.stringify(command));
  const resize = (width: number) => {
    viewportWidth.set(width);
    const next = width > 0 && width < 1080;
    if (lastNarrow() === next) return;
    lastNarrow.set(next);
    railOpen.set(!next);
    inspectorOpen.set(!next);
  };
  const toggleRail = () => {
    if (narrow()) inspectorOpen.set(false);
    railOpen.update((open) => !open);
  };
  const toggleInspector = () => {
    if (narrow()) railOpen.set(false);
    inspectorOpen.update((open) => !open);
  };

  const submit = (text: string) => {
    const value = text.trim();
    if (!value) return;
    if (value.startsWith("/")) {
      const body = value.slice(1);
      const parts = body.split(/\s+/).filter(Boolean);
      const path = parts.length > 0 ? [parts[0]] : [];
      if (path.length > 0) {
        const command = snapshot()
          .composer.commands.find((entry) => entry.path.join(" ") === path.join(" "));
        if (command) {
          send({ kind: "invoke_command", path: command.path, tokens: parts.slice(1) });
          draft.set("");
          return;
        }
      }
    }
    send({ kind: "submit", text: value, attachments: attachments() });
    draft.set("");
    attachments.set([]);
  };

  const showInspector = (tab: string) => {
    if (narrow()) railOpen.set(false);
    inspectorOpen.set(true);
    send({ kind: "set_inspector", tab });
  };
  keymap.bind({
    "mod+k": () => overlay.set({ kind: "command_center" }),
    "mod+,": () => overlay.set({ kind: "settings" }),
    "mod+b": toggleRail,
    "mod+alt+b": toggleInspector,
    "mod+n": () => send({ kind: "new_session" }),
    "mod+enter": () => submit(draft()),
    "mod+.": () => send({ kind: "cancel_run" }),
    "mod+shift+l": () => send({ kind: "back_to_bottom" }),
    "mod+1": () => showInspector("run"),
    "mod+2": () => showInspector("goals"),
    "mod+3": () => showInspector("subagents"),
    "mod+4": () => showInspector("context"),
    "mod+5": () => showInspector("background"),
    escape: dismissTopmost,
  }, { when: "always" });
  function dismissTopmost() {
    if (menu()) {
      menu.set(null);
      return;
    }
    if (overlay()) {
      overlay.set(null);
      return;
    }
    if (reader()) {
      reader.set(null);
      return;
    }
    if (toast()) {
      toast.set(null);
      return;
    }
    if (narrow() && railOpen()) {
      railOpen.set(false);
      return;
    }
    if (narrow() && inspectorOpen()) {
      inspectorOpen.set(false);
      return;
    }
    const gate = snapshot().pending_gates[0];
    if (gate) {
      send({
        kind: "respond_gate",
        session_epoch: gate.session_epoch,
        request_id: gate.request_id,
        selected: [],
        single: null,
      });
      return;
    }
    if (snapshot().session.phase === "running") {
      send({ kind: "cancel_run" });
    }
  }

  const openReader = (kind: string, id: number | string | null) => {
    send({
      kind: "open_reader",
      reader_kind: kind,
      id: typeof id === "number" ? id : null,
      key: typeof id === "string" ? id : null,
    });
  };

  const onTranscriptScroll = (event: {
    offset: { y: number };
    contentSize: { height: number };
    size: { height: number };
  }) => {
    const distance =
      event.contentSize.height - event.size.height - event.offset.y;
    const next = distance <= 32;
    if (next === followingTail()) return;
    followingTail.set(next);
    send({ kind: "set_tail_following", following: next });
  };

  const current = computed(() => snapshot());
  const rows = computed(() => current().rows);
  effect(() => {
    followingTail.set(snapshot().tail_following);
  });
  effect(() => {
    snapshot().revision;
    rows().length;
    const list = transcriptRef();
    if (!followingTail() || !list) return;
    if (scrollTimer != null) return;
    scrollTimer = setTimeout(() => {
      scrollTimer = null;
      list.scrollToEnd({ animated: false });
    }, 0);
  });
  const activeTask = computed(() => {
    const tasks = current().inspector.tasks;
    return (
      tasks.find((task) => task.status === "inprogress") ??
      tasks.find((task) => task.status === "pending") ??
      null
    );
  });
  const pendingGate = computed(() => current().pending_gates[0] ?? null);

  return (
    <View
      onLayout={(bounds: { size: { width: number; height: number } }) =>
        resize(bounds.size.width)
      }
      style={{
        flex: 1,
        flexDirection: "column",
        backgroundColor: color("background"),
      }}
    >
      <TitleBar
        snapshot={snapshot}
        menu={menu}
        overlay={overlay}
        railOpen={railOpen}
        inspectorOpen={inspectorOpen}
        narrow={narrow()}
        toggleRail={toggleRail}
        toggleInspector={toggleInspector}
        send={send}
      />
      <View style={{ flex: 1, flexDirection: "row" }}>
        {railOpen() && !narrow() ? (
          <Rail snapshot={snapshot} controller={controller} overlay={overlay} />
        ) : null}
        <View style={{ flex: 1, flexDirection: "column", minWidth: 0 }}>
          <RunTabs runTab={runTab} />
          {current().failure ? (
            <FailureBanner
              message={current().failure!}
              onSessions={() => overlay.set({ kind: "sessions" })}
              onSettings={() => overlay.set({ kind: "settings" })}
            />
          ) : null}
          {runTab() === "chat" ? (
            <View style={{ flex: 1, flexDirection: "column", minHeight: 0 }}>
              <RunHeader snapshot={snapshot} draft={draft} />
              {activeTask() ? <StickyTask task={activeTask()!} /> : null}
              {rows().length === 0 ? (
                <EmptyState snapshot={snapshot} draft={draft} send={send} />
              ) : (
                <List
                  testID="transcript-list"
                  each={rows()}
                  keyExtractor={(row) => String(row.id)}
                  ref={(node) => transcriptRef.set(node)}
                  onScroll={onTranscriptScroll}
                  scrollEventThrottle={16}
                  maintainVisibleContentPosition={{ autoscrollToBottomThreshold: 32 }}
                  style={{ flex: 1 }}
                >
                  {(row) => (
                    <View
                      style={{
                        width: "100%",
                        maxWidth: 900,
                        alignSelf: "center",
                        paddingLeft: 12,
                        paddingRight: 12,
                        paddingTop: 4,
                        paddingBottom: 4,
                      }}
                    >
                      <RowView
                        row={row()}
                        snapshot={snapshot}
                        controller={controller}
                        draft={draft}
                        expandedThinking={expandedThinking}
                        openReader={openReader}
                        send={send}
                      />
                    </View>
                  )}
                </List>
              )}
              {pendingGate() ? (
                <GateBanner gate={pendingGate()!} send={send} />
              ) : null}
              <Composer
                snapshot={snapshot}
                controller={controller}
                draft={draft}
                attachments={attachments}
                voice={voice}
                historyCursor={historyCursor}
                slashCursor={slashCursor}
                overlay={overlay}
                send={send}
                submit={submit}
              />
            </View>
          ) : (
            <TrajectorySnapshot snapshot={snapshot} controller={controller} />
          )}
        </View>
        {inspectorOpen() && !narrow() ? (
          <Inspector
            snapshot={snapshot}
            controller={controller}
            send={send}
            openReader={openReader}
          />
        ) : null}
      </View>
      {narrow() && railOpen() ? (
        <View
          testID="rail-drawer-scrim"
          onPress={() => railOpen.set(false)}
          style={{
            position: "fixed",
            top: 44,
            left: 0,
            right: 0,
            bottom: 0,
            zIndex: 20,
            backgroundColor: color("scrim"),
          }}
        >
          <View onPress={() => {}} style={{ width: 250, height: "100%" }}>
            <Rail snapshot={snapshot} controller={controller} overlay={overlay} />
          </View>
        </View>
      ) : null}
      {narrow() && inspectorOpen() ? (
        <View
          testID="inspector-drawer-scrim"
          onPress={() => inspectorOpen.set(false)}
          style={{
            position: "fixed",
            top: 44,
            left: 0,
            right: 0,
            bottom: 0,
            zIndex: 20,
            alignItems: "flex-end",
            backgroundColor: color("scrim"),
          }}
        >
          <View onPress={() => {}} style={{ width: 318, height: "100%" }}>
            <Inspector
              snapshot={snapshot}
              controller={controller}
              send={send}
              openReader={openReader}
            />
          </View>
        </View>
      ) : null}
      {menu() ? (
        <AppMenu
          menu={menu}
          overlay={overlay}
          toggleRail={toggleRail}
          toggleInspector={toggleInspector}
          send={send}
        />
      ) : null}
      {overlay() ? (
        <OverlayPanel
          overlay={overlay()!}
          snapshot={snapshot}
          controller={controller}
          draft={draft}
          send={send}
          close={() => overlay.set(null)}
        />
      ) : null}
      {reader() ? (
        <ReaderPanel reader={reader()!} close={() => reader.set(null)} />
      ) : null}
      {toast() ? (
        <ToastPanel toast={toast()!} close={() => toast.set(null)} />
      ) : null}
    </View>
  );
}

function TitleBar(props: {
  snapshot: ReturnType<typeof signal<Snapshot>>;
  overlay: ReturnType<typeof signal<Overlay | null>>;
  railOpen: ReturnType<typeof signal<boolean>>;
  inspectorOpen: ReturnType<typeof signal<boolean>>;
  narrow: boolean;
  toggleRail: () => void;
  toggleInspector: () => void;
  menu: ReturnType<typeof signal<string | null>>;
  send: (command: object) => void;
}) {
  const { snapshot, overlay, railOpen, inspectorOpen, toggleRail, toggleInspector, menu, send } =
    props;
  const session = computed(() => snapshot().session);
  const account = computed(() => snapshot().account);
  return (
    <View
      style={{
        height: 44,
        flexDirection: "row",
        alignItems: "center",
        gap: 14,
        paddingLeft: 78,
        paddingRight: 16,
        backgroundColor: color("title-bar"),
        borderBottomWidth: 1,
        borderBottomColor: color("title-bar-border"),
      }}
    >
      <View style={{ width: 250, flexDirection: "row", alignItems: "center", gap: 8 }}>
        <View
          style={{
            width: 27,
            height: 27,
            borderRadius: 7,
            backgroundColor: color("primary"),
            alignItems: "center",
            justifyContent: "center",
          }}
        >
          <Text style={{ color: color("primary-foreground"), fontSize: 15, fontWeight: 600 }}>
            T
          </Text>
        </View>
        <Text style={{ fontSize: 13, fontWeight: 600 }}>Tact</Text>
        {["File", "View", "Session"].map((label) => (
          <Text
            role="button"
            accessibilityLabel={`${label} menu`}
            onPress={() => menu.set(menu() === label ? null : label)}
            style={{
              fontSize: 11,
              paddingLeft: 6,
              paddingRight: 6,
              paddingTop: 3,
              paddingBottom: 3,
              borderRadius: 5,
              color: menu() === label ? color("foreground") : color("muted-foreground"),
              backgroundColor: menu() === label ? color("muted") : "transparent",
            }}
          >
            {label}
          </Text>
        ))}
      </View>
      {props.narrow ? (
        <View style={{ flex: 1 }} />
      ) : (
        <Text
          testID="titlebar-project"
          style={{
            flex: 1,
            fontSize: 13,
            color: color("muted-foreground"),
          }}
        >
          {snapshot().project.name}
          {snapshot().project.branch ? ` · ${snapshot().project.branch}` : ""}
        </Text>
      )}
      {account() ? <AccountChip account={account()!} /> : null}
      <ChromeButton
        label={railOpen() ? "Hide sessions" : "Show sessions"}
        text="Sessions"
        onPress={toggleRail}
      />
      <ChromeButton
        label={inspectorOpen() ? "Hide inspector" : "Show inspector"}
        text="Inspector"
        onPress={toggleInspector}
      />
      <ChromeButton
        label="Open command center"
        text="Commands"
        onPress={() => overlay.set({ kind: "command_center" })}
      />
      <ChromeButton
        label="Open settings"
        text="Settings"
        onPress={() => overlay.set({ kind: "settings" })}
      />
    </View>
  );
}

function AppMenu(props: {
  menu: ReturnType<typeof signal<string | null>>;
  overlay: ReturnType<typeof signal<Overlay | null>>;
  toggleRail: () => void;
  toggleInspector: () => void;
  send: (command: object) => void;
}) {
  const items: Array<[string, () => void]> =
    props.menu() === "File"
      ? [
          ["New Session", () => props.send({ kind: "new_session" })],
          ["Command Center", () => props.overlay.set({ kind: "command_center" })],
          ["Settings", () => props.overlay.set({ kind: "settings" })],
          ["Extensions", () => props.overlay.set({ kind: "extensions" })],
        ]
      : props.menu() === "View"
        ? [
            ["Toggle Sessions", props.toggleRail],
            ["Toggle Inspector", props.toggleInspector],
            ["Back to Bottom", () => props.send({ kind: "back_to_bottom" })],
          ]
        : [["Stop", () => props.send({ kind: "cancel_run" })]];
  return (
    <View
      testID="app-menu-scrim"
      onPress={() => props.menu.set(null)}
      style={{
        position: "fixed",
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
        zIndex: 39,
      }}
    >
      <View
        onPress={() => {}}
        style={{
          position: "absolute",
          top: 44,
          left: props.menu() === "File" ? 198 : props.menu() === "View" ? 236 : 284,
          width: 220,
          flexDirection: "column",
          gap: 1,
          padding: 4,
          borderRadius: 12,
          backgroundColor: color("popover"),
          borderWidth: 1,
          borderColor: color("line-strong"),
          boxShadow: "0 24px 60px rgba(22, 22, 30, 0.26)",
          zIndex: 40,
        }}
      >
        {items.map(([label, action]) => (
          <Text
            role="button"
            accessibilityLabel={label}
            onPress={() => {
              action();
              props.menu.set(null);
            }}
            style={{
              fontSize: 12,
              paddingLeft: 12,
              paddingRight: 12,
              paddingTop: 6,
              paddingBottom: 6,
              borderRadius: 6,
              color: color("foreground"),
            }}
          >
            {label}
          </Text>
        ))}
      </View>
    </View>
  );
}

function AccountChip(props: { account: NonNullable<Snapshot["account"]> }) {
  const label =
    props.account.kind === "balance"
      ? `${props.account.total} ${props.account.currency}`
      : props.account.kind === "usage_quota"
        ? `${props.account.used ?? 0} / ${props.account.limit ?? "—"}`
        : "";
  if (!label) return null;
  return (
    <Text
      style={{
        fontSize: 10,
        fontFamily: mono,
        paddingLeft: 6,
        paddingRight: 6,
        paddingTop: 2,
        paddingBottom: 2,
        borderRadius: 5,
        backgroundColor: color("muted"),
        color: color("muted-foreground"),
      }}
    >
      {label}
    </Text>
  );
}

function ChromeButton(props: { label: string; text: string; onPress: () => void }) {
  return (
    <Text
      role="button"
      accessibilityLabel={props.label}
      onPress={props.onPress}
      style={{
        fontSize: 10,
        paddingLeft: 8,
        paddingRight: 8,
        paddingTop: 6,
        paddingBottom: 6,
        borderRadius: 5,
        color: color("muted-foreground"),
      }}
    >
      {props.text}
    </Text>
  );
}

function Rail(props: {
  snapshot: ReturnType<typeof signal<Snapshot>>;
  controller: Controller;
  overlay: ReturnType<typeof signal<Overlay | null>>;
}) {
  const { snapshot, controller, overlay } = props;
  const send = (command: object) => controller.dispatch(JSON.stringify(command));
  const query = computed(() => snapshot().sessions.query);
  return (
    <View
      testID="rail"
      style={{
        width: 250,
        height: "100%",
        flexDirection: "column",
        backgroundColor: color("sidebar"),
        borderRightWidth: 1,
        borderRightColor: color("sidebar-border"),
        paddingLeft: 12,
        paddingRight: 12,
        paddingTop: 14,
        paddingBottom: 12,
        gap: 10,
      }}
    >
      <View style={{ flexDirection: "row", alignItems: "center", gap: 8 }}>
        <View style={{ flex: 1, flexDirection: "column", gap: 1 }}>
          <Text
            role="button"
            accessibilityLabel="Switch project"
            onPress={() => overlay.set({ kind: "projects" })}
            style={{ fontSize: 14, fontWeight: 600, color: color("sidebar-foreground") }}
          >
            {snapshot().project.name} ▾
          </Text>
          <Text style={{ fontSize: 11, color: color("muted-foreground") }}>
            {snapshot().project.branch ?? ""}
          </Text>
        </View>
        <Text
          role="button"
          accessibilityLabel="New session"
          onPress={() => send({ kind: "new_session" })}
          style={{
            width: 27,
            height: 27,
            borderRadius: 7,
            textAlign: "center",
            backgroundColor: color("primary"),
            color: color("primary-foreground"),
            fontSize: 14,
            paddingTop: 4,
          }}
        >
          +
        </Text>
      </View>
      <View style={{ flexDirection: "row", gap: 4 }}>
        {[
          ["active", "Active"],
          ["archived", "Archived"],
          ["all", "All"],
        ].map(([value, label]) => (
          <Text
            role="button"
            accessibilityLabel={`${label} sessions`}
            onPress={() => send({ kind: "set_session_filter", filter: value })}
            style={{
              flex: 1,
              textAlign: "center",
              fontSize: 11,
              paddingTop: 4,
              paddingBottom: 4,
              borderRadius: 6,
              backgroundColor:
                snapshot().sessions.filter === value ? color("sidebar-accent") : "transparent",
              color:
                snapshot().sessions.filter === value
                  ? color("sidebar-accent-foreground")
                  : color("muted-foreground"),
            }}
          >
            {label}
          </Text>
        ))}
      </View>
      <Input
        testID="session-search"
        value={query()}
        placeholder="Search sessions"
        onChange={(text: string) => send({ kind: "set_session_query", query: text })}
        style={{
          height: 33,
          fontSize: 12,
          paddingLeft: 10,
          paddingRight: 10,
          borderRadius: 7,
          borderWidth: 1,
          borderColor: color("sidebar-border"),
          backgroundColor: "transparent",
          color: color("sidebar-foreground"),
        }}
      />
      <View style={{ flexDirection: "row", alignItems: "center", paddingTop: 8 }}>
        <Text
          style={{
            flex: 1,
            fontSize: 10,
            fontFamily: mono,
            letterSpacing: 1.5,
            color: color("muted-foreground"),
          }}
        >
          SESSIONS
        </Text>
        <Text
          role="button"
          accessibilityLabel="Manage sessions"
          onPress={() => overlay.set({ kind: "sessions" })}
          style={{ fontSize: 10, color: color("primary") }}
        >
          Manage
        </Text>
      </View>
      <List
        testID="session-list"
        each={snapshot().sessions.items}
        keyExtractor={(session) => session.id}
        style={{ flex: 1 }}
        empty={
          <Text style={{ fontSize: 11, color: color("muted-foreground"), paddingLeft: 10 }}>
            No sessions
          </Text>
        }
      >
        {(session) => (
          <View
            role="button"
            accessibilityLabel={displayTitle(session().title)}
            onPress={() => send({ kind: "resume_session", session_id: session().id })}
            style={{
              flexDirection: "column",
              gap: 2,
              minHeight: 37,
              marginBottom: 2,
              paddingLeft: 10,
              paddingRight: 10,
              paddingTop: 6,
              paddingBottom: 6,
              borderRadius: 7,
              backgroundColor:
                snapshot().sessions.selected === session().id
                  ? color("sidebar-accent")
                  : "transparent",
            }}
          >
            <Text
              style={{
                fontSize: 13,
                fontWeight: 500,
                color: color("sidebar-foreground"),
              }}
            >
              {displayTitle(session().title)}
            </Text>
            <Text style={{ fontSize: 11, color: color("muted-foreground") }}>
              {relativeTime(session().updated_at)} · {session().message_count} messages
            </Text>
          </View>
        )}
      </List>
    </View>
  );
}

function relativeTime(iso: string): string {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return "";
  const seconds = Math.max(0, Math.floor((Date.now() - then) / 1000));
  if (seconds < 60) return "just now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  if (seconds < 172800) return "Yesterday";
  if (seconds < 604800) return `${Math.floor(seconds / 86400)}d ago`;
  return new Date(then).toLocaleDateString();
}

function displayTitle(title: string): string {
  return title.trim() || "Untitled session";
}

function RunTabs(props: { runTab: ReturnType<typeof signal<"chat" | "trajectory">> }) {
  return (
    <View
      style={{
        height: 40,
        flexDirection: "row",
        alignItems: "center",
        gap: 4,
        paddingLeft: 16,
        paddingRight: 16,
        borderBottomWidth: 1,
        borderBottomColor: color("border"),
      }}
    >
      {(["chat", "trajectory"] as const).map((tab) => (
        <Text
          role="button"
          accessibilityLabel={tab === "chat" ? "Chat" : "Trajectory"}
          onPress={() => props.runTab.set(tab)}
          style={{
            fontSize: 12,
            paddingLeft: 10,
            paddingRight: 10,
            paddingTop: 5,
            paddingBottom: 5,
            borderRadius: 7,
            fontWeight: props.runTab() === tab ? 600 : 400,
            backgroundColor: props.runTab() === tab ? color("muted") : "transparent",
            color:
              props.runTab() === tab ? color("foreground") : color("muted-foreground"),
          }}
        >
          {tab === "chat" ? "Chat" : "Trajectory"}
        </Text>
      ))}
    </View>
  );
}

function RunHeader(props: {
  snapshot: ReturnType<typeof signal<Snapshot>>;
  draft: ReturnType<typeof signal<string>>;
}) {
  const { snapshot, draft } = props;
  const title = computed(() => {
    const first = snapshot().rows.find((row) => row.body.kind === "user");
    if (first && first.body.kind === "user") {
      return first.body.text.split("\n")[0].slice(0, 120);
    }
    return "New session";
  });
  return (
    <View
      style={{
        width: "100%",
        maxWidth: 900,
        alignSelf: "center",
        paddingLeft: 24,
        paddingRight: 24,
        paddingTop: 30,
        paddingBottom: 16,
        gap: 8,
        borderBottomWidth: 1,
        borderBottomColor: color("border"),
      }}
    >
      <View style={{ flexDirection: "row", alignItems: "center", gap: 8 }}>
        <View
          style={{
            width: 6,
            height: 6,
            borderRadius: 3,
            backgroundColor:
              snapshot().session.phase === "running"
                ? color("success")
                : color("muted-foreground"),
          }}
        />
        <Text style={{ fontSize: 10, fontFamily: mono, fontWeight: 600, color: color("muted-foreground") }}>
          RUN {shortSession(snapshot().session.id)}
        </Text>
        <Text style={{ fontSize: 10, fontFamily: mono, color: color("muted-foreground") }}>
          {snapshot().runtime.elapsed_seconds}s · {snapshot().runtime.turns} TURNS
        </Text>
      </View>
      <Text style={{ fontSize: 30, lineHeight: 37, fontWeight: 600 }}>{title()}</Text>
      <View style={{ flexDirection: "row", gap: 8 }}>
        <InsertChip label="/" insertion="/" draft={draft} />
        <InsertChip label="@" insertion="@" draft={draft} />
      </View>
    </View>
  );
}

function shortSession(id: string | null): string {
  if (!id) return "—";
  const tail = id.split("-").pop() ?? id;
  return tail.slice(0, 4).toUpperCase();
}

function InsertChip(props: {
  label: string;
  insertion: string;
  draft: ReturnType<typeof signal<string>>;
}) {
  return (
    <Text
      role="button"
      accessibilityLabel={`Insert ${props.label}`}
      onPress={() => props.draft.set(props.draft() + props.insertion)}
      style={{
        fontSize: 10,
        fontFamily: mono,
        paddingLeft: 8,
        paddingRight: 8,
        paddingTop: 2,
        paddingBottom: 2,
        borderRadius: 5,
        backgroundColor: color("muted"),
        color: color("muted-foreground"),
      }}
    >
      {props.label}
    </Text>
  );
}

function StickyTask(props: { task: Task }) {
  return (
    <View
      style={{
        flexDirection: "row",
        alignItems: "center",
        gap: 8,
        paddingLeft: 24,
        paddingRight: 24,
        paddingTop: 8,
        paddingBottom: 8,
        backgroundColor: color("muted"),
      }}
    >
      <Text style={{ fontSize: 9, fontFamily: mono, fontWeight: 600, color: color("muted-foreground") }}>
        TASK
      </Text>
      <Text style={{ flex: 1, fontSize: 11 }}>{props.task.subject}</Text>
      <Text style={{ fontSize: 9, color: color("muted-foreground") }}>{props.task.status}</Text>
    </View>
  );
}

function FailureBanner(props: {
  message: string;
  onSessions: () => void;
  onSettings: () => void;
}) {
  return (
    <View
      style={{
        flexDirection: "row",
        alignItems: "center",
        gap: 12,
        marginLeft: 16,
        marginRight: 16,
        marginTop: 10,
        paddingLeft: 12,
        paddingRight: 12,
        paddingTop: 9,
        paddingBottom: 9,
        borderRadius: 8,
        borderWidth: 1,
        borderColor: color("danger"),
        backgroundColor: color("muted"),
      }}
    >
      <Text style={{ flex: 1, fontSize: 11, color: color("danger") }}>{props.message}</Text>
      <Text
        role="button"
        accessibilityLabel="Open sessions after a failure"
        onPress={props.onSessions}
        style={{ fontSize: 10, color: color("primary") }}
      >
        Sessions
      </Text>
      <Text
        role="button"
        accessibilityLabel="Open settings to fix the failure"
        onPress={props.onSettings}
        style={{ fontSize: 10, color: color("primary") }}
      >
        Settings
      </Text>
    </View>
  );
}

function EmptyState(props: {
  snapshot: ReturnType<typeof signal<Snapshot>>;
  draft: ReturnType<typeof signal<string>>;
  send: (command: object) => void;
}) {
  return (
    <View style={{ flex: 1, alignItems: "center", justifyContent: "center", padding: 32 }}>
      <View
        style={{
          width: "100%",
          maxWidth: 520,
          flexDirection: "column",
          gap: 12,
          borderRadius: 12,
          borderWidth: 1,
          borderColor: color("border"),
          backgroundColor: color("popover"),
          padding: 24,
        }}
      >
        <Text style={{ fontSize: 20, fontWeight: 600 }}>Start a run</Text>
        <Text style={{ fontSize: 13, color: color("muted-foreground") }}>
          Ask a question, name a task, or type / for a command.
        </Text>
        <View style={{ flexDirection: "row", gap: 8 }}>
          <InsertChip label="/" insertion="/" draft={props.draft} />
          <InsertChip label="@" insertion="@" draft={props.draft} />
        </View>
        <Text
          role="button"
          accessibilityLabel="New session"
          onPress={() => props.send({ kind: "new_session" })}
          style={{
            alignSelf: "flex-start",
            fontSize: 12,
            paddingLeft: 12,
            paddingRight: 12,
            paddingTop: 5,
            paddingBottom: 5,
            borderRadius: 7,
            backgroundColor: color("primary"),
            color: color("primary-foreground"),
          }}
        >
          New session
        </Text>
      </View>
    </View>
  );
}

function RowView(props: {
  row: Row;
  snapshot: ReturnType<typeof signal<Snapshot>>;
  controller: Controller;
  draft: ReturnType<typeof signal<string>>;
  expandedThinking: ReturnType<typeof signal<number[]>>;
  openReader: (kind: string, id: number | null) => void;
  send: (command: object) => void;
}) {
  const { row, draft, expandedThinking, openReader, send } = props;
  const body = row.body;
  switch (body.kind) {
    case "user":
      return (
        <View style={{ flexDirection: "row", justifyContent: "flex-end", gap: 12, paddingTop: 10, paddingRight: 10 }}>
          <View style={{ maxWidth: 620, alignItems: "flex-end", gap: 4 }}>
            <View style={{ flexDirection: "row", gap: 7, alignItems: "baseline" }}>
              <Text style={{ fontSize: 11, fontWeight: 600 }}>You</Text>
              {row.time ? (
                <Text style={{ fontSize: 10, fontFamily: mono, color: color("muted-foreground") }}>
                  {row.time}
                </Text>
              ) : null}
            </View>
            <View
              style={{
                borderRadius: 12,
                borderWidth: 1,
                borderColor: color("border"),
                backgroundColor: color("popover"),
                paddingLeft: 14,
                paddingRight: 14,
                paddingTop: 10,
                paddingBottom: 10,
              }}
            >
              <TextView markdown={body.text} selectable />
            </View>
            {body.attachments.length > 0 ? (
              <View style={{ flexDirection: "row", flexWrap: "wrap", gap: 4 }}>
                {body.attachments.map((name) => (
                  <Text
                    style={{
                      fontSize: 10,
                      paddingLeft: 6,
                      paddingRight: 6,
                      paddingTop: 2,
                      paddingBottom: 2,
                      borderRadius: 5,
                      backgroundColor: color("muted"),
                      color: color("muted-foreground"),
                    }}
                  >
                    {name}
                  </Text>
                ))}
              </View>
            ) : null}
          </View>
          <View
            style={{
              width: 28,
              height: 28,
              borderRadius: 8,
              backgroundColor: color("foreground"),
              alignItems: "center",
              justifyContent: "center",
            }}
          >
            <Text style={{ fontSize: 10, fontFamily: mono, fontWeight: 600, color: color("background") }}>
              ME
            </Text>
          </View>
        </View>
      );
    case "assistant":
      return (
        <View style={{ flexDirection: "row", gap: 12, paddingTop: 12, paddingBottom: 12 }}>
          <View
            style={{
              width: 28,
              height: 28,
              borderRadius: 8,
              borderWidth: 1,
              borderColor: color("border"),
              backgroundColor: color("card"),
              alignItems: "center",
              justifyContent: "center",
            }}
          >
            <Text style={{ fontSize: 16, color: color("primary") }}>T</Text>
          </View>
          <View style={{ flex: 1, flexDirection: "column", gap: 8 }}>
            <View style={{ flexDirection: "row", gap: 7, alignItems: "baseline" }}>
              <Text style={{ fontSize: 11, fontWeight: 600 }}>Tact</Text>
              {row.time ? (
                <Text style={{ fontSize: 10, fontFamily: mono, color: color("muted-foreground") }}>
                  {row.time}
                </Text>
              ) : null}
              <Text style={{ fontSize: 10, fontFamily: mono, color: color("muted-foreground") }}>
                · assistant
              </Text>
            </View>
            <AssistantBody markdown={body.markdown} />
            {body.suggestions.length > 0 ? (
              <View style={{ flexDirection: "row", flexWrap: "wrap", gap: 4 }}>
                {body.suggestions.map((suggestion) => (
                  <Text
                    role="button"
                    accessibilityLabel={`Use suggestion ${suggestion}`}
                    onPress={() => draft.set(suggestion)}
                    style={{
                      fontSize: 11,
                      paddingLeft: 8,
                      paddingRight: 8,
                      paddingTop: 3,
                      paddingBottom: 3,
                      borderRadius: 6,
                      borderWidth: 1,
                      borderColor: color("border"),
                      color: color("primary"),
                    }}
                  >
                    {suggestion}
                  </Text>
                ))}
              </View>
            ) : null}
          </View>
        </View>
      );
    case "thinking": {
      const open = computed(() => expandedThinking().includes(row.id));
      return (
        <View style={{ flexDirection: "column", gap: 4, paddingTop: 6, paddingBottom: 6 }}>
          <View
            role="button"
            accessibilityLabel="Toggle thinking"
            onPress={() =>
              expandedThinking.update((ids) =>
                ids.includes(row.id) ? ids.filter((id) => id !== row.id) : [...ids, row.id],
              )
            }
            style={{ flexDirection: "row", alignItems: "center", gap: 8 }}
          >
            <Text style={{ fontSize: 11, fontWeight: 500, color: color("muted-foreground") }}>
              Thinking
              {body.duration_ms != null ? ` · ${duration(body.duration_ms)}` : ""}
            </Text>
            <Text style={{ fontSize: 10, color: color("muted-foreground") }}>
              {open() ? "▾" : "▸"}
            </Text>
          </View>
          {open() ? (
            <View style={{ paddingLeft: 12 }}>
              <TextView markdown={body.markdown} selectable />
            </View>
          ) : null}
        </View>
      );
    }
    case "plan":
      if (body.tool_execution) return null;
      return (
        <View
          style={{
            flexDirection: "column",
            gap: 8,
            padding: 12,
            borderRadius: 12,
            borderWidth: 1,
            borderColor: color("border"),
            backgroundColor: color("card"),
          }}
        >
          <Text style={{ fontSize: 12, fontWeight: 600 }}>{body.title}</Text>
          {body.steps.map((step, index) => (
            <View style={{ flexDirection: "row", alignItems: "center", gap: 8 }}>
              <Text style={{ fontSize: 11, color: statusColor(step.status) }}>
                {step.status === "success" || step.status === "resolved"
                  ? "✓"
                  : step.status === "running"
                    ? "●"
                    : step.status === "failed"
                      ? "×"
                      : "○"}
              </Text>
              <Text style={{ flex: 1, fontSize: 12 }}>{step.label}</Text>
              <Text style={{ fontSize: 9, fontFamily: mono, color: color("muted-foreground") }}>
                #{index + 1}
              </Text>
            </View>
          ))}
        </View>
      );
    case "tool_group":
      return (
        <View
          style={{
            flexDirection: "column",
            gap: 8,
            padding: 12,
            borderRadius: 12,
            backgroundColor: color("card"),
            borderWidth: 1,
            borderColor: color("border"),
          }}
        >
          <View
            role="button"
            accessibilityLabel={`Toggle tool group ${body.summary}`}
            onPress={() => send({ kind: "toggle_group", id: row.id })}
            style={{ flexDirection: "row", alignItems: "center", gap: 8 }}
          >
            <Text style={{ flex: 1, fontSize: 11, fontFamily: mono, color: color("muted-foreground") }}>
              {body.summary}
            </Text>
            <Text style={{ fontSize: 10, color: color("muted-foreground") }}>
              {body.collapsed ? "▸" : "▾"}
            </Text>
          </View>
          {body.collapsed
            ? null
            : body.items.map((item, index) =>
                item.kind === "tool" ? (
                  <ToolCard
                    group={row.id}
                    tool={item.tool}
                    send={send}
                    openReader={openReader}
                    draft={draft}
                  />
                ) : (
                  <GateCard gate={item.gate} send={send} />
                ),
              )}
        </View>
      );
    case "diff":
      return (
        <View style={{ flexDirection: "column", gap: 4 }}>
          <View style={{ flexDirection: "row", alignItems: "center", gap: 8 }}>
            <Text style={{ flex: 1, fontSize: 12, fontWeight: 600 }}>
              {body.path ?? "diff"}
            </Text>
            <OpenButton onPress={() => openReader("diff", row.id)} />
          </View>
          <DiffView patch={body.patch} layout="unified" fontSize={11} />
        </View>
      );
    case "gate":
      return <GateCard gate={body.gate} send={send} />;
    case "notice":
      return (
        <Text
          style={{
            fontSize: 12,
            color: color("muted-foreground"),
            backgroundColor: color("muted"),
            borderRadius: 7,
            paddingLeft: 12,
            paddingRight: 12,
            paddingTop: 6,
            paddingBottom: 6,
          }}
        >
          {body.text}
        </Text>
      );
    case "hook_context":
      return (
        <View
          style={{
            flexDirection: "column",
            gap: 4,
            borderLeftWidth: 1,
            borderLeftColor: color("border"),
            paddingLeft: 12,
          }}
        >
          <Text style={{ fontSize: 10, fontFamily: mono, color: color("muted-foreground") }}>
            {body.source ?? "hook"}
          </Text>
          <TextView markdown={body.markdown} selectable />
        </View>
      );
    case "hook_status":
      return <Text style={{ fontSize: 11, color: color("muted-foreground") }}>{body.message}</Text>;
    case "error":
      return (
        <View
          style={{
            flexDirection: "row",
            alignItems: "center",
            gap: 8,
            paddingLeft: 12,
            paddingRight: 12,
            paddingTop: 8,
            paddingBottom: 8,
            borderRadius: 7,
            backgroundColor: color("danger"),
          }}
        >
          <Text style={{ flex: 1, fontSize: 12, color: color("primary-foreground") }}>
            {body.message}
          </Text>
          {body.retryable ? (
            <Text
              role="button"
              accessibilityLabel="Retry provider"
              onPress={() => send({ kind: "retry_provider" })}
              style={{
                fontSize: 10,
                paddingLeft: 8,
                paddingRight: 8,
                paddingTop: 2,
                paddingBottom: 2,
                borderRadius: 5,
                backgroundColor: color("primary-foreground"),
                color: color("danger"),
              }}
            >
              Retry
            </Text>
          ) : null}
        </View>
      );
    default:
      return null;
  }
}

function AssistantBody(props: { markdown: string }) {
  const split = splitMermaid(props.markdown);
  if (split.diagrams.length === 0) {
    return <TextView markdown={props.markdown} selectable />;
  }
  return (
    <View style={{ flexDirection: "column", gap: 8 }}>
      <TextView markdown={split.body} selectable />
      {split.diagrams.map((diagram, index) => (
        <MermaidDiagram diagram={diagram} index={index} />
      ))}
    </View>
  );
}

function ToolCard(props: {
  group: number;
  tool: Tool;
  send: (command: object) => void;
  openReader: (kind: string, id: number | null) => void;
  draft: ReturnType<typeof signal<string>>;
}) {
  const { group, tool, send, openReader } = props;
  return (
    <View style={{ flexDirection: "column", gap: 4, paddingLeft: 8, paddingRight: 8, paddingTop: 3, paddingBottom: 3, borderRadius: 6 }}>
      <View
        role="button"
        accessibilityLabel={`Toggle tool ${tool.name}`}
        onPress={() => send({ kind: "toggle_tool", group, tool: tool.id })}
        style={{ flexDirection: "row", alignItems: "center", gap: 8 }}
      >
        <Text style={{ fontSize: 9, color: color("muted-foreground") }}>
          {tool.expanded ? "▾" : "▸"}
        </Text>
        <Text style={{ fontSize: 11.5, fontFamily: mono, fontWeight: 600 }}>{tool.name}</Text>
        <Text style={{ flex: 1, fontSize: 11, color: color("muted-foreground") }}>
          {tool.arg_summary}
        </Text>
        {tool.duration_ms != null ? (
          <Text style={{ fontSize: 9, fontFamily: mono, color: color("muted-foreground") }}>
            {duration(tool.duration_ms)}
          </Text>
        ) : null}
        <Text style={{ fontSize: 9, color: statusColor(tool.status) }}>{tool.status}</Text>
        <OpenButton onPress={() => openReader("tool_output", tool.id)} />
      </View>
      {!tool.expanded && tool.output_preview ? (
        <View style={{ paddingLeft: 18 }}>
          <OutputBlock text={tool.output_preview} preview />
        </View>
      ) : null}
      {tool.expanded ? (
        <View style={{ flexDirection: "column", gap: 4 }}>
          <ToolBodyView body={tool.body} />
          {tool.output_detail || tool.output_preview ? (
            <OutputBlock text={tool.output_detail ?? tool.output_preview ?? ""} />
          ) : null}
        </View>
      ) : null}
    </View>
  );
}

function OutputBlock(props: { text: string; preview?: boolean }) {
  const text = () =>
    props.preview ? props.text.split("\n").slice(0, 3).join("\n") : props.text;
  return text().includes("\u001b[") ? (
    <AnsiBlock text={text()} />
  ) : (
    <Text
      style={{
        fontSize: 11,
        fontFamily: mono,
        backgroundColor: color("background"),
        borderRadius: 6,
        paddingLeft: 8,
        paddingRight: 8,
        paddingTop: props.preview ? 2 : 8,
        paddingBottom: props.preview ? 2 : 8,
        color: props.preview ? color("muted-foreground") : color("foreground"),
      }}
    >
      {text()}
    </Text>
  );
}

function ToolBodyView(props: { body: Tool["body"] }) {
  switch (props.body.kind) {
    case "none":
      return null;
    case "text":
      return <TextView markdown={props.body.text} selectable />;
    case "code":
      return (
        <TextView
          markdown={`\`\`\`${props.body.language ?? ""}\n${props.body.text}\n\`\`\``}
          selectable
        />
      );
    case "diff":
      return (
        <View style={{ flexDirection: "column", gap: 4 }}>
          <Text style={{ fontSize: 11, fontWeight: 600 }}>{props.body.path ?? "diff"}</Text>
          <DiffView patch={props.body.patch} layout="unified" fontSize={11} />
        </View>
      );
    default:
      return null;
  }
}

function OpenButton(props: { onPress: () => void }) {
  return (
    <Text
      role="button"
      accessibilityLabel="Open detail"
      onPress={props.onPress}
      style={{ fontSize: 10, color: color("primary"), paddingLeft: 6, paddingRight: 6 }}
    >
      Open
    </Text>
  );
}

function GateCard(props: { gate: Gate; send: (command: object) => void }) {
  const gate = props.gate;
  const multi = gate.kind === "multi_select";
  return (
    <View
      style={{
        flexDirection: "column",
        gap: 8,
        padding: 12,
        borderRadius: 12,
        borderWidth: 1,
        borderColor: color("border"),
        backgroundColor: color("card"),
      }}
    >
      <Text style={{ fontSize: 13 }}>{gate.prompt}</Text>
      <View style={{ flexDirection: "row", flexWrap: "wrap", gap: 8 }}>
        {gate.options.map((option, index) => {
          const selected = gate.selected.includes(index);
          return (
            <Text
              role="button"
              accessibilityLabel={`Choose ${option}`}
              onPress={() => {
                if (multi) {
                  const next = selected
                    ? gate.selected.filter((value) => value !== index)
                    : [...gate.selected, index];
                  props.send({
                    kind: "set_gate_draft",
                    session_epoch: gate.session_epoch,
                    request_id: gate.request_id,
                    selected: next,
                  });
                } else {
                  props.send({
                    kind: "respond_gate",
                    session_epoch: gate.session_epoch,
                    request_id: gate.request_id,
                    selected: [],
                    single: index,
                  });
                }
              }}
              style={{
                fontSize: 12,
                paddingLeft: 12,
                paddingRight: 12,
                paddingTop: 5,
                paddingBottom: 5,
                borderRadius: 7,
                backgroundColor: selected ? color("primary") : color("muted"),
                color: selected ? color("primary-foreground") : color("foreground"),
              }}
            >
              {option}
            </Text>
          );
        })}
        {multi ? (
          <Text
            role="button"
            accessibilityLabel="Submit gate selection"
            onPress={() =>
              props.send({
                kind: "respond_gate",
                session_epoch: gate.session_epoch,
                request_id: gate.request_id,
                selected: gate.selected,
                single: null,
              })
            }
            style={{
              fontSize: 12,
              paddingLeft: 12,
              paddingRight: 12,
              paddingTop: 5,
              paddingBottom: 5,
              borderRadius: 7,
              backgroundColor: color("primary"),
              color: color("primary-foreground"),
            }}
          >
            Submit
          </Text>
        ) : null}
      </View>
    </View>
  );
}

function Composer(props: {
  snapshot: ReturnType<typeof signal<Snapshot>>;
  controller: Controller;
  draft: ReturnType<typeof signal<string>>;
  attachments: ReturnType<typeof signal<string[]>>;
  voice: ReturnType<typeof signal<{ phase: string; error?: string }>>;
  historyCursor: ReturnType<typeof signal<number | null>>;
  slashCursor: ReturnType<typeof signal<number>>;
  overlay: ReturnType<typeof signal<Overlay | null>>;
  send: (command: object) => void;
  submit: (text: string) => void;
}) {
  const { snapshot, controller, draft, attachments, voice, historyCursor, slashCursor, overlay, send, submit } = props;
  const running = computed(() => snapshot().session.phase === "running");
  const slash = computed(() => {
    const text = draft();
    if (!text.startsWith("/")) return [];
    const fragment = text.slice(1).toLowerCase();
    return snapshot()
      .composer.commands.filter((command) => command.title.toLowerCase().includes(fragment))
      .slice(0, 8);
  });
  const onKey = (key: string) => {
    const items = slash();
    if (items.length > 0) {
      if (key === "down") {
        slashCursor.update((cursor) => (cursor + 1) % items.length);
        return;
      }
      if (key === "up") {
        slashCursor.update((cursor) => Math.max(0, cursor - 1));
        return;
      }
      if (key === "tab") {
        const item = items[slashCursor()];
        if (item) draft.set("/" + item.path.join(" ") + (item.needs_arguments ? " " : ""));
        return;
      }
    }
    const history = snapshot().composer.history;
    if (history.length === 0) return;
    if (key === "up") {
      const next = Math.min((historyCursor() ?? -1) + 1, history.length - 1);
      historyCursor.set(next);
      draft.set(history[next]);
    } else if (key === "down") {
      const cursor = historyCursor();
      if (cursor == null) return;
      if (cursor === 0) {
        historyCursor.set(null);
        draft.set("");
      } else {
        historyCursor.set(cursor - 1);
        draft.set(history[cursor - 1]);
      }
    }
  };
  return (
    <View style={{ width: "100%", alignItems: "center" }}>
      <View
        style={{
          width: "100%",
          maxWidth: 900,
          paddingLeft: 22,
          paddingRight: 22,
          paddingBottom: 18,
          flexDirection: "column",
          gap: 8,
        }}
      >
        {slash().length > 0 ? (
          <View
            style={{
              flexDirection: "column",
              gap: 1,
              padding: 4,
              borderRadius: 8,
              borderWidth: 1,
              borderColor: color("border"),
              backgroundColor: color("popover"),
            }}
          >
            {slash().map((command, index) => (
              <View
                onPress={() => {
                  if (command.path.length > 0 && snapshot().composer.commands.some((c) => c.path.length > command.path.length && c.path[0] === command.path[0])) {
                    draft.set("/" + command.path.join(" "));
                  } else {
                    send({ kind: "invoke_command", path: command.path, tokens: [] });
                    draft.set("");
                  }
                }}
                style={{
                  flexDirection: "column",
                  gap: 1,
                  paddingLeft: 12,
                  paddingRight: 12,
                  paddingTop: 3,
                  paddingBottom: 3,
                  borderRadius: 6,
                  backgroundColor:
                    slashCursor() === index ? color("muted") : "transparent",
                }}
              >
                <Text style={{ fontSize: 12, fontFamily: mono }}>{command.title}</Text>
                <Text style={{ fontSize: 10, color: color("muted-foreground") }}>
                  {command.description}
                </Text>
              </View>
            ))}
          </View>
        ) : null}
        {snapshot().composer.queue.length > 0 ? (
          <View style={{ flexDirection: "column", gap: 2 }}>
            {snapshot()
              .composer.queue.slice(0, 4)
              .map((item) => (
                <QueueRow
                  item={item}
                  count={snapshot().composer.queue.length}
                  send={send}
                />
              ))}
          </View>
        ) : null}
        {attachments().length > 0 ? (
          <View style={{ flexDirection: "row", flexWrap: "wrap", gap: 4 }}>
            {attachments().map((path) => (
              <View
                style={{
                  flexDirection: "row",
                  alignItems: "center",
                  gap: 4,
                  paddingLeft: 8,
                  paddingRight: 4,
                  paddingTop: 2,
                  paddingBottom: 2,
                  borderRadius: 6,
                  borderWidth: 1,
                  borderColor: color("border"),
                  backgroundColor: color("card"),
                }}
              >
                <Text style={{ fontSize: 10 }}>{path.split("/").pop() ?? path}</Text>
                <Text
                  onPress={() =>
                    attachments.update((list) => list.filter((entry) => entry !== path))
                  }
                  style={{ fontSize: 10, color: color("muted-foreground") }}
                >
                  ×
                </Text>
              </View>
            ))}
          </View>
        ) : null}
        <View
          style={{
            flexDirection: "row",
            alignItems: "flex-end",
            gap: 8,
            padding: 8,
            borderRadius: 12,
            borderWidth: 1,
            borderColor: color("line-strong"),
            backgroundColor: color("popover"),
          }}
        >
          <Input
            testID="composer"
            value={draft()}
            multiline
            placeholder="Steer the run or queue a follow-up…"
            submitBehavior={draft().startsWith("/") ? "submit" : "newline"}
            onChange={(text: string) => draft.set(text)}
            onSubmit={(event: { text: string }) => submit(event.text)}
            onKeyPress={(event: { key: string }) => onKey(event.key)}
            style={{
              flex: 1,
              fontSize: 13,
              lineHeight: 20,
              paddingLeft: 10,
              paddingRight: 10,
              paddingTop: 6,
              paddingBottom: 6,
              color: color("foreground"),
            }}
          />
          <Text
            role="button"
            accessibilityLabel={running() ? "Stop" : "Send"}
            onPress={() => {
              if (running()) {
                send({ kind: "cancel_run" });
              } else {
                submit(draft());
              }
            }}
            style={{
              width: 28,
              height: 27,
              borderRadius: 5,
              textAlign: "center",
              paddingTop: 6,
              backgroundColor: running() ? color("danger") : color("primary"),
              color: color("primary-foreground"),
              fontSize: 11,
            }}
          >
            {running() ? "■" : "↑"}
          </Text>
        </View>
        <View
          style={{
            flexDirection: "row",
            flexWrap: "wrap",
            alignItems: "center",
            gap: 8,
          }}
        >
          <Text
            role="button"
            accessibilityLabel="Attach image"
            onPress={async () => {
              const picked = await Picker.pickImages({ multiple: true, limit: 4 });
              for (const image of picked) {
                const bytes = new Uint8Array(await image.file.arrayBuffer());
                let binary = "";
                for (let index = 0; index < bytes.length; index += 0x8000) {
                  binary += String.fromCharCode(...bytes.subarray(index, index + 0x8000));
                }
                const path = controller.attachment(image.file.name, btoa(binary));
                if (path) attachments.update((list) => [...list, path]);
              }
            }}
            style={{ fontSize: 11, color: color("muted-foreground") }}
          >
            Attach
          </Text>
          <Text
            role="button"
            accessibilityLabel="Voice input"
            onPress={() => {
              if (!snapshot().voice_enabled || snapshot().voice_missing_api_key) return;
              const phase = voice().phase;
              if (phase === "idle") send({ kind: "voice", command: "start" });
              else if (phase === "recording") send({ kind: "voice", command: "stop" });
            }}
            style={{
              fontSize: 11,
              color:
                voice().phase === "recording"
                  ? color("primary-foreground")
                  : color("muted-foreground"),
              backgroundColor:
                voice().phase === "recording" ? color("danger") : "transparent",
              paddingLeft: 6,
              paddingRight: 6,
              borderRadius: 5,
            }}
          >
            {voice().phase === "recording"
              ? "● Stop"
              : voice().phase === "transcribing"
                ? "…"
                : "Mic"}
          </Text>
          <Text
            role="button"
            accessibilityLabel="Choose model"
            onPress={() => overlay.set({ kind: "models" })}
            style={{
              fontSize: 11,
              color: color("muted-foreground"),
              paddingLeft: 6,
              paddingRight: 6,
              paddingTop: 2,
              paddingBottom: 2,
              borderRadius: 5,
              backgroundColor: color("muted"),
            }}
          >
            {snapshot().composer.model || "Model"}
          </Text>
          <Text
            role="button"
            accessibilityLabel="Cycle permission mode"
            onPress={() =>
              send({
                kind: "set_setting",
                id: "permission-mode",
                value: {
                  kind: "choice",
                  value:
                    snapshot().composer.permission === "default"
                      ? "plan"
                      : snapshot().composer.permission === "plan"
                        ? "auto"
                        : "default",
                },
              })
            }
            style={{
              fontSize: 11,
              color: color("muted-foreground"),
              paddingLeft: 6,
              paddingRight: 6,
              paddingTop: 2,
              paddingBottom: 2,
              borderRadius: 5,
              backgroundColor: color("muted"),
            }}
          >
            {snapshot().composer.permission}
          </Text>
          <View style={{ flex: 1 }} />
          <Text style={{ fontSize: 10, fontFamily: mono, color: color("muted-foreground") }}>
            {snapshot().runtime.usage.prompt_tokens}
            {snapshot().runtime.usage.context_window
              ? ` / ${snapshot().runtime.usage.context_window}`
              : " tokens"}
          </Text>
        </View>
        {voice().error ? (
          <Text style={{ fontSize: 10, color: color("destructive") }}>{voice().error}</Text>
        ) : null}
      </View>
    </View>
  );
}

function QueueRow(props: {
  item: { index: number; text: string };
  count: number;
  send: (command: object) => void;
}) {
  const text = signal(props.item.text);
  return (
    <View
      style={{
        flexDirection: "row",
        alignItems: "center",
        gap: 6,
        paddingLeft: 8,
        paddingRight: 8,
        paddingTop: 3,
        paddingBottom: 3,
        borderRadius: 6,
        backgroundColor: color("muted"),
      }}
    >
      <Input
        value={text()}
        onChange={(value: string) => text.set(value)}
        onSubmit={(event: { text: string }) =>
          props.send({ kind: "edit_queued", index: props.item.index, text: event.text })
        }
        style={{
          flex: 1,
          fontSize: 11,
          paddingLeft: 4,
          paddingRight: 4,
          paddingTop: 2,
          paddingBottom: 2,
          backgroundColor: "transparent",
          color: color("muted-foreground"),
        }}
      />
      <Text
        role="button"
        accessibilityLabel="Move queued prompt up"
        onPress={() =>
          props.item.index > 0 &&
          props.send({
            kind: "move_queued",
            from: props.item.index,
            to: props.item.index - 1,
          })
        }
        style={{ fontSize: 10, color: color("muted-foreground") }}
      >
        ↑
      </Text>
      <Text
        role="button"
        accessibilityLabel="Move queued prompt down"
        onPress={() =>
          props.item.index + 1 < props.count &&
          props.send({
            kind: "move_queued",
            from: props.item.index,
            to: props.item.index + 1,
          })
        }
        style={{ fontSize: 10, color: color("muted-foreground") }}
      >
        ↓
      </Text>
      <Text
        role="button"
        accessibilityLabel="Send queued prompt now"
        onPress={() => props.send({ kind: "send_queued_now", index: props.item.index })}
        style={{ fontSize: 9, color: color("primary") }}
      >
        Send now
      </Text>
      <Text
        role="button"
        accessibilityLabel="Remove queued prompt"
        onPress={() => props.send({ kind: "remove_queued", index: props.item.index })}
        style={{ fontSize: 10, color: color("muted-foreground") }}
      >
        ×
      </Text>
    </View>
  );
}

function Inspector(props: {
  snapshot: ReturnType<typeof signal<Snapshot>>;
  controller: Controller;
  send: (command: object) => void;
  openReader: (kind: string, id: number | string | null) => void;
}) {
  const { snapshot, send, openReader } = props;
  const tab = computed(() => snapshot().inspector.tab);
  return (
    <View
      testID="inspector-panel"
      style={{
        width: 318,
        height: "100%",
        flexDirection: "column",
        borderLeftWidth: 1,
        borderLeftColor: color("border"),
        backgroundColor: color("background"),
      }}
    >
      <View style={{ flexDirection: "row", flexWrap: "wrap", gap: 4, paddingLeft: 12, paddingRight: 12, paddingTop: 8, paddingBottom: 8, borderBottomWidth: 1, borderBottomColor: color("border") }}>
        {[
          ["run", "Run"],
          ["goals", "Goals"],
          ["subagents", "Subagents"],
          ["background", "Background"],
          ["context", "Context"],
        ].map(([value, label]) => (
          <Text
            role="button"
            accessibilityLabel={`${label} inspector`}
            onPress={() => send({ kind: "set_inspector", tab: value })}
            style={{
              fontSize: 11,
              paddingLeft: 8,
              paddingRight: 8,
              paddingTop: 4,
              paddingBottom: 4,
              borderRadius: 6,
              backgroundColor: tab() === value ? color("muted") : "transparent",
              color: tab() === value ? color("foreground") : color("muted-foreground"),
            }}
          >
            {label}
          </Text>
        ))}
      </View>
      <Scroll style={{ flex: 1 }}>
        <View style={{ flexDirection: "column", gap: 8, paddingLeft: 16, paddingRight: 16, paddingTop: 12 }}>
          {tab() === "run" ? <RunInspector snapshot={snapshot} send={send} /> : null}
          {tab() === "goals" ? <GoalsInspector goals={snapshot().inspector.goals} /> : null}
          {tab() === "subagents" ? (
            <SubagentsInspector
              agents={snapshot().inspector.subagents}
              send={send}
              openReader={openReader}
            />
          ) : null}
          {tab() === "background" ? (
            <BackgroundInspector
              tasks={snapshot().inspector.background}
              send={send}
              openReader={openReader}
            />
          ) : null}
          {tab() === "context" ? <ContextInspector snapshot={snapshot} /> : null}
        </View>
      </Scroll>
    </View>
  );
}

function RunInspector(props: {
  snapshot: ReturnType<typeof signal<Snapshot>>;
  send: (command: object) => void;
}) {
  const { snapshot, send } = props;
  const runtime = computed(() => snapshot().runtime);
  return (
    <View style={{ flexDirection: "column", gap: 6 }}>
      <Text style={{ fontSize: 12, fontWeight: 600 }}>{runtime().phase}</Text>
      <Text style={{ fontSize: 10, fontFamily: mono, color: color("muted-foreground") }}>
        {runtime().elapsed_seconds}s elapsed · {runtime().turns} turns
      </Text>
      <View style={{ flexDirection: "row", alignItems: "center", gap: 8 }}>
        <Text style={{ flex: 1, fontSize: 11, fontFamily: mono, color: color("muted-foreground") }}>
          {runtime().latest_tool ?? "No tool running"}
        </Text>
        {runtime().latest_tool_row != null ? (
          <Text
            role="button"
            accessibilityLabel="Open tool output"
            onPress={() =>
              send({ kind: "open_reader", reader_kind: "tool_output", id: runtime().latest_tool_row })
            }
            style={{ fontSize: 9, color: color("primary") }}
          >
            Open
          </Text>
        ) : null}
      </View>
      <Text style={{ fontSize: 10, fontFamily: mono, color: color("muted-foreground") }}>
        {runtime().usage.prompt_tokens} / {runtime().usage.context_window ?? "—"} context
      </Text>
      <Text style={{ fontSize: 10, fontFamily: mono, color: color("muted-foreground"), paddingTop: 8 }}>
        TASKS
      </Text>
      {snapshot().inspector.tasks.map((task) => (
        <View style={{ flexDirection: "column", gap: 0 }}>
          <Text style={{ fontSize: 11 }}>{task.subject}</Text>
          <Text style={{ fontSize: 9, color: color("muted-foreground") }}>
            {task.status} · {task.owner}
          </Text>
        </View>
      ))}
    </View>
  );
}

function GoalsInspector(props: { goals: Goal[] }) {
  return (
    <View style={{ flexDirection: "column", gap: 8 }}>
      {props.goals.length === 0 ? (
        <Text style={{ fontSize: 11, color: color("muted-foreground") }}>No goals</Text>
      ) : null}
      {props.goals.map((goal) => (
        <View style={{ flexDirection: "column", gap: 2 }}>
          <Text style={{ fontSize: 12, fontWeight: 600 }}>{goal.title}</Text>
          <Text style={{ fontSize: 11, color: color("muted-foreground") }}>{goal.objective}</Text>
          <Text style={{ fontSize: 9, color: color("muted-foreground") }}>
            {goal.status} · {goal.task_completed}/{goal.task_total}
          </Text>
        </View>
      ))}
    </View>
  );
}

function SubagentsInspector(props: {
  agents: Subagent[];
  send: (command: object) => void;
  openReader: (kind: string, id: number | string | null) => void;
}) {
  return (
    <View style={{ flexDirection: "column", gap: 8 }}>
      {props.agents.length === 0 ? (
        <Text style={{ fontSize: 11, color: color("muted-foreground") }}>No subagents</Text>
      ) : null}
      {props.agents.map((agent) => (
        <View style={{ flexDirection: "column", gap: 2 }}>
          <Text style={{ fontSize: 11, fontFamily: mono }}>{agent.child_id}</Text>
          <Text style={{ fontSize: 10, color: color("muted-foreground") }}>{agent.summary}</Text>
          <View style={{ flexDirection: "row", gap: 8 }}>
            <Text style={{ fontSize: 9, color: color("muted-foreground") }}>{agent.status}</Text>
            <Text
              role="button"
              accessibilityLabel={`Open subagent transcript ${agent.child_id}`}
              onPress={() => props.openReader("subagent", agent.child_id)}
              style={{ fontSize: 9, color: color("primary") }}
            >
              Transcript
            </Text>
            <Text
              role="button"
              accessibilityLabel={`Cancel subagent ${agent.child_id}`}
              onPress={() => props.send({ kind: "cancel_subagent", child_id: agent.child_id })}
              style={{ fontSize: 9, color: color("muted-foreground") }}
            >
              Cancel
            </Text>
          </View>
        </View>
      ))}
    </View>
  );
}

function BackgroundInspector(props: {
  tasks: Snapshot["inspector"]["background"];
  send: (command: object) => void;
  openReader: (kind: string, id: number | string | null) => void;
}) {
  return (
    <View style={{ flexDirection: "column", gap: 8 }}>
      {props.tasks.length === 0 ? (
        <Text style={{ fontSize: 11, color: color("muted-foreground") }}>No background tasks</Text>
      ) : null}
      {props.tasks.map((task) => (
        <View style={{ flexDirection: "column", gap: 2 }}>
          <Text style={{ fontSize: 11, fontFamily: mono }}>{task.command}</Text>
          <Text style={{ fontSize: 10, color: color("muted-foreground") }}>
            {task.status}
            {task.output_tail ? ` · ${task.output_tail}` : ""}
          </Text>
          <View style={{ flexDirection: "row", gap: 8 }}>
            <Text
              role="button"
              accessibilityLabel={`Open background output ${task.id}`}
              onPress={() => props.openReader("background", task.id)}
              style={{ fontSize: 9, color: color("primary") }}
            >
              Output
            </Text>
            <Text
              role="button"
              accessibilityLabel={`Stop background task ${task.id}`}
              onPress={() =>
                props.send({ kind: "cancel_background_task", task_id: task.id })
              }
              style={{ fontSize: 9, color: color("muted-foreground") }}
            >
              Stop
            </Text>
          </View>
        </View>
      ))}
    </View>
  );
}

function ContextInspector(props: { snapshot: ReturnType<typeof signal<Snapshot>> }) {
  const usage = computed(() => props.snapshot().inspector.context);
  return (
    <View style={{ flexDirection: "column", gap: 4 }}>
      <Text style={{ fontSize: 12 }}>{usage().prompt_tokens} prompt</Text>
      <Text style={{ fontSize: 12 }}>{usage().completion_tokens} completion</Text>
      <Text style={{ fontSize: 12 }}>{usage().total_tokens} total</Text>
      <Text style={{ fontSize: 11, color: color("muted-foreground") }}>
        {usage().context_window
          ? `${usage().total_tokens} of ${usage().context_window}`
          : "No context window"}
      </Text>
    </View>
  );
}

function OverlayPanel(props: {
  overlay: Overlay;
  snapshot: ReturnType<typeof signal<Snapshot>>;
  controller: Controller;
  draft: ReturnType<typeof signal<string>>;
  send: (command: object) => void;
  close: () => void;
}) {
  const { overlay, snapshot, draft, send, close } = props;
  return (
    <View
      style={{
        position: "fixed",
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
        alignItems: "center",
        justifyContent: "center",
        padding: 24,
        backgroundColor: color("scrim"),
      }}
    >
      <View
        testID="overlay-scrim"
        onPress={close}
        style={{ position: "absolute", top: 0, left: 0, right: 0, bottom: 0 }}
      />
      <View
        style={{
          width: "100%",
          maxWidth: 640,
          height:
            overlay.kind === "models" || overlay.kind === "projects" ? "52%" : "70%",
          flexShrink: 0,
          flexDirection: "column",
          borderRadius: 12,
          backgroundColor: color("popover"),
          borderWidth: 1,
          borderColor: color("line-strong"),
          boxShadow: "0 24px 60px rgba(22, 22, 30, 0.26)",
        }}
      >
        <View
          style={{
            height: 44,
            flexDirection: "row",
            alignItems: "center",
            gap: 8,
            paddingLeft: 16,
            paddingRight: 16,
            borderBottomWidth: 1,
            borderBottomColor: color("border"),
          }}
        >
          <Text style={{ flex: 1, fontSize: 13, fontWeight: 600 }}>{overlayTitle(overlay)}</Text>
          <Text
            role="button"
            accessibilityLabel="Close"
            onPress={close}
            style={{
              fontSize: 11,
              color: color("muted-foreground"),
              paddingLeft: 8,
              paddingRight: 8,
              paddingTop: 4,
              paddingBottom: 4,
              borderRadius: 5,
              backgroundColor: color("muted"),
            }}
          >
            Close
          </Text>
        </View>
        {overlay.kind === "sessions" ? (
          <View
            style={{
              flex: 1,
              paddingLeft: 16,
              paddingRight: 16,
              paddingTop: 12,
              paddingBottom: 12,
            }}
          >
            <SessionsPanel snapshot={snapshot} send={send} close={close} />
          </View>
        ) : (
          <Scroll style={{ flex: 1 }}>
            <View style={{ flexDirection: "column", gap: 8, paddingLeft: 16, paddingRight: 16, paddingTop: 12, paddingBottom: 12 }}>
            {overlay.kind === "command_center" ? (
              <CommandCenter snapshot={snapshot} draft={draft} send={send} close={close} />
            ) : null}
            {overlay.kind === "settings" ? (
              <SettingsPanel snapshot={snapshot} send={send} />
            ) : null}
            {overlay.kind === "extensions" ? (
              <ExtensionsPanel snapshot={snapshot} send={send} />
            ) : null}
            {overlay.kind === "models" ? (
              <ModelsPanel snapshot={snapshot} send={send} close={close} />
            ) : null}
            {overlay.kind === "projects" ? (
              <ProjectsPanel snapshot={snapshot} send={send} close={close} />
            ) : null}
            </View>
          </Scroll>
        )}
      </View>
    </View>
  );
}

function overlayTitle(overlay: Overlay): string {
  switch (overlay.kind) {
    case "command_center":
      return "Command center";
    case "settings":
      return "Settings";
    case "sessions":
      return "Sessions";
    case "extensions":
      return "Extensions";
    case "models":
      return "Model";
    case "projects":
      return "Projects";
    default:
      return "";
  }
}

function CommandCenter(props: {
  snapshot: ReturnType<typeof signal<Snapshot>>;
  draft: ReturnType<typeof signal<string>>;
  send: (command: object) => void;
  close: () => void;
}) {
  const query = signal("");
  const cursor = signal(0);
  const commands = computed(() => {
    const terms = query()
      .trim()
      .toLowerCase()
      .split(/\s+/)
      .filter(Boolean);
    return props
      .snapshot()
      .composer.commands
      .filter((command) => {
        if (terms.length === 0) return true;
        const haystack = `${command.title} ${command.path.join(" ")} ${command.description}`.toLowerCase();
        return terms.every((term) => haystack.includes(term));
      })
      .slice(0, 60);
  });
  const choose = (command: Snapshot["composer"]["commands"][number] | undefined) => {
    if (!command) return;
    if (command.needs_arguments) {
      props.draft.set(`/${command.path.join(" ")} `);
    } else {
      props.send({ kind: "invoke_command", path: command.path, tokens: [] });
    }
    props.close();
  };
  const onKey = (key: string) => {
    const count = commands().length;
    if (count === 0) return;
    if (key === "down") cursor.update((index) => (index + 1) % count);
    if (key === "up") cursor.update((index) => (index - 1 + count) % count);
  };
  keymap.bind(
    {
      Down: () => onKey("down"),
      Up: () => onKey("up"),
      Return: () => choose(commands()[cursor()]),
    },
    { when: "always" },
  );
  return (
    <View style={{ flexDirection: "column", gap: 8 }}>
      <Input
        value={query()}
        autoFocus
        submitBehavior="submit"
        placeholder="Search commands"
        onChange={(text: string) => {
          query.set(text);
          cursor.set(0);
        }}
        style={{
          fontSize: 12,
          paddingLeft: 12,
          paddingRight: 12,
          paddingTop: 7,
          paddingBottom: 7,
          borderRadius: 8,
          borderWidth: 1,
          borderColor: color("border"),
          backgroundColor: color("background"),
        }}
      />
      <View style={{ flexDirection: "column", gap: 2 }}>
        {commands().map((command, index) => (
          <View
            role="button"
            accessibilityLabel={command.path.join(" ")}
            onPress={() => choose(command)}
            style={{
              flexDirection: "column",
              gap: 1,
              paddingLeft: 12,
              paddingRight: 12,
              paddingTop: 6,
              paddingBottom: 6,
              borderRadius: 7,
              backgroundColor: cursor() === index ? color("muted") : "transparent",
            }}
          >
            <Text style={{ fontSize: 13, fontFamily: mono }}>{command.title}</Text>
            <Text style={{ fontSize: 11, color: color("muted-foreground") }}>
              {command.description}
            </Text>
          </View>
        ))}
        {commands().length === 0 ? (
          <Text style={{ fontSize: 11, color: color("muted-foreground") }}>
            No matching commands
          </Text>
        ) : null}
      </View>
    </View>
  );
}

function SettingsPanel(props: {
  snapshot: ReturnType<typeof signal<Snapshot>>;
  send: (command: object) => void;
}) {
  const current = computed(() => props.snapshot());
  const rows: Array<[string, string, "bool" | "choice" | "text", string]> = [
    ["theme", "Theme", "choice", current().appearance],
    ["language", "Language", "choice", current().language],
    ["model", "Model", "text", current().composer.model],
    ["model-subagent", "Subagent model", "text", current().composer.subagent_model],
    ["permission-mode", "Permission", "choice", current().composer.permission],
    ["reasoning-effort", "Reasoning effort", "choice", current().composer.effort ?? ""],
    [
      "thinking-budget",
      "Thinking budget",
      "text",
      current().composer.thinking_budget?.toString() ?? "",
    ],
    ["notifications", "Notifications", "bool", current().notifications ? "on" : "off"],
    ["hook-output", "Hook output", "bool", current().show_hook_output ? "on" : "off"],
    ["provider", "Provider", "text", current().settings.provider],
    ["base-url", "Base URL", "text", current().settings.base_url],
    ["api-key", "API key", "text", ""],
    ["max-tokens", "Max tokens", "text", current().settings.max_tokens?.toString() ?? ""],
    [
      "model-context-window",
      "Context window",
      "text",
      current().settings.context_window?.toString() ?? "",
    ],
    [
      "snapshot-max-items",
      "Snapshot max items",
      "text",
      current().settings.snapshot_max_items?.toString() ?? "",
    ],
    ["micro-compact", "Micro compact", "bool", current().settings.micro_compact ? "on" : "off"],
    ["tokio-console", "Tokio console", "bool", current().settings.tokio_console ? "on" : "off"],
    [
      "skill-body-auto-inject",
      "Skill body inject",
      "bool",
      current().settings.skill_body_auto_inject ? "on" : "off",
    ],
    ["config", "Config path", "text", current().settings.config_path ?? ""],
  ];
  return (
    <View style={{ flexDirection: "column", gap: 8 }}>
      {rows.map(([id, label, kind, value]) => (
        <View style={{ flexDirection: "row", alignItems: "center", gap: 12 }}>
          <Text style={{ flex: 1, fontSize: 12 }}>{label}</Text>
          {kind === "text" ? (
            <SettingTextRow
              id={id}
              value={value}
              placeholder={
                id === "api-key"
                  ? current().settings.api_key_set
                    ? "Configured"
                    : "Not set"
                  : undefined
              }
              send={props.send}
            />
          ) : (
            <Text
              role="button"
              accessibilityLabel={`${label} setting`}
              onPress={() => {
                if (kind === "bool") {
                  props.send({
                    kind: "set_setting",
                    id,
                    value: { kind: "bool", value: value !== "on" },
                  });
                } else {
                  const options =
                    id === "theme"
                      ? ["light", "dark"]
                      : id === "language"
                        ? ["en", "zh"]
                        : id === "permission-mode"
                          ? ["default", "plan", "auto"]
                          : ["low", "medium", "high", "xhigh", "max"];
                  const index = Math.max(0, options.indexOf(value));
                  props.send({
                    kind: "set_setting",
                    id,
                    value: { kind: "choice", value: options[(index + 1) % options.length] },
                  });
                }
              }}
              style={{
                fontSize: 12,
                paddingLeft: 12,
                paddingRight: 12,
                paddingTop: 5,
                paddingBottom: 5,
                borderRadius: 7,
                backgroundColor: color("muted"),
                color: color("foreground"),
              }}
            >
              {value || "—"}
            </Text>
          )}
        </View>
      ))}
    </View>
  );
}

function SettingTextRow(props: {
  id: string;
  value: string;
  placeholder?: string;
  send: (command: object) => void;
}) {
  const text = signal(props.value);
  return (
    <Input
      testID={`setting-${props.id}`}
      value={text()}
      placeholder={props.placeholder}
      onChange={(value: string) => text.set(value)}
      onSubmit={(event: { text: string }) =>
        props.send({
          kind: "set_setting",
          id: props.id,
          value: { kind: "text", value: event.text },
        })
      }
      style={{
        width: 240,
        fontSize: 12,
        paddingLeft: 8,
        paddingRight: 8,
        paddingTop: 4,
        paddingBottom: 4,
        borderRadius: 6,
        borderWidth: 1,
        borderColor: color("border"),
        backgroundColor: color("background"),
      }}
    />
  );
}

function SessionsPanel(props: {
  snapshot: ReturnType<typeof signal<Snapshot>>;
  send: (command: object) => void;
  close: () => void;
}) {
  const drafts = signal<Record<string, string>>({});
  const titleOf = (session: Snapshot["sessions"]["items"][number]) =>
    drafts()[session.id] ?? session.title;
  const setDraft = (id: string, title: string) =>
    drafts.update((current) => ({ ...current, [id]: title }));
  const clearDraft = (id: string) =>
    drafts.update((current) => {
      const next = { ...current };
      delete next[id];
      return next;
    });
  return (
    <View testID="sessions-panel" style={{ flex: 1, flexDirection: "column", gap: 4 }}>
      <View style={{ flexDirection: "row", gap: 4, marginBottom: 4 }}>
        {[
          ["active", "Active"],
          ["archived", "Archived"],
          ["all", "All"],
        ].map(([value, label]) => (
          <Text
            role="button"
            accessibilityLabel={`${label} sessions`}
            onPress={() => props.send({ kind: "set_session_filter", filter: value })}
            style={{
              flex: 1,
              textAlign: "center",
              fontSize: 11,
              paddingTop: 4,
              paddingBottom: 4,
              borderRadius: 6,
              backgroundColor:
                props.snapshot().sessions.filter === value
                  ? color("muted")
                  : "transparent",
              color:
                props.snapshot().sessions.filter === value
                  ? color("foreground")
                  : color("muted-foreground"),
            }}
          >
            {label}
          </Text>
        ))}
      </View>
      <Input
        testID="session-manager-search"
        value={props.snapshot().sessions.query}
        placeholder={`Search ${props.snapshot().project.name} sessions`}
        onChange={(text: string) =>
          props.send({ kind: "set_session_query", query: text })
        }
        style={{
          fontSize: 11,
          paddingLeft: 10,
          paddingRight: 10,
          paddingTop: 5,
          paddingBottom: 5,
          marginBottom: 4,
          borderRadius: 7,
          borderWidth: 1,
          borderColor: color("border"),
          backgroundColor: color("background"),
        }}
      />
      <List
        testID="session-manager-list"
        each={props.snapshot().sessions.items}
        keyExtractor={(session) => session.id}
        style={{ flex: 1 }}
        empty={
          <Text style={{ fontSize: 11, color: color("muted-foreground") }}>No sessions</Text>
        }
      >
        {(session) => (
          <SessionRow
            session={session()}
            title={titleOf(session())}
            onChangeTitle={(text) => setDraft(session().id, text)}
            onRename={(text) => {
              props.send({
                kind: "rename_session",
                session_id: session().id,
                title: text,
              });
              clearDraft(session().id);
            }}
            send={props.send}
            close={props.close}
          />
        )}
      </List>
    </View>
  );
}

function SessionRow(props: {
  session: Snapshot["sessions"]["items"][number];
  title: string;
  onChangeTitle: (text: string) => void;
  onRename: (text: string) => void;
  send: (command: object) => void;
  close: () => void;
}) {
  return (
    <View
      style={{
        flexDirection: "row",
        alignItems: "center",
        gap: 8,
        paddingLeft: 8,
        paddingRight: 8,
        paddingTop: 4,
        paddingBottom: 4,
        borderRadius: 7,
      }}
    >
      <Input
        testID="session-title"
        value={props.title}
        placeholder={props.title.trim() ? undefined : "Untitled session"}
        onChange={(text: string) => props.onChangeTitle(text)}
        onSubmit={(event: { text: string }) => props.onRename(event.text)}
        style={{
          flex: 1,
          fontSize: 12,
          paddingLeft: 8,
          paddingRight: 8,
          paddingTop: 3,
          paddingBottom: 3,
          borderRadius: 5,
          borderWidth: 1,
          borderColor: color("border"),
          backgroundColor: color("background"),
        }}
      />
      <Text
        role="button"
        accessibilityLabel={props.session.archived ? "Restore session" : "Archive session"}
        onPress={() =>
          props.send({
            kind: "set_session_archived",
            session_id: props.session.id,
            archived: !props.session.archived,
          })
        }
        style={{ fontSize: 10, color: color("muted-foreground") }}
      >
        {props.session.archived ? "Restore" : "Archive"}
      </Text>
      <Text
        role="button"
        accessibilityLabel="Open session"
        onPress={() => {
          props.send({ kind: "resume_session", session_id: props.session.id });
          props.close();
        }}
        style={{ fontSize: 10, color: color("primary") }}
      >
        Open
      </Text>
    </View>
  );
}

function ExtensionsPanel(props: {
  snapshot: ReturnType<typeof signal<Snapshot>>;
  send: (command: object) => void;
}) {
  const tab = signal(props.snapshot().extensions.tab);
  const query = signal("");
  const extensions = computed(() => props.snapshot().extensions);
  const terms = computed(() =>
    query()
      .trim()
      .toLowerCase()
      .split(/\s+/)
      .filter(Boolean),
  );
  const skills = computed(() =>
    extensions().skills.filter((skill) => {
      const words = terms();
      if (words.length === 0) return true;
      const haystack = `${skill.name} ${skill.description}`.toLowerCase();
      return words.every((word) => haystack.includes(word));
    }),
  );
  const plugins = computed(() =>
    extensions().plugins.filter((plugin) => {
      const words = terms();
      if (words.length === 0) return true;
      const haystack =
        `${plugin.name} ${plugin.marketplace} ${plugin.version ?? ""}`.toLowerCase();
      return words.every((word) => haystack.includes(word));
    }),
  );
  return (
    <View style={{ flexDirection: "column", gap: 8 }}>
      <View style={{ flexDirection: "row", gap: 4 }}>
        {[
          ["mcp", "MCP"],
          ["skills", "Skills"],
          ["plugins", "Plugins"],
          ["hooks", "Hooks"],
        ].map(([value, label]) => (
          <Text
            role="button"
            accessibilityLabel={`${label} extensions`}
            onPress={() => {
              tab.set(value);
              query.set("");
            }}
            style={{
              fontSize: 10,
              paddingLeft: 8,
              paddingRight: 8,
              paddingTop: 3,
              paddingBottom: 3,
              borderRadius: 5,
              backgroundColor: tab() === value ? color("muted") : "transparent",
              color: tab() === value ? color("foreground") : color("muted-foreground"),
            }}
          >
            {label}
          </Text>
        ))}
      </View>
      {tab() === "skills" || tab() === "plugins" ? (
        <Input
          testID="extensions-search"
          value={query()}
          placeholder={tab() === "skills" ? "Search skills" : "Search plugins"}
          onChange={(text: string) => query.set(text)}
          style={{
            fontSize: 11,
            paddingLeft: 10,
            paddingRight: 10,
            paddingTop: 5,
            paddingBottom: 5,
            borderRadius: 7,
            borderWidth: 1,
            borderColor: color("border"),
            backgroundColor: color("background"),
          }}
        />
      ) : null}
      {tab() === "skills" && skills().length === 0 ? (
        <Text style={{ fontSize: 11, color: color("muted-foreground") }}>No skills</Text>
      ) : null}
      {tab() === "plugins" && plugins().length === 0 ? (
        <Text style={{ fontSize: 11, color: color("muted-foreground") }}>No plugins</Text>
      ) : null}
      {tab() === "skills"
        ? skills().map((skill) => (
            <View style={{ flexDirection: "row", alignItems: "center", gap: 8 }}>
              <View style={{ flex: 1, flexDirection: "column" }}>
                <Text style={{ fontSize: 12 }}>{skill.name}</Text>
                <Text style={{ fontSize: 10, color: color("muted-foreground") }}>
                  {skill.description}
                </Text>
              </View>
              <Text
                role="button"
                accessibilityLabel={`${skill.enabled ? "Disable" : "Enable"} skill ${skill.name}`}
                onPress={() =>
                  props.send({
                    kind: "set_skill_enabled",
                    name: skill.name,
                    enabled: !skill.enabled,
                  })
                }
                style={{ fontSize: 10, color: skill.enabled ? color("muted-foreground") : color("primary") }}
              >
                {skill.enabled ? "Disable" : "Enable"}
              </Text>
            </View>
          ))
        : null}
      {tab() === "plugins"
        ? plugins().map((plugin) => (
            <Text style={{ fontSize: 12 }}>
              {plugin.name} · {plugin.marketplace}
              {plugin.version ? ` · ${plugin.version}` : ""}
            </Text>
          ))
        : null}
      {tab() === "mcp" || tab() === "hooks" ? (
        <View style={{ flexDirection: "column", gap: 6 }}>
          <Text style={{ fontSize: 10, fontWeight: 600, color: color("muted-foreground") }}>
            {tab() === "mcp" ? "MCP servers" : "Hooks"}
          </Text>
          {props.snapshot().surface ? (
            <TextView markdown={props.snapshot().surface!.body} selectable />
          ) : (
            <Text style={{ fontSize: 11, color: color("muted-foreground") }}>
              Run the command to load this list.
            </Text>
          )}
          <Text
            role="button"
            accessibilityLabel={`Load ${tab() === "mcp" ? "MCP servers" : "hooks"}`}
            onPress={() =>
              props.send({
                kind: "invoke_command",
                path: tab() === "mcp" ? ["mcp", "list"] : ["hooks", "list"],
                tokens: [],
              })
            }
            style={{
              alignSelf: "flex-start",
              fontSize: 10,
              paddingLeft: 8,
              paddingRight: 8,
              paddingTop: 3,
              paddingBottom: 3,
              borderRadius: 6,
              backgroundColor: color("muted"),
              color: color("primary"),
            }}
          >
            Load
          </Text>
        </View>
      ) : null}
    </View>
  );
}

function ModelsPanel(props: {
  snapshot: ReturnType<typeof signal<Snapshot>>;
  send: (command: object) => void;
  close: () => void;
}) {
  return (
    <View style={{ flexDirection: "column", gap: 2 }}>
      {props.snapshot().composer.models.length === 0 ? (
        <Text style={{ fontSize: 11, color: color("muted-foreground") }}>
          No models configured
        </Text>
      ) : null}
      {props.snapshot().composer.models.map((model) => (
        <Text
          role="button"
          accessibilityLabel={`Use model ${model}`}
          onPress={() => {
            props.send({
              kind: "set_setting",
              id: "model",
              value: { kind: "text", value: model },
            });
            props.close();
          }}
          style={{
            fontSize: 12,
            paddingLeft: 12,
            paddingRight: 12,
            paddingTop: 6,
            paddingBottom: 6,
            borderRadius: 7,
            backgroundColor:
              props.snapshot().composer.model === model ? color("muted") : "transparent",
          }}
        >
          {model}
        </Text>
      ))}
    </View>
  );
}

function ProjectsPanel(props: {
  snapshot: ReturnType<typeof signal<Snapshot>>;
  send: (command: object) => void;
  close: () => void;
}) {
  const path = signal("");
  return (
    <View style={{ flexDirection: "column", gap: 8 }}>
      {props.snapshot().projects.map((project) => (
        <View
          role="button"
          accessibilityLabel={`Open project ${project.name}`}
          onPress={() => {
            if (project.root) {
              props.send({ kind: "open_project", root: project.root });
              props.close();
            }
          }}
          style={{
            flexDirection: "row",
            alignItems: "center",
            gap: 8,
            paddingLeft: 8,
            paddingRight: 8,
            paddingTop: 6,
            paddingBottom: 6,
            borderRadius: 7,
            backgroundColor:
              props.snapshot().project.id === project.id ? color("muted") : "transparent",
          }}
        >
          <Text style={{ flex: 1, fontSize: 12 }}>{project.name}</Text>
          <Text style={{ fontSize: 10, color: color("muted-foreground") }}>
            {project.branch ?? ""}
          </Text>
        </View>
      ))}
      <View style={{ flexDirection: "row", gap: 8 }}>
        <Input
          testID="project-path"
          value={path()}
          placeholder="Project path"
          onChange={(text: string) => path.set(text)}
          style={{
            flex: 1,
            fontSize: 12,
            paddingLeft: 10,
            paddingRight: 10,
            paddingTop: 5,
            paddingBottom: 5,
            borderRadius: 7,
            borderWidth: 1,
            borderColor: color("border"),
            backgroundColor: color("background"),
          }}
        />
        <Text
          role="button"
          accessibilityLabel="Open project path"
          onPress={() => {
            if (path().trim()) {
              props.send({ kind: "open_project", root: path().trim() });
              props.close();
            }
          }}
          style={{ fontSize: 10, color: color("primary") }}
        >
          Open
        </Text>
      </View>
    </View>
  );
}

function ReaderPanel(props: {
  reader: { title: string; markdown: string };
  close: () => void;
}) {
  const isAnsi = props.reader.markdown.includes("\u001b[");
  const isDiff =
    props.reader.markdown.startsWith("diff --git") ||
    props.reader.markdown.startsWith("--- ") ||
    props.reader.markdown.includes("\n@@ ");
  return (
    <View
      style={{
        position: "fixed",
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
        alignItems: "center",
        justifyContent: "center",
        padding: 24,
        backgroundColor: color("scrim"),
      }}
    >
      <View
        testID="reader-scrim"
        onPress={props.close}
        style={{ position: "absolute", top: 0, left: 0, right: 0, bottom: 0 }}
      />
      <View
        testID="reader"
        style={{
          width: "100%",
          maxWidth: 760,
          height: "70%",
          flexDirection: "column",
          borderRadius: 12,
          backgroundColor: color("popover"),
          borderWidth: 1,
          borderColor: color("line-strong"),
          boxShadow: "0 24px 60px rgba(22, 22, 30, 0.26)",
        }}
      >
        <View
          style={{
            height: 44,
            flexDirection: "row",
            alignItems: "center",
            gap: 8,
            paddingLeft: 16,
            paddingRight: 16,
            borderBottomWidth: 1,
            borderBottomColor: color("border"),
          }}
        >
          <Text style={{ flex: 1, fontSize: 13, fontWeight: 600 }}>{props.reader.title}</Text>
          <Text
            role="button"
            accessibilityLabel="Close"
            onPress={props.close}
            style={{
              fontSize: 11,
              color: color("muted-foreground"),
              paddingLeft: 8,
              paddingRight: 8,
              paddingTop: 4,
              paddingBottom: 4,
              borderRadius: 5,
              backgroundColor: color("muted"),
            }}
          >
            Close
          </Text>
        </View>
        <Scroll style={{ flex: 1 }}>
          <View style={{ paddingLeft: 20, paddingRight: 20, paddingTop: 16, paddingBottom: 16 }}>
            {isAnsi ? (
              <AnsiBlock text={props.reader.markdown} />
            ) : isDiff ? (
              <DiffView patch={props.reader.markdown} layout="unified" fontSize={11} />
            ) : (
              <TextView markdown={props.reader.markdown} selectable />
            )}
          </View>
        </Scroll>
      </View>
    </View>
  );
}

function ToastPanel(props: {
  toast: { title: string; body: string };
  close: () => void;
}) {
  return (
    <View
      style={{
        position: "fixed",
        left: 0,
        right: 0,
        bottom: 24,
        alignItems: "center",
      }}
    >
      <View
        role="button"
        accessibilityLabel="Dismiss notification"
        onPress={props.close}
        style={{
          maxWidth: 520,
          flexDirection: "column",
          gap: 2,
          paddingLeft: 16,
          paddingRight: 16,
          paddingTop: 12,
          paddingBottom: 12,
          borderRadius: 12,
          backgroundColor: color("popover"),
          borderWidth: 1,
          borderColor: color("warning"),
        }}
      >
        <Text style={{ fontSize: 12, fontWeight: 600 }}>{props.toast.title}</Text>
        <Text style={{ fontSize: 11, color: color("muted-foreground") }}>{props.toast.body}</Text>
      </View>
    </View>
  );
}

function GateBanner(props: { gate: Gate; send: (command: object) => void }) {
  return (
    <View style={{ alignItems: "center" }}>
      <View
        style={{
          width: "100%",
          maxWidth: 900,
          marginLeft: 22,
          marginRight: 22,
          marginBottom: 8,
          padding: 12,
          flexDirection: "column",
          gap: 8,
          borderRadius: 12,
          backgroundColor: color("card"),
          borderWidth: 1,
          borderColor: color("warning"),
        }}
      >
        <Text style={{ fontSize: 10, fontWeight: 600, color: color("muted-foreground") }}>
          Pending decision
        </Text>
        <Text style={{ fontSize: 12 }}>{props.gate.prompt}</Text>
        <View style={{ flexDirection: "row", flexWrap: "wrap", gap: 8 }}>
          {props.gate.options.map((option, index) => (
            <Text
              role="button"
              accessibilityLabel={`Answer ${option}`}
              onPress={() =>
                props.send({
                  kind: "respond_gate",
                  session_epoch: props.gate.session_epoch,
                  request_id: props.gate.request_id,
                  selected: [],
                  single: index,
                })
              }
              style={{
                fontSize: 11,
                paddingLeft: 12,
                paddingRight: 12,
                paddingTop: 4,
                paddingBottom: 4,
                borderRadius: 6,
                backgroundColor: color("primary"),
                color: color("primary-foreground"),
              }}
            >
              {option}
            </Text>
          ))}
        </View>
      </View>
    </View>
  );
}

const DETAIL_TABS = ["Summary", "Payload", "Result", "Schema", "Timing"];

function TrajectorySnapshot(props: {
  snapshot: ReturnType<typeof signal<Snapshot>>;
  controller: Controller;
}) {
  const { snapshot, controller } = props;
  const selected = signal<number | null>(null);
  const tab = signal("Summary");
  const query = signal("");
  const detail = signal<TrajectoryDetail | null>(null);
  const readTrajectory = (): TrajectoryState => {
    try {
      return JSON.parse(controller.trajectory()) as TrajectoryState;
    } catch {
      return {
        summary: {
          turns: 0,
          records: 0,
          tool_calls: 0,
          measured_ms: 0,
          measured: 0,
          input_tokens: 0,
          output_tokens: 0,
          cache_hit_tokens: 0,
          cache_miss_tokens: 0,
          context_window: null,
          context_used: 0,
        },
        spans: [],
        records: [],
      };
    }
  };
  const trajectory = signal<TrajectoryState>(readTrajectory());
  effect(() => {
    snapshot().revision;
    trajectory.set(readTrajectory());
  });
  const records = computed(() => {
    const search = query().trim().toLowerCase();
    const all = trajectory().records;
    if (!search) return all;
    return all.filter(
      (record) =>
        record.text.toLowerCase().includes(search) ||
        (record.tool_name ?? "").toLowerCase().includes(search) ||
        record.kind.toLowerCase().includes(search),
    );
  });
  const select = (index: number) => {
    selected.set(index);
    try {
      detail.set(JSON.parse(controller.trajectoryRecord(index)) as TrajectoryDetail);
    } catch {
      detail.set(null);
    }
  };
  const body = computed(() => {
    const record = detail();
    if (!record) return "Select a record";
    if (tab() === "Payload") return record.input ?? "No payload";
    if (tab() === "Result") return record.output ?? "No result";
    if (tab() === "Schema") return record.tool_name ?? "No tool";
    if (tab() === "Timing") {
      const duration =
        record.duration_ms != null ? `${record.duration_ms} ms` : "No duration";
      const usage = record.usage
        ? `${record.usage.prompt_tokens} prompt · ${record.usage.completion_tokens} completion`
        : "";
      return `${duration}\n${usage}`;
    }
    return `${record.text}\n\n${record.status_label}\n${record.tool_name ?? ""}\n${record.at ?? ""}`;
  });
  return (
    <View style={{ flex: 1, flexDirection: "row" }}>
      <View
        style={{
          width: 280,
          flexDirection: "column",
          borderRightWidth: 1,
          borderRightColor: color("border"),
        }}
      >
        <View
          style={{
            flexDirection: "column",
            gap: 2,
            paddingLeft: 16,
            paddingRight: 16,
            paddingTop: 12,
            paddingBottom: 10,
            borderBottomWidth: 1,
            borderBottomColor: color("border"),
          }}
        >
          <Text style={{ fontSize: 12, fontWeight: 600 }}>
            {trajectory().summary.records} records · {trajectory().summary.turns} turns ·{" "}
            {trajectory().summary.tool_calls} tools
          </Text>
          <Text style={{ fontSize: 10, color: color("muted-foreground") }}>
            {trajectory().summary.measured} measured · {trajectory().summary.measured_ms} ms ·{" "}
            {trajectory().summary.input_tokens} in / {trajectory().summary.output_tokens} out
          </Text>
        </View>
        <View
          style={{
            flexDirection: "column",
            gap: 1,
            paddingLeft: 16,
            paddingRight: 16,
            paddingTop: 8,
            paddingBottom: 8,
          }}
        >
          {["Input", "Model", "Tools"].map((lane) => (
            <View style={{ flexDirection: "row", height: 8, gap: 1 }}>
              {trajectory().spans.map((span) => (
                <View
                  role="button"
                  accessibilityLabel={`Open trajectory record ${span.index}`}
                  onPress={() => select(span.index)}
                  style={{
                    flex: 1,
                    height: 6,
                    borderRadius: 2,
                    backgroundColor:
                      span.lane !== lane
                        ? color("background")
                        : selected() === span.index
                          ? color("foreground")
                          : span.is_error
                            ? color("danger")
                            : lane === "Input"
                              ? color("muted-foreground")
                              : lane === "Model"
                                ? color("primary")
                                : color("success"),
                  }}
                />
              ))}
            </View>
          ))}
        </View>
        <View style={{ paddingLeft: 16, paddingRight: 16, paddingTop: 4, paddingBottom: 8 }}>
          <Input
            testID="trajectory-search"
            value={query()}
            placeholder="Search the ledger"
            onChange={(text: string) => query.set(text)}
            style={{
              fontSize: 11,
              paddingLeft: 8,
              paddingRight: 8,
              paddingTop: 4,
              paddingBottom: 4,
              borderRadius: 6,
              borderWidth: 1,
              borderColor: color("border"),
              backgroundColor: color("background"),
            }}
          />
        </View>
        <List
          testID="trajectory-records"
          each={records()}
          keyExtractor={(record) => String(record.index)}
          style={{ flex: 1 }}
        >
          {(record) => (
            <View
              role="button"
              accessibilityLabel={`Open record ${record().index} ${record().text}`}
              onPress={() => select(record().index)}
              style={{
                flexDirection: "row",
                alignItems: "center",
                gap: 8,
                marginLeft: 12,
                marginRight: 12,
                marginBottom: 1,
                paddingLeft: 8,
                paddingRight: 8,
                paddingTop: 3,
                paddingBottom: 3,
                borderRadius: 5,
                backgroundColor:
                  selected() === record().index ? color("muted") : "transparent",
              }}
            >
              <Text style={{ width: 34, fontSize: 10, fontFamily: mono, color: color("muted-foreground") }}>
                #{record().index}
              </Text>
              <Text style={{ flex: 1, fontSize: 11 }}>{record().text}</Text>
              <Text style={{ fontSize: 9, color: statusColor(record().status) }}>
                {record().status_label}
              </Text>
            </View>
          )}
        </List>
      </View>
      <View style={{ flex: 1, flexDirection: "column" }}>
        <View
          style={{
            flexDirection: "row",
            gap: 4,
            paddingLeft: 16,
            paddingRight: 16,
            paddingTop: 8,
            paddingBottom: 8,
            borderBottomWidth: 1,
            borderBottomColor: color("border"),
          }}
        >
          {DETAIL_TABS.map((value) => (
            <Text
              role="button"
              accessibilityLabel={`${value} detail`}
              onPress={() => tab.set(value)}
              style={{
                fontSize: 10,
                paddingLeft: 8,
                paddingRight: 8,
                paddingTop: 3,
                paddingBottom: 3,
                borderRadius: 5,
                backgroundColor: tab() === value ? color("muted") : "transparent",
                color:
                  tab() === value ? color("foreground") : color("muted-foreground"),
              }}
            >
              {value}
            </Text>
          ))}
        </View>
        <Scroll style={{ flex: 1 }}>
          <View style={{ paddingLeft: 20, paddingRight: 20, paddingTop: 12 }}>
            {body().includes("\u001b[") ? (
              <AnsiBlock text={body()} />
            ) : (
              <TextView markdown={body()} selectable />
            )}
          </View>
        </Scroll>
      </View>
    </View>
  );
}

function statusColor(status: string): string {
  if (status === "failed") return color("danger");
  if (status === "success" || status === "resolved") return color("success");
  if (status === "running") return color("primary");
  return color("muted-foreground");
}

function duration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.floor(ms / 60000)}m`;
}
