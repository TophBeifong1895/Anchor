//! Saved rich replies, validated before channel delivery.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::ImageFormat;
use md5::{Digest as Md5Digest, Md5};
use serde_json::Value;
use std::{fs, path::Path};

pub(crate) const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
pub(crate) const MAX_IMAGE_COUNT: usize = 10;
pub(crate) const MAX_IMAGE_TOTAL_BASE64: usize = 14 * 1024 * 1024;

/// Read the rich reply one assistant Turn registered for the channel gateway.
///
/// The file is the Host's own record, but it is read back after the node
/// finished, so every item is re-validated here: this is the last point that
/// still holds the Host's authority over the bytes the gateway will deliver.
pub(crate) fn read_reply_images_for_invocation(
    state_root: &Path,
    key: &anchor_runtime::graph::InvocationKey,
) -> Result<Vec<Value>, String> {
    let binding = crate::assistant::binding(state_root, key)?
        .ok_or("reply images require a trusted Turn binding")?;
    let path = state_root
        .join("channel-replies")
        .join(format!("turn-{}.json", binding.turn));
    read_reply_images_at(&path)
}

pub(crate) fn read_reply_images_at(path: &Path) -> Result<Vec<Value>, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let values: Vec<Value> =
        serde_json::from_slice(&bytes).map_err(|_| "saved channel reply is invalid".to_string())?;
    if values.len() > MAX_IMAGE_COUNT {
        return Err("saved channel reply exceeds the image count limit".into());
    }
    let mut total = 0usize;
    for value in &values {
        let object = value
            .as_object()
            .filter(|object| {
                object.len() == 2 && object.get("msgtype").and_then(Value::as_str) == Some("image")
            })
            .ok_or("saved channel reply has an unsupported item")?;
        let image = object
            .get("image")
            .and_then(Value::as_object)
            .filter(|image| image.len() == 2)
            .ok_or("saved channel reply has an unsupported item")?;
        let encoded = image
            .get("base64")
            .and_then(Value::as_str)
            .ok_or("saved channel reply has no image content")?;
        let digest = image
            .get("md5")
            .and_then(Value::as_str)
            .filter(|digest| {
                digest.len() == 32
                    && digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
            .ok_or("saved channel reply image digest is invalid")?;
        total = total.saturating_add(encoded.len());
        if total > MAX_IMAGE_TOTAL_BASE64 {
            return Err("saved channel reply exceeds the total image limit".into());
        }
        let data = STANDARD
            .decode(encoded)
            .map_err(|_| "saved channel reply image is not valid base64".to_string())?;
        if data.is_empty() || data.len() > MAX_IMAGE_BYTES {
            return Err("saved channel reply image size is invalid".into());
        }
        let mime = match image::guess_format(&data) {
            Ok(ImageFormat::Png) => "image/png",
            Ok(ImageFormat::Jpeg) => "image/jpeg",
            _ => return Err("saved channel reply image is not PNG or JPEG".into()),
        };
        crate::attachments::validate_image(&data, mime).map_err(|error| error.to_string())?;
        let mut hasher = Md5::new();
        Md5Digest::update(&mut hasher, &data);
        if format!("{:x}", Md5Digest::finalize(hasher)) != digest {
            return Err("saved channel reply image digest does not match its content".into());
        }
    }
    Ok(values)
}
