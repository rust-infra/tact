//! Internationalization module — supports English and Chinese toggling,
//! consistent with the theme switching mechanism.

/// Language enum.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Language {
    English,
    Chinese,
}

impl Language {
    fn all() -> &'static [Language] {
        &[Language::English, Language::Chinese]
    }

    /// Cycle to the next language.
    pub fn next(self) -> Self {
        let all = Self::all();
        let idx = all.iter().position(|t| *t == self).unwrap();
        all[(idx + 1) % all.len()]
    }

    /// Returns the display label for the language.
    pub fn label(self) -> &'static str {
        match self {
            Language::English => "EN",
            Language::Chinese => "中文",
        }
    }
}

/// All translatable UI strings.
///
/// Naming conventions:
///   `_tmpl` suffix = template strings with `{}` placeholders, filled via `format!()`.
///   Others are static text.
///   `_pl` suffix = strings involving plural forms (can generally be ignored in Chinese).
#[allow(non_snake_case)]
pub struct Messages {
    // ---- 面板标题 ----
    pub log_title: &'static str,
    pub thinking_card_title: &'static str, // "🧠 Thinking ({} line{})"
    pub thinking_card_title_pl: &'static str, // "s" / "" for plural
    pub thinking_card_bottom: &'static str, // "↕ {}/{} lines | Click for full content"
    pub diff_card_title: &'static str,     // "+{} {}"
    pub diff_card_bottom: &'static str,    // "Double-click for full code"
    pub code_card_bottom: &'static str,    // " Click for full code "
    pub diff_overflow_tmpl: &'static str,  // "... and {} more lines ..."
    pub plan_title: &'static str,
    pub palette_title: &'static str,
    pub command_title: &'static str,
    pub input_box_title: &'static str,
    pub history_title: &'static str,
    pub help_title: &'static str,
    pub thinking_popup_title: &'static str,
    pub diff_popup_title: &'static str, // "{}" (file path)

    // ---- 状态栏 ----
    pub mode_normal: &'static str,
    pub mode_insert: &'static str,
    pub mode_search: &'static str,
    pub mode_palette: &'static str,
    pub mode_select: &'static str,
    pub focus_plan: &'static str,
    pub focus_log: &'static str,
    pub theme_dark: &'static str,
    pub theme_light: &'static str,
    pub theme_solarized_dark: &'static str,
    pub theme_solarized_light: &'static str,
    pub theme_gruvbox_dark: &'static str,
    pub theme_nord: &'static str,
    pub theme_retro: &'static str,
    pub theme_kawaii: &'static str,
    pub theme_japanese: &'static str,
    pub status_idle_tmpl: &'static str,
    pub status_planning: &'static str,
    pub status_executing_tmpl: &'static str,
    pub status_waiting_user_tmpl: &'static str,
    pub status_done_tmpl: &'static str,
    pub status_party_tmpl: &'static str,

    // ---- 底部栏 ----
    pub bottom_focus_log_plan: &'static str, // "🐬 Plan" / "🐬 Log"
    pub bottom_focus_log: &'static str,
    pub bottom_tips_log: &'static str,
    pub bottom_tips_plan: &'static str,
    pub bottom_branch_unknown: &'static str,
    pub bottom_model_unknown: &'static str,
    pub bottom_top_tmpl: &'static str,
    pub bottom_cache_hit: &'static str,
    pub bottom_mid_tmpl: &'static str,
    pub bottom_balance_ok: &'static str,
    pub bottom_balance_err: &'static str,
    pub bottom_balance_tmpl: &'static str,

    // ---- 弹窗通用 ----
    pub popup_copy_hint: &'static str,
    pub popup_close_hint: &'static str,
    pub popup_scroll_hint: &'static str,
    pub palette_empty: &'static str,
    pub select_empty: &'static str,
    pub select_arrow: &'static str,

    // ---- 帮助面板 ---
    pub help_header_shortcuts: &'static str,
    pub help_normal_header: &'static str,
    pub help_tab: &'static str,
    pub help_e: &'static str,
    pub help_jk: &'static str,
    pub help_gg: &'static str,
    pub help_G: &'static str,
    pub help_y: &'static str,
    pub help_t: &'static str,
    pub help_slash: &'static str,
    pub help_nN: &'static str,
    pub help_colon: &'static str,
    pub help_insert_header: &'static str,
    pub help_type_task: &'static str,
    pub help_ctrl_z: &'static str,
    pub help_global_header: &'static str,
    pub help_yn: &'static str,
    pub help_ctrl_h: &'static str,
    pub help_ctrl_t: &'static str,
    pub help_ctrl_l: &'static str,
    pub help_ctrl_qmark: &'static str,
    pub help_q: &'static str,
    pub help_mouse_header: &'static str,
    pub help_click_drag: &'static str,
    pub help_scroll: &'static str,
    pub help_y_copy: &'static str,

