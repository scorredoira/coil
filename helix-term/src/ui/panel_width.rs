use std::path::PathBuf;

use anyhow::Context as _;

/// The width of a panel whose separator can be dragged, kept between sessions: the
/// sidebar's and the Markdown preview's, each in its own file of the data directory.
#[derive(serde::Serialize, serde::Deserialize)]
struct SavedState {
    width: u16,
}

fn state_file(name: &str) -> PathBuf {
    helix_loader::data_dir().join(format!("{name}.toml"))
}

/// The width the separator was last dragged to; none before it ever was.
pub fn load(name: &str) -> anyhow::Result<Option<u16>> {
    let state: Option<SavedState> = load_state(name)?;
    Ok(state.map(|state| state.width))
}

/// Shared atomic state storage for panel layouts with more than one dimension.
pub fn load_state<T: serde::de::DeserializeOwned>(name: &str) -> anyhow::Result<Option<T>> {
    let path = state_file(name);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };

    let state = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(Some(state))
}

/// Written aside and renamed over, so another editor reading it never sees half.
pub fn save(name: &str, width: u16) -> anyhow::Result<()> {
    save_state(name, &SavedState { width })
}

pub fn save_state<T: serde::Serialize>(name: &str, state: &T) -> anyhow::Result<()> {
    let path = state_file(name);
    let dir = helix_loader::data_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let text = toml::to_string(state)?;
    let temp = dir.join(format!(".{name}.{}.toml", std::process::id()));
    std::fs::write(&temp, text).with_context(|| format!("writing {}", temp.display()))?;
    std::fs::rename(&temp, &path).with_context(|| format!("replacing {}", path.display()))?;

    Ok(())
}
