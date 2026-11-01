use gpui::Styled;
use gpui_component::theme::TextStyleToken;

pub(crate) trait SemanticStyledExt: Styled + Sized {
    fn text_token(self, token: TextStyleToken) -> Self {
        self.text_size(token.size)
            .line_height(token.line_height)
            .font_weight(token.weight)
    }
}

impl<T: Styled + Sized> SemanticStyledExt for T {}

#[cfg(test)]
mod tests {
    use super::SemanticStyledExt as _;
    use gpui::{FontWeight, StyleRefinement, px};
    use gpui_component::theme::TextStyleToken;

    #[test]
    fn text_token_applies_the_complete_typography_role() {
        let token = TextStyleToken {
            size: px(15.),
            line_height: px(22.),
            weight: FontWeight::SEMIBOLD,
        };
        let style = StyleRefinement::default().text_token(token);
        assert_eq!(style.text.font_size, Some(token.size.into()));
        assert_eq!(style.text.line_height, Some(token.line_height.into()));
        assert_eq!(style.text.font_weight, Some(token.weight));
    }
}
