use std::path::PathBuf;

pub fn find_project_root() -> Option<PathBuf> {
    let project_root = std::env::current_dir().ok().and_then(|c| {
        let mut current_dir = c;
        loop {
            if current_dir.join(".git").exists() {
                return Some(current_dir);
            }
            if !current_dir.pop() {
                break;
            }
        }
        None
    });

    if let Some(root) = project_root {
        let notes_path = root.join(".minerve");
        std::fs::create_dir_all(&notes_path).ok();
    }

    let mut current_dir = std::env::current_dir().ok()?;
    loop {
        if current_dir.join(".git").exists() {
            return Some(current_dir);
        }
        if !current_dir.pop() {
            break;
        }
    }
    None
}
