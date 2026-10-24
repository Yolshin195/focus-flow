//! Секреты плагина: по файлу на секрет в `<data_dir>/secrets/<id>/`,
//! каталог `0700`, файлы `0600`. Переезд в системную связку ключей
//! (`keyring`) — позже, интерфейс не изменится.

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;

use anyhow::Context as _;

/// Хранилище секретов одного плагина.
#[derive(Debug, Clone)]
pub struct Secrets {
    dir: PathBuf,
}

impl Secrets {
    /// Секреты в каталоге `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, name: &str) -> anyhow::Result<PathBuf> {
        let ok = !name.is_empty()
            && name.len() <= 64
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        anyhow::ensure!(ok, "недопустимое имя секрета «{name}»");
        Ok(self.dir.join(name))
    }

    /// Прочитать секрет; `None`, если его нет.
    pub fn get(&self, name: &str) -> anyhow::Result<Option<String>> {
        let path = self.path(name)?;
        match fs::read_to_string(&path) {
            Ok(s) => Ok(Some(s.trim_end_matches(['\r', '\n']).to_owned())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("не удалось прочитать {}", path.display())),
        }
    }

    /// Записать секрет (атомарно: временный файл `0600` + переименование).
    pub fn set(&self, name: &str, value: &str) -> anyhow::Result<()> {
        let path = self.path(name)?;
        create_private_dir(&self.dir)?;
        let tmp = self.dir.join(format!(".{name}.tmp"));
        let _ = fs::remove_file(&tmp);
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600);
        }
        let mut f = opts
            .open(&tmp)
            .with_context(|| format!("не удалось создать {}", tmp.display()))?;
        f.write_all(value.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, &path)
            .with_context(|| format!("не удалось записать {}", path.display()))?;
        Ok(())
    }

    /// Удалить секрет (если его нет — не ошибка).
    pub fn delete(&self, name: &str) -> anyhow::Result<()> {
        let path = self.path(name)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("не удалось удалить {}", path.display())),
        }
    }
}

fn create_private_dir(dir: &std::path::Path) -> anyhow::Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("не удалось создать {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_permissions() {
        let root =
            std::env::temp_dir().join(format!("focus-secrets-{}-{}", std::process::id(), line!()));
        let s = Secrets::new(root.join("jira"));
        assert_eq!(s.get("token").unwrap(), None);
        s.set("token", "abc").unwrap();
        assert_eq!(s.get("token").unwrap().as_deref(), Some("abc"));
        s.set("token", "def").unwrap();
        assert_eq!(s.get("token").unwrap().as_deref(), Some("def"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(root.join("jira/token"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
            let mode = fs::metadata(root.join("jira"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        s.delete("token").unwrap();
        s.delete("token").unwrap();
        assert_eq!(s.get("token").unwrap(), None);
        assert!(s.set("../x", "y").is_err());
        let _ = fs::remove_dir_all(root);
    }
}
