use anchor_mcp_host::ImageBudget;
use anchor_runtime::ToolResultContent;
use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(super) struct ToolOutput {
    pub(super) observation: Value,
    pub(super) media: Option<Vec<ContentBlock>>,
    pub(super) images: Vec<(Vec<u8>, String)>,
}

pub(super) fn encode(contents: Vec<ToolResultContent>) -> Result<ToolOutput, String> {
    let mut images = ImageBudget::default();
    let mut decoded_images = Vec::new();
    let mut has_images = false;
    let mut observation = Vec::with_capacity(contents.len());
    let mut blocks = Vec::with_capacity(contents.len());
    for content in &contents {
        if let Some((data, mime_type)) = image_parts(content)? {
            let bytes = images
                .validate(data, mime_type)
                .map_err(|error| error.to_string())?;
            has_images = true;
            decoded_images.push((bytes.clone(), mime_type.to_owned()));
            observation.push(json!({
                "type":"image","mime_type":mime_type,"bytes":bytes.len(),
                "data_sha256":format!("{:x}", Sha256::digest(&bytes))
            }));
            blocks.push(ContentBlock::image(data, mime_type));
        } else {
            observation.push(serde_json::to_value(content).map_err(|error| error.to_string())?);
            if let Some(text) = content.as_text() {
                blocks.push(ContentBlock::text(text));
            } else if let Some(value) = content.as_json() {
                blocks.push(ContentBlock::text(value.to_string()));
            } else {
                return Err("unsupported Anchor tool content".into());
            }
        }
    }
    Ok(ToolOutput {
        observation: Value::Array(observation),
        media: has_images.then_some(blocks),
        images: decoded_images,
    })
}

pub(super) fn success(output: Value, media: Option<Vec<ContentBlock>>) -> CallToolResult {
    let Some(mut blocks) = media else {
        return CallToolResult::success(vec![ContentBlock::text(output.to_string())]);
    };
    let summaries = output.get("output").unwrap_or(&output);
    let images: Vec<_> = summaries
        .as_array()
        .into_iter()
        .flatten()
        .filter(|content| content["type"] == "image" && content["data_sha256"].is_string())
        .cloned()
        .collect();
    let mut header = json!({"ok":true,"images":images});
    if let Some(receipt) = output.get("anchor_receipt") {
        header["anchor_receipt"] = receipt.clone();
    }
    blocks.insert(0, ContentBlock::text(header.to_string()));
    CallToolResult::success(blocks)
}

fn image_parts(content: &ToolResultContent) -> Result<Option<(&str, &str)>, String> {
    Ok(match content {
        ToolResultContent::Image { data, mime_type } => Some((data, mime_type)),
        _ => None,
    })
}

#[cfg(test)]
mod tests;

pub(super) fn image_prompt(bytes: &[u8], media_type: &str) -> Result<Value, String> {
    use base64::Engine;
    anchor_mcp_host::validate_image_bytes(bytes, media_type)?;
    Ok(
        json!({"type":"image","data":base64::engine::general_purpose::STANDARD.encode(bytes),"mimeType":media_type}),
    )
}
