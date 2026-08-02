# DeepSeek Responses Protocol Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the DeepSeek provider use `protocol = "responses"` by relaxing config validation and routing DeepSeek to the existing generic Responses adapter.

**Architecture:** Two code changes — `resolve_llm` accepts Responses for `openai` and `deepseek`; `ProviderInfo::build_client` matches on `protocol` in the DeepSeek branch so Responses builds `LlmProvider::OpenAiResponses` against the DeepSeek base URL. The adapter, compaction, conversation state, and error paths are reused unchanged; docs and a live smoke test complete the work.

**Tech Stack:** Rust workspace (`crates/tact`, `crates/tact_llm`), `async-openai-responses` adapter, TOML config, bilingual book docs, `cargo test`.

## Global Constraints

- Per `AGENTS.md`: do **not** commit unless the user explicitly asks; each task ends by staging (`git add`) and reporting.
- Per `AGENTS.md`: never run two cargo build/test/clippy processes in parallel (shared `target/` lock); run one command and wait for it to exit.
- `protocol = "responses"` is valid only for `openai` and `deepseek`; Kimi and Anthropic keep rejecting it.
- The `reasoning_effort` config field stays OpenAI-only; `thinking_budget` still maps to effort automatically on the Responses path.
- No chat-completions fallback if the DeepSeek `/responses` endpoint is incompatible.
- Bilingual files stay structurally aligned: update `book/21_chapter_config.md` + `_zh.md` and `book/26_chapter_issue.md` + `_zh.md` in the same task.
- Never print or log the DeepSeek API key; the smoke test reads it from the environment and reports only status and reply text.
- Spec of record: `docs/superpowers/specs/2026-08-02-deepseek-responses-design.md`.

---

### Task 1: Config validation accepts DeepSeek Responses

**Files:**
- Modify: `crates/tact/src/config/resolve.rs:238-243` (validation)
- Modify: `crates/tact/src/config/resolve.rs:991-1010` (replace `reject_responses_protocol_for_non_openai_provider`)

**Interfaces:**
- Consumes: existing `resolve_config(&CliArgs, &TactTomlConfig, Option<PathBuf>) -> anyhow::Result<ResolvedConfig>` and the `empty_cli_args()` test helper (both already in the test module).
- Produces: `resolve_llm` accepts `protocol = "responses"` when `provider` is `OpenAi` or `DeepSeek`; Anthropic/Kimi get error `protocol 'responses' is only supported for provider 'openai' or 'deepseek'`.

- [ ] **Step 1: Replace the existing rejection test with three tests**

Delete `reject_responses_protocol_for_non_openai_provider` and add these three tests in its place (same `#[cfg(test)] mod tests` in `crates/tact/src/config/resolve.rs`):

```rust
    #[test]
    fn deepseek_responses_protocol_resolves() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "deepseek"

[llm.providers.deepseek]
api_key = "sk-test"
model = "deepseek-chat"
protocol = "responses"
"#,
        )
        .unwrap();

        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.llm.provider, ProviderKind::DeepSeek);
        assert_eq!(resolved.llm.protocol, tact_llm::OpenAiProtocol::Responses);
    }

    #[test]
    fn reject_responses_protocol_for_anthropic() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "anthropic"

[llm.providers.anthropic]
api_key = "sk-test"
model = "claude-sonnet-4-20250514"
base_url = "https://api.anthropic.com"
protocol = "responses"
"#,
        )
        .unwrap();

        let error = resolve_config(&empty_cli_args(), &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("only supported for provider 'openai' or 'deepseek'"));
    }

    #[test]
    fn reject_responses_protocol_for_kimi() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "kimi"

[llm.providers.kimi]
api_key = "sk-test"
model = "kimi-for-coding"
protocol = "responses"
"#,
        )
        .unwrap();

        let error = resolve_config(&empty_cli_args(), &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("only supported for provider 'openai' or 'deepseek'"));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tact --lib config::resolve`

Expected: `deepseek_responses_protocol_resolves` FAILS with `protocol 'responses' is only supported for provider 'openai'`; `reject_responses_protocol_for_anthropic` and `reject_responses_protocol_for_kimi` FAIL because the current message does not contain `'openai' or 'deepseek'`. Wait for the command to exit before starting any other cargo command.

- [ ] **Step 3: Update the validation code**

In `crates/tact/src/config/resolve.rs`, replace:

