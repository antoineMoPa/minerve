//! Image files shown to the model: pasted from the clipboard, dragged onto the terminal, or
//! opened by the `view_image` tool. Messages keep only the path of a copy minerve owns in
//! `~/.minerve/images`; `RequestImage::read` encodes it for each request. A task deleting or
//! overwriting its screenshot so never changes or breaks the history.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::chat::image_media_type;

/// Longest edge sent to the model. Anthropic scales larger images down to this anyway, and a
/// full Retina screenshot would otherwise pass its 5 MB per-image limit.
const MAX_IMAGE_EDGE_PX: u32 = 1568;

/// Largest image file sent. Anthropic refuses an image over 5 MB once base64-encoded (4/3 of
/// its bytes), and one refused image fails every request of the session.
const MAX_IMAGE_BYTES: u64 = 3_500_000;

/// Where pasted images and the copies of sent images are written.
fn images_dir() -> PathBuf {
    dirs::home_dir()
        .expect("no home directory")
        .join(".minerve")
        .join("images")
}

fn create_new_image_path(extension: &str) -> Result<PathBuf, String> {
    let dir = images_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("cannot create {}: {}", dir.display(), err))?;
    Ok(dir.join(format!("{}.{}", uuid::Uuid::new_v4(), extension)))
}

/// The file to send for the image at `path`: a copy in `~/.minerve/images`, downscaled to PNG
/// when its longest edge passes `MAX_IMAGE_EDGE_PX`, then made a JPEG when still over
/// `MAX_IMAGE_BYTES`. A file already in that folder (a pasted image) is sent as is. The
/// returned path is absolute, so a resumed session finds it from any directory.
pub fn image_to_send(path: &Path) -> Result<PathBuf, String> {
    shrink_image_to_byte_cap(&image_within_edge_limit(path)?)
}

/// The images of a prompt sent by the GUI, made ready like a pasted one, and an error for each
/// image that could not be.
pub fn prepare_images_to_send(paths: Vec<PathBuf>) -> (Vec<PathBuf>, Vec<String>) {
    let mut ready = Vec::new();
    let mut errors = Vec::new();
    for path in paths {
        match image_to_send(&path) {
            Ok(path) => ready.push(path),
            Err(err) => errors.push(format!("image not attached: {}", err)),
        }
    }
    (ready, errors)
}

/// `path` itself when within `MAX_IMAGE_BYTES`, otherwise a JPEG copy, scaled down until it fits.
fn shrink_image_to_byte_cap(path: &Path) -> Result<PathBuf, String> {
    let size = |path: &Path| std::fs::metadata(path).map(|m| m.len()).unwrap_or(u64::MAX);
    if size(path) <= MAX_IMAGE_BYTES {
        return Ok(path.to_path_buf());
    }
    let mut image = image::DynamicImage::ImageRgb8(
        image::open(path)
            .map_err(|err| format!("cannot read image {}: {}", path.display(), err))?
            .to_rgb8(),
    );
    loop {
        let jpeg = create_new_image_path("jpg")?;
        image
            .save_with_format(&jpeg, image::ImageFormat::Jpeg)
            .map_err(|err| format!("cannot write {}: {}", jpeg.display(), err))?;
        if size(&jpeg) <= MAX_IMAGE_BYTES || image.width().max(image.height()) <= 64 {
            return Ok(jpeg);
        }
        let _ = std::fs::remove_file(&jpeg);
        image = image.resize(
            image.width() * 3 / 4,
            image.height() * 3 / 4,
            image::imageops::FilterType::Triangle,
        );
    }
}

/// A copy of `path` in `~/.minerve/images`, downscaled to PNG past `MAX_IMAGE_EDGE_PX`.
fn image_within_edge_limit(path: &Path) -> Result<PathBuf, String> {
    let path = std::fs::canonicalize(path)
        .map_err(|err| format!("cannot open {}: {}", path.display(), err))?;
    if image_media_type(&path).is_none() {
        return Err(format!(
            "{} is not an image file (png, jpg, gif or webp)",
            path.display()
        ));
    }
    let (width, height) = image::image_dimensions(&path)
        .map_err(|err| format!("cannot read image {}: {}", path.display(), err))?;
    if width.max(height) <= MAX_IMAGE_EDGE_PX {
        return copy_image_to_images_dir(&path);
    }
    let image = image::open(&path)
        .map_err(|err| format!("cannot read image {}: {}", path.display(), err))?;
    let downscaled = image.resize(
        MAX_IMAGE_EDGE_PX,
        MAX_IMAGE_EDGE_PX,
        image::imageops::FilterType::Triangle,
    );
    let copy = create_new_image_path("png")?;
    downscaled
        .save(&copy)
        .map_err(|err| format!("cannot write {}: {}", copy.display(), err))?;
    Ok(copy)
}

