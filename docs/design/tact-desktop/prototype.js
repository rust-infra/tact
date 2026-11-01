const $ = (selector, root = document) => root.querySelector(selector);
const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];

const body = document.body;
const palette = $("#palette-modal");
const paletteSearch = $("#command-search");
const commandList = $("#command-list");
const drawer = $("#agent-drawer");
const drawerScrim = $("#drawer-scrim");
const toast = $("#toast");
const composer = $("#composer");
const promptInput = $("#prompt");
const composerHint = $("#composer-hint");
const stopButton = $("#stop-run");
const workbench = $("#workbench");
const railToggle = $("#rail-toggle");
const inspectorToggle = $("#inspector-toggle");
const panelScrim = $("#panel-scrim");
const sessionMenu = $("#session-menu");
const archivedSection = $("#archived-section");
const archivedList = $("#archived-list");
const archivedCount = $("#archived-count");
const slashPopup = $("#slash-popup");
const slashList = $("#slash-list");
const slashPopupTitle = $("#slash-popup-title");
const slashPopupFoot = $("#slash-popup-foot");
const mentionPopup = $("#mention-popup");
const mentionList = $("#mention-list");
const modelMenu = $("#model-menu");
const permissionMenu = $("#permission-menu");
const modelSelect = $("#model-select");
const permissionSelect = $("#permission-select");
const sessionManager = $("#session-manager");
const sessionManagerList = $("#session-manager-list");
const sessionManagerSearch = $("#session-manager-search");
const sessionManagerProject = $("#session-manager-project");
const sessionManagerCount = $("#session-manager-count");
const stickyBackgroundToggle = $("#sticky-background-toggle");
const stickyBackgroundPopover = $("#sticky-background-popover");
const extensionsPopover = $("#extensions-popover");
const extensionsBody = $("#extensions-popover-body");
const extensionsSearch = $("#extensions-popover-search");
const extensionsTitle = $("#extensions-popover-title");
const extensionsCount = $("#extensions-popover-count");
const extensionsManager = $("#extensions-manager");
const extensionsManagerList = $("#extensions-manager-list");
const extensionsManagerSearch = $("#extensions-manager-search");
const extensionsManagerCount = $("#extensions-manager-count");
const commandSheet = $("#command-sheet");
const commandSheetBody = $("#command-sheet-body");
const commandSheetTitle = $("#command-sheet-title");
const commandSheetEyebrow = $("#command-sheet-eyebrow");
const commandSheetDescription = $("#command-sheet-description");
const commandSheetStatus = $("#command-sheet-status");
const commandSheetActions = $("#command-sheet-actions");
const attachmentStrip = $("#attachment-strip");
const queueItems = $("#queue-items");
const queueStrip = $("#queue-strip");
const queueSummary = $("#queue-summary");
const queueCount = $("#queue-count");
const voiceButton = $("#voice-button");
const selectionActions = $("#selection-actions");
const stickyPermissionBanner = $("#sticky-permission-banner");
const threadScroll = $("#thread-scroll");
const backToBottom = $("#back-to-bottom");
const stickyHost = $("#sticky-host");

const RUNS = {
  desktop: {
    label: "Run 8F2A",
    session: "session 8f2a",
    title:
      "分析下这个库能否满足当前tact所有的功能，如果要接入desktop的话",
    lede:
      "以第2个目标来设计，可以先出prd原型图，用html画原型，采用适合的pstack skill来处理",
  },
  compaction: {
    label: "Run 41C9",
    session: "session 41c9",
    title: "Make tool output<br /><em>survive compaction</em>.",
    lede:
      "Keep the evidence a reviewer needs while old terminal output leaves the model context.",
  },
  kernel: {
    label: "Run 6BD2",
    session: "session 6bd2",
    title: "Finish the agent kernel<br /><em>parity pass</em>.",
    lede:
      "Close the remaining host boundaries without changing the wire protocol or session state.",
  },
};

const PROJECTS = [
  {
    id: "rust-infra/tact",
    initials: "RT",
    label: "rust-infra/tact",
    sessions: [
      ["8f2a", "分析 gpui-ai 能否满足 Tact", "feat/desktop", "2m ago", 46, "active", "desktop"],
      ["41c9", "Tool output compaction", "feat/desktop", "Yesterday", 112, "active", "compaction"],
      ["6bd2", "Agent kernel parity", "codex/agent-kernel-design", "Oct 28", 203, "active", "kernel"],
      ["9c11", "Markdown rendering pass", "feat/desktop", "Oct 28", 78, "active", null],
      ["2d7e", "Session archive flow", "feat/desktop", "Oct 27", 39, "active", null],
      ["74af", "Permission gate polish", "feat/desktop", "Oct 27", 64, "active", null],
      ["11bc", "Command center coverage", "feat/desktop", "Oct 26", 151, "active", null],
      ["8a0d", "Sticky task panel", "feat/desktop", "Oct 26", 52, "active", null],
      ["6f31", "Project switcher", "feat/desktop", "Oct 25", 33, "active", null],
      ["4e92", "Session manager scale", "feat/desktop", "Oct 24", 91, "active", null],
      ["1b66", "Window chrome cleanup", "feat/desktop", "Oct 22", 27, "archived", null],
      ["0c40", "Release packaging", "feat/desktop", "Oct 20", 188, "archived", null],
    ],
  },
  {
    id: "acme/gateway",
    initials: "AG",
    label: "acme/gateway",
    sessions: [
      ["a201", "Rate limit recovery", "feat/limits", "1h ago", 67, "active", null],
      ["a202", "Auth token refresh", "feat/auth", "Yesterday", 88, "active", null],
      ["a203", "Gateway tracing", "feat/tracing", "Oct 27", 123, "active", null],
      ["a204", "Provider failover", "feat/failover", "Oct 25", 44, "active", null],
      ["a205", "Config validation", "feat/config", "Oct 23", 71, "active", null],
      ["a206", "Old dashboard", "feat/dashboard", "Oct 18", 160, "archived", null],
      ["a207", "Legacy migration", "feat/migration", "Oct 12", 240, "archived", null],
    ],
  },
  {
    id: "rg/dotfiles",
    initials: "RD",
    label: "rg/dotfiles",
    sessions: [
      ["d101", "Zsh startup audit", "main", "3d ago", 25, "active", null],
      ["d102", "Neovim config cleanup", "main", "Oct 19", 38, "active", null],
      ["d103", "Old shell aliases", "main", "Oct 8", 12, "archived", null],
    ],
  },
];

const EXTENSION_DATA = {
  mcp: {
    title: "MCP servers",
    count: "4 live",
    items: [
      ["plug", "DeepWiki", "connected · streamable HTTP", "live"],
      ["plug", "Linear", "OAuth authorization required", "Authorize", true],
      ["terminal", "filesystem", "connected · local stdio", "live"],
      ["grid", "Figma", "connected · remote MCP", "live"],
    ],
  },
  skills: {
    title: "Skills",
    count: "12 enabled",
    items: [
      ["grid", "brainstorming", "requirements and design exploration", "enabled"],
      ["grid", "gpui-ai", "AI component usage and verification", "enabled"],
      ["grid", "code-review", "findings-first review workflow", "enabled"],
      ["grid", "test-driven-development", "red-green-refactor loop", "enabled"],
    ],
  },
  plugins: {
    title: "Plugins",
    count: "6 installed",
    items: [
      ["puzzle", "superpowers", "official workflow plugin · up to date", "update"],
      ["puzzle", "frontend-design", "design and prototype guidance", "enabled"],
      ["puzzle", "claude-plugins-official", "built-in marketplace", "enabled"],
      ["puzzle", "tact-lab", "local plugin · 3 skills", "reload"],
    ],
  },
};

const ENTRY_GROUPS = [
  {
    label: "Session",
    items: [
      ["new-session", "plus", "New session", "Start a fresh run in this workspace", "⌘N"],
      ["resume-last", "clock", "Resume last session", "Reopen the most recent run", "CLI"],
      ["session-list", "search", "Session list", "Search, resume and reveal session actions", "—"],
      ["session-archive", "archive", "Archive current session", "Open the current session's actions menu", "—"],
      ["save-transcript", "file", "Save transcript", "Write the visible run log to a text file", "/save"],
      ["quit", "x", "Quit Tact", "Close the current window and release the session lock", "/quit"],
    ],
  },
  {
    label: "Run",
    items: [
      ["cancel-run", "stop", "Cancel run", "Stop the active agent task", "/cancel"],
      ["compact", "archive", "Compact context", "Summarize and rebuild the current context", "/compact"],
      ["stats", "cpu", "Session stats", "Tokens, cache, turns, timing and request details", "/stats"],
      ["background", "terminal", "Background tasks", "Inspect running and completed background commands", "/background"],
      ["history", "clock", "Task history", "Browse this session's submitted tasks", "/history"],
      ["help", "command", "Keyboard help", "Show shortcuts and command reference", "/help"],
      ["system-prompt", "file", "View system prompt", "Raw template or assembled current prompt", "/view-system-prompt"],
      ["tasks-dag", "branch", "Task dependency graph", "Open the task DAG view", "/tasks-dag"],
      ["headless", "terminal", "Run headless task", "Execute one task without the interactive shell", "CLI"],
      ["toggle-sidebar", "panel-left", "Toggle session sidebar", "Show or hide the left panel", "⌘B"],
      ["toggle-inspector", "panel-right", "Toggle inspector", "Show or hide the right panel", "⌘⌥B"],
    ],
  },
  {
    label: "Agent",
    items: [
      ["model", "cpu", "Main model", "Choose model, reasoning effort and thinking budget", "/model"],
      ["model-subagent", "users", "Subagent model", "Choose the model inherited by child agents", "/model-subagent"],
      ["permission", "target", "Permission mode", "Switch default, plan or auto approval", "/permission"],
      ["provider", "plug", "Provider and credentials", "Provider, API key, base URL and max tokens", "config"],
      ["context-window", "cpu", "Context window", "Model context size, compaction thresholds and reservations", "config"],
      ["snapshot-items", "file", "Project snapshot size", "Maximum entries in the system-prompt project snapshot", "config"],
      ["micro-compact", "archive", "Micro-compaction", "Stub old tool results before each model call", "config"],
      ["memory", "file", "Memory", "Enable or disable memory injection and tools", "config"],
      ["config-file", "settings", "Configuration file", "Open the active config and resolved values", "CLI"],
    ],
  },
  {
    label: "Goals",
    items: [
      ["goal-open", "target", "Open Goals", "Inspect active and archived durable Goals", "—"],
      ["goal-create", "plus", "Create Goal", "Define objective, title and acceptance criteria", "/goal create"],
      ["goal-list", "target", "List Goals", "Show the working set or every archived Goal", "/goal list"],
      ["goal-show", "search", "Show Goal", "Open one Goal by ID", "/goal show"],
      ["goal-steer", "command", "Steer Goal", "Add user steering to the Goal event stream", "/goal steer"],
      ["goal-pause", "pause", "Pause Goal", "Pause after the active activation releases", "/goal pause"],
      ["goal-resume", "play", "Resume Goal", "Start one bounded activation", "/goal resume"],
      ["goal-cancel", "x", "Cancel Goal", "Cancel a non-terminal Goal", "/goal cancel"],
      ["goal-accept", "check", "Accept Goal", "Record human acceptance and resume", "/goal accept"],
      ["goal-archive", "archive", "Archive Goal", "File a finished Goal out of the working panel", "/goal archive"],
      ["goal-unarchive", "arrow-up", "Unarchive Goal", "Return an archived Goal to the working set", "/goal unarchive"],
    ],
  },
  {
    label: "Subagents and team",
    items: [
      ["agents-open", "users", "Open Subagents", "Inspect child status, context and transcripts", "—"],
      ["agent-transcript", "users", "Open subagent transcript", "Read the selected child session", "—"],
      ["agent-cancel", "x", "Cancel subagent", "Stop a running background child", "/subagent_cancel"],
      ["tasks", "grid", "Persistent tasks", "Create, inspect and update task records", "tools"],
      ["team", "users", "Team inbox", "Teammates, messages, broadcasts and shutdown requests", "tools"],
      ["worktrees", "branch", "Worktrees", "Create, inspect and run commands in isolated worktrees", "tools"],
    ],
  },
  {
    label: "Skills and plugins",
    items: [
      ["skill-list", "grid", "Skills", "List, enable or disable skills in the catalog", "/skill list"],
      ["skill-reload", "arrow-up", "Reload skills", "Refresh discovered skills", "/skill reload"],
      ["skill-enable", "check", "Enable skill", "Show a skill in the system-prompt catalog", "/skill enable"],
      ["skill-disable", "x", "Disable skill", "Hide a skill without deleting its files", "/skill disable"],
      ["plugin-list", "puzzle", "Plugins", "List installed plugins", "/plugin list"],
      ["plugin-install", "plus", "Install plugin", "Install from a registered marketplace", "/plugin install"],
      ["plugin-uninstall", "x", "Uninstall plugin", "Remove an installed plugin", "/plugin uninstall"],
      ["plugin-update", "arrow-up", "Update plugin", "Pull the latest plugin revision", "/plugin update"],
      ["plugin-reload", "arrow-up", "Reload plugins", "Refresh plugin skills, commands and hooks", "/plugin reload"],
      ["marketplace-list", "puzzle", "Marketplaces", "List registered marketplace sources", "/plugin marketplace list"],
      ["marketplace-add", "plus", "Add marketplace", "Add a Git URL or GitHub owner/repo", "/plugin marketplace add"],
      ["marketplace-update", "arrow-up", "Update marketplace", "Re-fetch a marketplace catalog", "/plugin marketplace update"],
      ["marketplace-remove", "x", "Remove marketplace", "Remove a non-built-in marketplace", "/plugin marketplace remove"],
    ],
  },
  {
    label: "MCP and hooks",
    items: [
      ["mcp-list", "plug", "MCP servers", "Live server status, transport and source", "/mcp list"],
      ["mcp-get", "search", "Inspect MCP server", "Show one server, its status and tools", "CLI"],
      ["mcp-add", "plus", "Add MCP server", "Add a remote HTTP, local stdio or OAuth server", "CLI"],
      ["mcp-remove", "x", "Remove MCP server", "Remove the declaration from project or user config", "CLI"],
      ["mcp-login", "plug", "Authorize MCP server", "Run OAuth and store the token", "/mcp login"],
      ["mcp-logout", "x", "Delete MCP credentials", "Remove stored OAuth credentials", "CLI"],
      ["mcp-prompts", "command", "MCP prompts", "List prompts published by connected servers", "/mcp prompts"],
      ["mcp-prompt", "play", "Run MCP prompt", "Fetch a prompt and submit it as a task", "/mcp prompt"],
      ["hooks-list", "command", "Hooks", "Review configured command hooks", "/hooks list"],
      ["hooks-trust", "check", "Trust hooks", "Approve all hooks or one reviewed source", "/hooks trust"],
      ["hooks-forget", "x", "Forget hook approvals", "Revoke every hook approval", "/hooks forget"],
      ["hook-output", "terminal", "Hook output", "Show or hide hook progress and injected context", "/hook-output"],
    ],
  },
  {
    label: "Composer and account",
    items: [
      ["attach-file", "paperclip", "Attach file", "Add a text, image or workspace file", "@"],
      ["mention-file", "at", "Mention file", "Insert an @path workspace reference", "@"],
      ["voice-input", "command", "Voice input", "Start or stop microphone transcription", "voice"],
      ["queue-message", "send", "Queue message", "Hold a follow-up until the active run settles", "—"],
      ["balance", "cpu", "Account balance", "Query provider balance and quota", "/balance"],
      ["appearance", "sun", "Appearance", "Switch Light or Dark", "theme"],
      ["language", "command", "Language", "Switch the desktop interface language", "/lang"],
      ["notifications", "bell", "Notifications", "Enable or disable desktop notifications", "config"],
    ],
  },
  {
    label: "System",
    items: [
      ["upgrade-check", "arrow-up", "Check for updates", "Compare against the latest GitHub release", "CLI"],
      ["upgrade-install", "arrow-up", "Install update", "Download, verify and replace the binary", "CLI"],
      ["skill-auto-inject", "grid", "Skill body injection", "Choose summary-only or full-body prompt injection", "config"],
      ["tokio-console", "terminal", "Tokio diagnostics", "Open the runtime diagnostics entry", "config"],
      ["settings", "settings", "Settings", "Open all desktop and agent settings", "⌘,"],
    ],
  },
];

const RECENT_COMMAND_IDS = new Set([
  "model",
  "permission",
  "compact",
  "background",
  "history",
  "session-list",
  "help",
  "hooks-list",
]);

const SLASH_COMMANDS = [
  ["theme", "sun", "Appearance", "Choose the active interface theme", "appearance"],
  ["model", "cpu", "Agent", "Choose main model, effort and thinking budget", "model"],
  ["model-subagent", "users", "Agent", "Choose the model used by child agents", "model-subagent"],
  ["permission", "target", "Agent", "Choose default, plan or auto permission mode", "permission"],
  ["view-system-prompt", "file", "Inspect", "Read the raw or assembled system prompt", "system-prompt"],
  ["save", "file", "Session", "Save the visible transcript to a text file", "save-transcript"],
  ["compact", "archive", "Run", "Summarize and rebuild conversation context", "compact"],
  ["cancel", "stop", "Run", "Cancel the active agent task", "cancel-run"],
  ["subagent_cancel", "x", "Subagents", "Cancel a running background subagent", "agent-cancel"],
  ["quit", "x", "System", "Close the desktop window", "quit"],
  ["help", "command", "System", "Show keyboard and command help", "help"],
  ["history", "clock", "Run", "Browse submitted tasks in this session", "history"],
  ["skill", "grid", "Extensions", "List, reload, enable or disable skills", "skill-list"],
  ["plugin", "puzzle", "Extensions", "Manage plugins and marketplaces", "plugin-list"],
  ["mcp", "plug", "Extensions", "Inspect servers, authorize and run prompts", "mcp-list"],
  ["hooks", "command", "Extensions", "Review, trust or forget command hooks", "hooks-list"],
  ["hook-output", "terminal", "Extensions", "Show or hide hook output", "hook-output"],
  ["balance", "cpu", "Account", "Query provider balance and quota", "balance"],
  ["lang", "command", "Appearance", "Switch interface language", "language"],
  ["stats", "cpu", "Inspect", "Show session statistics", "stats"],
  ["tasks-dag", "branch", "Inspect", "Open the task dependency graph", "tasks-dag"],
  ["background", "terminal", "Run", "Inspect background tasks", "background"],
  ["goal", "target", "Goals", "Create, steer and manage durable Goals", "goal-open"],
];