```rust
    if protocol == OpenAiProtocol::Responses && provider != ProviderKind::OpenAi {
        anyhow::bail!("protocol 'responses' is only supported for provider 'openai'");
    }
```

with:

```rust
    if protocol == OpenAiProtocol::Responses
        && provider != ProviderKind::OpenAi
        && provider != ProviderKind::DeepSeek
    {
        anyhow::bail!("protocol 'responses' is only supported for provider 'openai' or 'deepseek'");
    }
```

- [ ] **Step 4: Re-run the tests to verify they pass**

Run: `cargo test -p tact --lib config::resolve`

Expected: all three new tests PASS; no other resolve tests regress.

- [ ] **Step 5: Stage the change (no commit without user approval)**

Run: `git add crates/tact/src/config/resolve.rs`

---

### Task 2: Client routing — DeepSeek + Responses builds the Responses adapter

**Files:**
- Modify: `crates/tact_llm/src/provider.rs:43-55` (`build_client`)
- Modify: `crates/tact_llm/src/provider.rs` tests (after `deepseek_builds_deepseek_adapter_with_default_base_url`)

**Interfaces:**
- Consumes: `ProviderInfo { provider, protocol, api_key, base_url, model, reasoning_effort, responses_compact_threshold }` and the existing `build_openai_responses()` method.
- Produces: `build_client()` returns `LlmProvider::OpenAiResponses(OpenAiResponsesAdapter)` for `provider == DeepSeek && protocol == Responses`; `adapter.base_url()` is the DeepSeek base URL (`https://api.deepseek.com` default).

- [ ] **Step 1: Add the failing test**

In `crates/tact_llm/src/provider.rs` test module, directly after `deepseek_builds_deepseek_adapter_with_default_base_url`, add:

```rust
    #[test]
    fn deepseek_responses_protocol_builds_responses_adapter() {
        let mut p = provider_info(ProviderKind::DeepSeek, "sk-test", "", "deepseek-v4-flash");
        p.protocol = OpenAiProtocol::Responses;
        let result = p.build_client();
        assert!(result.is_ok());
        let LlmProvider::OpenAiResponses(adapter) = result.unwrap() else {
            panic!("expected OpenAI Responses adapter for deepseek responses");
        };
        assert_eq!(adapter.base_url(), "https://api.deepseek.com");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p tact_llm --lib provider::tests::deepseek -- --nocapture`

Expected: the new test FAILS with `panic! "expected OpenAI Responses adapter for deepseek responses"` (current DeepSeek branch always returns `LlmProvider::DeepSeek`); `deepseek_builds_deepseek_adapter_with_default_base_url` still PASSES. Wait for exit before any other cargo command.

- [ ] **Step 3: Implement the routing**

In `crates/tact_llm/src/provider.rs`, replace the `build_client` match:

```rust
        match self.provider {
            ProviderKind::Anthropic => self.build_anthropic(),
            ProviderKind::DeepSeek => self.build_deepseek(),
            ProviderKind::Kimi => self.build_kimi(),
            ProviderKind::OpenAi => match self.protocol {
                OpenAiProtocol::ChatCompletions => self.build_openai_compatible(),
                OpenAiProtocol::Responses => self.build_openai_responses(),
            },
        }
```

with:

```rust
        match self.provider {
            ProviderKind::Anthropic => self.build_anthropic(),
            ProviderKind::DeepSeek => match self.protocol {
                OpenAiProtocol::ChatCompletions => self.build_deepseek(),
                OpenAiProtocol::Responses => self.build_openai_responses(),
            },
            ProviderKind::Kimi => self.build_kimi(),
            ProviderKind::OpenAi => match self.protocol {
                OpenAiProtocol::ChatCompletions => self.build_openai_compatible(),
                OpenAiProtocol::Responses => self.build_openai_responses(),
            },
        }
```

`build_openai_responses()` already falls back to `ProviderKind::DeepSeek::default_base_url()` (`https://api.deepseek.com`) when `base_url` is empty, so no other change is needed.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tact_llm --lib provider::tests`

Expected: all provider tests PASS — DeepSeek default still builds `DeepSeekAdapter`, DeepSeek + Responses builds `OpenAiResponses` with `https://api.deepseek.com`, and the OpenAI/Kimi/Anthropic cases are unchanged.

- [ ] **Step 5: Stage the change**

Run: `git add crates/tact_llm/src/provider.rs`

---

### Task 3: Docs sync (config example, book 21, Ch 26)

