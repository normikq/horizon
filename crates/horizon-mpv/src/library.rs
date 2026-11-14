//! Where a native mod's third-party libraries are and whether they are the
//! declared ones.
//!
//! The launcher copies each library a mod declares (`native-libraries.toml`)
//! into the version folder's [`DIR`] after checking its SHA-256, and lists
//! them in [`MANIFEST`]. The game checks the hash again before it loads one,
//! so a file swapped after the build is refused.

use crate::Error;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

/// The folder of a version folder that holds the mods' libraries.
pub const DIR: &str = "native-libraries";
/// The list the launcher writes beside the game.
pub const MANIFEST: &str = "native-libraries.json";

/// One installed library, as [`MANIFEST`] lists it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Installed {
    /// The mod that declared it.
    #[serde(rename = "mod")]
    pub mod_id: String,
    /// What the game asks for (`libmpv`).
    pub name: String,
    pub version: String,
    /// Its file name in [`DIR`].
    pub file: String,
    /// Lowercase hex.
    pub sha256: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct List {
    libraries: Vec<Installed>,
}

/// The text of [`MANIFEST`] for `libraries`.
pub fn manifest_text(libraries: &[Installed]) -> String {
    serde_json::json!({ "libraries": libraries }).to_string()
}

/// A library's file name: one path component, nothing that leaves [`DIR`].
pub fn valid_file(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// 64 lowercase hex digits.
pub fn valid_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The libraries installed in `version_dir`; none without a manifest.
pub fn installed(version_dir: &Path) -> Result<Vec<Installed>, Error> {
    let path = version_dir.join(MANIFEST);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::Io(path, e)),
    };
    let text = text.trim_start_matches('\u{feff}');
    let list: List = serde_json::from_str(text)
        .map_err(|e| Error::Manifest(format!("{}: {e}", path.display())))?;
    for library in &list.libraries {
        if !valid_file(&library.file) {
            return Err(Error::Manifest(format!(
                "{}: {:?} is not a file name",
                path.display(),
                library.file
            )));
        }
        if !valid_sha256(&library.sha256) {
            return Err(Error::Manifest(format!(
                "{}: {} has no SHA-256",
                path.display(),
                library.file
            )));
        }
    }
    Ok(list.libraries)
}

/// The SHA-256 of the file at `path`, lowercase hex.
pub fn sha256_of(path: &Path) -> Result<String, Error> {
    let mut file = std::fs::File::open(path).map_err(|e| Error::Io(path.to_owned(), e))?;
    sha256_of_file(&mut file, path)
}

/// The SHA-256 of what is left to read of `file` (at `path`, for errors).
fn sha256_of_file(file: &mut std::fs::File, path: &Path) -> Result<String, Error> {
    let io = |e| Error::Io(path.to_owned(), e);
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer).map_err(io)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The full path of library `name` in `version_dir`, once its file is there,
/// is a plain file and has the declared SHA-256. Loading by this path later
/// leaves a moment in which the file could be swapped: [`verified_open`]
/// closes it.
pub fn verified(version_dir: &Path, name: &str) -> Result<PathBuf, Error> {
    verified_open(version_dir, name).map(|v| v.path)
}

/// A library whose hash was checked through the open file that is kept here.
/// On Windows the file stays open without write or delete sharing, so it
/// cannot be changed, replaced or renamed while this lives; on Unix
/// [`still_same`](Self::still_same) tells whether the path still names it.
#[derive(Debug)]
pub struct Verified {
    /// Absolute.
    pub path: PathBuf,
    file: std::fs::File,
}

impl Verified {
    /// Whether `path` still names the file that was hashed (no swap since).
    pub fn still_same(&self) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let (Ok(now), Ok(then)) = (std::fs::metadata(&self.path), self.file.metadata()) else {
                return false;
            };
            now.dev() == then.dev() && now.ino() == then.ino()
        }
        #[cfg(not(unix))]
        {
            // The open file denies writing, deleting and renaming.
            let _ = &self.file;
            true
        }
    }
}

