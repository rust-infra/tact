use tact_kernel::tool::{IntoToolCallResult, ToolCallResult, ToolEffect, ToolImage, YieldReason};

#[test]
fn text_result_has_no_image_or_effects() {
    let result = ToolCallResult::text("ok");
    assert_eq!(result.content, "ok");
    assert!(result.image.is_none());
    assert!(result.effects.is_empty());
}

#[test]
fn text_image_round_trips_owned_image() {
    let image = ToolImage {
        mime_type: "image/png".to_string(),
        data: "AA==".to_string(),
    };
    let result = ToolCallResult::text_image("ok", image.clone());
    assert_eq!(result.image, Some(image));
}

#[test]
fn yield_turn_carries_a_generic_reason() {
    let effect = ToolEffect::YieldTurn {
        reason: YieldReason::NeedsUser,
    };
    assert_eq!(
        effect,
        ToolEffect::YieldTurn {
            reason: YieldReason::NeedsUser
        }
    );
}

#[test]
fn string_converts_to_effect_free_result() {
    assert_eq!("ok".into_tool_call_result().content, "ok");
}
