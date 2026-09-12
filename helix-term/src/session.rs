//! What a project had open, so that opening the editor on it again opens the same tabs.

use std::path::{Path, PathBuf};

use anyhow::Context as _;

/// The files a project had open, in the order their tabs were in.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Session {
    pub files: Vec<PathBuf>,
    /// The one that was in front.
    pub focused: Option<PathBuf>,
}

fn sessions_dir() -> PathBuf {
    helix_loader::data_dir().join("sessions")
}

/// A stable name for a project: its own folder's, and a hash of the whole path, so that two
/// projects whose folders are called the same are not one session.
fn key(workspace: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in workspace.as_os_str().as_encoded_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }

    let name = workspace
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workspace");

    format!("{name}-{hash:016x}")
}

fn session_file(workspace: &Path) -> PathBuf {
    sessions_dir().join(format!("{}.toml", key(workspace)))
}

/// What this project had open last; none before it ever had any.
pub fn load(workspace: &Path) -> anyhow::Result<Option<Session>> {
    let path = session_file(workspace);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };

    let session: Session =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;

    Ok(Some(session))
}

/// Written aside and renamed over, so another editor reading it never sees half.
pub fn save(workspace: &Path, session: &Session) -> anyhow::Result<()> {
    let dir = sessions_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let path = session_file(workspace);
    let text = toml::to_string(session)?;
    let temp = dir.join(format!(".{}.{}.toml", key(workspace), std::process::id()));
    std::fs::write(&temp, text).with_context(|| format!("writing {}", temp.display()))?;
    std::fs::rename(&temp, &path).with_context(|| format!("replacing {}", path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_projects_called_the_same_are_two_sessions() {
        let one = key(Path::new("/home/someone/work/site"));
        let two = key(Path::new("/home/someone/play/site"));

        assert!(one.starts_with("site-"));
        assert!(two.starts_with("site-"));
        assert_ne!(one, two);

        // And the same project is the same session, run after run.
        assert_eq!(one, key(Path::new("/home/someone/work/site")));
    }
}