const PLUGIN_OPTIONS = [
  { name: "superpowers", detail: "official workflow plugin" },
  { name: "frontend-design", detail: "design and prototype guidance" },
  { name: "claude-plugins-official", detail: "built-in marketplace" },
  { name: "tact-lab", detail: "local plugin · 3 skills" },
];

const MCP_SERVER_OPTIONS = [
  { name: "DeepWiki", detail: "connected · streamable HTTP" },
  { name: "Linear", detail: "OAuth authorization required" },
  { name: "filesystem", detail: "connected · local stdio" },
  { name: "Figma", detail: "connected · remote MCP" },
];

const GOAL_OPTIONS = [
  { id: "216ae6be", title: "Desktop parity", status: "running", archived: false },
  { id: "f6a1c240", title: "Desktop packaging", status: "queued", archived: false },
  { id: "90cd2b11", title: "Release candidate", status: "accepted", archived: true },
];

const SUBAGENT_OPTIONS = [
  { name: "frontend-mapper", detail: "mapping gpui-ai gaps · 64%" },
  { name: "a11y-audit", detail: "checking focus paths · 31%" },
  { name: "protocol-reader", detail: "AgentUpdate inventory · done" },
];

const HOOK_SOURCES = [
  {
    id: "project",
    label: ".tact/hooks.json",
    detail: "Project hooks · cargo fmt, cargo check",
    trusted: true,
  },
  {
    id: "user",
    label: "~/.tact/hooks.json",
    detail: "User hooks · session notification",
    trusted: false,
  },
  {
    id: "plugin-ponytail",
    label: "plugin ponytail",
    detail: "Plugin hooks · post-tool formatter",
    trusted: false,
  },
];

const MCP_PROMPTS = [
  { name: "review_pr", detail: "DeepWiki · review a pull request", server: "DeepWiki" },
  { name: "summarize_thread", detail: "Linear · summarize an issue thread", server: "Linear" },
  { name: "design_context", detail: "Figma · load the current frame context", server: "Figma" },
];

const slashValue = (name, detail, extra = {}) => ({
  name,
  detail,
  kind: "value",
  ...extra,
});

const SLASH_SUBCOMMANDS = {
  skill: [
    { name: "list", detail: "List skills in the system-prompt catalog", icon: "grid" },
    { name: "reload", detail: "Refresh discovered skills", icon: "arrow-up" },
    {
      name: "disable",
      detail: "Hide a skill from the catalog",
      hint: "<name>",
      icon: "x",
      values: () =>
        EXTENSION_DATA.skills.items
          .filter(([, , , status]) => status === "enabled")
          .map(([, title, detail]) => slashValue(title, detail)),
    },
    {
      name: "enable",
      detail: "Show a skill in the catalog",
      hint: "<name>",
      icon: "check",
      values: () =>
        EXTENSION_DATA.skills.items
          .filter(([, , , status]) => status !== "enabled")
          .map(([, title, detail]) => slashValue(title, detail)),
    },
  ],
  plugin: [
    { name: "list", detail: "List installed plugins", icon: "puzzle" },
    { name: "reload", detail: "Reload plugin skills, commands and hooks", icon: "arrow-up" },
    {
      name: "uninstall",
      detail: "Uninstall an installed plugin",
      hint: "<name>",
      icon: "x",
      values: () => PLUGIN_OPTIONS.map((plugin) => slashValue(plugin.name, plugin.detail)),
    },
    {
      name: "update",
      detail: "Update an installed plugin",
      hint: "<name>",
      icon: "arrow-up",
      values: () => PLUGIN_OPTIONS.map((plugin) => slashValue(plugin.name, plugin.detail)),
    },
    {
      name: "marketplace",
      detail: "Manage registered marketplaces",
      icon: "puzzle",
      children: [
        { name: "list", detail: "List registered marketplaces", icon: "puzzle" },
      ],
    },
  ],
  mcp: [
    {
      name: "auth",
      detail: "Authorize a remote MCP server",
      hint: "<server>",
      icon: "plug",
      values: () => MCP_SERVER_OPTIONS.map((server) => slashValue(server.name, server.detail)),
    },
    {
      name: "login",
      detail: "Authorize a remote MCP server",
      hint: "<server>",
      icon: "plug",
      values: () => MCP_SERVER_OPTIONS.map((server) => slashValue(server.name, server.detail)),
    },
    { name: "list", detail: "List configured servers and status", icon: "plug" },
    { name: "prompts", detail: "List prompts from connected servers", icon: "command" },
    {
      name: "prompt",
      detail: "Run a prompt from an MCP server",
      hint: "<server> <name> [key=value ...]",
      icon: "play",
      takesValue: true,
      form: "mcp-prompt",
    },
  ],
  hooks: [
    { name: "list", detail: "Review configured command hooks", icon: "command" },
    {
      name: "trust",
      detail: "Approve all hooks or one source",
      icon: "check",
      children: [
        { name: "--all", detail: "Approve every reviewed hook source", icon: "check" },
        {
          name: "--source",
          detail: "Approve one hook source",
          hint: "<label>",
          icon: "command",
          values: () =>
            HOOK_SOURCES.map((source) =>
              slashValue(source.id, `${source.label} · ${source.detail}`),
            ),
        },
      ],
    },
    {
      name: "forget",
      detail: "Revoke every hook approval",
      icon: "x",
      children: [
        { name: "--all", detail: "Forget every hook approval", icon: "x" },
      ],
    },
  ],
  goal: [
    {
      name: "create",
      detail: "Create a durable Goal",
      hint: "<objective>",
      icon: "plus",
      takesValue: true,
      form: "goal-create",
    },
    { name: "list", detail: "List active or archived Goals", hint: "[--all]", icon: "target" },
    {
      name: "show",
      detail: "Show one Goal by ID",
      hint: "<goal-id>",
      icon: "search",
      values: () => GOAL_OPTIONS.map((goal) => slashValue(goal.id, goal.title)),
    },
    {
      name: "steer",
      detail: "Add steering to a Goal",
      hint: "<goal-id> <text>",
      icon: "command",
      values: () =>
        GOAL_OPTIONS.filter((goal) => !goal.archived).map((goal) =>
          slashValue(goal.id, goal.title, { thenText: "steering text" }),
        ),
    },
    {
      name: "pause",
      detail: "Pause a Goal",
      hint: "<goal-id>",
      icon: "pause",
      values: () =>
        GOAL_OPTIONS.filter((goal) => goal.status === "running").map((goal) =>
          slashValue(goal.id, goal.title),
        ),
    },
    {
      name: "resume",
      detail: "Resume a Goal",
      hint: "<goal-id>",
      icon: "play",
      values: () =>
        GOAL_OPTIONS.filter((goal) => ["paused", "queued"].includes(goal.status)).map((goal) =>
          slashValue(goal.id, goal.title),
        ),
    },
    {
      name: "cancel",
      detail: "Cancel a Goal",
      hint: "<goal-id>",
      icon: "x",
      values: () =>
        GOAL_OPTIONS.filter((goal) => !goal.archived).map((goal) =>
          slashValue(goal.id, goal.title),
        ),
    },
    {
      name: "accept",
      detail: "Accept Goal completion",
      hint: "<goal-id> <text>",
      icon: "check",
      values: () =>
        GOAL_OPTIONS.filter((goal) => goal.status === "running").map((goal) =>
          slashValue(goal.id, goal.title, { thenText: "acceptance note" }),
        ),
    },
    {
      name: "archive",
      detail: "Archive a Goal",
      hint: "<goal-id>",
      icon: "archive",
      values: () =>
        GOAL_OPTIONS.filter((goal) => !goal.archived).map((goal) =>
          slashValue(goal.id, goal.title),
        ),
    },
    {
      name: "unarchive",
      detail: "Restore an archived Goal",
      hint: "<goal-id>",
      icon: "arrow-up",
      values: () =>
        GOAL_OPTIONS.filter((goal) => goal.archived).map((goal) =>
          slashValue(goal.id, goal.title),
        ),
    },
  ],
};

const MENTIONS = [
  ["Cargo.toml", "Workspace manifest", "file"],
  ["crates/tact_host/README.md", "Host integration guide", "file"],
  ["crates/protocol/src/agent.rs", "AgentUpdate and UserCommand", "file"],
  ["crates/tui/src/widgets/state/slash.rs", "Built-in slash command enum", "file"],
  ["docs/superpowers/specs/2026-10-30-tact-desktop-prd.md", "Desktop PRD", "file"],
  ["docs/design/tact-desktop/prototype.html", "Current prototype", "file"],
  ["docs/design/tact-desktop/previews/light.png", "Light appearance", "image"],
  ["docs/design/tact-desktop/previews/dark.png", "Dark appearance", "image"],
];

let toastTimer;
let runPaused = false;
let menuSession = null;
let slashMode = "root";
let slashRoot = null;
let managerProjectId = "rust-infra/tact";
let managerFilter = "active";
let managerQuery = "";
let extensionTab = "mcp";
let extensionPopoverQuery = "";
let extensionManagerTab = "mcp";
let extensionManagerQuery = "";
let attachments = [];
let voiceActive = false;

const prefersReducedMotion = () =>
  window.matchMedia("(prefers-reduced-motion: reduce)").matches;

const delay = (milliseconds) =>
  new Promise((resolve) => window.setTimeout(resolve, milliseconds));

async function animateElement(element, keyframes, options) {
  if (!element || prefersReducedMotion()) return;
  const animation = element.animate(keyframes, {
    easing: "cubic-bezier(0.22, 1, 0.36, 1)",
    fill: "both",
    ...options,
  });
  try {
    await animation.finished;
  } finally {
    animation.cancel();
  }
}