    // ---- 剪贴板 ----
    pub copied_tmpl: &'static str,
    pub copied_terminal_tmpl: &'static str,
    pub copied_internal_tmpl: &'static str,

    // ---- 用户操作反馈 ----
    pub step_rejected: &'static str,
    pub step_approved: &'static str,
    pub approval_cancelled: &'static str,
    pub approval_banner_tmpl: &'static str,
    pub approval_banner_keys: &'static str,
    pub no_options: &'static str,
    pub selected_tmpl: &'static str,
    pub selection_cancelled: &'static str,
    pub log_saved_tmpl: &'static str,
    pub log_save_failed: &'static str,

    // ---- 命令面板描述 ----
    pub cmd_theme: &'static str,
    pub cmd_save: &'static str,
    pub cmd_cancel: &'static str,
    pub cmd_quit: &'static str,
    pub cmd_help: &'static str,
    pub cmd_history: &'static str,
    pub cmd_search: &'static str,
    pub cmd_balance: &'static str,
    pub cmd_lang: &'static str,

    // ---- 系统消息 ----
    pub plan_generated_tmpl: &'static str,
    pub plan_step_tmpl: &'static str,
    pub step_started_tmpl: &'static str,
    pub step_success_prefix: &'static str,
    pub step_fail_prefix: &'static str,
    pub step_finished_simple_tmpl: &'static str,
    pub step_finished_args_tmpl: &'static str,
    pub step_bytes_tmpl: &'static str,
    pub step_ms_tmpl: &'static str,
    pub step_sec_tmpl: &'static str,
    pub step_failed_tmpl: &'static str,
    pub need_approval_tmpl: &'static str,
    pub error_tmpl: &'static str,
    pub thinking_title: &'static str,
    pub thinking_line_prefix: &'static str,
    pub user_msg_prefix: &'static str,
    pub user_msg_cont: &'static str,
    pub theme_changed_tmpl: &'static str,
    pub lang_changed_tmpl: &'static str,

    // ---- 启动/退出 ----
    pub startup_welcome: &'static str,
    pub startup_mode_hint: &'static str,
    pub exit_bye: &'static str,

    // ---- 派对模式 ----
    pub party_msg_1: &'static str,
    pub party_msg_2: &'static str,
    pub party_msg_3: &'static str,
    pub party_hint: &'static str,
    pub party_exit: &'static str,

    // ---- 其他 ----
    pub scroll_indicator_tmpl: &'static str,
}

impl Messages {
    /// Returns the string set for the given language.
    pub fn by_language(lang: Language) -> Self {
        match lang {
            Language::English => Self::english(),
            Language::Chinese => Self::chinese(),
        }
    }