**Files:**
- Modify: `config.example.toml:58-63` (DeepSeek entry)
- Modify: `book/21_chapter_config.md:169-172` (protocol paragraph)
- Modify: `book/21_chapter_config_zh.md:160-163` (protocol paragraph)
- Modify: `book/26_chapter_issue.md` (insert newest entry at the top of the entries list, before the first `## 1. 2026-08-01` section)
- Modify: `book/26_chapter_issue_zh.md` (same entry, Chinese, same position)

**Interfaces:** none — docs only.

- [ ] **Step 1: Update the DeepSeek entry in `config.example.toml`**

Replace:

```toml
[llm.providers.deepseek]
api_key = "sk-..."
model = "deepseek-chat"
# base_url defaults to https://api.deepseek.com
```

with:

```toml
[llm.providers.deepseek]
api_key = "sk-..."
model = "deepseek-chat"
# base_url defaults to https://api.deepseek.com
# Optional protocol: omit or set "chat_completions" (default) for the DeepSeek
# Chat Completions API; "responses" talks to {base_url}/responses with the same
# Responses semantics as the openai provider (native compaction, reasoning effort).
```

- [ ] **Step 2: Update the English protocol paragraph in `book/21_chapter_config.md`**

Replace:

```markdown
Optional `protocol` defaults to `chat_completions`. `responses` is valid only
for the `openai` provider; configuration resolution rejects it for Anthropic,
DeepSeek, or Kimi. There is no CLI override for this field.
```

with:

```markdown
Optional `protocol` defaults to `chat_completions`. `responses` is valid for
the `openai` and `deepseek` providers; configuration resolution rejects it for
Anthropic or Kimi. DeepSeek with `responses` uses the same Responses adapter as
OpenAI against its configured `base_url` (native compaction and reasoning
effort included). There is no CLI override for this field.
```

- [ ] **Step 3: Update the Chinese protocol paragraph in `book/21_chapter_config_zh.md`**

Replace:

```markdown
可选 `protocol` 默认为 `chat_completions`。`responses` 仅对 `openai` provider 有效；配置 resolve 会拒绝 Anthropic、DeepSeek 或 Kimi 使用该值。此字段没有 CLI override。
```

with:

```markdown
可选 `protocol` 默认为 `chat_completions`。`responses` 对 `openai` 与 `deepseek` provider 有效；配置 resolve 会拒绝 Anthropic 或 Kimi 使用该值。DeepSeek 配 `responses` 时复用与 OpenAI 相同的 Responses 适配器，指向其配置的 `base_url`（含原生压缩与 reasoning effort）。此字段没有 CLI override。
```

- [ ] **Step 4: Append the Ch 26 entry (English)**

Insert at the very top of the entries list in `book/26_chapter_issue.md` (before the existing `## 1. 2026-08-01 — Responses compact threshold now reaches ordinary ...` section):

```markdown
## 1. 2026-08-02 — DeepSeek can now use the OpenAI Responses protocol

| Field | Value |
|-------|-------|
| Type | `feature` |
| Related | Ch 21, Ch 5 |
| Symptom / motivation | `protocol = "responses"` was rejected for every non-OpenAI provider, so DeepSeek was pinned to Chat Completions even though the Responses adapter is endpoint-agnostic and the DeepSeek endpoint can serve `/responses`. |
| Decision | Accept `responses` for the DeepSeek provider in `resolve_llm` and route `ProviderInfo::build_client()` by protocol: DeepSeek + `chat_completions` keeps the dedicated `DeepSeekAdapter`; DeepSeek + `responses` builds the same generic `OpenAiResponsesAdapter` used by OpenAI, pointed at the DeepSeek `base_url`. All Responses features apply unchanged — `context_management` compaction, `reasoning.effort` from `thinking_budget`, and Responses conversation-state continuation. Kimi and Anthropic still reject `responses`. |
| Behavior after | A DeepSeek entry may set `protocol = "responses"`; requests go to `{base_url}/responses` with full Responses semantics. The default remains `chat_completions`. There is no chat-completions fallback if the endpoint is incompatible. |
| Pointers | `crates/tact/src/config/resolve.rs` (`resolve_llm` validation); `crates/tact_llm/src/provider.rs` (`build_client`); `docs/superpowers/specs/2026-08-02-deepseek-responses-design.md`; `docs/superpowers/plans/2026-08-02-deepseek-responses.md`; Ch 21 (config), Ch 5 (compaction) |
```

- [ ] **Step 5: Append the Ch 26 entry (Chinese)**

