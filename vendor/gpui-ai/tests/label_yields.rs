//! The rule that a row's label yields before its value does.
//!
//! A row that states something and its value has one part that can be
//! shortened without losing the point and one that cannot. A heading cut
//! short still reads; a count, a number, or the only control in a header
//! does not survive being pushed out of the row. Before this rule neither
//! part could shrink, so a long label pushed the other one past the row's
//! edge, where it was clipped — the class of defect the 0.5.0 review found
//! in a table's sort glyph, measured 245px outside its own header cell.

use gpui::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, div, px,
};
use gpui_ai::prelude::{TodoItem, TodoList, TodoStatus};

/// A to-do list whose title cannot fit beside its count.
struct SqueezedProbe;

impl Render for SqueezedProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            // Narrow on purpose, and narrower than the title alone wants.
            .w(px(220.))
            .debug_selector(|| "squeezed-frame".to_owned())
            .child(
                TodoList::new("squeezed")
                    .title("A plan title far longer than this column can show")
                    .items([
                        TodoItem::new("a", "First").status(TodoStatus::Done),
                        TodoItem::new("b", "Second"),
                    ]),
            )
    }
}

/// A squeezed header keeps its count and shortens its title.
///
/// The count is the header's whole reason to exist — it is the only thing
/// that says how far the list has got. A title is free to be as long as the
/// application likes, so the title is what gives way.
#[gpui::test]
fn a_squeezed_todo_header_keeps_its_count(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (_, cx) = cx.add_window_view(|_, _| SqueezedProbe);
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let frame = cx
        .debug_bounds("squeezed-frame")
        .expect("the probe frame renders");
    let count = cx
        .debug_bounds("todo-list-count")
        .expect("the header renders its count");

    assert!(
        count.size.width > px(0.),
        "the count must occupy real width rather than collapse to nothing"
    );
    assert!(
        count.origin.x + count.size.width <= frame.origin.x + frame.size.width,
        "the count must stay inside the row: it ends at {}, the row ends at {}",
        count.origin.x + count.size.width,
        frame.origin.x + frame.size.width
    );
}