    fn english() -> Self {
        Self {
            log_title: " [Log] ",
            thinking_card_title: " 🧠 Thinking ({} line{}) ",
            thinking_card_title_pl: "s",
            thinking_card_bottom: " ↕ {}/{} lines | Click for full content ",
            diff_card_title: " +{} {} ",
            diff_card_bottom: " Double-click for full code ",
            code_card_bottom: " Click for full code ",
            diff_overflow_tmpl: " ... and {} more lines ...",
            plan_title: " 🔥 [Execution Plan] ",
            palette_title: " Palette /{} ",
            command_title: " Command ",
            input_box_title: " Input (Shift/Alt+Enter=newline) ",
            history_title: " Task History (Enter to retry, Esc to close) ",
            help_title: " Help (Esc to close) ",
            thinking_popup_title: " (╭ರ_•́) Thinking ",
            diff_popup_title: " {} ",

            mode_normal: " NORMAL ",
            mode_insert: " INSERT ",
            mode_search: " SEARCH ",
            mode_palette: " PALETTE ",
            mode_select: " SELECT ",
            focus_plan: "[Plan]",
            focus_log: "[Log]",
            theme_dark: "Dark",
            theme_light: "Light",
            theme_solarized_dark: "SolarizedDark",
            theme_solarized_light: "SolarizedLight",
            theme_gruvbox_dark: "GruvboxDark",
            theme_nord: "Nord",
            theme_retro: "Retro",
            theme_kawaii: "Kawaii",
            theme_japanese: "Wa",
            status_idle_tmpl: "{} {} | [Tab] Focus | [Ctrl+H] Hist | [Ctrl+T] {} | [Ctrl+L] {} | [Ctrl+?] Help | [q] Quit",
            status_planning: "Planning...",
            status_executing_tmpl: "Executing step {}/{}",
            status_waiting_user_tmpl: "{} {} | ⚠️ {} (Enter/Esc)",
            status_done_tmpl: "{} {} | ✅ Task completed",
            status_party_tmpl: "🎉 PARTY MODE 🎉 | {}",

            bottom_focus_log_plan: "🐬 Plan",
            bottom_focus_log: "🐬 Log",
            bottom_tips_log: "[j/k scroll] [g/G top/bottom] [y copy] [Y copy code] [e toggle plan]",
            bottom_tips_plan: "[j/k move] [y copy] [e toggle plan]",
            bottom_branch_unknown: "unknown",
            bottom_model_unknown: "-",
            bottom_top_tmpl: "Focus:{} | {} | {}",
            bottom_cache_hit: " | 💾 cache:{}",
            bottom_mid_tmpl: " {} | Tok:{}(p{})+{}(c){} | Cost:{} | Up:{}",
            bottom_balance_ok: "✅",
            bottom_balance_err: "❌",
            bottom_balance_tmpl: " 💰 Balance:{} {}",

            popup_copy_hint: " [y] Copy ",
            popup_close_hint: " [Esc] Close ",
            popup_scroll_hint: " [j/k] Scroll ",
            palette_empty: "No matching commands",
            select_empty: "No options",
            select_arrow: "▶ ",

            help_header_shortcuts: "Keyboard Shortcuts:",
            help_normal_header: "  Normal Mode (Esc from Insert)",
            help_tab: "    Tab         Switch panel focus (Log/Plan)",
            help_e: "    e           Toggle Execution Plan panel",
            help_jk: "    j/k         Scroll log / move plan selection",
            help_gg: "    gg          Go to top of log",
            help_G: "    G           Go to bottom of log",
            help_y: "    y           Copy selected to clipboard",
            help_t: "    t           Open/close thinking card popup",
            help_slash: "    /           Search in log (/<term> Enter, n/N navigate)",
            help_nN: "    n/N         Next/previous search match",
            help_colon: "    :           Command palette (fuzzy filter & execute)",
            help_insert_header: "  Insert Mode (i or Enter from Normal)",
            help_type_task: "    Type task, Enter to submit",
            help_ctrl_z: "    Ctrl+Z/Y    Undo/redo input",
            help_global_header: "  Global",
            help_yn: "    Enter/Esc   Approve/reject step (y/n also work)",
            help_ctrl_h: "    Ctrl+H      Show history",
            help_ctrl_t: "    Ctrl+T      Toggle theme",
            help_ctrl_l: "    Ctrl+L      Toggle language",
            help_ctrl_qmark: "    Ctrl+?      This help",
            help_q: "    q           Quit",
            help_mouse_header: "  Mouse",
            help_click_drag: "    Click/Drag     Select (2x:word, 3x:line)",
            help_scroll: "    Scroll wheel   Scroll panel",
            help_y_copy: "    y              Copy selection to clipboard",

            copied_tmpl: "📋 Copied: {}",
            copied_terminal_tmpl: "📋 Copied to terminal clipboard: {}",
            copied_internal_tmpl: "📋 Copied to internal buffer (clipboard unavailable): {}",

            step_rejected: "✗ Step rejected",
            step_approved: "✓ Step approved",
            approval_cancelled: "⚠️  Previous approval cancelled by new task",
            approval_banner_tmpl: " ⚠️  APPROVAL: {} ",
            approval_banner_keys: " [Enter] Approve   [Esc] Reject   (y/n also work) ",
            no_options: "⚠ No options available",
            selected_tmpl: "✓ Selected: {}",
            selection_cancelled: "✗ Selection cancelled",
            log_saved_tmpl: "Log saved to {}",
            log_save_failed: "Failed to save log",

            cmd_theme: "Toggle color theme",
            cmd_save: "Save log to file",
            cmd_cancel: "Cancel current task",
            cmd_quit: "Quit application",
            cmd_help: "Show help panel",
            cmd_history: "Show task history",
            cmd_search: "Search log messages",
            cmd_balance: "Query account balance (DeepSeek)",
            cmd_lang: "Toggle language (EN/中文)",

            plan_generated_tmpl: "Generated {} steps:",
            plan_step_tmpl: "  {}. {}",
            step_started_tmpl: "▶ Executing: {}",
            step_success_prefix: "✓",
            step_fail_prefix: "✗",
            step_finished_simple_tmpl: "{} Step {}: {}",
            step_finished_args_tmpl: "{} Step {}: {}({})",
            step_bytes_tmpl: " [{}B]",
            step_ms_tmpl: " [{}ms]",
            step_sec_tmpl: " [{}s]",
            step_failed_tmpl: "✗ Step {} failed: {}",
            need_approval_tmpl: "⚠️  Need approval: {} (press Enter/Esc)",
            error_tmpl: "❌ Error: {}",
            thinking_title: "(╭ರ_•́) Thinking...",
            thinking_line_prefix: "│ {}",
            user_msg_prefix: "💬 {}",
            user_msg_cont: "  {}",
            theme_changed_tmpl: "🎨 Theme: {}",
            lang_changed_tmpl: "🌐 Language: {}",

            startup_welcome: "Agent TUI started. Press 'i' for insert mode, ':' for commands, '/' for search.",
            startup_mode_hint: "Current mode: Insert. Type a task and press Enter. Shift+Enter for new line.",
            exit_bye: "Bye! 🔔",

            party_msg_1: "  ✨  Hey there! ✨",
            party_msg_2: "  You're doing great!",
            party_msg_3: "  This cat believes in you 🐱",
            party_hint: "  (Press Konami or \":party\" to toggle)",
            party_exit: "👋 Party's over! But remember: you're still awesome.",

            scroll_indicator_tmpl: "↕ {}/{} ",
        }
    }