Insert at the very top of the entries list in `book/26_chapter_issue_zh.md` (before the existing `## 1. 2026-08-01 — Responses 压缩阈值现在会进入普通 ...` section):

```markdown
## 1. 2026-08-02 — DeepSeek 现在可以使用 OpenAI Responses 协议

| 字段 | 值 |
|------|------|
| 类型 | `feature` |
| 相关 | 第 21、5 章 |
| 现象 / 动机 | `protocol = "responses"` 对除 OpenAI 外的所有 provider 一律拒绝，DeepSeek 因此被钉死在 Chat Completions，尽管 Responses 适配器本身与端点无关，DeepSeek 端点可以服务 `/responses`。 |
| 决策 | 在 `resolve_llm` 中接受 DeepSeek 使用 `responses`，并让 `ProviderInfo::build_client()` 按 protocol 路由：DeepSeek + `chat_completions` 继续使用专用 `DeepSeekAdapter`；DeepSeek + `responses` 构建与 OpenAI 相同的通用 `OpenAiResponsesAdapter`，指向 DeepSeek `base_url`。所有 Responses 能力原样生效——`context_management` 压缩、由 `thinking_budget` 派生的 `reasoning.effort`、Responses 会话状态续传。Kimi 与 Anthropic 仍拒绝 `responses`。 |
| 改后行为 | DeepSeek 条目可设置 `protocol = "responses"`；请求发往 `{base_url}/responses`，具备完整 Responses 语义。默认仍为 `chat_completions`。端点不兼容时不做 chat-completions 回退。 |
| 指针 | `crates/tact/src/config/resolve.rs`（`resolve_llm` 校验）；`crates/tact_llm/src/provider.rs`（`build_client`）；`docs/superpowers/specs/2026-08-02-deepseek-responses-design.md`；`docs/superpowers/plans/2026-08-02-deepseek-responses.md`；第 21 章（配置）、第 5 章（压缩） |
```

- [ ] **Step 6: Verify structural alignment**

Run: `rg -n "2026-08-02|responses.*deepseek|deepseek.*responses" config.example.toml book/21_chapter_config.md book/21_chapter_config_zh.md book/26_chapter_issue.md book/26_chapter_issue_zh.md`

Expected: the new protocol wording appears in both book 21 files; the `2026-08-02` entry is the first entry in both Ch 26 files.

- [ ] **Step 7: Stage the changes**

Run: `git add config.example.toml book/21_chapter_config.md book/21_chapter_config_zh.md book/26_chapter_issue.md book/26_chapter_issue_zh.md`

---

### Task 4: Live smoke test + user config restore

**Files:**
- Create: `crates/tact_llm/src/test_deepseek_responses.rs`
- Modify: `crates/tact_llm/src/lib.rs:23-26` (register module)
- Modify (user machine, needs approval): `/Users/rg/.tact/config.toml`

**Interfaces:**
- Consumes: `ProviderInfo::build_client`, `LlmClient::create_message`, `CreateMessageParams`, `RequiredMessageParams`, `Message::new_text`, `ContentBlock` (all re-exported at the `tact_llm` crate root).
- Produces: `#[ignore]`d live test `deepseek_responses_smoke` proving the configured DeepSeek endpoint accepts `/responses`; user config restored to `protocol = "responses"`.

- [ ] **Step 1: Create the live smoke test**

Create `crates/tact_llm/src/test_deepseek_responses.rs`:

```rust
//! Live DeepSeek Responses-protocol smoke test.
//!
//! Verifies that `protocol = "responses"` on the DeepSeek provider routes to
//! the generic Responses adapter and that the configured endpoint actually
//! accepts `/responses` requests.
//!
//! Skips when `DEEPSEEK_API_KEY` is unset or empty.
//! Optional: `DEEPSEEK_BASE_URL` (default `https://api.deepseek.com`),
//! `DEEPSEEK_MODEL` (default `deepseek-v4-flash`).
//!
//!   cargo test -p tact_llm deepseek_responses_smoke -- --ignored --nocapture

use crate::{
    ContentBlock, CreateMessageParams, LlmClient, Message, OpenAiProtocol, ProviderInfo,
    ProviderKind, RequiredMessageParams, Role,
};

