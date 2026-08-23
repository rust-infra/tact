use anyhow::{Result, anyhow};
use base64::Engine as _;
use image::GenericImageView;
use schemars::JsonSchema;
use serde::Deserialize;
use tact_protocol::ToolVisualKind;
use tool_refactor_macros::tool;

use crate::tool::{
    ArgumentSummaryPolicy, DetailPolicy, LiveOutputPolicy, OutputPolicy, PermissionPolicy,
    PermissionPromptPolicy, PopupPolicy, ResourcePolicy, ToolCallResult, ToolContext, ToolDomain,
    ToolMetadata, ToolPresentation, safe_path,
};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadImageInput {
    #[schemars(description = "Path to the image file, relative to the current workspace.")]
    pub file_path: String,
}

pub const READ_IMAGE_METADATA: ToolMetadata = ToolMetadata {
    name: "read_image",
    description: "Read a PNG/JPEG/WebP/GIF file and return the image itself to a vision model.",
    permission: PermissionPolicy::Read,
    permission_prompt: PermissionPromptPolicy::Path { field: "file_path" },
    resources: ResourcePolicy::ReadPath { field: "file_path" },
    domain: ToolDomain::Generic,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::FileRead,
        display_name: "🌄 Read Image",
        live_output: LiveOutputPolicy::Standard,
        detail: DetailPolicy::Result,
        popup: PopupPolicy::None,
        compact_result_to_meta: false,
    },
    output: OutputPolicy::KeepInline,
    argument_summary: ArgumentSummaryPolicy::Path { field: "file_path" },
};

fn is_supported_image_ext(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tiff" | "tif"
    )
}

fn encode_jpeg(
    bytes: &[u8],
    max_edge: u32,
    jpeg_quality: u8,
) -> anyhow::Result<(Vec<u8>, u32, u32)> {
    let img = image::load_from_memory(bytes).map_err(|e| anyhow!("invalid image bytes: {e}"))?;
    let (w, h) = img.dimensions();
    let long_edge = w.max(h);
    let img = if long_edge > max_edge {
        let scale = max_edge as f32 / long_edge as f32;
        let nw = ((w as f32 * scale).round() as u32).max(1);
        let nh = ((h as f32 * scale).round() as u32).max(1);
        img.resize(nw, nh, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgb = img.to_rgb8();
    let mut out = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut out);
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, jpeg_quality);
    encoder
        .encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| anyhow!("jpeg encode failed: {e}"))?;
    Ok((out, rgb.width(), rgb.height()))
}

#[tool]
/// # Errors
///
/// Returns an error if:
/// - The file path is invalid or outside the workspace.
/// - The file does not exist or is not a supported image type.
/// - The current model does not accept image input.
pub async fn read_image(ctx: ToolContext, input: ReadImageInput) -> Result<ToolCallResult> {
    // Align with DeepSeek Harness `read_image`: gate on the current model
    // declaring image support before doing any I/O; a text-only target cannot
    // consume the returned image block.
    if !tact_llm::supports_vision() {
        return Err(anyhow!(
            "read_image: the current model does not accept image input; switch to an image-capable model to read images"
        ));
    }

    let path = safe_path(&ctx.work_dir, &input.file_path)?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !is_supported_image_ext(&ext) {
        return Err(anyhow!(
            "read_image: {ext} files are not supported; pass a PNG/JPEG/WebP/GIF path"
        ));
    }

    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| anyhow!("failed to read {}: {e}", path.display()))?;

    let settings = crate::config::settings().ui.vision_image;
    let (encoded, width, height) = encode_jpeg(&bytes, settings.max_edge, settings.jpeg_quality)?;

    let b64 = base64::engine::general_purpose::STANDARD.encode(&encoded);
    let media_type = "image/jpeg".to_string();

    // Text envelope mirrors Harness `formatImageReadOutput`: the model sees a
    // stable description; the image rides the adjacent `Image` block.
    let envelope = format!(
        "<path>{}</path>\n<type>image</type>\n<content>\n{} image, {}x{} px, {} bytes\n</content>",
        path.display(),
        media_type,
        width,
        height,
        encoded.len(),
    );

    Ok(ToolCallResult::text_image(
        envelope,
        tact_llm::ImageSource {
            type_: "base64".to_string(),
            media_type,
            data: b64,
        },
    ))
}