    fn chinese() -> Self {
        Self {
            log_title: " [日志] ",
            thinking_card_title: " 🧠 思考中 ({} 行) ",
            thinking_card_title_pl: "", // Chinese has no plural form
            thinking_card_bottom: " ↕ {}/{} 行 | 点击查看完整内容 ",
            diff_card_title: " +{} {} ",
            diff_card_bottom: " 双击查看完整代码 ",
            code_card_bottom: " 点击查看完整代码 ",
            diff_overflow_tmpl: " ... 还有 {} 行 ...",
            plan_title: " 🔥 [执行计划] ",
            palette_title: " 命令面板 /{} ",
            command_title: " 命令 ",
            input_box_title: " 输入 (Shift/Alt+Enter 换行) ",
            history_title: " 任务历史 (Enter 重试, Esc 关闭) ",
            help_title: " 帮助 (Esc 关闭) ",
            thinking_popup_title: " (╭ರ_•́) 思考 ",
            diff_popup_title: " {} ",

            mode_normal: " 普通 ",
            mode_insert: " 插入 ",
            mode_search: " 搜索 ",
            mode_palette: " 面板 ",
            mode_select: " 选择 ",
            focus_plan: "[计划]",
            focus_log: "[日志]",
            theme_dark: "暗色",
            theme_light: "亮色",
            theme_solarized_dark: "Solarized暗",
            theme_solarized_light: "Solarized亮",
            theme_gruvbox_dark: "Gruvbox暗",
            theme_nord: "Nord",
            theme_retro: "复古",
            theme_kawaii: "可爱",
            theme_japanese: "和風",
            status_idle_tmpl: "{} {} | [Tab] 切换焦点 | [Ctrl+H] 历史 | [Ctrl+T] {} | [Ctrl+L] {} | [Ctrl+?] 帮助 | [q] 退出",
            status_planning: "规划中...",
            status_executing_tmpl: "正在执行步骤 {}/{}",
            status_waiting_user_tmpl: "{} {} | ⚠️ {} (Enter/Esc)",
            status_done_tmpl: "{} {} | ✅ 任务完成",
            status_party_tmpl: "🎉 派对模式 🎉 | {}",

            bottom_focus_log_plan: "🐬 计划",
            bottom_focus_log: "🐬 日志",
            bottom_tips_log: "[j/k 滚动] [g/G 顶部/底部] [y 复制] [Y 复制代码] [e 切换计划]",
            bottom_tips_plan: "[j/k 移动] [y 复制] [e 切换计划]",
            bottom_branch_unknown: "未知",
            bottom_model_unknown: "-",
            bottom_top_tmpl: "焦点:{} | {} | {}",
            bottom_cache_hit: " | 💾 缓存命中:{}",
            bottom_mid_tmpl: " {} | 令牌:{}(输入{})+{}(输出){} | 耗时:{} | 运行:{}",
            bottom_balance_ok: "✅",
            bottom_balance_err: "❌",
            bottom_balance_tmpl: " 💰 余额:{} {}",

            popup_copy_hint: " [y] 复制 ",
            popup_close_hint: " [Esc] 关闭 ",
            popup_scroll_hint: " [j/k] 滚动 ",
            palette_empty: "没有匹配的命令",
            select_empty: "无选项",
            select_arrow: "▶ ",

            help_header_shortcuts: "键盘快捷键:",
            help_normal_header: "  普通模式 (在插入模式按 Esc)",
            help_tab: "    Tab         切换焦点面板 (日志/计划)",
            help_e: "    e           切换执行计划面板",
            help_jk: "    j/k         滚动日志 / 移动计划选择",
            help_gg: "    gg          跳到日志顶部",
            help_G: "    G           跳到日志底部",
            help_y: "    y           复制选中内容到剪贴板",
            help_t: "    t           打开/关闭思考卡片弹窗",
            help_slash: "    /           搜索日志 (/关键词 Enter, n/N 导航)",
            help_nN: "    n/N         下一个/上一个搜索结果",
            help_colon: "    :           命令面板 (模糊过滤并执行)",
            help_insert_header: "  插入模式 (在普通模式按 i 或 Enter)",
            help_type_task: "    输入任务，按 Enter 提交",
            help_ctrl_z: "    Ctrl+Z/Y    撤销/重做输入",
            help_global_header: "  全局",
            help_yn: "    Enter/Esc   批准/拒绝步骤 (y/n 也可)",
            help_ctrl_h: "    Ctrl+H      显示历史",
            help_ctrl_t: "    Ctrl+T      切换主题",
            help_ctrl_l: "    Ctrl+L      切换语言",
            help_ctrl_qmark: "    Ctrl+?      显示帮助",
            help_q: "    q           退出",
            help_mouse_header: "  鼠标",
            help_click_drag: "    双击/三击    选词/选行, 拖拽选择",
            help_scroll: "    滚轮         滚动面板",
            help_y_copy: "    y            复制选中内容到剪贴板",

            copied_tmpl: "📋 已复制: {}",
            copied_terminal_tmpl: "📋 已复制到终端剪贴板: {}",
            copied_internal_tmpl: "📋 已复制到内部缓冲区 (剪贴板不可用): {}",

            step_rejected: "✗ 步骤已拒绝",
            step_approved: "✓ 步骤已批准",
            approval_cancelled: "⚠️  新任务已取消之前的审批",
            approval_banner_tmpl: " ⚠️  需要审批: {} ",
            approval_banner_keys: " [Enter] 批准   [Esc] 拒绝   (y/n 也可) ",
            no_options: "⚠ 无可用选项",
            selected_tmpl: "✓ 已选择: {}",
            selection_cancelled: "✗ 选择已取消",
            log_saved_tmpl: "日志已保存到 {}",
            log_save_failed: "保存日志失败",

            cmd_theme: "切换颜色主题",
            cmd_save: "保存日志到文件",
            cmd_cancel: "取消当前任务",
            cmd_quit: "退出应用",
            cmd_help: "显示帮助面板",
            cmd_history: "显示任务历史",
            cmd_search: "搜索日志消息",
            cmd_balance: "查询账户余额 (DeepSeek)",
            cmd_lang: "切换语言 (EN/中文)",

            plan_generated_tmpl: "生成了 {} 个步骤:",
            plan_step_tmpl: "  {}. {}",
            step_started_tmpl: "▶ 正在执行: {}",
            step_success_prefix: "✓",
            step_fail_prefix: "✗",
            step_finished_simple_tmpl: "{} 步骤 {}: {}",
            step_finished_args_tmpl: "{} 步骤 {}: {}({})",
            step_bytes_tmpl: " [{}字节]",
            step_ms_tmpl: " [{}毫秒]",
            step_sec_tmpl: " [{}秒]",
            step_failed_tmpl: "✗ 步骤 {} 失败: {}",
            need_approval_tmpl: "⚠️  需要审批: {} (按 Enter/Esc)",
            error_tmpl: "❌ 错误: {}",
            thinking_title: "(╭ರ_•́) 思考中...",
            thinking_line_prefix: "│ {}",
            user_msg_prefix: "💬 {}",
            user_msg_cont: "  {}",
            theme_changed_tmpl: "🎨 主题: {}",
            lang_changed_tmpl: "🌐 语言: {}",

            startup_welcome: "Agent TUI 已启动。按 'i' 进入插入模式, ':' 打开命令面板, '/' 搜索。",
            startup_mode_hint: "当前模式: 插入。输入任务并按 Enter 提交。Shift+Enter 换行。",
            exit_bye: "再见! 🔔",

            party_msg_1: "  ✨  你好呀! ✨",
            party_msg_2: "  你做得很棒!",
            party_msg_3: "  这只猫相信你 🐱",
            party_hint: "  (按 Konami 码或 \":party\" 切换)",
            party_exit: "👋 派对结束了! 但记住: 你依然很棒。",

            scroll_indicator_tmpl: "↕ {}/{} ",
        }
    }
}
