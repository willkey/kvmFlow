use clipboard_rs::{ClipboardContent, common::RustImage};
use clipboard_win::{Clipboard, formats, options::NoClear, raw};

use super::{ClipboardAdapterError, retry_clipboard_operation};

pub(super) fn write(contents: Vec<ClipboardContent>) -> Result<(), ClipboardAdapterError> {
    // Convert images before opening the system clipboard. Every representation
    // below shares one open/close pair and must not clear the other formats.
    let bitmap = contents
        .iter()
        .find_map(|content| match content {
            ClipboardContent::Image(image) => Some(image.to_bitmap()),
            _ => None,
        })
        .transpose()
        .map_err(|error| ClipboardAdapterError::InvalidContent(error.to_string()))?;
    let png = contents
        .iter()
        .find_map(|content| match content {
            ClipboardContent::Image(image) => Some(image.to_png()),
            _ => None,
        })
        .transpose()
        .map_err(|error| ClipboardAdapterError::InvalidContent(error.to_string()))?;
    // Retry only acquisition, never a partially published write. Holding the
    // clipboard makes the multi-format update atomic to other applications.
    let _clipboard = retry_clipboard_operation(
        "open_for_write",
        || Clipboard::new().map_err(|error| ClipboardAdapterError::Unavailable(error.to_string())),
        std::thread::sleep,
    )?;
    raw::empty().map_err(write_error)?;
    for content in contents {
        match content {
            ClipboardContent::Text(text) => {
                raw::set_string_with(&text, NoClear).map_err(write_error)?
            }
            ClipboardContent::Html(html) => {
                let format = formats::Html::new().ok_or_else(|| {
                    ClipboardAdapterError::WriteFailed("register HTML format failed".into())
                })?;
                raw::set_html_with(format.code(), &html, NoClear).map_err(write_error)?;
            }
            ClipboardContent::Rtf(rtf) => write_buffer("Rich Text Format", rtf.as_bytes())?,
            ClipboardContent::Files(files) => {
                raw::set_file_list_with(&files, NoClear).map_err(write_error)?
            }
            ClipboardContent::Image(_) => {
                write_buffer("PNG", png.as_ref().expect("image encoded").get_bytes())?;
                raw::set_bitmap_with(bitmap.as_ref().expect("image encoded").get_bytes(), NoClear)
                    .map_err(write_error)?;
            }
            ClipboardContent::Other(format, bytes) => write_buffer(&format, &bytes)?,
        }
    }
    Ok(())
}

fn write_buffer(format: &str, bytes: &[u8]) -> Result<(), ClipboardAdapterError> {
    let format = clipboard_win::register_format(format).ok_or_else(|| {
        ClipboardAdapterError::WriteFailed("register clipboard format failed".into())
    })?;
    raw::set_without_clear(format.get(), bytes).map_err(write_error)
}

fn write_error(error: impl std::fmt::Display) -> ClipboardAdapterError {
    ClipboardAdapterError::WriteFailed(error.to_string())
}