function normalizeSearchText(value) {
  return String(value ?? "")
    .normalize("NFKD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, " ")
    .trim();
}

function fuzzyTokenScore(token, text) {
  const direct = text.indexOf(token);
  if (direct !== -1) {
    const boundary = direct === 0 || text[direct - 1] === " ";
    return 1000 + (boundary ? 180 : 0) - direct - text.length * 0.02;
  }

  let score = 0;
  let cursor = 0;
  let previous = -2;
  let streak = 0;
  for (const character of token) {
    const index = text.indexOf(character, cursor);
    if (index === -1) return Number.NEGATIVE_INFINITY;
    const boundary = index === 0 || text[index - 1] === " ";
    score += 24 - Math.min(index - cursor, 16);
    if (boundary) score += 20;
    if (index === previous + 1) {
      streak += 1;
      score += streak * 4;
    } else {
      streak = 0;
    }
    previous = index;
    cursor = index + 1;
  }
  return score - Math.min(text.length, 160) * 0.03;
}

function fuzzyScore(query, text) {
  const needle = normalizeSearchText(query);
  if (!needle) return 0;
  const haystack = normalizeSearchText(text);
  if (!haystack) return Number.NEGATIVE_INFINITY;

  if (haystack === needle) return 5000;
  const direct = haystack.indexOf(needle);
  if (direct !== -1) {
    return 4000 + (direct === 0 ? 240 : 0) - direct;
  }

  let score = 0;
  for (const token of needle.split(" ")) {
    const tokenScore = fuzzyTokenScore(token, haystack);
    if (!Number.isFinite(tokenScore)) return Number.NEGATIVE_INFINITY;
    score += tokenScore;
  }
  return score;
}

function fuzzyScoreFields(query, fields) {
  const needle = normalizeSearchText(query);
  if (!needle) return 0;
  const normalizedFields = fields.map(normalizeSearchText).filter(Boolean);
  if (!normalizedFields.length) return Number.NEGATIVE_INFINITY;

  let best = Number.NEGATIVE_INFINITY;
  for (const field of normalizedFields) {
    if (field === needle) return 5000;
    const direct = field.indexOf(needle);
    if (direct !== -1) {
      best = Math.max(best, 4000 + (direct === 0 ? 240 : 0) - direct);
    }
  }
  if (best > 0) return best;

  let score = 0;
  for (const token of needle.split(" ")) {
    let tokenBest = Number.NEGATIVE_INFINITY;
    for (const field of normalizedFields) {
      tokenBest = Math.max(tokenBest, fuzzyTokenScore(token, field));
    }
    if (!Number.isFinite(tokenBest)) return Number.NEGATIVE_INFINITY;
    score += tokenBest;
  }
  return score;
}

function fuzzyRankFields(query, entries, fieldsFor) {
  const ranked = entries
    .map((entry, index) => ({
      entry,
      index,
      score: fuzzyScoreFields(query, fieldsFor(entry)),
    }))
    .filter(({ score }) => Number.isFinite(score))
    .sort((left, right) => right.score - left.score || left.index - right.index);
  if (!ranked.length) return [];

  const threshold = ranked[0].score > 0 ? ranked[0].score * 0.72 : 0;
  return ranked
    .filter(({ score }) => score >= threshold)
    .map(({ entry }) => entry);
}

function fuzzyRank(query, entries, textFor) {
  return fuzzyRankFields(query, entries, (entry) => [textFor(entry)]);
}

function showToast(message) {
  window.clearTimeout(toastTimer);
  toast.textContent = message;
  toast.hidden = false;
  toastTimer = window.setTimeout(() => {
    toast.hidden = true;
  }, 2600);
}

function setTheme(theme) {
  const dark = theme === "dark";
  body.dataset.theme = dark ? "dark" : "light";
  $("#theme-label").textContent = dark ? "Dark" : "Light";
  $("#theme-icon").setAttribute("href", dark ? "#i-sun" : "#i-moon");
  try {
    localStorage.setItem("tact-desktop-appearance", dark ? "dark" : "light");
  } catch (_) {
    // File URLs can disable storage; the prototype still works without it.
  }
}

function toggleTheme() {
  const next = body.dataset.theme === "dark" ? "light" : "dark";
  if (document.startViewTransition && !prefersReducedMotion()) {
    document.startViewTransition(() => setTheme(next));
    return;
  }
  setTheme(next);
}

function commandMetaHtml(meta) {
  if (meta === "—" || !meta) return "";
  if (meta.startsWith("⌘")) return `<kbd>${meta}</kbd>`;
  return `<span class="command-meta">${meta}</span>`;
}

function renderCommandCatalog() {
  let count = 0;
  let first = true;
  commandList.innerHTML = ENTRY_GROUPS.flatMap((group) =>
    group.items.map(([id, icon, label, detail, meta]) => {
      count += 1;
      const selected = first ? " is-selected" : "";
      first = false;
      return `<button class="command-item${selected}" type="button" data-command="${id}" data-label="${label}" data-recent="${RECENT_COMMAND_IDS.has(id)}">
        <span class="command-icon"><svg class="icon" aria-hidden="true"><use href="#i-${icon}"></use></svg></span>
        <span><strong>${label}</strong><small><span class="command-group-inline">${group.label}</span> · ${detail}</small></span>
        ${commandMetaHtml(meta)}
      </button>`;
    }),
  ).join("");
  $("#palette-title").textContent = `Recent commands · ${Math.min(RECENT_COMMAND_IDS.size, count)} of ${count}`;
}

let slashItems = [];
let mentionItems = [];
let suppressedSlashValue = "";

function closeSlashPopup() {
  slashPopup.hidden = true;
  slashItems = [];
}

function closeMentionPopup() {
  mentionPopup.hidden = true;
  mentionItems = [];
}

function closeChoiceMenus() {
  modelMenu.hidden = true;
  permissionMenu.hidden = true;
}

function closeComposerPopovers() {
  closeSlashPopup();
  closeMentionPopup();
  closeChoiceMenus();
}

function setComposerSelection(container, direction = 0) {
  const entries = $$(".composer-entry:not([hidden])", container);
  if (!entries.length) return;
  const current = entries.findIndex((entry) => entry.classList.contains("is-selected"));
  const next =
    direction === 0
      ? Math.max(0, current)
      : (Math.max(0, current) + direction + entries.length) % entries.length;
  entries.forEach((entry) => entry.classList.remove("is-selected"));
  entries[next].classList.add("is-selected");
  entries[next].scrollIntoView({ block: "nearest" });
}

function renderSlashItems(items, title, footer) {
  slashItems = items;
  slashPopupTitle.textContent = title;
  slashPopupFoot.textContent = footer;
  if (!items.length) {
    closeSlashPopup();
    return;
  }

  let previousGroup = null;
  slashList.innerHTML = items
    .map((item, index) => {
      const heading =
        item.group && item.group !== previousGroup
          ? `<div class="composer-entry-group">${item.group}</div>`
          : "";
      previousGroup = item.group || previousGroup;
      const selected = index === 0 ? " is-selected" : "";
      const meta = item.meta ? `<span class="composer-entry-meta">${item.meta}</span>` : "";
      return `${heading}<button class="composer-entry${selected}" type="button" role="option" data-slash-index="${index}">
        <span class="composer-entry-icon"><svg class="icon small" aria-hidden="true"><use href="#i-${item.icon}"></use></svg></span>
        <span><strong>${item.display || `/${item.name}`}</strong><small>${item.detail}</small></span>
        ${meta}
      </button>`;
    })
    .join("");
  slashPopup.hidden = false;
}

function slashCommandMeta(name) {
  return SLASH_COMMANDS.find(([command]) => command === name);
}

function slashTreeAt(root, completed) {
  let nodes = SLASH_SUBCOMMANDS[root] || [];
  let node = null;
  for (const token of completed) {
    node = nodes.find((candidate) => candidate.name === token);
    if (!node) return null;
    nodes = node.children || [];
  }
  return { nodes, node };
}

function slashNodeItem(root, parentPath, node) {
  const path = parentPath ? `${parentPath} ${node.name}` : `${root} ${node.name}`;
  const rootMeta = slashCommandMeta(root);
  return {
    kind: "node",
    name: node.name,
    display: `/${path}`,
    path,
    detail: node.detail,
    hint: node.hint || "",
    icon: node.icon || rootMeta?.[1] || "command",
    meta: node.hint || "",
    hasChildren: Boolean(node.children?.length),
    hasValues: typeof node.values === "function",
    takesValue: Boolean(node.takesValue),
    form: node.form || "",
  };
}

function slashValueItems(root, parentPath, node, query) {
  const values = typeof node.values === "function" ? node.values() : [];
  return fuzzyRankFields(query, values, (value) => [value.name, value.detail]).map(
    (value) => ({
      ...value,
      kind: "value",
      display: `/${parentPath} ${value.name}`,
      path: `${parentPath} ${value.name}`,
      icon: node.icon || slashCommandMeta(root)?.[1] || "command",
      meta: value.thenText || "",
    }),
  );
}

function updateSlashPopup() {
  const value = promptInput.value;
  if (value === suppressedSlashValue) {
    suppressedSlashValue = "";
    return;
  }
  if (!value.startsWith("/")) {
    closeSlashPopup();
    return;
  }

  const body = value.slice(1);
  const endsWithSpace = /\s$/.test(body);
  const tokens = body.trimStart().split(/\s+/).filter(Boolean);
  if (tokens.length <= 1 && !endsWithSpace) {
    slashMode = "root";
    slashRoot = null;
    const query = tokens[0] || "";
    const items = fuzzyRankFields(
      query,
      SLASH_COMMANDS,
      ([name, , group, detail]) => [name, group, detail],
    ).map(([name, icon, group, detail]) => ({
      kind: "command",
      name,
      display: `/${name}`,
      path: name,
      icon,
      group,
      detail,
      hasChildren: Boolean(SLASH_SUBCOMMANDS[name]?.length),
      hasValues: name === "subagent_cancel",
      meta: SLASH_SUBCOMMANDS[name]?.length ? "subcommands" : name === "subagent_cancel" ? "value" : "",
    }));
    renderSlashItems(
      items,
      `Slash commands · ${items.length}`,
      "Fuzzy search · Tab to complete",
    );
    return;
  }

  const root = tokens[0];
  slashMode = "sub";
  slashRoot = root;
  const completed = endsWithSpace ? tokens.slice(1) : tokens.slice(1, -1);
  const partial = endsWithSpace ? "" : tokens.at(-1) || "";

  if (root === "subagent_cancel") {
    if (completed.length) {
      closeSlashPopup();
      return;
    }
    const node = {
      values: () =>
        SUBAGENT_OPTIONS.filter((agent) => !agent.detail.includes("done")).map((agent) =>
          slashValue(agent.name, agent.detail),
        ),
      icon: "users",
    };
    const items = slashValueItems(root, root, node, partial);
    renderSlashItems(
      items,
      `/subagent_cancel values · ${items.length}`,
      "Pick a child agent · Enter cancels it",
    );
    return;
  }

  const tree = slashTreeAt(root, completed);
  if (!tree) {
    closeSlashPopup();
    return;
  }
  const parentPath = [root, ...completed].join(" ");
  if (tree.node?.values) {
    const items = slashValueItems(root, parentPath, tree.node, partial);
    renderSlashItems(
      items,
      `/${parentPath} values · ${items.length}`,
      "Fuzzy search · Enter selects a value",
    );
    return;
  }
  if (tree.node && !tree.nodes.length) {
    closeSlashPopup();
    return;
  }

  const items = fuzzyRankFields(
    partial,
    tree.nodes,
    (node) => [node.name, node.detail, node.hint || ""],
  ).map((node) => slashNodeItem(root, parentPath, node));
  renderSlashItems(
    items,
    `/${parentPath} · ${items.length}`,
    "Fuzzy search · Enter opens or runs · Tab completes",
  );
}

function openSlashPopup() {
  closeMentionPopup();
  closeChoiceMenus();
  if (!promptInput.value.startsWith("/")) {
    promptInput.value = "/";
  }
  suppressedSlashValue = "";
  updateSlashPopup();
  promptInput.focus();
  promptInput.setSelectionRange(promptInput.value.length, promptInput.value.length);
}

function activateSlashItem(index, { completeOnly = false } = {}) {
  const item = slashItems[index];
  if (!item) return;

  if (item.kind === "command" && (item.hasChildren || item.hasValues)) {
    promptInput.value = `/${item.path} `;
    suppressedSlashValue = "";
    updateSlashPopup();
    promptInput.focus();
    promptInput.setSelectionRange(promptInput.value.length, promptInput.value.length);
    return;
  }

  if (item.kind === "node" && (item.hasChildren || item.hasValues || item.takesValue)) {
    if (item.form && !item.hasValues && !item.hasChildren) {
      if (completeOnly) {
        promptInput.value = `/${item.path} `;
        suppressedSlashValue = promptInput.value;
        closeSlashPopup();
        promptInput.focus();
        promptInput.setSelectionRange(promptInput.value.length, promptInput.value.length);
        return;
      }
      promptInput.value = "";
      closeSlashPopup();
      sizePrompt();
      if (item.form === "goal-create") {
        openGoalCreateForm();
      } else if (item.form === "mcp-prompt") {
        openMcpPromptForm();
      }
      return;
    }
    promptInput.value = `/${item.path} `;
    suppressedSlashValue = "";
    updateSlashPopup();
    promptInput.focus();
    promptInput.setSelectionRange(promptInput.value.length, promptInput.value.length);
    if (item.takesValue && !item.hasValues && !item.hasChildren) {
      showToast(`Enter ${item.hint || "the required argument"}`);
    }
    return;
  }

  if (completeOnly) {
    promptInput.value = `/${item.path} `;
    suppressedSlashValue = promptInput.value;
    closeSlashPopup();
    promptInput.focus();
    promptInput.setSelectionRange(promptInput.value.length, promptInput.value.length);
    showToast(`/${item.path} completed`);
    return;
  }

  if (item.kind === "value" && item.thenText) {
    promptInput.value = `/${item.path} `;
    suppressedSlashValue = promptInput.value;
    closeSlashPopup();
    promptInput.focus();
    promptInput.setSelectionRange(promptInput.value.length, promptInput.value.length);
    showToast(`Add ${item.thenText} and press Enter`);
    return;
  }

  promptInput.value = "";
  closeSlashPopup();
  executeSlashPath(item.path);
}

function escapeHtml(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function openCommandSheet({
  command,
  eyebrow = "SLASH COMMAND",
  title,
  description = "",
  body = "",
  status = "Ready",
  actions = '<button class="small-primary" type="button" data-command-sheet-action="done">Done</button>',
}) {
  closeExtensionsPopover();
  closeExtensionsManager();
  closePalette();
  closeDrawer();
  closeSessionManager();
  closeSessionMenu();
  closeComposerPopovers();
  setStickyBackgroundOpen(false);
  commandSheet.dataset.command = command || "";
  commandSheetEyebrow.textContent = eyebrow;
  commandSheetTitle.textContent = title;
  commandSheetDescription.textContent = description;
  commandSheetDescription.hidden = !description;
  commandSheetBody.innerHTML = body;
  commandSheetStatus.textContent = status;
  commandSheetActions.innerHTML = actions;
  commandSheet.hidden = false;
  window.requestAnimationFrame(() => $("#close-command-sheet").focus());
}

function closeCommandSheet({ restoreFocus = false } = {}) {
  if (commandSheet.hidden) return;
  commandSheet.hidden = true;
  commandSheet.dataset.command = "";
  if (restoreFocus) promptInput.focus();
}

function commandPill(label, variant = "") {
  return `<span class="command-pill${variant ? ` ${variant}` : ""}">${escapeHtml(label)}</span>`;
}

function commandAction(label, action, attributes = "") {
  return `<button class="command-action" type="button" data-command-sheet-action="${action}" ${attributes}>${escapeHtml(label)}</button>`;
}

function commandListRows(rows) {
  return rows
    .map(
      (row) => `<div class="command-list-row">
        <span class="command-list-icon"><svg class="icon small" aria-hidden="true"><use href="#i-${row.icon || "command"}"></use></svg></span>
        <span class="command-list-copy"><strong>${escapeHtml(row.title)}</strong><small>${escapeHtml(row.detail)}</small></span>
        <span class="command-list-meta">${escapeHtml(row.meta || "")}</span>
        ${row.action || commandPill(row.status || "", row.statusClass || "")}
      </div>`,
    )
    .join("");
}

function commandSection(title, content, meta = "") {
  return `<section class="command-section">
    <div class="command-section-title"><span>${escapeHtml(title)}</span><span>${escapeHtml(meta)}</span></div>
    ${content}
  </section>`;
}

function hooksSheetBody() {
  const pending = HOOK_SOURCES.filter((source) => !source.trusted).length;
  const rows = HOOK_SOURCES.map((source) => ({
    icon: "command",
    title: source.label,
    detail: source.detail,
    action: commandAction(
      source.trusted ? "Forget" : "Trust",
      source.trusted ? "forget-source" : "trust-source",
      `data-source-id="${source.id}"`,
    ),
  }));
  return commandSection(
    "Hook sources",
    `<div class="command-list">${commandListRows(rows)}</div>`,
    pending ? `${pending} need review` : "all trusted",
  );
}

function openHooksSheet() {
  const pending = HOOK_SOURCES.filter((source) => !source.trusted).length;
  openCommandSheet({
    command: "hooks list",
    title: "Command hooks",
    description:
      "Review the exact hook sources Tact would execute. Nothing runs until its source is trusted.",
    body: hooksSheetBody(),
    status: pending
      ? `${pending} source${pending === 1 ? "" : "s"} need review`
      : "All hook sources trusted",
    actions: `
      <button class="command-action" type="button" data-command-sheet-action="forget-all">Forget all</button>
      <button class="small-primary" type="button" data-command-sheet-action="trust-all">Trust all</button>
    `,
  });
}

function skillsSheetBody() {
  const rows = EXTENSION_DATA.skills.items.map(([, title, detail, status]) => ({
    icon: "grid",
    title,
    detail,
    action: commandAction(
      status === "enabled" ? "Hide" : "Show",
      "skill-toggle",
      `data-skill-name="${escapeHtml(title)}"`,
    ),
  }));
  return commandSection(
    "Catalog",
    `<div class="command-list">${commandListRows(rows)}</div>`,
    `${rows.length} shown`,
  );
}

function openSkillsSheet() {
  openCommandSheet({
    command: "skill list",
    title: "Skills",
    description:
      "Hidden skills stay runnable; hiding only removes them from automatic discovery in the system prompt.",
    body: skillsSheetBody(),
    status: `${EXTENSION_DATA.skills.count} in the catalog`,
  });
}

function pluginsSheetBody() {
  const rows = EXTENSION_DATA.plugins.items.map(([, title, detail, status, action]) => ({
    icon: "puzzle",
    title,
    detail,
    action: action
      ? commandAction("Update", "plugin-update", `data-plugin-name="${escapeHtml(title)}"`)
      : commandPill(status),
  }));
  return commandSection(
    "Installed plugins",
    `<div class="command-list">${commandListRows(rows)}</div>`,
    `${rows.length} installed`,
  );
}

function openPluginsSheet() {
  openCommandSheet({
    command: "plugin list",
    title: "Plugins",
    description:
      "Installed plugins can contribute skills, slash commands, hooks and MCP servers.",
    body: pluginsSheetBody(),
    status: `${EXTENSION_DATA.plugins.count} installed`,
    actions: `
      <button class="command-action" type="button" data-command-sheet-action="plugin-reload">Reload</button>
      <button class="small-primary" type="button" data-command-sheet-action="done">Done</button>
    `,
  });
}

function openMarketplaceSheet() {
  openCommandSheet({
    command: "plugin marketplace list",
    title: "Marketplaces",
    description: "Marketplace sources available to the plugin installer.",
    body: commandSection(
      "Registered sources",
      `<div class="command-list">${commandListRows([
        {
          icon: "puzzle",
          title: "claude-plugins-official",
          detail: "Built-in marketplace · 18 plugins",
          status: "built-in",
        },
        {
          icon: "branch",
          title: "github:labcoder/gpui-ai",
          detail: "GitHub source · last refreshed 2h ago",
          status: "synced",
        },
      ])}</div>`,
      "2 sources",
    ),
  });
}

function openMcpPromptsSheet() {
  const rows = MCP_PROMPTS.map((prompt) => ({
    icon: "command",
    title: `/${prompt.name}`,
    detail: prompt.detail,
    meta: prompt.server,
    action: commandAction(
      "Run",
      "mcp-run-prompt",
      `data-prompt-name="${prompt.name}"`,
    ),
  }));
  openCommandSheet({
    command: "mcp prompts",
    title: "MCP prompts",
    description:
      "Prompts published by connected MCP servers. Running one creates a normal user task.",
    body: commandSection(
      "Available prompts",
      `<div class="command-list">${commandListRows(rows)}</div>`,
      `${rows.length} prompts`,
    ),
  });
}

function openMcpPromptForm(promptName = "") {
  const prompt =
    MCP_PROMPTS.find((item) => item.name === promptName) || MCP_PROMPTS[0];
  openCommandSheet({
    command: "mcp prompt",
    title: "Run MCP prompt",
    description:
      "Resolve the server prompt with typed arguments, then submit it as a user task.",
    body: `<form class="command-form" id="mcp-prompt-form">
      <div class="command-form-row"><label for="mcp-prompt-server">Server</label><select id="mcp-prompt-server">${MCP_SERVER_OPTIONS.map((server) => `<option ${server.name === prompt.server ? "selected" : ""}>${escapeHtml(server.name)}</option>`).join("")}</select></div>
      <div class="command-form-row"><label for="mcp-prompt-name">Prompt</label><select id="mcp-prompt-name">${MCP_PROMPTS.map((item) => `<option value="${item.name}" ${item.name === prompt.name ? "selected" : ""}>${item.name}</option>`).join("")}</select></div>
      <div class="command-form-row"><label for="mcp-prompt-args">Arguments</label><input id="mcp-prompt-args" value="owner=labcoder repo=gpui-ai" /></div>
      <button class="small-primary command-form-submit" type="submit">Run prompt</button>
    </form>`,
    status: "Arguments are passed as key=value pairs",
  });
}

function openStatsSheet() {
  openCommandSheet({
    command: "stats",
    title: "Session stats",
    description: "Token, cache, turn and request counters for the current session.",
    body: `<div class="command-summary-grid" data-gpui-ai="InsightCard">
      <div class="command-summary-card"><span>INPUT TOKENS</span><strong>74.2k</strong></div>
      <div class="command-summary-card"><span>CACHE HIT</span><strong>61%</strong></div>
      <div class="command-summary-card"><span>TURNS</span><strong>7</strong></div>
      <div class="command-summary-card"><span>LLM CALLS</span><strong>18</strong></div>
      <div class="command-summary-card"><span>ELAPSED</span><strong>18:42</strong></div>
      <div class="command-summary-card"><span>OUTPUT TOKENS</span><strong>8.4k</strong></div>
    </div>`,
    status: "Provider request details retained for this session",
  });
}

function openBalanceSheet() {
  openCommandSheet({
    command: "balance",
    title: "Account balance",
    description: "Provider quota and reset window for the active account.",
    body: `<div class="command-summary-grid" data-gpui-ai="InsightCard">
      <div class="command-summary-card"><span>REMAINING</span><strong>78%</strong></div>
      <div class="command-summary-card"><span>RESET IN</span><strong>3h 12m</strong></div>
      <div class="command-summary-card"><span>PROVIDER</span><strong>Kimi</strong></div>
    </div>`,
    status: "Quota endpoint queried just now",
  });
}

function openHistorySheet() {
  const tasks = [
    ["Analyze gpui-ai component coverage", "Running · 18:42", "active"],
    ["Wire the desktop shell to tact_host", "Completed · 12m", "done"],
    ["Define the mixed transcript contract", "Completed · 24m", "done"],
    ["Review permission gate copy", "Cancelled · 41m", "cancelled"],
  ];
  openCommandSheet({
    command: "history",
    title: "Task history",
    description: "Every submitted task in this session, newest first.",
    body: commandSection(
      "Submitted tasks",
      `<div class="command-list" data-gpui-ai="RecordsTable">${commandListRows(
        tasks.map(([title, detail, state]) => ({
          icon: state === "active" ? "play" : state === "done" ? "check" : "x",
          title,
          detail,
          status: state,
          statusClass:
            state === "active" ? "" : state === "cancelled" ? "is-warn" : "is-off",
        })),
      )}</div>`,
      `${tasks.length} tasks`,
    ),
  });
}

function openHelpSheet() {
  const shortcuts = [
    ["⌘K", "Command palette", "Search every desktop entry"],
    ["⌘B", "Toggle sidebar", "Show or hide sessions"],
    ["⌘⌥B", "Toggle inspector", "Show or hide run details"],
    ["⌘1–4", "Inspector tabs", "Run, Goals, Agents, Context"],
    ["/", "Slash commands", "Commands and subcommands"],
    ["@", "Mention file", "Insert a workspace path"],
    ["Esc", "Dismiss overlay", "Close the topmost surface"],
  ];
  openCommandSheet({
    command: "help",
    title: "Keyboard help",
    description: "Global shortcuts and composer entry points.",
    body: commandSection(
      "Shortcuts",
      `<div class="command-list">${commandListRows(
        shortcuts.map(([key, title, detail]) => ({
          icon: "command",
          title,
          detail,
          meta: key,
          status: "shortcut",
          statusClass: "is-off",
        })),
      )}</div>`,
      `${shortcuts.length} shortcuts`,
    ),
  });
}

function openSystemPromptSheet() {
  openCommandSheet({
    command: "view-system-prompt",
    title: "System prompt",
    description: "Assembled prompt for the current model and workspace.",
    body: `<div class="command-section-title"><span>ASSEMBLED PROMPT</span><span>74.2k chars</span></div>
      <pre class="command-code">You are Tact, a coding agent running in the user's workspace.

Respect the active permission mode.
Use tools for evidence rather than guessing.
Treat hook-injected context as context, never as a user turn.
Keep Goals, tasks and subagent state attached to the run.

# Available skills
brainstorming, gpui-ai, code-review, test-driven-development

# Workspace
/Users/rg/Projects/tact · rust-infra/tact · feat/desktop</pre>`,
    status: "Read-only assembled snapshot",
  });
}

function openTasksDagSheet() {
  openCommandSheet({
    command: "tasks-dag",
    title: "Task dependency graph",
    description: "Current task nodes and their dependency order.",
    body: `<div class="command-dag">
      <div class="command-dag-node"><strong>Host bridge</strong><small>completed · 09:12</small></div>
      <div class="command-dag-node is-active"><strong>Mixed transcript</strong><small>running · 18:42</small></div>
      <div class="command-dag-node"><strong>Accessibility matrix</strong><small>waiting on transcript</small></div>
      <div class="command-dag-node"><strong>Session persistence</strong><small>completed · 10:06</small></div>
      <div class="command-dag-node"><strong>Desktop packaging</strong><small>queued</small></div>
      <div class="command-dag-node"><strong>Release candidate</strong><small>blocked by packaging</small></div>
    </div>`,
    status: "2 complete · 1 running · 3 waiting",
  });
}

function openLanguageSheet() {
  const current = body.dataset.language || "en";
  openCommandSheet({
    command: "lang",
    title: "Interface language",
    description: "Switch the desktop interface language for this session.",
    body: commandSection(
      "Language",
      `<div class="command-list">${commandListRows([
        {
          icon: "command",
          title: "English",
          detail: "English interface labels",
          action: commandAction(
            current === "en" ? "Current" : "Use",
            "language-select",
            'data-language="en"',
          ),
        },
        {
          icon: "command",
          title: "中文",
          detail: "中文界面标签",
          action: commandAction(
            current === "zh" ? "Current" : "Use",
            "language-select",
            'data-language="zh"',
          ),
        },
      ])}</div>`,
      "2 languages",
    ),
  });
}

function openQuitSheet() {
  openCommandSheet({
    command: "quit",
    title: "Quit Tact?",
    description:
      "The active session is already persisted. Quitting releases the session lock.",
    body: `<div class="command-list">${commandListRows([
      {
        icon: "clock",
        title: "Session 8f2a",
        detail: "Last saved just now · 46 messages",
        status: "saved",
      },
    ])}</div>`,
    status: "No unsaved changes",
    actions: `
      <button class="command-action" type="button" data-command-sheet-action="done">Cancel</button>
      <button class="small-primary" type="button" data-command-sheet-action="quit-confirm">Quit</button>
    `,
  });
}

function openModelSubagentSheet() {
  openCommandSheet({
    command: "model-subagent",
    title: "Subagent model",
    description: "The selected model is inherited by newly launched child agents.",
    body: commandSection(
      "Model",
      `<div class="command-list">${commandListRows([
        { icon: "cpu", title: "Claude Sonnet 4.5", detail: "Anthropic · 200K", status: "current" },
        { icon: "cpu", title: "Kimi K2.5", detail: "Moonshot · 256K", status: "available", statusClass: "is-off" },
        { icon: "cpu", title: "DeepSeek V4 Pro", detail: "DeepSeek · 128K", status: "available", statusClass: "is-off" },
      ])}</div>`,
      "inherited",
    ),
  });
}

function openModelSettingsSheet() {
  openCommandSheet({
    command: "model",
    title: "Main model",
    description:
      "Choose the model, reasoning effort and thinking budget for the next turn.",
    body: `<form class="command-form" id="model-settings-form">
      <div class="command-form-row"><label for="main-model-setting">Model</label><select id="main-model-setting">
        <option>Claude Sonnet 4.5</option><option>Kimi K2.5</option><option>DeepSeek V4 Pro</option>
      </select></div>
      <div class="command-form-row"><label for="model-effort-setting">Reasoning effort</label><select id="model-effort-setting">
        <option>Low</option><option selected>Medium</option><option>High</option><option>Max</option>
      </select></div>
      <div class="command-form-row"><label for="thinking-budget-setting">Thinking budget</label><input id="thinking-budget-setting" value="8192 tokens" /></div>
      <button class="small-primary command-form-submit" type="submit">Apply model settings</button>
    </form>`,
    status: "Applies to the next model request",
  });
}

function openBackgroundSheet() {
  openCommandSheet({
    command: "background",
    title: "Background tasks",
    description: "Commands running outside the main agent turn.",
    body: commandSection(
      "Tasks",
      `<div class="command-list">${commandListRows([
        {
          icon: "terminal",
          title: "cargo check -p tact_desktop",
          detail: "Running · 41s · latest: Finished dev profile",
          action: commandAction("Output", "background-output", 'data-background-id="018f3a2c"'),
        },
        {
          icon: "check",
          title: "cargo fmt --check",
          detail: "Completed · 12s",
          status: "done",
          statusClass: "is-off",
        },
      ])}</div>`,
      "1 running",
    ),
  });
}

const GENERIC_ENTRY_SHEETS = {
  provider: {
    title: "Provider and credentials",
    description: "Provider, API key, base URL and max output tokens.",
    fields: [
      ["Provider", "select", ["Anthropic", "OpenAI compatible", "Kimi", "DeepSeek"]],
      ["API key", "password", "sk-••••••••••••"],
      ["Base URL", "text", "https://api.anthropic.com"],
      ["Max tokens", "text", "8192"],
    ],
  },
  "context-window": {
    title: "Context window",
    description: "Model context size, compaction thresholds and reservations.",
    fields: [
      ["Context window", "text", "200000"],
      ["Compact threshold", "text", "160000"],
      ["Reserved output", "text", "12000"],
    ],
  },
  "snapshot-items": {
    title: "Project snapshot size",
    description: "Maximum project entries injected into the system prompt.",
    fields: [["Maximum entries", "text", "200"]],
  },
  "micro-compact": {
    title: "Micro-compaction",
    description: "Stub old tool results before each model call.",
    fields: [["Enabled", "toggle", true]],
  },
  memory: {
    title: "Memory",
    description: "Enable or disable memory injection and memory tools.",
    fields: [["Enabled", "toggle", true]],
  },
  notifications: {
    title: "Notifications",
    description: "Desktop notifications for completion and permission gates.",
    fields: [["Enabled", "toggle", true]],
  },
  "skill-auto-inject": {
    title: "Skill body injection",
    description: "Choose whether skill prompts receive summaries or full bodies.",
    fields: [["Injection mode", "select", ["Summary only", "Full body"]]],
  },
  "tokio-console": {
    title: "Tokio diagnostics",
    description: "Runtime diagnostics endpoint for the current process.",
    fields: [["Console address", "text", "127.0.0.1:6669"]],
  },
  "config-file": {
    title: "Configuration file",
    description: "Active config path and resolved values.",
    fields: [["Path", "text", "~/.config/tact/config.toml"]],
  },
  headless: {
    title: "Run headless task",
    description: "Execute one task without opening the interactive shell.",
    fields: [["Task", "textarea", "Summarize the current desktop parity gaps."]],
  },
  "plugin-install": {
    title: "Install plugin",
    description: "Install a plugin from a registered marketplace.",
    fields: [
      ["Marketplace", "select", ["claude-plugins-official", "github:labcoder/gpui-ai"]],
      ["Plugin", "text", "superpowers"],
    ],
  },
  "marketplace-add": {
    title: "Add marketplace",
    description: "Register a Git URL or GitHub owner/repo source.",
    fields: [["Source", "text", "github:labcoder/gpui-ai"]],
  },
  "marketplace-update": {
    title: "Update marketplace",
    description: "Re-fetch one registered marketplace catalog.",
    fields: [["Marketplace", "select", ["claude-plugins-official", "github:labcoder/gpui-ai"]]],
  },
  "marketplace-remove": {
    title: "Remove marketplace",
    description: "Remove a non-built-in marketplace source.",
    fields: [["Marketplace", "select", ["github:labcoder/gpui-ai"]]],
  },
  "mcp-get": {
    title: "Inspect MCP server",
    description: "Show status, transport, tools and source for one server.",
    fields: [["Server", "select", MCP_SERVER_OPTIONS.map((server) => server.name)]],
  },
  "mcp-add": {
    title: "Add MCP server",
    description: "Add a remote HTTP, local stdio or OAuth server.",
    fields: [
      ["Name", "text", "new-server"],
      ["Transport", "select", ["Remote HTTP", "Local stdio"]],
      ["URL or command", "text", "https://example.com/mcp"],
    ],
  },
  "mcp-remove": {
    title: "Remove MCP server",
    description: "Remove a server declaration from project or user config.",
    fields: [
      ["Server", "select", MCP_SERVER_OPTIONS.map((server) => server.name)],
      ["Scope", "select", ["Project", "User"]],
    ],
  },
  "mcp-logout": {
    title: "Delete MCP credentials",
    description: "Remove stored OAuth credentials for one server.",
    fields: [["Server", "select", MCP_SERVER_OPTIONS.map((server) => server.name)]],
  },
};

function genericFieldHtml([label, type, value]) {
  const id = `generic-${normalizeSearchText(label).replaceAll(" ", "-")}`;
  if (type === "toggle") {
    return `<label class="command-toggle-row"><span>${escapeHtml(label)}</span><input type="checkbox" ${value ? "checked" : ""} /></label>`;
  }
  if (type === "select") {
    return `<div class="command-form-row"><label for="${id}">${escapeHtml(label)}</label><select id="${id}">${value.map((option) => `<option>${escapeHtml(option)}</option>`).join("")}</select></div>`;
  }
  if (type === "textarea") {
    return `<div class="command-form-row"><label for="${id}">${escapeHtml(label)}</label><textarea id="${id}">${escapeHtml(value)}</textarea></div>`;
  }
  return `<div class="command-form-row"><label for="${id}">${escapeHtml(label)}</label><input id="${id}" type="${type}" value="${escapeHtml(value)}" /></div>`;
}

function openGenericEntrySheet(command, label) {
  const config = GENERIC_ENTRY_SHEETS[command] || {
    title: label,
    description: "Desktop control for this tact-ui entry.",
    fields: [["Value", "text", "Ready"]],
  };
  openCommandSheet({
    command,
    title: config.title,
    description: config.description,
    body: `<form class="command-form" id="generic-entry-form">
      ${config.fields.map(genericFieldHtml).join("")}
      <button class="small-primary command-form-submit" type="submit">Apply</button>
    </form>`,
    status: "Changes are applied to this workspace",
  });
}

function openSubagentCancelSheet() {
  const running = SUBAGENT_OPTIONS.filter((agent) => !agent.detail.includes("done"));
  openCommandSheet({
    command: "subagent_cancel",
    title: "Cancel subagent",
    description: "Choose the running child agent to stop.",
    body: commandSection(
      "Running agents",
      `<div class="command-list">${commandListRows(
        running.map((agent) => ({
          icon: "users",
          title: agent.name,
          detail: agent.detail,
          action: commandAction(
            "Stop",
            "subagent-stop",
            `data-agent-name="${agent.name}"`,
          ),
        })),
      )}</div>`,
      `${running.length} running`,
    ),
  });
}

function openCompactSheet() {
  openCommandSheet({
    command: "compact",
    title: "Compacting context",
    description: "Summarize retained evidence and rebuild the current context.",
    body: `<div class="command-section">
      <div class="command-section-title"><span>PROGRESS</span><span>64%</span></div>
      <div class="command-progress" data-gpui-ai="LoadingState"><span></span></div>
      <div class="command-list" style="margin-top:12px">${commandListRows([
        { icon: "check", title: "Retain tool evidence", detail: "diffs, command results and approvals", status: "kept" },
        { icon: "archive", title: "Stub old transcript", detail: "replace low-value turns with summaries", status: "working", statusClass: "is-warn" },
      ])}</div>
    </div>`,
    status: "Compaction runs only while the agent is idle",
  });
  window.setTimeout(() => {
    if (commandSheet.hidden || commandSheet.dataset.command !== "compact") return;
    commandSheetStatus.textContent = "Context compacted · 74.2k → 38.6k tokens";
  }, 900);
}

function openGoalListSheet(includeArchived = false) {
  focusTab("goals");
  const goals = GOAL_OPTIONS.filter((goal) => includeArchived || !goal.archived);
  openCommandSheet({
    command: "goal list",
    title: includeArchived ? "All Goals" : "Active Goals",
    description: "Durable Goals keep their own event stream and run budget.",
    body: commandSection(
      "Goals",
      `<div class="command-list">${commandListRows(
        goals.map((goal) => ({
          icon: "target",
          title: goal.title,
          detail: `${goal.id} · ${goal.status}`,
          status: goal.archived ? "archived" : goal.status,
          statusClass:
            goal.archived ? "is-off" : goal.status === "queued" ? "is-warn" : "",
        })),
      )}</div>`,
      `${goals.length} Goals`,
    ),
  });
}

function openGoalCreateForm() {
  openCommandSheet({
    command: "goal create",
    title: "Create Goal",
    description: "Define the objective and optional acceptance criteria.",
    body: `<form class="command-form" id="goal-create-form">
      <div class="command-form-row"><label for="goal-objective">Objective</label><textarea id="goal-objective" placeholder="Ship a native Tact desktop client without forking agent semantics."></textarea></div>
      <div class="command-form-row"><label for="goal-criteria">Acceptance criteria · one per line</label><textarea id="goal-criteria" placeholder="Host bridge proven&#10;Mixed transcript implemented&#10;Accessibility matrix passes"></textarea></div>
      <button class="small-primary command-form-submit" type="submit">Create Goal</button>
    </form>`,
    status: "The Goal runner starts when the objective is accepted",
  });
}

function openGoalDetailSheet(goalId) {
  const goal = GOAL_OPTIONS.find((item) => item.id === goalId);
  if (!goal) {
    showToast(`Unknown Goal ${goalId}`);
    return;
  }
  openCommandSheet({
    command: "goal show",
    title: goal.title,
    description: `Goal ${goal.id} · ${goal.status}`,
    body: commandSection(
      "Objective",
      `<div class="command-list">${commandListRows([
        {
          icon: "target",
          title: "Objective",
          detail: "Ship a native Tact client without forking agent semantics.",
          status: goal.status,
        },
        { icon: "check", title: "Host bridge proven", detail: "Required · passed", status: "passed" },
        { icon: "play", title: "Mixed transcript implemented", detail: "Required · active", status: "active" },
        { icon: "target", title: "Accessibility matrix", detail: "Required · pending", status: "pending", statusClass: "is-warn" },
      ])}</div>`,
      goal.archived ? "archived" : "working set",
    ),
  });
}

function openPluginActionSheet(action, pluginName = "") {
  openCommandSheet({
    command: `plugin ${action}`,
    title: action === "update" ? "Update plugin" : "Uninstall plugin",
    description: pluginName
      ? `${action === "update" ? "Update" : "Uninstall"} ${pluginName} from this workspace.`
      : `Choose a plugin to ${action}.`,
    body: commandSection(
      "Installed plugins",
      `<div class="command-list">${commandListRows(
        PLUGIN_OPTIONS.map((plugin) => ({
          icon: "puzzle",
          title: plugin.name,
          detail: plugin.detail,
          action: commandAction(
            action === "update" ? "Update" : "Uninstall",
            `plugin-${action}-confirm`,
            `data-plugin-name="${plugin.name}"`,
          ),
        })),
      )}</div>`,
      `${PLUGIN_OPTIONS.length} installed`,
    ),
  });
}

function executeSlashPath(path) {
  const tokens = path.trim().split(/\s+/).filter(Boolean);
  const command = tokens.shift() || "";
  const args = tokens;
  closeSlashPopup();
  promptInput.value = "";
  sizePrompt();

  switch (command) {
    case "theme":
      toggleTheme();
      return;
    case "model":
      openModelSettingsSheet();
      return;
    case "model-subagent":
      openModelSubagentSheet();
      return;
    case "permission":
      openChoiceMenu(permissionMenu, permissionSelect);
      return;
    case "view-system-prompt":
      openSystemPromptSheet();
      return;
    case "save":
      saveTranscript();
      return;
    case "compact":
      openCompactSheet();
      return;
    case "cancel":
      setRunPaused(true);
      showToast("Run cancelled");
      return;
    case "subagent_cancel":
      if (args[0]) {
        showToast(`Stopping subagent ${args[0]}`);
      } else {
        openSubagentCancelSheet();
      }
      return;
    case "quit":
      openQuitSheet();
      return;
    case "help":
      openHelpSheet();
      return;
    case "history":
      openHistorySheet();
      return;
    case "hook-output":
      body.dataset.hookOutput = body.dataset.hookOutput === "off" ? "on" : "off";
      showToast(
        `Hook output ${body.dataset.hookOutput === "off" ? "hidden" : "shown"}`,
      );
      return;
    case "balance":
      openBalanceSheet();
      return;
    case "lang":
      openLanguageSheet();
      return;
    case "stats":
      openStatsSheet();
      return;
    case "tasks-dag":
      openTasksDagSheet();
      return;
    case "background":
      openBackgroundSheet();
      return;
    case "skill":
      executeSkillCommand(args);
      return;
    case "plugin":
      executePluginCommand(args);
      return;
    case "mcp":
      executeMcpCommand(args);
      return;
    case "hooks":
      executeHooksCommand(args);
      return;
    case "goal":
      executeGoalCommand(args);
      return;
    default:
      showToast(`Unknown slash command /${command}`);
  }
}

function executeSkillCommand(args) {
  const [verb, name] = args;
  if (!verb || verb === "list") {
    openSkillsSheet();
    return;
  }
  if (verb === "reload") {
    showToast("Reloading skills");
    openCommandSheet({
      command: "skill reload",
      title: "Skills reloaded",
      description:
        "Discovered skills were refreshed from project, user and plugin sources.",
      body: commandSection(
        "Result",
        `<div class="command-list">${commandListRows([
          {
            icon: "check",
            title: "12 skills available",
            detail: "1 project · 4 user · 7 plugin",
            status: "ready",
          },
          {
            icon: "grid",
            title: "System-prompt catalog updated",
            detail: "Next model turn uses the refreshed catalog",
            status: "updated",
          },
        ])}</div>`,
      ),
      status: "Reload complete",
    });
    return;
  }
  if (["enable", "disable"].includes(verb)) {
    if (!name) {
      openSkillsSheet();
      return;
    }
    const item = EXTENSION_DATA.skills.items.find(([, title]) => title === name);
    if (item) item[3] = verb === "enable" ? "enabled" : "disabled";
    openSkillsSheet();
    showToast(`${name} ${verb === "enable" ? "shown" : "hidden"}`);
    return;
  }
  showToast("Usage: /skill list | reload | enable <name> | disable <name>");
}

function executePluginCommand(args) {
  const [verb, ...rest] = args;
  if (!verb || verb === "list") {
    openPluginsSheet();
    return;
  }
  if (verb === "reload") {
    showToast("Plugins reloaded");
    openPluginsSheet();
    return;
  }
  if (verb === "marketplace" && rest[0] === "list") {
    openMarketplaceSheet();
    return;
  }
  if (["update", "uninstall"].includes(verb)) {
    openPluginActionSheet(verb, rest[0] || "");
    return;
  }
  showToast(
    "Usage: /plugin list | reload | uninstall <name> | update <name> | marketplace list",
  );
}

function executeMcpCommand(args) {
  const [verb, name] = args;
  if (!verb || verb === "list") {
    openExtensionsManager("mcp");
    return;
  }
  if (["auth", "login"].includes(verb)) {
    if (!name) {
      openExtensionsManager("mcp");
      return;
    }
    const item = EXTENSION_DATA.mcp.items.find(([, title]) => title === name);
    if (item) {
      item[3] = "live";
      item[4] = false;
    }
    openExtensionsManager("mcp");
    showToast(`${name} authorized`);
    return;
  }
  if (verb === "prompts") {
    openMcpPromptsSheet();
    return;
  }
  if (verb === "prompt") {
    openMcpPromptForm(name || "");
    return;
  }
  showToast("Usage: /mcp auth <server> | list | prompts | prompt <server> <name>");
}

function executeHooksCommand(args) {
  const [verb, flag, value] = args;
  if (!verb || verb === "list") {
    openHooksSheet();
    return;
  }
  if (verb === "trust") {
    if (flag === "--all") {
      HOOK_SOURCES.forEach((source) => {
        source.trusted = true;
      });
      openHooksSheet();
      showToast("All hook sources trusted");
      return;
    }
    if (flag === "--source" && value) {
      const source = HOOK_SOURCES.find(
        (item) => item.id === value || item.label === value,
      );
      if (source) source.trusted = true;
      openHooksSheet();
      showToast(
        source ? `${source.label} trusted` : `Unknown hook source ${value}`,
      );
      return;
    }
    openHooksSheet();
    showToast("Choose a source or use /hooks trust --all");
    return;
  }
  if (verb === "forget" && flag === "--all") {
    HOOK_SOURCES.forEach((source) => {
      source.trusted = false;
    });
    openHooksSheet();
    showToast("All hook approvals forgotten");
    return;
  }
  openHooksSheet();
  showToast(
    "Usage: /hooks list | trust --all | trust --source <label> | forget --all",
  );
}

function executeGoalCommand(args) {
  const [verb, id, ...textParts] = args;
  if (!verb) {
    openGoalListSheet(false);
    return;
  }
  if (verb === "list") {
    openGoalListSheet(id === "--all");
    return;
  }
  if (verb === "create") {
    if (!id) {
      openGoalCreateForm();
    } else {
      showToast("Goal created");
      openGoalListSheet(false);
    }
    return;
  }
  if (verb === "show") {
    if (id) openGoalDetailSheet(id);
    else openGoalListSheet(false);
    return;
  }
  if (
    ["pause", "resume", "cancel", "archive", "unarchive", "steer", "accept"].includes(
      verb,
    )
  ) {
    const goal = GOAL_OPTIONS.find((item) => item.id === id);
    if (!goal) {
      showToast(`Unknown Goal ${id || ""}`);
      return;
    }
    if (verb === "pause") goal.status = "paused";
    if (verb === "resume") goal.status = "running";
    if (verb === "cancel") goal.status = "cancelled";
    if (verb === "archive") goal.archived = true;
    if (verb === "unarchive") goal.archived = false;
    if (["steer", "accept"].includes(verb) && !textParts.length) {
      showToast(
        `Add ${verb === "steer" ? "steering text" : "an acceptance note"}`,
      );
      return;
    }
    focusTab("goals");
    showToast(`${goal.title} · /goal ${verb} complete`);
    openGoalDetailSheet(goal.id);
    return;
  }
  openGoalListSheet(false);
}

function saveTranscript() {
  const text = $$(".row")
    .map((row) => row.innerText.trim())
    .filter(Boolean)
    .join("\n\n");
  const blob = new Blob([text], { type: "text/plain" });
  const link = document.createElement("a");
  link.href = URL.createObjectURL(blob);
  link.download = "tact-transcript-8f2a.txt";
  link.click();
  URL.revokeObjectURL(link.href);
  showToast("Transcript saved to Downloads");
}

function executeSlashInput(value) {
  if (!value.startsWith("/")) return false;
  const path = value.slice(1).trim();
  if (!path) return false;
  const root = path.split(/\s+/)[0];
  if (!slashCommandMeta(root)) {
    showToast(`Unknown slash command /${root}`);
    return true;
  }
  executeSlashPath(path);
  return true;
}

function renderMentionItems(items) {
  mentionItems = items;
  if (!items.length) {
    closeMentionPopup();
    return;
  }
  mentionList.innerHTML = items
    .map(
      ([path, detail, icon], index) => `<button class="composer-entry${index === 0 ? " is-selected" : ""}" type="button" role="option" data-mention-index="${index}">
        <span class="composer-entry-icon"><svg class="icon small" aria-hidden="true"><use href="#i-${icon}"></use></svg></span>
        <span><strong>@${path}</strong><small>${detail}</small></span>
        <span class="composer-entry-meta">${icon === "image" ? "image" : "file"}</span>
      </button>`,
    )
    .join("");
  mentionPopup.hidden = false;
}

function updateMentionPopup() {
  const match = promptInput.value.match(/(?:^|\s)@([^\s]*)$/);
  if (!match) {
    closeMentionPopup();
    return;
  }
  const query = match[1].toLowerCase();
  renderMentionItems(
    fuzzyRankFields(query, MENTIONS, ([path, detail]) => [path, detail]),
  );
}

function openMentionPopup() {
  closeSlashPopup();
  closeChoiceMenus();
  const separator = promptInput.value && !promptInput.value.endsWith(" ") ? " " : "";
  promptInput.value += `${separator}@`;
  updateMentionPopup();
  promptInput.focus();
  promptInput.setSelectionRange(promptInput.value.length, promptInput.value.length);
}

function activateMentionItem(index) {
  const item = mentionItems[index];
  if (!item) return;
  promptInput.value = promptInput.value.replace(/@[^\s]*$/, `@${item[0]} `);
  closeMentionPopup();
  promptInput.focus();
  promptInput.setSelectionRange(promptInput.value.length, promptInput.value.length);
}

function openChoiceMenu(menu, trigger) {
  closeSlashPopup();
  closeMentionPopup();
  closeChoiceMenus();
  menu.hidden = false;
  const rect = trigger.getBoundingClientRect();
  const menuRect = menu.getBoundingClientRect();
  menu.style.left = `${Math.min(window.innerWidth - menuRect.width - 10, Math.max(10, rect.left))}px`;
  menu.style.top = `${Math.max(10, rect.top - menuRect.height - 8)}px`;
}

function openPalette() {
  closeExtensionsPopover();
  closeExtensionsManager();
  closeCommandSheet();
  closeComposerPopovers();
  closeSessionMenu();
  closeSessionManager();
  palette.hidden = false;
  paletteSearch.value = "";
  filterCommands("");
  window.requestAnimationFrame(() => paletteSearch.focus());
}

async function closePalette() {
  if (palette.hidden) return;
  await animateElement(
    $(".palette", palette),
    [
      { opacity: 1, transform: "translateY(0) scale(1)" },
      { opacity: 0, transform: "translateY(-6px) scale(0.99)" },
    ],
    { duration: 150 },
  );
  palette.hidden = true;
  $("#command-trigger").focus();
}

function filterCommands(query) {
  const needle = query.trim().toLowerCase();
  const items = $$(".command-item", commandList);
  let selected = null;
  let visible;
  if (!needle) {
    visible = new Set(items.filter((item) => item.dataset.recent === "true"));
  } else {
    const ranked = items
      .map((item) => ({
        item,
        score: fuzzyScoreFields(needle, [
          $("strong", item)?.textContent || "",
          $("small", item)?.textContent || "",
          $(".command-meta", item)?.textContent || "",
        ]),
      }))
      .filter(({ score }) => Number.isFinite(score))
      .sort((left, right) => right.score - left.score);
    const threshold =
      ranked.length && ranked[0].score > 0 ? ranked[0].score * 0.72 : 0;
    visible = new Set(
      ranked.filter(({ score }) => score >= threshold).map(({ item }) => item),
    );
  }

  items.forEach((item) => {
    const matches = visible.has(item);
    item.hidden = !matches;
    item.classList.remove("is-selected");
    if (matches && !selected) selected = item;
  });

  $$(".command-group", commandList).forEach((group) => {
    const hasVisibleItem = $$(".command-item", group).some((item) => !item.hidden);
    group.classList.toggle("is-empty", !hasVisibleItem);
  });

  selected?.classList.add("is-selected");
  $("#palette-title").textContent = needle
    ? `Search results · ${visible.size} of ${items.length}`
    : `Recent commands · ${visible.size} of ${items.length} · type to search`;
}

function moveCommand(direction) {
  const visible = $$(".command-item", commandList).filter((item) => !item.hidden);
  if (!visible.length) return;
  const current = visible.findIndex((item) => item.classList.contains("is-selected"));
  const next = (current + direction + visible.length) % visible.length;
  visible.forEach((item) => item.classList.remove("is-selected"));
  visible[next].classList.add("is-selected");
  visible[next].scrollIntoView({ block: "nearest" });
}

const railIsOpen = () => !workbench.classList.contains("rail-hidden");
const inspectorIsOpen = () => !workbench.classList.contains("inspector-hidden");

function syncPanelControls() {
  const railOpen = railIsOpen();
  const inspectorOpen = inspectorIsOpen();
  railToggle.classList.toggle("is-active", railOpen);
  railToggle.setAttribute("aria-pressed", String(railOpen));
  railToggle.setAttribute(
    "aria-label",
    railOpen ? "Hide session sidebar" : "Show session sidebar",
  );
  inspectorToggle.classList.toggle("is-active", inspectorOpen);
  inspectorToggle.setAttribute("aria-pressed", String(inspectorOpen));
  inspectorToggle.setAttribute(
    "aria-label",
    inspectorOpen ? "Hide inspector" : "Show inspector",
  );
}

function updatePanelScrim() {
  const overlay =
    (window.innerWidth <= 1180 && inspectorIsOpen()) ||
    (window.innerWidth <= 860 && railIsOpen());
  panelScrim.hidden = !overlay;
}

function setRailOpen(open) {
  if (!open) closeSessionMenu();
  if (open && window.innerWidth <= 860) {
    workbench.classList.add("inspector-hidden");
  }
  workbench.classList.toggle("rail-hidden", !open);
  syncPanelControls();
  updatePanelScrim();
}

function setInspectorOpen(open) {
  if (open && window.innerWidth <= 860) {
    workbench.classList.add("rail-hidden");
  }
  workbench.classList.toggle("inspector-hidden", !open);
  syncPanelControls();
  updatePanelScrim();
}

function toggleRail() {
  setRailOpen(!railIsOpen());
}

function toggleInspector() {
  setInspectorOpen(!inspectorIsOpen());
}

function runEntry(command, label) {
  closePalette();

  if (command === "toggle-sidebar") return toggleRail();
  if (command === "toggle-inspector") return toggleInspector();
  if (command === "appearance") return toggleTheme();
  if (command === "agents-open") return focusTab("agents");
  if (command === "agent-transcript") return openDrawer();
  if (command === "attach-file") return addDemoAttachment();
  if (command === "mention-file") return openMentionPopup();
  if (command === "voice-input") return toggleVoiceInput();
  if (command === "queue-message") {
    promptInput.focus();
    showToast("Compose a prompt to add it to the queue");
    return;
  }

  if (command === "session-list") {
    setRailOpen(true);
    window.setTimeout(() => $(".rail-search")?.focus(), 380);
    return;
  }
  if (command === "session-archive") {
    setRailOpen(true);
    window.setTimeout(() => {
      const active = $(".run-item.is-active");
      $(".run-more", active)?.click();
    }, 390);
    return;
  }

  const slashPickers = {
    "skill-enable": "skill enable",
    "skill-disable": "skill disable",
    "plugin-uninstall": "plugin uninstall",
    "plugin-update": "plugin update",
    "mcp-login": "mcp login",
    "goal-show": "goal show",
    "goal-steer": "goal steer",
    "goal-pause": "goal pause",
    "goal-resume": "goal resume",
    "goal-cancel": "goal cancel",
    "goal-accept": "goal accept",
    "goal-archive": "goal archive",
    "goal-unarchive": "goal unarchive",
  };
  if (slashPickers[command]) {
    promptInput.value = `/${slashPickers[command]} `;
    suppressedSlashValue = "";
    openSlashPopup();
    return;
  }

  const slashAlias = {
    "save-transcript": "save",
    quit: "quit",
    "cancel-run": "cancel",
    compact: "compact",
    stats: "stats",
    background: "background",
    history: "history",
    help: "help",
    "system-prompt": "view-system-prompt",
    "tasks-dag": "tasks-dag",
    model: "model",
    "model-subagent": "model-subagent",
    permission: "permission",
    language: "lang",
    balance: "balance",
    "hook-output": "hook-output",
    "agent-cancel": "subagent_cancel",
    "skill-list": "skill list",
    "skill-reload": "skill reload",
    "plugin-list": "plugin list",
    "plugin-reload": "plugin reload",
    "marketplace-list": "plugin marketplace list",
    "mcp-list": "mcp list",
    "mcp-prompts": "mcp prompts",
    "mcp-prompt": "mcp prompt",
    "hooks-list": "hooks list",
    "hooks-trust": "hooks trust",
    "hooks-forget": "hooks forget --all",
    "goal-open": "goal list",
    "goal-create": "goal create",
    "goal-list": "goal list",
  };
  if (slashAlias[command]) {
    executeSlashPath(slashAlias[command]);
    return;
  }

  openGenericEntrySheet(command, label);
}

function updateTabIndicator() {
  const buttons = $$(".inspector-tabs button");
  const index = Math.max(
    0,
    buttons.findIndex((button) => button.classList.contains("is-active")),
  );
  $(".inspector-tab-indicator")?.style.setProperty("--tab-index", String(index));
}

async function focusTab(name) {
  if (!inspectorIsOpen()) setInspectorOpen(true);
  const next = $(`.inspector-panel[data-panel="${name}"]`);
  const current = $(".inspector-panel.is-active");
  if (!next || current === next) return;

  $$(".inspector-tabs button").forEach((button) => {
    const active = button.dataset.tab === name;
    button.classList.toggle("is-active", active);
    button.setAttribute("aria-selected", String(active));
  });
  updateTabIndicator();

  const inspector = $(".inspector");
  inspector.classList.add("is-switching");
  next.hidden = false;
  next.classList.add("is-active");

  if (prefersReducedMotion()) {
    if (current) {
      current.hidden = true;
      current.classList.remove("is-active");
    }
    inspector.classList.remove("is-switching");
    return;
  }

  if (current) current.classList.add("is-transitioning-out");
  await Promise.all([
    current
      ? animateElement(
          current,
          [
            { opacity: 1, transform: "translateX(0)" },
            { opacity: 0, transform: "translateX(-8px)" },
          ],
          { duration: 170 },
        )
      : Promise.resolve(),
    animateElement(
      next,
      [
        { opacity: 0, transform: "translateY(8px)" },
        { opacity: 1, transform: "translateY(0)" },
      ],
      { duration: 300, delay: 45 },
    ),
  ]);

  if (current) {
    current.hidden = true;
    current.classList.remove("is-active", "is-transitioning-out");
  }
  inspector.classList.remove("is-switching");
}

async function switchRun(button) {
  if (button.classList.contains("is-active")) return;
  const run = RUNS[button.dataset.run];
  if (!run) return;

  const thread = $(".thread");
  const swap = () => {
    $$(".run-item").forEach((item) => item.classList.toggle("is-active", item === button));
    $("#session-id-label").textContent = run.session;
    $("#run-label").textContent = run.label;
    $("#thread-title").innerHTML = run.title;
    $("#thread-lede").textContent = run.lede;
  };

  if (prefersReducedMotion()) {
    swap();
    return;
  }

  await animateElement(
    thread,
    [
      { opacity: 1, transform: "translateY(0)", filter: "blur(0)" },
      { opacity: 0, transform: "translateY(-7px)", filter: "blur(3px)" },
    ],
    { duration: 170 },
  );
  swap();
  await animateElement(
    thread,
    [
      { opacity: 0, transform: "translateY(8px)", filter: "blur(3px)" },
      { opacity: 1, transform: "translateY(0)", filter: "blur(0)" },
    ],
    { duration: 320 },
  );
}

function openDrawer() {
  closeExtensionsPopover();
  closeExtensionsManager();
  closeCommandSheet();
  closeComposerPopovers();
  closeSessionMenu();
  closeSessionManager();
  drawer.hidden = false;
  drawerScrim.hidden = false;
  $("#close-drawer").focus();
}

async function closeDrawer() {
  if (drawer.hidden) return;
  if (!prefersReducedMotion()) {
    drawerScrim.animate([{ opacity: 1 }, { opacity: 0 }], {
      duration: 160,
      fill: "both",
    });
    await animateElement(
      drawer,
      [
        { opacity: 1, transform: "translateX(0)" },
        { opacity: 0, transform: "translateX(24px)" },
      ],
      { duration: 180 },
    );
  }
  drawer.hidden = true;
  drawerScrim.hidden = true;
  $("#open-subagent").focus();
}

async function resolveGate(decision) {
  const actions = $("#gate-actions");
  if (!actions) return;

  const gate = $("#permission-gate");
  if (gate.classList.contains("is-approved") || gate.classList.contains("is-denied")) return;
  gate.classList.add("is-resolving");
  await delay(prefersReducedMotion() ? 0 : 150);

  const approved = decision !== "deny";
  const label =
    decision === "once"
      ? "Approved once"
      : decision === "session"
        ? "Approved for this session"
        : "Denied by user";
  actions.innerHTML = `<div class="gate-resolved ${approved ? "" : "is-denied"}">
    <span class="status-pill ${approved ? "status-done" : ""}">${approved ? "resolved" : "denied"}</span>
    <span>${label} · response sent through UserCommand::UiResponse</span>
  </div>`;

  gate.classList.remove("is-resolving");
  gate.classList.add(approved ? "is-approved" : "is-denied");
  const signal = $(".gate-signal", gate);
  signal.textContent = approved ? "✓" : "×";

  const rowStatus = $("#approval-status");
  const rowState = $("#approval-state");
  const rowButton = $("#approval-row .group-tool-head");
  const rowBody = $("#approval-body");
  rowStatus.textContent = approved
    ? decision === "session"
      ? "session"
      : "approved"
    : "denied";
  rowStatus.classList.add("is-resolved");
  rowState.className = `tool-state ${approved ? "is-done" : "is-cancelled"}`;
  rowState.innerHTML = approved
    ? '<svg class="icon tiny" aria-hidden="true"><use href="#i-check"></use></svg>'
    : '<svg class="icon tiny" aria-hidden="true"><use href="#i-x"></use></svg>';
  updateLoopToolGroupState();
  if (rowButton.getAttribute("aria-expanded") === "true") {
    rowButton.setAttribute("aria-expanded", "false");
    await toggleAnimatedBody(rowButton, rowBody, true);
  }

  const warning = $(".chip-warn");
  if (warning) {
    warning.classList.remove("chip-warn");
    warning.innerHTML = `<span class="chip-dot"></span>Running`;
  }

  const status = $(".run-status-card p");
  if (status) status.innerHTML = `<span class="live-dot"></span>Executing · clippy`;

  composerHint.innerHTML = "Running · <kbd>Esc</kbd> interrupts";
  stickyPermissionBanner.hidden = true;
  showToast(label);
}

async function reviewPermission() {
  const groupBody = $("#loop-7-body");
  const groupButton = $("#loop-7-tools .tool-group-head");
  if (groupBody.hidden) {
    groupButton.setAttribute("aria-expanded", "true");
    await toggleAnimatedBody(groupButton, groupBody, false);
  }

  const approvalBody = $("#approval-body");
  const approvalButton = $("#approval-row .group-tool-head");
  if (approvalBody.hidden) {
    approvalButton.setAttribute("aria-expanded", "true");
    await toggleAnimatedBody(approvalButton, approvalBody, false);
  }

  $("#approval-row").scrollIntoView({ behavior: "smooth", block: "center" });
  window.setTimeout(() => $("#gate-actions .gate-allow")?.focus(), 380);
}

function resolveDiff(decision) {
  const filebar = $(".diff-filebar");
  if (!filebar) return;
  const view = $(".diff-view");
  view.classList.remove("is-accepted", "is-rejected");
  view.classList.add(decision === "accept" ? "is-accepted" : "is-rejected");
  const pill = $(".status-pill", filebar);
  if (pill) {
    pill.textContent = decision === "accept" ? "Accepted" : "Rejected";
    pill.className = `status-pill ${decision === "accept" ? "status-done" : ""}`;
  }
  showToast(decision === "accept" ? "Hunk accepted" : "Hunk rejected");
}

async function collapseDecisionDetails(card) {
  const parts = [card.querySelector(":scope > p"), $(".decision-options", card)].filter(Boolean);
  if (prefersReducedMotion()) {
    parts.forEach((part) => {
      part.hidden = true;
    });
    card.classList.add("is-collapsed");
    return;
  }

  await Promise.all(
    parts.map(async (part) => {
      const height = part.getBoundingClientRect().height;
      const marginTop = getComputedStyle(part).marginTop;
      part.style.overflow = "hidden";
      part.style.willChange = "height, opacity, margin";
      await animateElement(
        part,
        [
          { height: `${height}px`, opacity: 1, marginTop },
          { height: "0px", opacity: 0, marginTop: "0px" },
        ],
        { duration: 220 },
      );
      part.hidden = true;
      part.style.overflow = "";
      part.style.willChange = "";
      part.style.height = "";
      part.style.marginTop = "";
    }),
  );
  card.classList.add("is-collapsed");
}

function updateLoopToolGroupState() {
  const statuses = [
    "ask-single-status",
    "ask-multi-status",
    "approval-status",
    "subagent-status",
  ].map(
    (id) => $(`#${id}`)?.textContent.trim() || "waiting",
  );
  const state = $("#loop-7-state");
  if (!state) return;
  const pending = statuses.some(
    (status) => status === "waiting" || status === "running" || status.endsWith("%"),
  );
  if (!pending) {
    state.className = "tool-state is-done";
    state.innerHTML = '<svg class="icon tiny" aria-hidden="true"><use href="#i-check"></use></svg>';
  } else {
    state.className = "tool-state is-waiting";
    state.innerHTML = "";
  }
}

async function finishDecisionRow(type, label, cancelled) {
  const rowStatus = $(`#ask-${type}-status`);
  const rowButton = $(`#ask-${type}-row .group-tool-head`);
  const rowBody = $(`#ask-${type}-body`);
  const rowState = $(`#ask-${type}-row .tool-state`);

  rowStatus.textContent = label;
  rowStatus.classList.add("is-resolved");
  rowState.className = `tool-state ${cancelled ? "is-cancelled" : "is-done"}`;
  rowState.innerHTML = cancelled
    ? '<svg class="icon tiny" aria-hidden="true"><use href="#i-x"></use></svg>'
    : '<svg class="icon tiny" aria-hidden="true"><use href="#i-check"></use></svg>';
  updateLoopToolGroupState();

  if (rowButton.getAttribute("aria-expanded") === "true") {
    rowButton.setAttribute("aria-expanded", "false");
    await toggleAnimatedBody(rowButton, rowBody, true);
  }
}

async function resolveDecision(type, cancelled = false) {
  const card = $(`#${type}-select-card`);
  if (!card || card.classList.contains("is-resolved") || card.classList.contains("is-cancelled")) {
    return;
  }
  const options = $$(".decision-option", card);
  const state = $(`#${type}-select-state`);
  const confirm = $(`[data-decision-confirm="${type}"]`, card);
  const cancel = $(`[data-decision-cancel="${type}"]`, card);
  const pill = $(".status-pill", card);

  options.forEach((option) => {
    option.disabled = true;
  });
  confirm.hidden = true;
  confirm.disabled = true;
  cancel.hidden = true;
  cancel.disabled = true;

  if (cancelled) {
    card.classList.add("is-cancelled");
    state.textContent = "Cancelled";
    pill.textContent = "cancelled";
    card.classList.add("is-collapsing");
    await collapseDecisionDetails(card);
    card.classList.remove("is-collapsing");
    await finishDecisionRow(type, "cancelled", true);
    showToast(`${type === "single" ? "Single" : "Multi"}-select cancelled`);
    return;
  }

  const selected = options
    .filter((option) => option.classList.contains("is-selected"))
    .map((option) => $("strong", option).textContent.trim());
  card.classList.add("is-resolved");
  state.textContent = `Selected: ${selected.join(", ")}`;
  pill.textContent = "resolved";
  pill.classList.add("status-done");
  card.classList.add("is-collapsing");
  await collapseDecisionDetails(card);
  card.classList.remove("is-collapsing");
  await finishDecisionRow(type, selected.join(", "), false);
  await animateElement(
    card,
    [
      { transform: "scale(1)" },
      { transform: "scale(0.995)" },
      { transform: "scale(1)" },
    ],
    { duration: 240 },
  );
  showToast(`Selection sent · ${selected.join(", ")}`);
}

function animateExpansionRows(target) {
  const rows = $$(".group-tool, .sticky-task", target).slice(0, 10);
  rows.forEach((row, index) => {
    const animation = row.animate(
      [
        { opacity: 0, transform: "translateY(5px)" },
        { opacity: 1, transform: "translateY(0)" },
      ],
      {
        duration: 230,
        delay: 35 + index * 24,
        easing: "cubic-bezier(0.22, 1, 0.36, 1)",
        fill: "both",
      },
    );
    animation.finished.catch(() => {}).finally(() => animation.cancel());
  });
}

async function toggleAnimatedBody(button, target, expanded) {
  const surface = button.closest(".tool-card, .sticky-host, .decision-card");
  const rowSurface = button.closest(".group-tool");
  const animationSurface = rowSurface || surface;
  button.classList.add(expanded ? "is-collapsing" : "is-expanding");
  animationSurface?.classList.add(expanded ? "is-collapsing" : "is-expanding");

  if (prefersReducedMotion()) {
    target.hidden = expanded;
    button.classList.remove("is-collapsing", "is-expanding");
    animationSurface?.classList.remove("is-collapsing", "is-expanding");
    return;
  }

  target.style.overflow = "hidden";
  target.style.willChange = "height, opacity, transform, filter";

  if (expanded) {
    const height = target.getBoundingClientRect().height;
    await animateElement(
      target,
      [
        { height: `${height}px`, opacity: 1, transform: "translateY(0)", filter: "blur(0)" },
        { height: "0px", opacity: 0, transform: "translateY(-4px)", filter: "blur(1px)" },
      ],
      { duration: 190 },
    );
    target.hidden = true;
  } else {
    target.hidden = false;
    const height = target.scrollHeight;
    animateExpansionRows(target);
    await animateElement(
      target,
      [
        { height: "0px", opacity: 0, transform: "translateY(-5px)", filter: "blur(2px)" },
        { height: `${height}px`, opacity: 1, transform: "translateY(0)", filter: "blur(0)" },
      ],
      { duration: 300 },
    );
  }

  target.style.overflow = "";
  target.style.willChange = "";
  button.classList.remove("is-collapsing", "is-expanding");
  animationSurface?.classList.remove("is-collapsing", "is-expanding");
}

function addUserRow(text) {
  const timeline = $("#timeline");
  const article = document.createElement("article");
  article.className = "row row-user reveal";
  article.innerHTML = `
    <div class="avatar">RG</div>
    <div class="speech">
      <div class="row-label">You <span>just now</span></div>
      <p></p>
    </div>`;
  $("p", article).textContent = text;
  timeline.appendChild(article);
  article.scrollIntoView({ behavior: "smooth", block: "center" });
}

function setRunPaused(paused) {
  runPaused = paused;
  stopButton.innerHTML = paused
    ? '<svg class="icon" aria-hidden="true"><use href="#i-play"></use></svg>'
    : '<svg class="icon" aria-hidden="true"><use href="#i-stop"></use></svg>';
  stopButton.setAttribute("aria-label", paused ? "Resume run" : "Stop run");
  composerHint.innerHTML = paused
    ? "Paused · queued prompts will wait"
    : "Running · <kbd>Esc</kbd> interrupts";
  showToast(paused ? "Run paused" : "Run resumed");
}

function sizePrompt() {
  promptInput.style.height = "auto";
  promptInput.style.height = `${Math.min(promptInput.scrollHeight, 130)}px`;
}

const DEMO_ATTACHMENTS = [
  ["prototype-html", "prototype.html", "file"],
  ["light-preview", "light.png", "image"],
  ["prd", "2026-10-30-tact-desktop-prd.md", "file"],
];

function renderAttachments() {
  attachmentStrip.hidden = attachments.length === 0;
  attachmentStrip.innerHTML = attachments
    .map(
      (attachment) => `<span class="attachment-chip" data-attachment-id="${attachment.id}">
        <svg class="icon tiny" aria-hidden="true"><use href="#i-${attachment.kind === "image" ? "image" : "file"}"></use></svg>
        <span>${escapeHtml(attachment.name)}</span>
        <button type="button" data-remove-attachment="${attachment.id}" aria-label="Remove ${escapeHtml(attachment.name)}">
          <svg class="icon tiny" aria-hidden="true"><use href="#i-x"></use></svg>
        </button>
      </span>`,
    )
    .join("");
}

function addDemoAttachment() {
  const next = DEMO_ATTACHMENTS.find(
    ([id]) => !attachments.some((attachment) => attachment.id === id),
  );
  if (!next) {
    showToast("All demo attachments are already present");
    return;
  }
  const [id, name, kind] = next;
  attachments.push({ id, name, kind });
  renderAttachments();
  showToast(`${name} attached`);
}

function toggleVoiceInput() {
  voiceActive = !voiceActive;
  voiceButton.classList.toggle("is-active", voiceActive);
  voiceButton.setAttribute("aria-pressed", String(voiceActive));
  voiceButton.setAttribute(
    "aria-label",
    voiceActive ? "Stop voice input" : "Start voice input",
  );
  if (voiceActive) {
    showToast("Voice input listening");
  } else {
    promptInput.value = `${promptInput.value}${promptInput.value ? " " : ""}Voice transcript ready`;
    sizePrompt();
    showToast("Voice transcript inserted");
  }
}

function reindexQueue() {
  const items = $$(".queue-item", queueItems);
  items.forEach((item, index) => {
    $(".mono", item).textContent = String(index + 1).padStart(2, "0");
  });
  queueCount.textContent = String(items.length);
  if (!items.length) {
    queueItems.innerHTML = '<span class="queue-empty">No queued prompts</span>';
    queueStrip.classList.remove("is-expanded");
    queueSummary.setAttribute("aria-expanded", "false");
  }
}

function toggleQueue() {
  const expanded = queueStrip.classList.toggle("is-expanded");
  queueSummary.setAttribute("aria-expanded", String(expanded));
}

function handleQueueAction(event) {
  const action = event.target.closest("[data-queue-action]");
  const item = event.target.closest("[data-queue-id]");
  if (!action || !item) return;
  const text = $("span:nth-child(2)", item)?.textContent.trim() || "Queued prompt";
  if (action.dataset.queueAction === "send") {
    addUserRow(text);
    showToast("Queued prompt sent now");
  } else {
    showToast("Queued prompt removed");
  }
  item.remove();
  reindexQueue();
}

function positionBackToBottom() {
  if (backToBottom.hidden) return;
  const sticky = stickyHost.getBoundingClientRect();
  const pill = backToBottom.getBoundingClientRect();
  backToBottom.style.left = `${Math.max(10, sticky.right - pill.width)}px`;
  backToBottom.style.top = `${Math.max(10, sticky.top - pill.height - 8)}px`;
}

function updateBackToBottom() {
  const distance =
    threadScroll.scrollHeight - threadScroll.scrollTop - threadScroll.clientHeight;
  backToBottom.hidden = distance < 72;
  if (!backToBottom.hidden) {
    window.requestAnimationFrame(positionBackToBottom);
  }
}

function renderExtensionsPopover() {
  const data = EXTENSION_DATA[extensionTab];
  extensionsTitle.textContent = data.title;
  extensionsSearch.placeholder = `Fuzzy search ${data.title.toLowerCase()}`;
  $$(".extensions-tabs button").forEach((button) => {
    button.classList.toggle("is-active", button.dataset.extensionTab === extensionTab);
  });
  const items = fuzzyRankFields(
    extensionPopoverQuery,
    data.items,
    ([, title, detail, status]) => [title, detail, status],
  );
  extensionsCount.textContent = extensionPopoverQuery.trim()
    ? `${items.length} of ${data.items.length}`
    : data.count;
  if (!items.length) {
    extensionsBody.innerHTML =
      '<div class="extensions-empty">No extensions match this fuzzy search.</div>';
    return;
  }
  extensionsBody.innerHTML = items
    .map(
      ([icon, title, detail, status, action]) => `<div class="extension-row">
        <span class="extension-row-icon"><svg class="icon small" aria-hidden="true"><use href="#i-${icon}"></use></svg></span>
        <span class="extension-row-copy"><strong>${title}</strong><small>${detail}</small></span>
        ${
          action
            ? `<button class="extension-row-action" type="button" data-extension-action="${action === true ? status : action}">${status}</button>`
            : `<span class="extension-row-status${extensionStatusClass(status)}">${status}</span>`
        }
      </div>`,
    )
    .join("");
}

function extensionStatusClass(status) {
  if (["disabled", "disconnected"].includes(status)) return " is-off";
  if (["required", "update", "reload"].some((word) => status.includes(word))) {
    return " is-warn";
  }
  return "";
}

function extensionIsOn(status) {
  return ["live", "enabled"].includes(status);
}

function renderExtensionsManager() {
  const data = EXTENSION_DATA[extensionManagerTab];
  extensionsManagerSearch.placeholder = `Fuzzy search ${data.title.toLowerCase()}`;
  const items = fuzzyRankFields(
    extensionManagerQuery,
    data.items.map((item, index) => ({ item, index })),
    ({ item }) => [item[1], item[2], item[3]],
  );

  $$(".extensions-manager-tabs button").forEach((button) => {
    button.classList.toggle(
      "is-active",
      button.dataset.extensionManagerTab === extensionManagerTab,
    );
  });
  extensionsManagerCount.textContent = `${items.length} of ${data.items.length}`;

  if (!items.length) {
    extensionsManagerList.innerHTML =
      '<div class="extensions-manager-empty">No extensions match this search.</div>';
    return;
  }

  extensionsManagerList.innerHTML = items
    .map(({ item, index }) => {
      const [icon, title, detail, status, action] = item;
      const on = extensionIsOn(status);
      const control = action
        ? `<button class="extension-manager-action" type="button" data-extension-manager-action="${action === true ? status : action}" data-extension-index="${index}">${status}</button>`
        : `<button class="extension-toggle" type="button" role="switch" aria-checked="${on}" aria-label="${on ? "Disable" : "Enable"} ${title}" data-extension-manager-toggle="${index}"></button>`;
      return `<div class="extension-manager-row">
        <span class="extension-row-icon"><svg class="icon" aria-hidden="true"><use href="#i-${icon}"></use></svg></span>
        <span class="extension-row-copy"><strong>${title}</strong><small>${detail}</small></span>
        <span class="extension-manager-status${extensionStatusClass(status)}">${status}</span>
        ${control}
      </div>`;
    })
    .join("");
}

function positionExtensionsPopover(trigger) {
  const rect = trigger.getBoundingClientRect();
  const popover = extensionsPopover.getBoundingClientRect();
  let left = rect.right + 8;
  if (left + popover.width > window.innerWidth - 10) {
    left = rect.left - popover.width - 8;
  }
  const top = Math.min(
    window.innerHeight - popover.height - 10,
    Math.max(10, rect.top - 4),
  );
  extensionsPopover.style.left = `${Math.max(10, left)}px`;
  extensionsPopover.style.top = `${top}px`;
}

function openExtensionsPopover(tab, trigger) {
  setStickyBackgroundOpen(false);
  closePalette();
  closeDrawer();
  closeSessionManager();
  closeExtensionsManager();
  closeCommandSheet();
  closeSessionMenu();
  closeComposerPopovers();
  extensionTab = tab;
  extensionPopoverQuery = "";
  extensionsSearch.value = "";
  extensionsPopover.hidden = false;
  renderExtensionsPopover();
  $$("#extensions-mcp, #extensions-skills, #extensions-plugins").forEach((item) => {
    item.classList.toggle("is-active", item.id === `extensions-${tab}`);
  });
  positionExtensionsPopover(trigger);
}

function closeExtensionsPopover() {
  extensionsPopover.hidden = true;
  $$("#extensions-mcp, #extensions-skills, #extensions-plugins").forEach((item) =>
    item.classList.remove("is-active"),
  );
}

function openExtensionsManager(tab = extensionTab, query = extensionPopoverQuery) {
  closeExtensionsPopover();
  closePalette();
  closeDrawer();
  closeSessionManager();
  closeCommandSheet();
  closeSessionMenu();
  closeComposerPopovers();
  setStickyBackgroundOpen(false);
  extensionManagerTab = tab;
  extensionManagerQuery = query;
  extensionsManagerSearch.value = query;
  extensionsManager.hidden = false;
  renderExtensionsManager();
  window.requestAnimationFrame(() => extensionsManagerSearch.focus());
}

function closeExtensionsManager({ restoreFocus = false } = {}) {
  if (extensionsManager.hidden) return;
  extensionsManager.hidden = true;
  if (restoreFocus) {
    $(`#extensions-${extensionManagerTab}`)?.focus();
  }
}

function setStickyBackgroundOpen(open) {
  if (open) closeExtensionsPopover();
  if (open) closeCommandSheet();
  stickyBackgroundPopover.hidden = !open;
  stickyBackgroundToggle.classList.toggle("is-active", open);
  stickyBackgroundToggle.setAttribute("aria-expanded", String(open));
  if (!open) return;

  const trigger = stickyBackgroundToggle.getBoundingClientRect();
  const popover = stickyBackgroundPopover.getBoundingClientRect();
  const left = Math.min(
    window.innerWidth - popover.width - 10,
    Math.max(10, trigger.right - popover.width),
  );
  const top = Math.max(10, trigger.top - popover.height - 8);
  stickyBackgroundPopover.style.left = `${left}px`;
  stickyBackgroundPopover.style.top = `${top}px`;
}

function sessionInfo(item) {
  return {
    id: item.dataset.session,
    title: $(".run-copy strong", item)?.textContent.trim() || "Session",
  };
}

function openSessionMenu(item, trigger) {
  closeComposerPopovers();
  menuSession = item;
  const info = sessionInfo(item);
  $("#session-menu-title").textContent = info.title;
  $("#session-menu-id").textContent = info.id;
  sessionMenu.hidden = false;

  const rect = trigger.getBoundingClientRect();
  const menuRect = sessionMenu.getBoundingClientRect();
  const left = Math.min(
    window.innerWidth - menuRect.width - 10,
    Math.max(10, rect.right - menuRect.width),
  );
  const below = rect.bottom + 6;
  const top =
    below + menuRect.height > window.innerHeight - 10
      ? rect.top - menuRect.height - 6
      : below;
  sessionMenu.style.left = `${left}px`;
  sessionMenu.style.top = `${Math.max(10, top)}px`;
  $("[data-session-action='copy']", sessionMenu).focus();
}

function closeSessionMenu({ restoreFocus = false } = {}) {
  if (sessionMenu.hidden) return;
  const trigger = menuSession ? $(".run-more", menuSession) : null;
  sessionMenu.hidden = true;
  menuSession = null;
  if (restoreFocus) trigger?.focus();
}

function renderSessionManager() {
  const project = PROJECTS.find((item) => item.id === managerProjectId) || PROJECTS[0];
  const activeSessionId = $(".run-item.is-active")?.dataset.session?.replace("session ", "");

  $$(".project-row[data-project-id]").forEach((row) => {
    row.classList.toggle("is-active", row.dataset.projectId === project.id);
  });

  const sessions = fuzzyRankFields(
    managerQuery,
    project.sessions.filter(
      ([, , , , , status]) => managerFilter === "all" || status === managerFilter,
    ),
    ([id, title, branch]) => [id, title, branch],
  );

  sessionManagerProject.textContent = project.label;
  sessionManagerCount.textContent = `${sessions.length} ${
    sessions.length === 1 ? "session" : "sessions"
  }`;

  if (!sessions.length) {
    sessionManagerList.innerHTML = `<div class="session-manager-empty">No sessions match this project and filter.</div>`;
    return;
  }

  sessionManagerList.innerHTML = sessions
    .map(([id, title, branch, time, messages, status, run]) => {
      const active = id === activeSessionId;
      return `<div class="managed-session${active ? " is-active" : ""}" data-manager-session data-session-id="${id}" data-project-id="${project.id}" data-run-id="${run || ""}">
        <span class="run-signal ${status === "archived" ? "is-idle" : ""}"></span>
        <span class="managed-session-title"><strong>${title}</strong><small>${branch} · session ${id}</small></span>
        <span class="managed-session-meta">${time}</span>
        <span class="managed-session-meta">${messages} msgs</span>
        <button class="managed-session-action" type="button" data-manager-action="${status === "archived" ? "restore" : "archive"}" aria-label="${status === "archived" ? "Restore" : "Archive"} session">
          <svg class="icon tiny" aria-hidden="true"><use href="#i-${status === "archived" ? "arrow-up" : "archive"}"></use></svg>
        </button>
      </div>`;
    })
    .join("");
}

function openSessionManager(projectId = managerProjectId) {
  closeExtensionsPopover();
  closeExtensionsManager();
  closeCommandSheet();
  closePalette();
  closeDrawer();
  closeComposerPopovers();
  closeSessionMenu();
  managerProjectId = projectId;
  sessionManager.hidden = false;
  sessionManagerSearch.value = managerQuery;
  renderSessionManager();
  window.requestAnimationFrame(() => sessionManagerSearch.focus());
}

function closeSessionManager({ restoreFocus = false } = {}) {
  if (sessionManager.hidden) return;
  sessionManager.hidden = true;
  if (restoreFocus) $("#manage-sessions").focus();
}

function updateArchivedSection() {
  const count = archivedList.children.length;
  archivedCount.textContent = String(count);
  archivedSection.hidden = count === 0;
}

async function archiveSession(item) {
  const info = sessionInfo(item);
  const wasActive = item.classList.contains("is-active");
  const height = item.getBoundingClientRect().height;

  if (!prefersReducedMotion()) {
    item.style.overflow = "hidden";
    await animateElement(
      item,
      [
        { height: `${height}px`, opacity: 1, transform: "translateX(0)" },
        { height: "0px", opacity: 0, transform: "translateX(-16px)" },
      ],
      { duration: 240 },
    );
  }

  item.hidden = true;
  item.style.overflow = "";
  item.style.height = "";

  const archived = document.createElement("div");
  archived.className = "archived-item";
  archived.dataset.archivedRun = item.dataset.run;
  archived.innerHTML = `
    <span class="run-signal is-idle"></span>
    <span class="run-copy"><strong></strong><small></small></span>
    <button class="archived-restore" type="button" aria-label="Restore session">
      <svg class="icon tiny" aria-hidden="true"><use href="#i-arrow-up"></use></svg>
    </button>`;
  $("strong", archived).textContent = info.title;
  $("small", archived).textContent = `${info.id} · archived`;
  $(".archived-restore", archived).addEventListener("click", () => restoreSession(archived));
  archivedList.prepend(archived);
  updateArchivedSection();

  if (wasActive) {
    const next = $(".run-item[data-run]:not([hidden])");
    if (next) await switchRun(next);
  }
  showToast(`${info.title} archived`);
}

async function restoreSession(archived) {
  const item = $(`.run-item[data-run="${archived.dataset.archivedRun}"]`);
  if (!item) return;
  const title = sessionInfo(item).title;
  archived.remove();
  updateArchivedSection();
  item.hidden = false;

  if (!prefersReducedMotion()) {
    const height = item.getBoundingClientRect().height;
    item.style.overflow = "hidden";
    await animateElement(
      item,
      [
        { height: "0px", opacity: 0, transform: "translateX(-12px)" },
        { height: `${height}px`, opacity: 1, transform: "translateX(0)" },
      ],
      { duration: 280 },
    );
    item.style.overflow = "";
    item.style.height = "";
  }

  if (!$(".run-item.is-active")) await switchRun(item);
  showToast(`${title} restored`);
}

function closeTopOverlay() {
  if (!commandSheet.hidden) {
    closeCommandSheet({ restoreFocus: true });
    return true;
  }
  if (!extensionsManager.hidden) {
    closeExtensionsManager({ restoreFocus: true });
    return true;
  }
  if (!extensionsPopover.hidden) {
    closeExtensionsPopover();
    return true;
  }
  if (!stickyBackgroundPopover.hidden) {
    setStickyBackgroundOpen(false);
    stickyBackgroundToggle.focus();
    return true;
  }
  if (!sessionManager.hidden) {
    closeSessionManager({ restoreFocus: true });
    return true;
  }
  if (!slashPopup.hidden || !mentionPopup.hidden || !modelMenu.hidden || !permissionMenu.hidden) {
    closeComposerPopovers();
    promptInput.focus();
    return true;
  }
  if (!sessionMenu.hidden) {
    closeSessionMenu({ restoreFocus: true });
    return true;
  }
  if (!palette.hidden) {
    closePalette();
    return true;
  }
  if (!drawer.hidden) {
    closeDrawer();
    return true;
  }
  if (!panelScrim.hidden) {
    if (window.innerWidth <= 860 && railIsOpen()) {
      setRailOpen(false);
    } else {
      setInspectorOpen(false);
    }
    return true;
  }
  return false;
}

$("#theme-toggle").addEventListener("click", toggleTheme);
$("#command-trigger").addEventListener("click", openPalette);
railToggle.addEventListener("click", toggleRail);
inspectorToggle.addEventListener("click", toggleInspector);
stickyBackgroundToggle.addEventListener("click", () =>
  setStickyBackgroundOpen(stickyBackgroundPopover.hidden),
);
$("#close-sticky-background").addEventListener("click", () =>
  setStickyBackgroundOpen(false),
);
$("#review-permission").addEventListener("click", reviewPermission);
threadScroll.addEventListener("scroll", updateBackToBottom, { passive: true });
backToBottom.addEventListener("click", () => {
  threadScroll.scrollTo({
    top: threadScroll.scrollHeight,
    behavior: prefersReducedMotion() ? "auto" : "smooth",
  });
});
window.addEventListener("resize", positionBackToBottom);
new ResizeObserver(positionBackToBottom).observe(stickyHost);
$("#open-background-output").addEventListener("click", () => {
  setStickyBackgroundOpen(false);
  showToast("Background output reader opened");
});
$("#extensions-mcp").addEventListener("click", (event) =>
  openExtensionsPopover("mcp", event.currentTarget),
);
$("#extensions-skills").addEventListener("click", (event) =>
  openExtensionsPopover("skills", event.currentTarget),
);
$("#extensions-plugins").addEventListener("click", (event) =>
  openExtensionsPopover("plugins", event.currentTarget),
);
$("#close-extensions-popover").addEventListener("click", closeExtensionsPopover);
$$(".extensions-tabs button").forEach((button) => {
  button.addEventListener("click", () => {
    extensionTab = button.dataset.extensionTab;
    extensionPopoverQuery = "";
    extensionsSearch.value = "";
    renderExtensionsPopover();
    extensionsSearch.focus();
  });
});
extensionsSearch.addEventListener("input", (event) => {
  extensionPopoverQuery = event.target.value;
  renderExtensionsPopover();
  positionExtensionsPopover(
    $(`#extensions-${extensionTab}`),
  );
});
extensionsBody.addEventListener("click", (event) => {
  const action = event.target.closest("[data-extension-action]");
  if (!action) return;
  showToast(`${action.dataset.extensionAction} · extension action ready`);
});
$("#extensions-manage").addEventListener("click", () =>
  openExtensionsManager(extensionTab, extensionPopoverQuery),
);
$("#close-extensions-manager").addEventListener("click", () =>
  closeExtensionsManager({ restoreFocus: true }),
);
$("#extensions-manager-done").addEventListener("click", () =>
  closeExtensionsManager({ restoreFocus: true }),
);
$$("[data-close='extensions-manager']").forEach((element) => {
  element.addEventListener("click", () =>
    closeExtensionsManager({ restoreFocus: true }),
  );
});
$$(".extensions-manager-tabs button").forEach((button) => {
  button.addEventListener("click", () => {
    extensionManagerTab = button.dataset.extensionManagerTab;
    renderExtensionsManager();
    extensionsManagerSearch.focus();
  });
});
extensionsManagerSearch.addEventListener("input", (event) => {
  extensionManagerQuery = event.target.value;
  renderExtensionsManager();
});
extensionsManagerList.addEventListener("click", (event) => {
  const data = EXTENSION_DATA[extensionManagerTab];
  const action = event.target.closest("[data-extension-manager-action]");
  if (action) {
    const item = data.items[Number(action.dataset.extensionIndex)];
    if (!item) return;
    const actionName = action.dataset.extensionManagerAction;
    item[3] = extensionManagerTab === "mcp" ? "live" : "enabled";
    item[4] = false;
    renderExtensionsManager();
    renderExtensionsPopover();
    showToast(`${item[1]} ${actionName.toLowerCase()} complete`);
    return;
  }

  const toggle = event.target.closest("[data-extension-manager-toggle]");
  if (!toggle) return;
  const item = data.items[Number(toggle.dataset.extensionManagerToggle)];
  if (!item) return;
  const enabled = toggle.getAttribute("aria-checked") !== "true";
  item[3] = enabled
    ? extensionManagerTab === "mcp"
      ? "live"
      : "enabled"
    : extensionManagerTab === "mcp"
      ? "disconnected"
      : "disabled";
  item[4] = false;
  renderExtensionsManager();
  renderExtensionsPopover();
  showToast(`${item[1]} ${enabled ? "enabled" : "disabled"}`);
});
$("#close-command-sheet").addEventListener("click", () =>
  closeCommandSheet({ restoreFocus: true }),
);
$$("[data-close='command-sheet']").forEach((element) => {
  element.addEventListener("click", () =>
    closeCommandSheet({ restoreFocus: true }),
  );
});
commandSheetActions.addEventListener("click", (event) => {
  const action = event.target.closest("[data-command-sheet-action]");
  if (!action) return;
  switch (action.dataset.commandSheetAction) {
    case "done":
      closeCommandSheet({ restoreFocus: true });
      return;
    case "trust-all":
      HOOK_SOURCES.forEach((source) => {
        source.trusted = true;
      });
      openHooksSheet();
      showToast("All hook sources trusted");
      return;
    case "forget-all":
      HOOK_SOURCES.forEach((source) => {
        source.trusted = false;
      });
      openHooksSheet();
      showToast("All hook approvals forgotten");
      return;
    case "plugin-reload":
      showToast("Plugins reloaded");
      openPluginsSheet();
      return;
    case "quit-confirm":
      closeCommandSheet();
      showToast("Window close requested");
      return;
    default:
      return;
  }
});
commandSheetBody.addEventListener("click", (event) => {
  const action = event.target.closest("[data-command-sheet-action]");
  if (!action) return;
  switch (action.dataset.commandSheetAction) {
    case "trust-source": {
      const source = HOOK_SOURCES.find(
        (item) => item.id === action.dataset.sourceId,
      );
      if (source) source.trusted = true;
      openHooksSheet();
      showToast(`${source?.label || "Hook source"} trusted`);
      return;
    }
    case "forget-source": {
      const source = HOOK_SOURCES.find(
        (item) => item.id === action.dataset.sourceId,
      );
      if (source) source.trusted = false;
      openHooksSheet();
      showToast(`${source?.label || "Hook source"} forgotten`);
      return;
    }
    case "skill-toggle": {
      const item = EXTENSION_DATA.skills.items.find(
        ([, title]) => title === action.dataset.skillName,
      );
      if (item) item[3] = item[3] === "enabled" ? "disabled" : "enabled";
      openSkillsSheet();
      showToast(
        `${action.dataset.skillName} ${item?.[3] === "enabled" ? "shown" : "hidden"}`,
      );
      return;
    }
    case "plugin-update": {
      const item = EXTENSION_DATA.plugins.items.find(
        ([, title]) => title === action.dataset.pluginName,
      );
      if (item) {
        item[3] = "enabled";
        item[4] = false;
      }
      openPluginsSheet();
      showToast(`${action.dataset.pluginName} updated`);
      return;
    }
    case "plugin-update-confirm":
      openPluginsSheet();
      showToast(`${action.dataset.pluginName} updated`);
      return;
    case "plugin-uninstall-confirm": {
      const index = EXTENSION_DATA.plugins.items.findIndex(
        ([, title]) => title === action.dataset.pluginName,
      );
      if (index !== -1) EXTENSION_DATA.plugins.items.splice(index, 1);
      EXTENSION_DATA.plugins.count = `${EXTENSION_DATA.plugins.items.length} installed`;
      openPluginsSheet();
      showToast(`${action.dataset.pluginName} uninstalled`);
      return;
    }
    case "mcp-run-prompt":
      openMcpPromptForm(action.dataset.promptName);
      return;
    case "background-output":
      closeCommandSheet();
      showToast(`Background output ${action.dataset.backgroundId} opened`);
      return;
    case "subagent-stop": {
      const agent = SUBAGENT_OPTIONS.find(
        (item) => item.name === action.dataset.agentName,
      );
      if (agent) agent.detail = `${agent.name} · stopping`;
      openSubagentCancelSheet();
      showToast(`Stopping ${action.dataset.agentName}`);
      return;
    }
    case "language-select":
      body.dataset.language = action.dataset.language;
      openLanguageSheet();
      showToast(
        `Interface language: ${action.dataset.language === "zh" ? "中文" : "English"}`,
      );
      return;
    default:
      return;
  }
});
commandSheetBody.addEventListener("submit", (event) => {
  event.preventDefault();
  if (event.target.id === "goal-create-form") {
    const objective = $("#goal-objective", event.target)?.value.trim();
    if (!objective) {
      showToast("Enter a Goal objective");
      return;
    }
    GOAL_OPTIONS.unshift({
      id: Math.random().toString(16).slice(2, 10),
      title: objective.slice(0, 80),
      status: "queued",
      archived: false,
    });
    openGoalListSheet(false);
    showToast("Goal created");
    return;
  }
  if (event.target.id === "mcp-prompt-form") {
    const server = $("#mcp-prompt-server", event.target)?.value;
    const prompt = $("#mcp-prompt-name", event.target)?.value;
    closeCommandSheet();
    showToast(`${server} · ${prompt} submitted as a task`);
    return;
  }
  if (event.target.id === "model-settings-form") {
    const model = $("#main-model-setting", event.target)?.value;
    const effort = $("#model-effort-setting", event.target)?.value;
    closeCommandSheet();
    showToast(`${model} · ${effort} effort applied`);
    return;
  }
  if (event.target.id === "generic-entry-form") {
    const command = commandSheet.dataset.command;
    closeCommandSheet();
    showToast(`${command} applied`);
  }
});
$("#project-switcher").addEventListener("click", () => openSessionManager());
$("#manage-sessions").addEventListener("click", () => openSessionManager());
$("#view-all-sessions").addEventListener("click", () => openSessionManager());
$(".rail-search").addEventListener("click", () => openSessionManager());
$("#close-session-manager").addEventListener("click", () =>
  closeSessionManager({ restoreFocus: true }),
);
panelScrim.addEventListener("click", () => {
  if (window.innerWidth <= 860 && railIsOpen()) {
    setRailOpen(false);
    return;
  }
  setInspectorOpen(false);
});

document.addEventListener("pointerdown", (event) => {
  if (stickyBackgroundPopover.hidden) return;
  if (
    stickyBackgroundPopover.contains(event.target) ||
    event.target.closest("#sticky-background-toggle")
  ) {
    return;
  }
  setStickyBackgroundOpen(false);
});

document.addEventListener("pointerdown", (event) => {
  if (extensionsPopover.hidden) return;
  if (
    extensionsPopover.contains(event.target) ||
    event.target.closest("#extensions-mcp, #extensions-skills, #extensions-plugins")
  ) {
    return;
  }
  closeExtensionsPopover();
});

$$("[data-close='session-manager']").forEach((element) => {
  element.addEventListener("click", () => closeSessionManager({ restoreFocus: true }));
});

sessionManagerSearch.addEventListener("input", (event) => {
  managerQuery = event.target.value;
  renderSessionManager();
});

$$(".session-filters button").forEach((button) => {
  button.addEventListener("click", () => {
    managerFilter = button.dataset.sessionFilter;
    $$(".session-filters button").forEach((item) =>
      item.classList.toggle("is-active", item === button),
    );
    renderSessionManager();
  });
});

$$(".project-row[data-project-id]").forEach((button) => {
  button.addEventListener("click", () => {
    managerProjectId = button.dataset.projectId;
    renderSessionManager();
  });
});

sessionManagerList.addEventListener("click", (event) => {
  const action = event.target.closest("[data-manager-action]");
  const row = event.target.closest("[data-manager-session]");
  if (!row) return;
  const project = PROJECTS.find((item) => item.id === row.dataset.projectId);
  const session = project?.sessions.find((item) => item[0] === row.dataset.sessionId);
  if (!session) return;

  if (action) {
    event.stopPropagation();
    session[5] = action.dataset.managerAction === "archive" ? "archived" : "active";
    renderSessionManager();
    showToast(
      `${session[1]} ${action.dataset.managerAction === "archive" ? "archived" : "restored"}`,
    );
    return;
  }

  const runId = row.dataset.runId;
  closeSessionManager();
  if (runId) {
    const runItem = $(`.run-item[data-run="${runId}"]`);
    if (runItem) {
      switchRun(runItem);
      return;
    }
  }
  showToast(`Opened session ${row.dataset.sessionId} · ${row.dataset.projectId}`);
});

paletteSearch.addEventListener("input", (event) => {
  filterCommands(event.target.value);
});

paletteSearch.addEventListener("keydown", (event) => {
  if (event.key === "ArrowDown") {
    event.preventDefault();
    moveCommand(1);
  }
  if (event.key === "ArrowUp") {
    event.preventDefault();
    moveCommand(-1);
  }
  if (event.key === "Enter") {
    event.preventDefault();
    $(".command-item.is-selected", commandList)?.click();
  }
});

commandList.addEventListener("click", (event) => {
  const item = event.target.closest(".command-item");
  if (!item) return;
  runEntry(item.dataset.command, item.dataset.label);
});

$("#slash-button").addEventListener("click", openSlashPopup);
$("#mention-button").addEventListener("click", openMentionPopup);
$("#attach-button").addEventListener("click", addDemoAttachment);
voiceButton.addEventListener("click", toggleVoiceInput);
attachmentStrip.addEventListener("click", (event) => {
  const remove = event.target.closest("[data-remove-attachment]");
  if (!remove) return;
  attachments = attachments.filter(
    (attachment) => attachment.id !== remove.dataset.removeAttachment,
  );
  renderAttachments();
  showToast("Attachment removed");
});
queueSummary.addEventListener("click", toggleQueue);
queueItems.addEventListener("click", handleQueueAction);
$("#clear-queue").addEventListener("click", () => {
  queueItems.innerHTML = '<span class="queue-empty">No queued prompts</span>';
  queueCount.textContent = "0";
  queueStrip.classList.remove("is-expanded");
  queueSummary.setAttribute("aria-expanded", "false");
  showToast("Queue cleared");
});
$$(".stream-suggestions button").forEach((button) => {
  button.addEventListener("click", () => {
    if (button.textContent.includes("diff")) {
      const toggle = $('[data-toggle="tool-edit-body"]');
      toggle?.click();
      showToast("Opening the bridge diff");
    } else if (button.textContent.includes("subagent")) {
      openDrawer();
    } else {
      showToast("Follow-up added to the composer");
      promptInput.value = "Explain how compaction affects the retained bridge evidence.";
      sizePrompt();
      promptInput.focus();
    }
  });
});
document.addEventListener("mouseup", (event) => {
  if (selectionActions.contains(event.target)) return;
  const selection = window.getSelection();
  if (!selection || selection.isCollapsed || !selection.toString().trim()) {
    selectionActions.hidden = true;
    return;
  }
  const anchor = selection.anchorNode?.parentElement;
  if (!anchor?.closest(".markdown-body")) {
    selectionActions.hidden = true;
    return;
  }
  const rect = selection.getRangeAt(0).getBoundingClientRect();
  selectionActions.style.left = `${Math.min(window.innerWidth - 150, Math.max(8, rect.left + rect.width / 2 - 65))}px`;
  selectionActions.style.top = `${Math.max(8, rect.top - 38)}px`;
  selectionActions.hidden = false;
});
selectionActions.addEventListener("click", (event) => {
  const action = event.target.closest("[data-selection-action]");
  if (!action) return;
  selectionActions.hidden = true;
  showToast(`Selection action: ${action.dataset.selectionAction}`);
});

modelSelect.addEventListener("click", () => openChoiceMenu(modelMenu, modelSelect));
permissionSelect.addEventListener("click", () =>
  openChoiceMenu(permissionMenu, permissionSelect),
);

slashList.addEventListener("click", (event) => {
  const entry = event.target.closest("[data-slash-index]");
  if (!entry) return;
  activateSlashItem(Number(entry.dataset.slashIndex));
});

mentionList.addEventListener("click", (event) => {
  const entry = event.target.closest("[data-mention-index]");
  if (!entry) return;
  activateMentionItem(Number(entry.dataset.mentionIndex));
});

modelMenu.addEventListener("click", (event) => {
  const choice = event.target.closest("[data-model]");
  if (!choice) return;
  $("#model-label").textContent = choice.dataset.model;
  $$(".composer-choice", modelMenu).forEach((item) =>
    item.classList.toggle("is-selected", item === choice),
  );
  closeChoiceMenus();
  showToast(`Main model: ${choice.dataset.model}`);
});

permissionMenu.addEventListener("click", (event) => {
  const choice = event.target.closest("[data-permission]");
  if (!choice) return;
  $("#permission-label").textContent = choice.dataset.permission;
  $$(".composer-choice", permissionMenu).forEach((item) =>
    item.classList.toggle("is-selected", item === choice),
  );
  closeChoiceMenus();
  showToast(`Permission mode: ${choice.dataset.permission}`);
});

document.addEventListener("pointerdown", (event) => {
  if (
    event.target.closest(".composer-zone") ||
    event.target.closest(".composer-choice-menu")
  ) {
    return;
  }
  closeComposerPopovers();
});

$$(".inspector-tabs button").forEach((button) => {
  button.addEventListener("click", () => focusTab(button.dataset.tab));
});

$$(".run-item[data-run]").forEach((item) => {
  $(".run-main", item).addEventListener("click", () => switchRun(item));
  $(".run-more", item).addEventListener("click", (event) => {
    event.stopPropagation();
    if (menuSession === item && !sessionMenu.hidden) {
      closeSessionMenu({ restoreFocus: true });
      return;
    }
    openSessionMenu(item, event.currentTarget);
  });
});

$$("[data-session-action]", sessionMenu).forEach((button) => {
  button.addEventListener("click", async () => {
    const item = menuSession;
    if (!item) return;
    const action = button.dataset.sessionAction;
    if (action === "copy") {
      const id = sessionInfo(item).id;
      try {
        await navigator.clipboard.writeText(id);
        showToast(`Copied ${id}`);
      } catch (_) {
        showToast(id);
      }
      closeSessionMenu({ restoreFocus: true });
      return;
    }
    if (action === "archive") {
      closeSessionMenu();
      await archiveSession(item);
    }
  });
});

document.addEventListener("pointerdown", (event) => {
  if (sessionMenu.hidden) return;
  if (sessionMenu.contains(event.target) || event.target.closest(".run-more")) return;
  closeSessionMenu();
});

$$("[data-focus-tab]").forEach((button) => {
  button.addEventListener("click", () => focusTab(button.dataset.focusTab));
});

$$("[data-toggle]").forEach((button) => {
  button.addEventListener("click", async () => {
    const id = button.dataset.toggle;
    const target = $(`#${id}`);
    if (!target) return;
    const expanded = button.getAttribute("aria-expanded") === "true";
    button.setAttribute("aria-expanded", String(!expanded));
    if (id === "sticky-tasks-body") {
      stickyHost.classList.toggle("is-collapsed", expanded);
      button.setAttribute("aria-label", expanded ? "Expand tasks" : "Collapse tasks");
    }
    await toggleAnimatedBody(button, target, expanded);
  });
});

$$("[data-gate]").forEach((button) => {
  button.addEventListener("click", () => resolveGate(button.dataset.gate));
});

$$("[data-single-option]").forEach((option) => {
  option.addEventListener("click", () => {
    const card = option.closest(".decision-card");
    if (card.classList.contains("is-resolved") || card.classList.contains("is-cancelled")) return;
    $$(".decision-option", card).forEach((item) => {
      const selected = item === option;
      item.classList.toggle("is-selected", selected);
      item.setAttribute("aria-checked", String(selected));
    });
    $("#single-select-state").textContent = `Selected: ${$("strong", option).textContent.trim()}`;
    $("[data-decision-confirm='single']", card).disabled = false;
  });
});

$$(".decision-options.is-single").forEach((group) => {
  group.addEventListener("keydown", (event) => {
    if (!["ArrowDown", "ArrowRight", "ArrowUp", "ArrowLeft"].includes(event.key)) {
      return;
    }
    event.preventDefault();
    const options = $$("[data-single-option]", group);
    const current = options.indexOf(document.activeElement);
    const direction = ["ArrowDown", "ArrowRight"].includes(event.key) ? 1 : -1;
    const next = options[(Math.max(0, current) + direction + options.length) % options.length];
    next?.focus();
    next?.click();
  });
});

$$("[data-multi-option]").forEach((option) => {
  option.addEventListener("click", () => {
    const card = option.closest(".decision-card");
    if (card.classList.contains("is-resolved") || card.classList.contains("is-cancelled")) return;
    const selected = option.classList.toggle("is-selected");
    option.setAttribute("aria-pressed", String(selected));
    const count = $$(".decision-option.is-selected", card).length;
    $("#multi-select-state").textContent = `${count} selected`;
    $("[data-decision-confirm='multi']", card).disabled = count === 0;
  });
});

$$("[data-decision-confirm]").forEach((button) => {
  button.addEventListener("click", () => resolveDecision(button.dataset.decisionConfirm));
});

$$("[data-decision-cancel]").forEach((button) => {
  button.addEventListener("click", () => resolveDecision(button.dataset.decisionCancel, true));
});

$$("[data-diff]").forEach((button) => {
  button.addEventListener("click", () => resolveDiff(button.dataset.diff));
});

$("#open-subagent").addEventListener("click", openDrawer);
$$("[data-open-agent]").forEach((button) => button.addEventListener("click", openDrawer));
$("#close-drawer").addEventListener("click", closeDrawer);
$("#close-drawer-foot").addEventListener("click", closeDrawer);
drawerScrim.addEventListener("click", closeDrawer);

$$("[data-close='palette']").forEach((element) => {
  element.addEventListener("click", closePalette);
});

$("#goal-pause").addEventListener("click", () => {
  const button = $("#goal-pause");
  const paused = button.textContent.includes("Pause");
  button.innerHTML = paused
    ? '<svg class="icon tiny" aria-hidden="true"><use href="#i-play"></use></svg> Resume'
    : '<svg class="icon tiny" aria-hidden="true"><use href="#i-pause"></use></svg> Pause';
  showToast(paused ? "Goal paused" : "Goal resumed");
});

stopButton.addEventListener("click", () => setRunPaused(!runPaused));

composer.addEventListener("submit", (event) => {
  event.preventDefault();
  const text = promptInput.value.trim();
  if (!text) {
    showToast("Write a prompt before sending");
    promptInput.focus();
    return;
  }
  if (executeSlashInput(text)) return;
  addUserRow(text);
  closeComposerPopovers();
  promptInput.value = "";
  sizePrompt();
  showToast("Prompt queued behind the active run");
  promptInput.focus();
});

promptInput.addEventListener("input", () => {
  sizePrompt();
  updateSlashPopup();
  updateMentionPopup();
});
promptInput.addEventListener("keydown", (event) => {
  if (!slashPopup.hidden) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setComposerSelection(slashList, 1);
      return;
    }
    if (event.key === "ArrowUp") {
      event.preventDefault();
      setComposerSelection(slashList, -1);
      return;
    }
    if (event.key === "Enter" || event.key === "Tab") {
      event.preventDefault();
      const selected = $(".composer-entry.is-selected", slashList);
      activateSlashItem(Number(selected?.dataset.slashIndex || 0), {
        completeOnly: event.key === "Tab",
      });
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      closeSlashPopup();
      return;
    }
  }

  if (!mentionPopup.hidden) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setComposerSelection(mentionList, 1);
      return;
    }
    if (event.key === "ArrowUp") {
      event.preventDefault();
      setComposerSelection(mentionList, -1);
      return;
    }
    if (event.key === "Enter" || event.key === "Tab") {
      event.preventDefault();
      const selected = $(".composer-entry.is-selected", mentionList);
      activateMentionItem(Number(selected?.dataset.mentionIndex || 0));
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      closeMentionPopup();
      return;
    }
  }

  if (
    event.key === "Enter" &&
    !event.shiftKey &&
    slashPopup.hidden &&
    promptInput.value.trimStart().startsWith("/")
  ) {
    event.preventDefault();
    executeSlashInput(promptInput.value.trim());
    return;
  }

  if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
    composer.requestSubmit();
  }
});

