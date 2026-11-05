//! Image files shown to the model: pasted from the clipboard, dragged onto the terminal, or
//! opened by the `view_image` tool. Messages keep only the path; `RequestImage::read` encodes
//! the file for each request.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::chat::image_media_type;

/// Longest edge sent to the model. Anthropic scales larger images down to this anyway, and a
/// full Retina screenshot would otherwise pass its 5 MB per-image limit.
const MAX_IMAGE_EDGE_PX: u32 = 1568;

/// Where pasted images and downscaled copies are written.
fn images_dir() -> PathBuf {
    dirs::home_dir()
        .expect("no home directory")
        .join(".minerve")
        .join("images")
}

fn new_image_path() -> Result<PathBuf, String> {
    let dir = images_dir();
    std::fs::create_dir_all(&dir).map_err(|err| format!("cannot create {}: {}", dir.display(), err))?;
    Ok(dir.join(format!("{}.png", uuid::Uuid::new_v4())))
}

/// The file to send for the image at `path`: the file itself when its longest edge fits
/// `MAX_IMAGE_EDGE_PX`, otherwise a downscaled PNG copy in `~/.minerve/images`.
/// The returned path is absolute, so a resumed session finds it from any directory.
pub fn image_to_send(path: &Path) -> Result<PathBuf, String> {
    let path = std::fs::canonicalize(path).map_err(|err| format!("cannot open {}: {}", path.display(), err))?;
    if image_media_type(&path).is_none() {
        return Err(format!("{} is not an image file (png, jpg, gif or webp)", path.display()));
    }
    let (width, height) =
        image::image_dimensions(&path).map_err(|err| format!("cannot read image {}: {}", path.display(), err))?;
    if width.max(height) <= MAX_IMAGE_EDGE_PX {
        return Ok(path);
    }
    let image = image::open(&path).map_err(|err| format!("cannot read image {}: {}", path.display(), err))?;
    let downscaled = image.resize(MAX_IMAGE_EDGE_PX, MAX_IMAGE_EDGE_PX, image::imageops::FilterType::Triangle);
    let copy = new_image_path()?;
    downscaled
        .save(&copy)
        .map_err(|err| format!("cannot write {}: {}", copy.display(), err))?;
    Ok(copy)
}

/// Saves the image on the clipboard to `~/.minerve/images` and returns the file to send.
/// `Ok(None)` when the clipboard holds no image. Reads the clipboard with `osascript`, so macOS only.
pub fn paste_clipboard_image() -> Result<Option<PathBuf>, String> {
    if !cfg!(target_os = "macos") {
        return Err(String::from("pasting an image needs macOS; drag the image file onto the terminal instead"));
    }
    let info = run_osascript(&["clipboard info"])?;
    if !info.contains("PNGf") {
        return Ok(None);
    }
    let saved = new_image_path()?;
    let write_png = format!(
        "set f to open for access POSIX file \"{}\" with write permission",
        saved.display()
    );
    run_osascript(&[
        &write_png,
        "write (the clipboard as «class PNGf») to f",
        "close access f",
    ])?;
    let to_send = image_to_send(&saved)?;
    if to_send != saved {
        // Only the downscaled copy is kept.
        std::fs::remove_file(&saved).map_err(|err| format!("cannot remove {}: {}", saved.display(), err))?;
    }
    Ok(Some(to_send))
}

fn run_osascript(lines: &[&str]) -> Result<String, String> {
    let mut command = Command::new("osascript");
    for line in lines {
        command.arg("-e").arg(line);
    }
    let output = command.output().map_err(|err| format!("cannot run osascript: {}", err))?;
    if !output.status.success() {
        return Err(format!(
            "osascript failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The image file a paste names, when the pasted text is one path to an existing image file.
/// Terminals paste a dragged file as its path, with spaces escaped by a backslash or the whole
/// path quoted.
pub fn pasted_image_path(pasted: &str) -> Option<PathBuf> {
    let trimmed = pasted.trim();
    if trimmed.contains('\n') {
        return None;
    }
    let unquoted = trimmed
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
        .map(str::to_string)
        .unwrap_or_else(|| trimmed.replace("\\ ", " "));
    let path = PathBuf::from(unquoted);
    (path.is_absolute() && path.is_file() && image_media_type(&path).is_some()).then_some(path)
}

/// Images attached to the prompt being typed. Each shows in the input as an `[Image #N]` token;
/// deleting the token drops the image.
#[derive(Default)]
pub struct PastedImages {
    images: Vec<(String, PathBuf)>,
}

impl PastedImages {
    /// Adds an image and returns the token to insert in the input.
    pub fn attach(&mut self, path: PathBuf) -> String {
        let token = format!("[Image #{}]", self.images.len() + 1);
        self.images.push((token.clone(), path));
        token
    }

    /// Images whose token is still in the submitted `input`, in paste order. Starts a new prompt's list.
    pub fn take_referenced(&mut self, input: &str) -> Vec<PathBuf> {
        std::mem::take(&mut self.images)
            .into_iter()
            .filter(|(token, _)| input.contains(token.as_str()))
            .map(|(_, path)| path)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_images_whose_token_is_kept_are_sent() {
        let mut pasted = PastedImages::default();
        assert_eq!(pasted.attach(PathBuf::from("/a.png")), "[Image #1]");
        assert_eq!(pasted.attach(PathBuf::from("/b.png")), "[Image #2]");
        assert_eq!(pasted.take_referenced("look at [Image #2]"), vec![PathBuf::from("/b.png")]);
        assert_eq!(pasted.attach(PathBuf::from("/c.png")), "[Image #1]");
    }

    #[test]
    fn small_image_is_sent_as_is_and_large_one_is_downscaled() {
        let dir = std::env::temp_dir().join(format!("minerve-images-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let small = dir.join("small.png");
        image::RgbImage::new(10, 20).save(&small).unwrap();
        assert_eq!(image_to_send(&small).unwrap(), std::fs::canonicalize(&small).unwrap());

        let large = dir.join("large.png");
        image::RgbImage::new(3136, 100).save(&large).unwrap();
        let copy = image_to_send(&large).unwrap();
        assert_eq!(image::image_dimensions(&copy).unwrap(), (1568, 50));
        std::fs::remove_file(copy).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn dragged_path_with_escaped_spaces_is_an_image_path() {
        let dir = std::env::temp_dir().join(format!("minerve drag test {}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("shot.PNG");
        std::fs::write(&file, b"x").unwrap();
        let escaped = file.display().to_string().replace(' ', "\\ ");
        assert_eq!(pasted_image_path(&format!("{} ", escaped)), Some(file.clone()));
        assert_eq!(pasted_image_path(&format!("'{}'", file.display())), Some(file.clone()));
        assert_eq!(pasted_image_path("hello world"), None);
        assert_eq!(pasted_image_path(&format!("{}\n{}", escaped, escaped)), None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