#[cfg(test)]
mod tests {
    use image::{ImageBuffer, Rgb};

    use super::*;
    use crate::tool::test_support::{run_tool_result, test_context};

    fn ensure_config() {
        if crate::config::try_settings().is_some() {
            // Another test already installed a config; reuse it so we do not
            // poison a global Once in parallel runs.
            return;
        }
        let config = crate::config::ResolvedConfig {
            llm: crate::config::LlmSettings {
                provider: tact_llm::ProviderKind::OpenAi,
                protocol: tact_llm::OpenAiProtocol::default(),
                reasoning_effort: None,
                api_key: String::new(),
                base_url: String::new(),
                model: "mock-model".to_string(),
                models: Vec::new(),
                model_profiles: Default::default(),
                responses_compact_threshold: None,
            },
            agent: crate::config::AgentSettings {
                model: "mock-model".to_string(),
                reasoning_effort: None,
                model_context_window: 500_000,
                max_tokens: 8192,
                thinking_budget: 0,
                snapshot_max_items: 80,
                notifications_enabled: false,
                micro_compact_enabled: true,
                skill_body_auto_inject: false,
                skill_dirs: Vec::new(),
                instruction_sources: crate::config::InstructionSources::default(),
                subagent: None,
            },
            ui: crate::config::UiSettings {
                theme: "retro".to_string(),
                vision_image: crate::config::VisionImageSettings {
                    compress: crate::config::VisionImageSettings::DEFAULT_COMPRESS,
                    max_edge: crate::config::VisionImageSettings::DEFAULT_MAX_EDGE,
                    jpeg_quality: crate::config::VisionImageSettings::DEFAULT_JPEG_QUALITY,
                },
            },
            tools: crate::config::ToolSettings {
                bash_timeout_secs: crate::config::ToolSettings::DEFAULT_BASH_TIMEOUT_SECS,
                bash_nice: crate::config::ToolSettings::DEFAULT_BASH_NICE,
                rtk_filter: false,
            },
            voice: crate::config::VoiceSettings::disabled_defaults(),
            permission_mode: None,
            tokio_console: false,
            config_path: None,
        };
        crate::config::install(config);
    }

    fn write_png(work_dir: &std::path::Path, name: &str) {
        let img: ImageBuffer<Rgb<u8>, Vec<u8>> =
            ImageBuffer::from_fn(6, 6, |_, _| Rgb([120, 80, 200]));
        let mut bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .expect("png");
        std::fs::write(work_dir.join(name), bytes).expect("write png");
    }

    #[tokio::test]
    async fn read_image_returns_envelope_and_image_block() {
        ensure_config();
        let context = test_context("read_image_ok");
        write_png(&context.work_dir, "pic.png");

        let result = run_tool_result(
            &context,
            ReadImageTool,
            "read_image",
            serde_json::json!({ "file_path": "pic.png" }),
        )
        .await
        .expect("read_image succeeds");

        assert!(result.content.contains("<path>"), "envelope has path");
        assert!(result.content.contains("image"), "envelope mentions image");
        let img = result.image.expect("read_image carries an image");
        assert_eq!(img.media_type, "image/jpeg");
        assert!(!img.data.is_empty());
    }

    #[tokio::test]
    async fn read_image_rejects_non_image_extension() {
        ensure_config();
        let context = test_context("read_image_non_image");
        std::fs::write(context.work_dir.join("a.txt"), b"hi").expect("write txt");
        let err = run_tool_result(
            &context,
            ReadImageTool,
            "read_image",
            serde_json::json!({ "file_path": "a.txt" }),
        )
        .await
        .expect_err("txt rejected");
        assert!(err.to_string().contains("not supported"));
    }
}
