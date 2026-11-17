use std::{
    fs::{self, DirBuilder, OpenOptions},
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};

pub fn prepare(home: &Path) -> io::Result<()> {
    let dir = home.join(".cache/ccs");
    DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
    let meta = fs::symlink_metadata(&dir)?;
    if !meta.is_dir() {
        return Err(io::Error::other("expected a cache directory"));
    }
    if meta.permissions().mode() & 0o777 != 0o700 {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

// Exclusive creation also avoids following an old temp file or symlink.
pub fn write_new(path: &Path, contents: &str) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
    let result = file.write_all(contents.as_bytes());
    if result.is_err() { let _ = fs::remove_file(path); }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn locks_existing_cache_and_creates_private_files() {
        let home = std::env::temp_dir().join(format!("ccs-cache-mode-{}", std::process::id()));
        let dir = home.join(".cache/ccs");
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        prepare(&home).unwrap();
        assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        let path = dir.join("sample");
        write_new(&path, "private").unwrap();
        assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
        fs::remove_dir_all(home).unwrap();
    }
}
