export interface Project {
  id: string | null;
  root: string | null;
  name: string;
  branch: string | null;
  repository: string | null;
  available: boolean;
}

export interface SessionSummary {
  id: string;
  project_id: string;
  title: string;
  updated_at: string;
  archived: boolean;
  locked: boolean;
  message_count: number;
}

export interface SessionList {
  filter: string;
  query: string;
  selected: string | null;
  loading: boolean;
  items: SessionSummary[];
}

export interface SessionView {
  id: string | null;
  title: string;
  phase: string;
  locked: boolean;
}

export interface Usage {
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
  cache_hit_tokens: number;
  reasoning_tokens: number;
  context_window: number | null;
}

export interface Runtime {
  phase: string;
  elapsed_seconds: number;
  turns: number;
  model: string | null;
  usage: Usage;
  latest_tool: string | null;
  latest_tool_row: number | null;
}

export type ToolBody =
  | { kind: "none" }
  | { kind: "text"; text: string }
  | { kind: "code"; language: string | null; text: string }
  | { kind: "diff"; path: string | null; patch: string };

export interface Gate {
  id: number;
  session_epoch: number;
  request_id: number;
  kind: string;
  prompt: string;
  options: string[];
  selected: number[];
  status: string;
}

export interface Tool {
  id: number;
  tool_id: string;
  name: string;
  arg_summary: string;
  status: string;
  body: ToolBody;
  output_preview: string;
  output_detail: string | null;
  expanded: boolean;
  duration_ms: number | null;
  model: string | null;
  task_id: string | null;
  subagent_id: string | null;
  subagent: Row[] | null;
}

export type ToolItem =
  | { kind: "tool"; tool: Tool }
  | { kind: "gate"; gate: Gate };

export interface PlanStep {
  label: string;
  status: string;
}

export type Body =
  | { kind: "user"; text: string; attachments: string[] }
  | { kind: "assistant"; markdown: string; status: string; suggestions: string[] }
  | { kind: "thinking"; markdown: string; status: string; duration_ms: number | null }
  | { kind: "plan"; title: string; steps: PlanStep[]; tool_execution: boolean }
  | { kind: "tool_group"; summary: string; collapsed: boolean; items: ToolItem[] }
  | { kind: "diff"; path: string | null; patch: string; status: string }
  | { kind: "gate"; gate: Gate }
  | { kind: "notice"; text: string }
  | { kind: "hook_context"; source: string | null; markdown: string }
  | { kind: "hook_status"; message: string }
  | { kind: "error"; message: string; retryable: boolean };

export interface Row {
  id: number;
  ordinal: number;
  time: string | null;
  body: Body;
}

export interface Queued {
  index: number;
  text: string;
}

export interface CommandInfo {
  path: string[];
  title: string;
  description: string;
  needs_arguments: boolean;
}

export interface ComposerState {
  commands: CommandInfo[];
  model: string;
  models: string[];
  subagent_model: string;
  subagent_models: string[];
  effort: string | null;
  permission: string;
  thinking_budget: number | null;
  queue: Queued[];
  history: string[];
}

export interface Goal {
  id: string;
  title: string;
  objective: string;
  status: string;
  phase: string;
  task_total: number;
  task_completed: number;
  wait_reason: string | null;
  archived: boolean;
}

export interface Task {
  id: number;
  subject: string;
  status: string;
  owner: string;
  started_at: number | null;
  completed_at: number | null;
}

export interface Subagent {
  child_id: string;
  status: string;
  summary: string;
  model: string | null;
  elapsed_ms: number;
  last_activity: string | null;
}

export interface BackgroundTask {
  id: string;
  command: string;
  status: string;
  started_at: number;
  output_tail: string;
}

export interface InspectorState {
  tab: string;
  goals: Goal[];
  tasks: Task[];
  subagents: Subagent[];
  background: BackgroundTask[];
  context: Usage;
}

export interface Skill {
  name: string;
  description: string;
  enabled: boolean;
}

export interface Plugin {
  name: string;
  marketplace: string;
  version: string | null;
  enabled: boolean;
}

export interface Marketplace {
  name: string;
  source: string;
}

export interface Extensions {
  tab: string;
  skills: Skill[];
  plugins: Plugin[];
  marketplaces: Marketplace[];
}

export interface Settings {
  provider: string;
  api_key_set: boolean;
  base_url: string;
  max_tokens: number | null;
  context_window: number | null;
  snapshot_max_items: number | null;
  micro_compact: boolean;
  tokio_console: boolean;
  skill_body_auto_inject: boolean;
  config_path: string | null;
}

export type Account =
  | { kind: "balance"; currency: string; total: string }
  | { kind: "usage_quota"; plan: string | null; used: number | null; limit: number | null }
  | { kind: "error"; message: string };

export interface Snapshot {
  revision: number;
  voice_enabled: boolean;
  voice_missing_api_key: boolean;
  appearance: "light" | "dark";
  language: "en" | "zh";
  text_scale: number;
  tail_following: boolean;
  show_hook_output: boolean;
  notifications: boolean;
  project: Project;
  projects: Project[];
  sessions: SessionList;
  session: SessionView;
  runtime: Runtime;
  rows: Row[];
  composer: ComposerState;
  inspector: InspectorState;
  pending_gates: Gate[];
  settings: Settings;
  extensions: Extensions;
  surface: { title: string; body: string } | null;
  account: Account | null;
  failure: string | null;
}

export interface TrajectorySummary {
  turns: number;
  records: number;
  tool_calls: number;
  measured_ms: number;
  measured: number;
  input_tokens: number;
  output_tokens: number;
  cache_hit_tokens: number;
  cache_miss_tokens: number;
  context_window: number | null;
  context_used: number;
}

export interface TrajectorySpan {
  index: number;
  lane: string;
  is_error: boolean;
}

export interface TrajectoryRecord {
  index: number;
  turn: number;
  step: number;
  kind: string;
  text: string;
  tool_name: string | null;
  at: string | null;
  status: string;
  status_label: string;
}

export interface TrajectoryState {
  summary: TrajectorySummary;
  spans: TrajectorySpan[];
  records: TrajectoryRecord[];
}

export interface TrajectoryDetail {
  index: number;
  text: string;
  kind: string;
  status: string;
  status_label: string;
  tool_name: string | null;
  at: string | null;
  input: string | null;
  output: string | null;
  duration_ms: number | null;
  usage: Usage | null;
}

export type ServerMessage =
  | Snapshot
  | { type: "reader"; title: string; markdown: string }
  | { type: "toast"; title: string; body: string }
  | { type: "voice"; event: string; text: string };

export function isReader(message: ServerMessage): message is { type: "reader"; title: string; markdown: string } {
  return "type" in message && message.type === "reader";
}

export function isToast(message: ServerMessage): message is { type: "toast"; title: string; body: string } {
  return "type" in message && message.type === "toast";
}

export function isVoice(message: ServerMessage): message is { type: "voice"; event: string; text: string } {
  return "type" in message && message.type === "voice";
}
