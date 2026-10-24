//! Токен доступа: генерируется при первом запуске и хранится в файле
//! `token` в data dir с правами `0600`.

use std::fs;
use std::io::Write as _;
use std::path::Path;

use anyhow::Context as _;

/// Байт случайности в токене (в файле — hex, 64 символа).
const TOKEN_BYTES: usize = 32;

/// Прочитать токен из `path` или создать новый (каталог создаётся).
/// На Unix у файла права `0600`; если шире — исправляются.
pub fn load_or_create_token(path: &Path) -> anyhow::Result<String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)
            .with_context(|| format!("не удалось создать каталог {}", dir.display()))?;
    }
    match fs::read_to_string(path) {
        Ok(s) => {
            let token = s.trim().to_owned();
            anyhow::ensure!(
                !token.is_empty(),
                "файл токена {} пуст — удалите его, токен создастся заново",
                path.display()
            );
            restrict_permissions(path)?;
            Ok(token)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => create(path),
        Err(e) => Err(e).with_context(|| format!("не удалось прочитать {}", path.display())),
    }
}

fn create(path: &Path) -> anyhow::Result<String> {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("нет источника случайности: {e}"))?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();

    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut file = opts
        .open(path)
        .with_context(|| format!("не удалось создать {}", path.display()))?;
    file.write_all(token.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    tracing::info!(path = %path.display(), "создан новый токен доступа");
    Ok(token)
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = fs::metadata(path)?.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        tracing::warn!(path = %path.display(), mode = format!("{mode:o}"), "права файла токена шире 0600 — исправляю");
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_once_then_reuses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("token");
        let a = load_or_create_token(&path).unwrap();
        assert_eq!(a.len(), TOKEN_BYTES * 2);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        let b = load_or_create_token(&path).unwrap();
        assert_eq!(a, b);

        let other = tempfile::tempdir().unwrap();
        let c = load_or_create_token(&other.path().join("token")).unwrap();
        assert_ne!(a, c);
    }

    #[cfg(unix)]
    #[test]
    fn file_is_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        load_or_create_token(&path).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        load_or_create_token(&path).unwrap();
        assert_eq!(mode(&path), 0o600);
    }

    #[test]
    fn empty_file_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        fs::write(&path, "\n").unwrap();
        assert!(load_or_create_token(&path).is_err());
    }
}