document.addEventListener("keydown", (event) => {
  if ((event.metaKey || event.ctrlKey) && event.code === "KeyB") {
    event.preventDefault();
    if (event.altKey) {
      toggleInspector();
    } else {
      toggleRail();
    }
    return;
  }

  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
    event.preventDefault();
    palette.hidden ? openPalette() : closePalette();
    return;
  }

  if ((event.metaKey || event.ctrlKey) && /^[1-4]$/.test(event.key)) {
    event.preventDefault();
    const tab = ["run", "goals", "agents", "context"][Number(event.key) - 1];
    focusTab(tab);
    return;
  }

  if (event.key === "Escape") {
    if (closeTopOverlay()) event.preventDefault();
  }
});

const requestedTheme = new URLSearchParams(window.location.search).get("theme");

try {
  const storedTheme = localStorage.getItem("tact-desktop-appearance");
  const legacyTheme = localStorage.getItem("tact-desktop-theme");
  if (requestedTheme === "dark" || requestedTheme === "light") {
    setTheme(requestedTheme);
  } else if (requestedTheme === "graphite" || requestedTheme === "bone") {
    setTheme(requestedTheme === "graphite" ? "dark" : "light");
  } else if (storedTheme === "dark" || storedTheme === "light") {
    setTheme(storedTheme);
  } else if (legacyTheme === "graphite" || legacyTheme === "bone") {
    setTheme(legacyTheme === "graphite" ? "dark" : "light");
  }
} catch (_) {
  // Ignore storage restrictions for local file previews.
}

renderCommandCatalog();
if (window.location.hash === "#palette") openPalette();
if (window.location.hash === "#subagent") openDrawer();
if (window.location.hash === "#sessions") openSessionManager();
if (window.location.hash === "#extensions") openExtensionsManager("mcp");
if (window.location.hash === "#hooks") openHooksSheet();

if (window.innerWidth <= 860) setRailOpen(false);
setInspectorOpen(false);
syncPanelControls();
updatePanelScrim();
window.requestAnimationFrame(() => workbench.classList.remove("is-initializing"));
updateTabIndicator();
let previousWidth = window.innerWidth;
window.addEventListener("resize", () => {
  if (previousWidth > 860 && window.innerWidth <= 860) setRailOpen(false);
  if (previousWidth > 1180 && window.innerWidth <= 1180) setInspectorOpen(false);
  previousWidth = window.innerWidth;
  updateTabIndicator();
  updatePanelScrim();
});
sizePrompt();
window.requestAnimationFrame(updateBackToBottom);
