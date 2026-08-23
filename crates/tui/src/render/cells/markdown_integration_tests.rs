//! App-integration tests for the kit's `MarkdownCell`.
//!
//! These stay in `crates/tui` because they exercise the full `App` pipeline
//! (`make_app`, `handle_agent_update`, log rendering); the cell itself and its
//! pure unit tests moved to `agent_tui_kit::render::cells::markdown`.

use tact_protocol::AgentUpdate;

use crate::{
    render::{
        render_md::format_table_lines,
        test_harness::{make_app, render_log_panel_text},
    },
    widgets::state::LogItemKind,
};

#[test]
fn assistant_history_mermaid_uses_width_aware_cell() {
    let mut app = make_app();
    let md = "```mermaid\nflowchart TD\n  A[This is a long start node] --> B[This is a long end node]\n```";
    app.load_history(vec![tact_llm::Message::new_text(
        tact_llm::Role::Assistant,
        md,
    )]);

    assert_eq!(app.log.items.len(), 1);
    assert!(
        app.log.items[0].markdown_cell.is_some(),
        "assistant history should use the width-aware MarkdownCell"
    );

    let text = render_log_panel_text(&mut app, 40, 20);
    assert!(text.contains('─') || text.contains('│'), "{text}");
    assert!(!text.contains("flowchart TD"), "raw Mermaid leaked: {text}");
}

#[test]
fn md_info_renders_as_one_markdown_cell() {
    let mut app = make_app();
    let md = "# Title\n\n- item one\n- item two\n\n```rust\nfn hi() {}\n```\n";
    app.handle_agent_update(AgentUpdate::MdInfo(md.into()));

    let text = render_log_panel_text(&mut app, 80, 20);
    assert!(text.contains("Title"), "{text}");
    assert!(
        text.contains("item one") && text.contains("item two"),
        "{text}"
    );
    assert!(text.contains("fn hi() {}"), "{text}");
    // One physical message: the markdown is a single cell.
    assert_eq!(
        app.log.items.len(),
        1,
        "MdInfo must append exactly one message"
    );
    assert!(app.log.items[0].markdown_cell.is_some());
    assert_eq!(app.log.items[0].raw, md);
}

#[test]
fn md_info_cell_is_followed_by_normal_message_without_layout_shift() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::MdInfo("# Head\n\nline two\n".into()));
    // A normal message after the markdown cell.
    app.append_msg(
        ratatui::text::Line::from("after markdown"),
        "after markdown".into(),
        LogItemKind::AssistantMarkdown,
    );

    let text = render_log_panel_text(&mut app, 40, 20);
    assert!(text.contains("after markdown"), "{text}");
    assert!(text.contains("Head"), "{text}");
}

#[test]
fn md_info_does_not_break_scroll_positions_of_following_rows() {
    let mut app = make_app();
    // Markdown block whose height is stable across renders.
    app.handle_agent_update(AgentUpdate::MdInfo("# A\n\npara\n".into()));
    app.append_msg(
        ratatui::text::Line::from("tail line"),
        "tail line".into(),
        LogItemKind::AssistantMarkdown,
    );

    // Render twice: the prefix-sum cache must stay consistent so the
    // tail line remains at the same visual offset.
    let t1 = render_log_panel_text(&mut app, 40, 20);
    let t2 = render_log_panel_text(&mut app, 40, 20);
    assert!(t1.contains("tail line"), "{t1}");
    assert!(t2.contains("tail line"), "{t2}");
    let pos1 = t1.lines().position(|l| l.contains("tail line"));
    let pos2 = t2.lines().position(|l| l.contains("tail line"));
    assert_eq!(pos1, pos2, "tail line must not drift: {t1:?} vs {t2:?}");
}

#[test]
fn md_info_skips_mouse_selection() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::MdInfo("# Title\n".into()));
    // Force a selection range over the markdown row.
    app.mouse.log_selection = Some(crate::widgets::state::LogSelection::new(
        crate::widgets::state::TextPosition {
            phys_idx: 0,
            byte_offset: 0,
        },
        crate::widgets::state::TextPosition {
            phys_idx: 0,
            byte_offset: 10,
        },
    ));

    let text = render_log_panel_text(&mut app, 80, 10);
    assert!(
        !text.contains('\u{7f}'),
        "markdown cell must not draw selection overlay (reversed), got:\n{text}"
    );
}

#[test]
fn streamed_table_rows_stay_aligned_after_reply_indent() {
    // 回归：流式表格按 log_scroll.width（含缩进的全内容宽度）布局，
    // 但渲染时 assistant 行缩进 LOG_THINKING_INDENT + 1 = 3 列，实际
    // 可用宽度少 3 —— 长表格行尾 pipe 被裁掉、列看起来错位。
    // `table_layout_width` 在布局时扣掉缩进，表格行永不超渲染宽度。
    use crate::render::test_harness::render_log_panel_terminal;
    use crate::widgets::state::LogItemKind;

    let md = "| 编号 | 问题描述 | 影响范围 | 处理建议 |\n|-----:|:---------|:---------|:---------|\n| 1 | 当用户连续快速点击「保存」按钮超过五次时，系统会偶发出现重复提交，导致数据库中产生两条内容完全一致但主键不同的记录 | 涉及所有使用表单保存功能的页面，包括用户管理、订单管理、商品管理、配置管理四个模块 | 在前端增加防抖与提交锁，后端在事务中增加唯一性约束校验，并对历史重复数据执行清理脚本 |";

    for width in [40u16, 60, 80] {
        let mut app = make_app();
        // First render sets log_scroll.width, as in real usage before
        // streaming table rows arrive.
        let _first = render_log_panel_terminal(&mut app, width, 5);
        let (styled, raw) = format_table_lines(
            &md.lines().map(|s| s.to_string()).collect::<Vec<_>>(),
            &app.theme,
            Some(app.table_layout_width()),
        );
        for (s, r) in styled.into_iter().zip(raw) {
            app.append_msg(s, r, LogItemKind::AssistantMarkdown);
        }
        let height = app.log.items.len() as u16 + 2;
        let terminal = render_log_panel_terminal(&mut app, width, height);
        let buf = terminal.backend().buffer();

        // Group pipe cells by row using real buffer coordinates.
        let mut rows: Vec<Vec<u16>> = Vec::new();
        for y in 0..buf.area.height {
            let xs: Vec<u16> = (0..buf.area.width)
                .filter(|&x| buf[(x, y)].symbol() == "|")
                .collect();
            if !xs.is_empty() {
                rows.push(xs);
            }
        }
        assert!(!rows.is_empty(), "expected table rows at width {width}");
        // Rows sharing a pipe pattern (same block / column count) must
        // have identical pipe columns.
        for w in rows.windows(2) {
            if w[0].len() == w[1].len() {
                assert_eq!(w[0], w[1], "same-block pipes misaligned at width {width}");
            }
        }
        // Every table row keeps its trailing pipe (no right clipping).
        assert!(
            rows.iter().all(|xs| xs.len() >= 2),
            "trailing pipe clipped at width {width}: {rows:?}"
        );
        // Every pipe column must sit inside the rendered content area
        // (panel width minus right border).
        for xs in &rows {
            for &x in xs {
                assert!(x < width, "pipe beyond panel at width {width}: {rows:?}");
            }
        }
    }
}