/// Opens `path` for hashing, as [`Verified`] keeps it.
fn open_locked(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        /// `FILE_SHARE_READ`: others may read (and load) it, never write,
        /// delete or rename it.
        const FILE_SHARE_READ: u32 = 0x1;
        options.share_mode(FILE_SHARE_READ);
    }
    options.open(path)
}

/// Library `name` in `version_dir`, checked as [`verified`] does, through a
/// file kept open so that what is loaded is what was hashed.
pub fn verified_open(version_dir: &Path, name: &str) -> Result<Verified, Error> {
    let library = installed(version_dir)?
        .into_iter()
        .find(|l| l.name == name)
        .ok_or_else(|| {
            Error::NotInstalled(format!(
                "{name} is not in {}",
                version_dir.join(MANIFEST).display()
            ))
        })?;
    let path = version_dir.join(DIR).join(&library.file);
    let metadata = std::fs::symlink_metadata(&path).map_err(|e| Error::Io(path.clone(), e))?;
    if !metadata.is_file() {
        return Err(Error::Manifest(format!(
            "{} is not a plain file",
            path.display()
        )));
    }
    let mut file = open_locked(&path).map_err(|e| Error::Io(path.clone(), e))?;
    let actual = sha256_of_file(&mut file, &path)?;
    if actual != library.sha256 {
        return Err(Error::Hash {
            file: path,
            expected: library.sha256,
            actual,
        });
    }
    // Absolute even when the version folder was given relative to the working directory.
    let path = std::path::absolute(&path).map_err(|e| Error::Io(path, e))?;
    Ok(Verified { path, file })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "horizon-mpv-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(DIR)).unwrap();
        dir
    }

    fn entry(file: &str, sha256: &str) -> Installed {
        Installed {
            mod_id: "video".into(),
            name: "libmpv".into(),
            version: "0.40.0".into(),
            file: file.into(),
            sha256: sha256.into(),
        }
    }

    // SHA-256 of "abc".
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn a_library_is_given_only_with_its_declared_hash() {
        let dir = scratch("hash");
        let file = dir.join(DIR).join("libmpv-2.dll");
        std::fs::write(&file, b"abc").unwrap();
        std::fs::write(
            dir.join(MANIFEST),
            manifest_text(&[entry("libmpv-2.dll", ABC)]),
        )
        .unwrap();
        let path = verified(&dir, "libmpv").unwrap();
        assert!(path.is_absolute());
        assert!(path.ends_with(Path::new(DIR).join("libmpv-2.dll")));

        // Swapped after the build.
        std::fs::write(&file, b"abd").unwrap();
        assert!(matches!(verified(&dir, "libmpv"), Err(Error::Hash { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_is_given_without_the_launchers_list() {
        let dir = scratch("none");
        std::fs::write(dir.join(DIR).join("libmpv-2.dll"), b"abc").unwrap();
        assert!(matches!(
            verified(&dir, "libmpv"),
            Err(Error::NotInstalled(_))
        ));
        std::fs::write(
            dir.join(MANIFEST),
            manifest_text(&[entry("libmpv-2.dll", ABC)]),
        )
        .unwrap();
        assert!(matches!(
            verified(&dir, "other"),
            Err(Error::NotInstalled(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_listed_file_cannot_leave_the_folder() {
        let dir = scratch("escape");
        for file in ["../libmpv-2.dll", "a/b.dll", "a\\b.dll", "C:evil.dll", ""] {
            std::fs::write(dir.join(MANIFEST), manifest_text(&[entry(file, ABC)])).unwrap();
            assert!(
                matches!(verified(&dir, "libmpv"), Err(Error::Manifest(_))),
                "{file}"
            );
        }
        assert!(valid_file("libmpv-2.dll"));
        assert!(!valid_file(".hidden"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hash_is_64_lowercase_hex_digits() {
        assert!(valid_sha256(ABC));
        assert!(!valid_sha256(&ABC.to_uppercase()));
        assert!(!valid_sha256(&ABC[1..]));
    }
}
