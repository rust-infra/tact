import { Scroll, Text, View, computed, signal, token } from "nsh";
import { DiffView, Input, TextView } from "nsh/base";
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
} from "./types";

type Controller = {
  dispatch(command: string): void;
  reader(kind: string, id: number | null): string;
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
}

const sans = "IBM Plex Sans";
const mono = "Lilex";

function color(name: string): string {
  return token(name as never) as string;
}

export function Shell(props: ShellProps) {
  const { snapshot, controller, reader, toast } = props;
  const overlay = signal<Overlay | null>(null);
  const draft = signal("");
  const attachments = signal<string[]>([]);
  const historyCursor = signal<number | null>(null);
  const expandedThinking = signal<number[]>([]);
  const runTab = signal<"chat" | "trajectory">("chat");
  const railOpen = signal(true);
  const inspectorOpen = signal(true);
  const slashCursor = signal(0);
  const renaming = signal<string | null>(null);

  const send = (command: object) => controller.dispatch(JSON.stringify(command));

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

  const openReader = (kind: string, id: number | null) => {
    send({ kind: "open_reader", reader_kind: kind, id });
  };

  const current = computed(() => snapshot());
  const rows = computed(() => current().rows);
  const firstUser = computed(() => rows().find((row) => row.body.kind === "user"));
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
      style={{
        flex: 1,
        flexDirection: "column",
        backgroundColor: color("background"),
      }}
    >
      <TitleBar
        snapshot={snapshot}
        overlay={overlay}
        railOpen={railOpen}
        inspectorOpen={inspectorOpen}
        send={send}
      />
      <View style={{ flex: 1, flexDirection: "row" }}>
        {railOpen() ? (
          <Rail snapshot={snapshot} controller={controller} overlay={overlay} />
        ) : null}
        <View style={{ flex: 1, flexDirection: "column", minWidth: 0 }}>
          <RunTabs runTab={runTab} />
          {runTab() === "chat" ? (
            <View style={{ flex: 1, flexDirection: "column", minHeight: 0 }}>
              <RunHeader snapshot={snapshot} draft={draft} />
              {activeTask() ? <StickyTask task={activeTask()!} /> : null}
              {rows().length === 0 ? (
                <EmptyState snapshot={snapshot} draft={draft} send={send} />
              ) : (
                <Scroll style={{ flex: 1 }}>
                  <View
                    style={{
                      width: "100%",
                      maxWidth: 900,
                      alignSelf: "center",
                      paddingLeft: 12,
                      paddingRight: 12,
                      paddingTop: 8,
                      paddingBottom: 8,
                      gap: 8,
                      flexDirection: "column",
                    }}
                  >
                    <Each of={rows()}>
                      {(row) => (
                        <RowView
                          row={row()}
                          snapshot={snapshot}
                          controller={controller}
                          draft={draft}
                          expandedThinking={expandedThinking}
                          openReader={openReader}
                          send={send}
                        />
                      )}
                    </Each>
                  </View>
                </Scroll>
              )}
              {pendingGate() ? (
                <GateBanner gate={pendingGate()!} send={send} />
              ) : null}
              <Composer
                snapshot={snapshot}
                draft={draft}
                attachments={attachments}
                historyCursor={historyCursor}
                slashCursor={slashCursor}
                overlay={overlay}
                send={send}
                submit={submit}
              />
            </View>
          ) : (
            <TrajectorySnapshot snapshot={snapshot} />
          )}
        </View>
        {inspectorOpen() ? (
          <Inspector snapshot={snapshot} controller={controller} send={send} />
        ) : null}
      </View>
      {overlay() ? (
        <OverlayPanel
          overlay={overlay()!}
          snapshot={snapshot}
          controller={controller}
          draft={draft}
          renaming={renaming}
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
  send: (command: object) => void;
}) {
  const { snapshot, overlay, railOpen, inspectorOpen, send } = props;
  const menu = signal<string | null>(null);
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
      <Text
        style={{
          flex: 1,
          fontSize: 13,
          color: color("muted-foreground"),
        }}
      >
        {snapshot().project.name}
        {snapshot().project.branch ? ` · ${snapshot().project.branch}` : ""}
      </Text>
      {account() ? <AccountChip account={account()!} /> : null}
      <ChromeButton
        label={railOpen() ? "Hide sessions" : "Show sessions"}
        text="Sessions"
        onPress={() => railOpen.update((open) => !open)}
      />
      <ChromeButton
        label={inspectorOpen() ? "Hide inspector" : "Show inspector"}
        text="Inspector"
        onPress={() => inspectorOpen.update((open) => !open)}
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
      {menu() ? (
        <View
          style={{
            position: "absolute",
            top: 44,
            left: 78,
            width: 220,
            flexDirection: "column",
            gap: 1,
            padding: 4,
            borderRadius: 12,
            backgroundColor: color("popover"),
            borderWidth: 1,
            borderColor: color("line-strong"),
            boxShadow: "0 24px 60px rgba(22, 22, 30, 0.26)",
            zIndex: 20,
          }}
        >
          {(menu() === "File"
            ? [
                ["New Session", () => send({ kind: "new_session" })],
                ["Command Center", () => overlay.set({ kind: "command_center" })],
                ["Settings", () => overlay.set({ kind: "settings" })],
              ]
            : menu() === "View"
              ? [
                  ["Toggle Sessions", () => railOpen.update((open) => !open)],
                  ["Toggle Inspector", () => inspectorOpen.update((open) => !open)],
                  ["Back to Bottom", () => send({ kind: "back_to_bottom" })],
                ]
              : [
                  ["Stop", () => send({ kind: "cancel_run" })],
                ]
          ).map(([label, action]) => (
            <Text
              onPress={() => {
                (action as () => void)();
                menu.set(null);
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
              {label as string}
            </Text>
          ))}
        </View>
      ) : null}
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
      style={{
        width: 250,
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
      <Text
        style={{
          fontSize: 10,
          fontFamily: mono,
          letterSpacing: 1.5,
          color: color("muted-foreground"),
          paddingTop: 8,
        }}
      >
        SESSIONS
      </Text>
      <Scroll style={{ flex: 1 }}>
        <View style={{ flexDirection: "column", gap: 2 }}>
          <Each of={snapshot().sessions.items}>
            {(session) => {
              const selected = computed(
                () => snapshot().sessions.selected === session().id,
              );
              return (
                <View
                  role="button"
                  accessibilityLabel={session().title}
                  onPress={() =>
                    send({ kind: "resume_session", session_id: session().id })
                  }
                  style={{
                    flexDirection: "column",
                    gap: 2,
                    minHeight: 37,
                    paddingLeft: 10,
                    paddingRight: 10,
                    paddingTop: 6,
                    paddingBottom: 6,
                    borderRadius: 7,
                    backgroundColor: selected() ? color("sidebar-accent") : "transparent",
                  }}
                >
                  <Text
                    style={{
                      fontSize: 13,
                      fontWeight: 500,
                      color: color("sidebar-foreground"),
                    }}
                  >
                    {session().title}
                  </Text>
                  <Text style={{ fontSize: 11, color: color("muted-foreground") }}>
                    {relativeTime(session().updated_at)} · {session().message_count} messages
                  </Text>
                </View>
              );
            }}
          </Each>
        </View>
      </Scroll>
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
            <TextView markdown={body.markdown} selectable />
            {body.suggestions.length > 0 ? (
              <View style={{ flexDirection: "row", flexWrap: "wrap", gap: 4 }}>
                {body.suggestions.map((suggestion) => (
                  <Text
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
      {tool.expanded ? (
        <View style={{ flexDirection: "column", gap: 4 }}>
          <ToolBodyView body={tool.body} />
          {tool.output_detail || tool.output_preview ? (
            <Text
              style={{
                fontSize: 11,
                fontFamily: mono,
                backgroundColor: color("background"),
                borderRadius: 6,
                padding: 8,
                color: color("foreground"),
              }}
            >
              {tool.output_detail ?? tool.output_preview}
            </Text>
          ) : null}
        </View>
      ) : null}
    </View>
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
  draft: ReturnType<typeof signal<string>>;
  attachments: ReturnType<typeof signal<string[]>>;
  historyCursor: ReturnType<typeof signal<number | null>>;
  slashCursor: ReturnType<typeof signal<number>>;
  overlay: ReturnType<typeof signal<Overlay | null>>;
  send: (command: object) => void;
  submit: (text: string) => void;
}) {
  const { snapshot, draft, historyCursor, slashCursor, overlay, send, submit } = props;
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
        if (item) draft.set("/" + item.path.join(" "));
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
            {snapshot().composer.queue.slice(0, 4).map((item) => (
              <View
                style={{
                  flexDirection: "row",
                  alignItems: "center",
                  gap: 8,
                  paddingLeft: 8,
                  paddingRight: 8,
                  paddingTop: 3,
                  paddingBottom: 3,
                  borderRadius: 6,
                  backgroundColor: color("muted"),
                }}
              >
                <Text style={{ flex: 1, fontSize: 11, color: color("muted-foreground") }}>
                  {item.text}
                </Text>
                <Text
                  onPress={() => send({ kind: "send_queued_now", index: item.index })}
                  style={{ fontSize: 9, color: color("primary") }}
                >
                  Send now
                </Text>
                <Text
                  onPress={() => send({ kind: "remove_queued", index: item.index })}
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
        <View style={{ flexDirection: "row", alignItems: "center", gap: 8 }}>
          <Text
            onPress={() => overlay.set({ kind: "models" })}
            style={{ fontSize: 11, color: color("muted-foreground") }}
          >
            {snapshot().composer.model || "Model"}
          </Text>
          <Text
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
            style={{ fontSize: 11, color: color("muted-foreground") }}
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
      </View>
    </View>
  );
}

function Inspector(props: {
  snapshot: ReturnType<typeof signal<Snapshot>>;
  controller: Controller;
  send: (command: object) => void;
}) {
  const { snapshot, send } = props;
  const tab = computed(() => snapshot().inspector.tab);
  return (
    <View
      style={{
        width: 318,
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
            <SubagentsInspector agents={snapshot().inspector.subagents} send={send} />
          ) : null}
          {tab() === "background" ? (
            <BackgroundInspector tasks={snapshot().inspector.background} send={send} />
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

function SubagentsInspector(props: { agents: Subagent[]; send: (command: object) => void }) {
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
}) {
  return (
    <View style={{ flexDirection: "column", gap: 8 }}>
      {props.tasks.length === 0 ? (
        <Text style={{ fontSize: 11, color: color("muted-foreground") }}>No background tasks</Text>
      ) : null}
      {props.tasks.map((task) => (
        <View style={{ flexDirection: "column", gap: 2 }}>
          <Text style={{ fontSize: 11, fontFamily: mono }}>{task.command}</Text>
          <Text style={{ fontSize: 10, color: color("muted-foreground") }}>{task.status}</Text>
          <Text
            onPress={() =>
              props.send({ kind: "cancel_background_task", task_id: task.id })
            }
            style={{ fontSize: 9, color: color("muted-foreground") }}
          >
            Stop
          </Text>
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
  renaming: ReturnType<typeof signal<string | null>>;
  send: (command: object) => void;
  close: () => void;
}) {
  const { overlay, snapshot, draft, renaming, send, close } = props;
  return (
    <View
      style={{
        position: "absolute",
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
        style={{
          width: "100%",
          maxWidth: 640,
          maxHeight: 540,
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
          <Text role="button" accessibilityLabel="Close" onPress={close} style={{ fontSize: 11, color: color("muted-foreground") }}>
            Close
          </Text>
        </View>
        <Scroll style={{ flex: 1 }}>
          <View style={{ flexDirection: "column", gap: 8, paddingLeft: 16, paddingRight: 16, paddingTop: 12, paddingBottom: 12 }}>
            {overlay.kind === "command_center" ? (
              <CommandCenter snapshot={snapshot} draft={draft} send={send} close={close} />
            ) : null}
            {overlay.kind === "settings" ? (
              <SettingsPanel snapshot={snapshot} send={send} />
            ) : null}
            {overlay.kind === "sessions" ? (
              <SessionsPanel snapshot={snapshot} renaming={renaming} send={send} close={close} />
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
  const commands = computed(() => {
    const fragment = query().toLowerCase();
    return props
      .snapshot()
      .composer.commands.filter((command) => command.title.toLowerCase().includes(fragment))
      .slice(0, 60);
  });
  return (
    <View style={{ flexDirection: "column", gap: 8 }}>
      <Input
        value={query()}
        autoFocus
        placeholder="Search commands"
        onChange={(text: string) => query.set(text)}
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
        {commands().map((command) => (
          <View
            onPress={() => {
              props.send({ kind: "invoke_command", path: command.path, tokens: [] });
              props.close();
            }}
            style={{ flexDirection: "column", gap: 1, paddingLeft: 12, paddingRight: 12, paddingTop: 6, paddingBottom: 6, borderRadius: 7 }}
          >
            <Text style={{ fontSize: 13, fontFamily: mono }}>{command.title}</Text>
            <Text style={{ fontSize: 11, color: color("muted-foreground") }}>
              {command.description}
            </Text>
          </View>
        ))}
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
    ["permission-mode", "Permission", "choice", current().composer.permission],
    ["reasoning-effort", "Reasoning effort", "choice", current().composer.effort ?? ""],
    ["notifications", "Notifications", "bool", "on"],
    ["hook-output", "Hook output", "bool", current().show_hook_output ? "on" : "off"],
    ["provider", "Provider", "text", current().settings.provider],
    ["base-url", "Base URL", "text", current().settings.base_url],
    ["api-key", "API key", "text", current().settings.api_key_set ? "••••••" : ""],
    ["max-tokens", "Max tokens", "text", current().settings.max_tokens?.toString() ?? ""],
    ["micro-compact", "Micro compact", "bool", current().settings.micro_compact ? "on" : "off"],
  ];
  return (
    <View style={{ flexDirection: "column", gap: 8 }}>
      {rows.map(([id, label, kind, value]) => (
        <View style={{ flexDirection: "row", alignItems: "center", gap: 12 }}>
          <Text style={{ flex: 1, fontSize: 12 }}>{label}</Text>
          {kind === "text" ? (
            <Input
              value={value}
              onSubmit={(event: { text: string }) =>
                props.send({
                  kind: "set_setting",
                  id,
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
          ) : (
            <Text
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

function SessionsPanel(props: {
  snapshot: ReturnType<typeof signal<Snapshot>>;
  renaming: ReturnType<typeof signal<string | null>>;
  send: (command: object) => void;
  close: () => void;
}) {
  return (
    <View style={{ flexDirection: "column", gap: 4 }}>
      {props.snapshot().sessions.items.map((session) => (
        <View
          style={{ flexDirection: "row", alignItems: "center", gap: 8, paddingLeft: 8, paddingRight: 8, paddingTop: 4, paddingBottom: 4, borderRadius: 7 }}
        >
          <Input
            value={props.renaming() === session.id ? session.title : session.title}
            onSubmit={(event: { text: string }) =>
              props.send({ kind: "rename_session", session_id: session.id, title: event.text })
            }
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
            onPress={() =>
              props.send({
                kind: "set_session_archived",
                session_id: session.id,
                archived: !session.archived,
              })
            }
            style={{ fontSize: 10, color: color("muted-foreground") }}
          >
            {session.archived ? "Restore" : "Archive"}
          </Text>
          <Text
            onPress={() => {
              props.send({ kind: "resume_session", session_id: session.id });
              props.close();
            }}
            style={{ fontSize: 10, color: color("primary") }}
          >
            Open
          </Text>
        </View>
      ))}
    </View>
  );
}

function ExtensionsPanel(props: {
  snapshot: ReturnType<typeof signal<Snapshot>>;
  send: (command: object) => void;
}) {
  const tab = signal(props.snapshot().extensions.tab);
  const extensions = computed(() => props.snapshot().extensions);
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
            onPress={() => tab.set(value)}
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
      {tab() === "skills"
        ? extensions().skills.map((skill) => (
            <View style={{ flexDirection: "row", alignItems: "center", gap: 8 }}>
              <View style={{ flex: 1, flexDirection: "column" }}>
                <Text style={{ fontSize: 12 }}>{skill.name}</Text>
                <Text style={{ fontSize: 10, color: color("muted-foreground") }}>
                  {skill.description}
                </Text>
              </View>
              <Text
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
        ? extensions().plugins.map((plugin) => (
            <Text style={{ fontSize: 12 }}>
              {plugin.name} · {plugin.marketplace}
            </Text>
          ))
        : null}
      {tab() === "mcp" ? (
        <Text style={{ fontSize: 11, color: color("muted-foreground") }}>
          Run /mcp list to load the servers.
        </Text>
      ) : null}
      {tab() === "hooks" ? (
        <Text style={{ fontSize: 11, color: color("muted-foreground") }}>
          Run /hooks list to load the hooks.
        </Text>
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
      {props.snapshot().composer.models.map((model) => (
        <Text
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
              props.snapshot().project.name === project.name ? color("muted") : "transparent",
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
        position: "absolute",
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
        style={{
          width: "100%",
          maxWidth: 760,
          maxHeight: 600,
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
          <Text role="button" accessibilityLabel="Close" onPress={props.close} style={{ fontSize: 11, color: color("muted-foreground") }}>
            Close
          </Text>
        </View>
        <Scroll style={{ flex: 1 }}>
          <View style={{ paddingLeft: 20, paddingRight: 20, paddingTop: 16, paddingBottom: 16 }}>
            {isAnsi ? (
              <Text style={{ fontSize: 11, fontFamily: mono }}>
                {props.reader.markdown}
              </Text>
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
        position: "absolute",
        left: 0,
        right: 0,
        bottom: 24,
        alignItems: "center",
      }}
    >
      <View
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

function TrajectorySnapshot(props: { snapshot: ReturnType<typeof signal<Snapshot>> }) {
  return (
    <Scroll style={{ flex: 1 }}>
      <View style={{ maxWidth: 900, alignSelf: "center", width: "100%", paddingLeft: 24, paddingRight: 24, paddingTop: 24, flexDirection: "column", gap: 4 }}>
        <Text style={{ fontSize: 12, color: color("muted-foreground") }}>
          {props.snapshot().rows.length} records · {props.snapshot().runtime.turns} turns
        </Text>
        {props.snapshot().rows.map((row) => (
          <View style={{ flexDirection: "row", gap: 8, paddingTop: 2, paddingBottom: 2 }}>
            <Text style={{ width: 34, fontSize: 10, fontFamily: mono, color: color("muted-foreground") }}>
              #{row.ordinal}
            </Text>
            <Text style={{ flex: 1, fontSize: 11 }}>{row.body.kind}</Text>
          </View>
        ))}
      </View>
    </Scroll>
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