#[tokio::test]
#[ignore]
async fn deepseek_responses_smoke() {
    dotenvy::dotenv().ok();
    let api_key = match std::env::var("DEEPSEEK_API_KEY") {
        Ok(k) if !k.trim().is_empty() => k,
        _ => {
            eprintln!("skipping: DEEPSEEK_API_KEY not set");
            return;
        }
    };
    let base_url = std::env::var("DEEPSEEK_BASE_URL")
        .unwrap_or_else(|_| "https://api.deepseek.com".to_string());
    let model =
        std::env::var("DEEPSEEK_MODEL").unwrap_or_else(|_| "deepseek-v4-flash".to_string());

    let provider = ProviderInfo {
        provider: ProviderKind::DeepSeek,
        protocol: OpenAiProtocol::Responses,
        reasoning_effort: None,
        responses_compact_threshold: None,
        api_key,
        base_url: base_url.clone(),
        model: model.clone(),
    };
    let client = provider.build_client().expect("build DeepSeek Responses client");

    let request = CreateMessageParams::new(RequiredMessageParams {
        model,
        max_tokens: 64,
        messages: vec![Message::new_text(Role::User, "Reply with exactly: pong")],
    });

    let response = client
        .create_message(&request, None)
        .await
        .expect("DeepSeek /responses request succeeded");

    let text = response
        .blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        text.to_ascii_lowercase().contains("pong"),
        "unexpected DeepSeek Responses reply: {text:?}"
    );
}
```

- [ ] **Step 2: Register the module**

In `crates/tact_llm/src/lib.rs`, add this line right after `#[cfg(test)] mod test_deepseek_reasoning;`:

```rust
#[cfg(test)]
mod test_deepseek_responses;
```

- [ ] **Step 3: Compile-check the test without running it**

Run: `cargo test -p tact_llm --lib --no-run`

Expected: compiles cleanly (the smoke test is `#[ignore]`d, so it is not executed). Wait for exit before any other cargo command.

- [ ] **Step 4: Run the live smoke test against the real DeepSeek endpoint**

The test reads `DEEPSEEK_API_KEY` from the environment. Export it from the user config **without printing it**, e.g.:

```bash
export DEEPSEEK_API_KEY="$(perl -ne 'if (/^\[llm\.providers\.deepseek\]$/) {$in=1} elsif (/^\[/) {$in=0} print $1 if $in && /^api_key = "(.*)"$/ and $1 ne "sk-..."' /Users/rg/.tact/config.toml)"
```

Then run (requires network approval — this sends one tiny `/responses` request consuming a small number of tokens):

Run: `cargo test -p tact_llm deepseek_responses_smoke -- --ignored --nocapture`

Expected: PASS with the reply containing `pong` (proves the DeepSeek `/responses` endpoint is compatible). If the endpoint rejects `/responses`, capture the exact `LlmError`/HTTP error and report it — do not add a fallback.

- [ ] **Step 5: Restore `protocol = "responses"` in the user config**

Build the restored file from the current config (section-aware insertion after the DeepSeek `thinking_budget` line), verify, then copy it over (copy, not in-place perl, to avoid the earlier line-duplication issue):

```bash
awk '/^\[llm\.providers\.deepseek\]$/ {insec=1} /^\[/ && !/^\[llm\.providers\.deepseek\]$/ {insec=0} {print} insec && /^thinking_budget = 64000$/ && !done {print "protocol = \"responses\""; done=1}' /Users/rg/.tact/config.toml > /tmp/tact.config.restored.toml
```

Verify the diff shows exactly one added line under `[llm.providers.deepseek]`:

Run: `diff /Users/rg/.tact/config.toml /tmp/tact.config.restored.toml`

Then (requires approval — writes outside the workspace):

Run: `cp /tmp/tact.config.restored.toml /Users/rg/.tact/config.toml`

Expected: `~/.tact/config.toml` has `protocol = "responses"` only under `[llm.providers.deepseek]` and `[llm.providers.openai]`; verify with `rg -n "protocol" /Users/rg/.tact/config.toml`.

- [ ] **Step 6: Final regression run (sequential, one cargo command at a time)**

Run: `cargo test -p tact --lib config::resolve`

Expected: PASS.

Run: `cargo test -p tact_llm --lib provider::tests`

Expected: PASS.

- [ ] **Step 7: Stage the changes**

Run: `git add crates/tact_llm/src/test_deepseek_responses.rs crates/tact_llm/src/lib.rs`

- [ ] **Step 8: Report**

Summarize: the two code changes, the smoke-test outcome (including the raw endpoint verdict), the docs touched, and the restored user config. Ask the user whether to commit (per `AGENTS.md`, no commit without explicit approval).