/// A copy of `path` in `~/.minerve/images`, or `path` itself when it is already there.
fn copy_image_to_images_dir(path: &Path) -> Result<PathBuf, String> {
    if let Ok(dir) = std::fs::canonicalize(images_dir()) {
        if path.starts_with(dir) {
            return Ok(path.to_path_buf());
        }
    }
    let extension = path
        .extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| String::from("png"));
    let copy = create_new_image_path(&extension)?;
    std::fs::copy(path, &copy).map_err(|err| {
        format!(
            "cannot copy {} to {}: {}",
            path.display(),
            copy.display(),
            err
        )
    })?;
    Ok(copy)
}

/// Saves the image on the clipboard to `~/.minerve/images` and returns the file to send.
/// `Ok(None)` when the clipboard holds no image. Reads the clipboard with `osascript`, so macOS only.
pub fn paste_clipboard_image() -> Result<Option<PathBuf>, String> {
    if !cfg!(target_os = "macos") {
        return Err(String::from(
            "pasting an image needs macOS; drag the image file onto the terminal instead",
        ));
    }
    let info = run_osascript(&["clipboard info"])?;
    if !info.contains("PNGf") {
        return Ok(None);
    }
    let saved = create_new_image_path("png")?;
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
        std::fs::remove_file(&saved)
            .map_err(|err| format!("cannot remove {}: {}", saved.display(), err))?;
    }
    Ok(Some(to_send))
}

fn run_osascript(lines: &[&str]) -> Result<String, String> {
    let mut command = Command::new("osascript");
    for line in lines {
        command.arg("-e").arg(line);
    }
    let output = command
        .output()
        .map_err(|err| format!("cannot run osascript: {}", err))?;
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
        assert_eq!(
            pasted.take_referenced("look at [Image #2]"),
            vec![PathBuf::from("/b.png")]
        );
        assert_eq!(pasted.attach(PathBuf::from("/c.png")), "[Image #1]");
    }

    #[test]
    fn small_image_is_copied_and_large_one_is_downscaled() {
        let dir =
            std::env::temp_dir().join(format!("minerve-images-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let small = dir.join("small.PNG");
        image::RgbImage::new(10, 20).save(&small).unwrap();
        let small_copy = image_to_send(&small).unwrap();
        assert!(small_copy.starts_with(std::fs::canonicalize(images_dir()).unwrap()));
        assert_eq!(small_copy.extension().unwrap(), "png");
        assert_eq!(image::image_dimensions(&small_copy).unwrap(), (10, 20));
        // A copy already in the images folder is not copied again.
        assert_eq!(image_to_send(&small_copy).unwrap(), small_copy);
        std::fs::remove_file(small_copy).unwrap();

        let large = dir.join("large.png");
        image::RgbImage::new(3136, 100).save(&large).unwrap();
        let copy = image_to_send(&large).unwrap();
        assert_eq!(image::image_dimensions(&copy).unwrap(), (1568, 50));
        std::fs::remove_file(copy).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_image_too_heavy_to_send_becomes_a_jpeg_under_the_byte_cap() {
        let dir =
            std::env::temp_dir().join(format!("minerve-images-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        // Noise does not compress: about 7 MB as PNG, within the edge limit.
        let noisy = dir.join("noisy.png");
        let mut seed: u32 = 1;
        image::RgbImage::from_fn(1500, 1500, |_, _| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let [r, g, b, _] = seed.to_le_bytes();
            image::Rgb([r, g, b])
        })
        .save(&noisy)
        .unwrap();
        assert!(std::fs::metadata(&noisy).unwrap().len() > MAX_IMAGE_BYTES);

        let sent = image_to_send(&noisy).unwrap();
        assert_eq!(sent.extension().unwrap(), "jpg");
        assert!(std::fs::metadata(&sent).unwrap().len() <= MAX_IMAGE_BYTES);

        std::fs::remove_file(sent).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn deleting_or_overwriting_a_viewed_image_leaves_the_sent_one_unchanged() {
        let dir =
            std::env::temp_dir().join(format!("minerve-images-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let shot = dir.join("end-check.png");
        image::RgbImage::new(4, 4).save(&shot).unwrap();
        let sent = image_to_send(&shot).unwrap();

        image::RgbImage::new(8, 8).save(&shot).unwrap();
        assert_eq!(image::image_dimensions(&sent).unwrap(), (4, 4));
        std::fs::remove_file(&shot).unwrap();
        assert!(crate::chat::RequestImage::read(&sent).is_ok());

        std::fs::remove_file(sent).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn dragged_path_with_escaped_spaces_is_an_image_path() {
        let dir = std::env::temp_dir().join(format!("minerve drag test {}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("shot.PNG");
        std::fs::write(&file, b"x").unwrap();
        let escaped = file.display().to_string().replace(' ', "\\ ");
        assert_eq!(
            pasted_image_path(&format!("{} ", escaped)),
            Some(file.clone())
        );
        assert_eq!(
            pasted_image_path(&format!("'{}'", file.display())),
            Some(file.clone())
        );
        assert_eq!(pasted_image_path("hello world"), None);
        assert_eq!(
            pasted_image_path(&format!("{}\n{}", escaped, escaped)),
            None
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
