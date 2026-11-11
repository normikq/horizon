//! Third-party libraries a native mod needs beside the game, such as libmpv
//! (docs/native-libraries.md).
//!
//! A bundle declares them in `native-libraries.toml`, beside its
//! `horizon-mixin.toml`: a name, a version, and for each platform a file and
//! its SHA-256. The file comes from the bundle or from the folder the player
//! puts libraries in; the launcher never downloads one. A file whose hash is
//! not the declared one stops the build. The checked files are copied into
//! the version folder's `native-libraries/` and listed in
//! `native-libraries.json`, which the game checks again before it loads one
//! (`horizon_mpv::library`).
use super::Bundle;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// A bundle's declaration, beside its `horizon-mixin.toml`.
pub(super) const MANIFEST: &str = "native-libraries.toml";
/// The folder of a version folder the libraries are installed in.
pub(super) const DIR: &str = "native-libraries";
/// The list of what is installed there, beside the game.
pub(super) const INSTALLED: &str = "native-libraries.json";
/// Where the player puts a library a bundle does not carry, under the native build folder.
pub(super) const SUPPLIED: &str = "libraries";

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Declared {
    name: String,
    version: String,
    platform: String,
    file: String,
    sha256: String,
    /// Where the file is in the bundle; `data/<file>` when not given.
    path: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    #[serde(default)]
    library: Vec<Declared>,
}

/// A library of this build's platform, found and checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Library {
    pub mod_id: String,
    pub name: String,
    pub version: String,
    pub file: String,
    pub sha256: String,
    pub source: PathBuf,
}

const PLATFORMS: [&str; 6] = [
    "windows-x64",
    "windows-arm64",
    "linux-x64",
    "linux-arm64",
    "macos-x64",
    "macos-arm64",
];

/// The platform a build is for, as a manifest names it: from the Rust target
/// the game is compiled for, or this launcher's own when the build has none.
pub(super) fn platform(rust_target: Option<&str>) -> Option<&'static str> {
    let (arch, os) = match rust_target {
        Some(target) => (
            target.split('-').next().unwrap_or_default(),
            ["windows", "linux", "darwin"]
                .into_iter()
                .find(|os| target.contains(os))?,
        ),
        None => (std::env::consts::ARCH, std::env::consts::OS),
    };
    let os = if os == "darwin" { "macos" } else { os };
    let arch = match arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => return None,
    };
    PLATFORMS.into_iter().find(|p| *p == format!("{os}-{arch}"))
}

/// A library's name: what the game asks for.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'))
}

/// A file name of one component that stays in the folder it is put in.
fn valid_file(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

fn valid_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A path inside the bundle: relative, with no step out of it.
fn inside(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', ':'])
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

/// The SHA-256 of the file at `path`, lowercase hex.
pub(super) fn sha256_of(path: &Path) -> Result<String, String> {
    let error = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut file = fs::File::open(path).map_err(error)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer).map_err(error)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The libraries `mods` declare for `platform`, each found (in its bundle,
/// else in `supplied`) and with the declared SHA-256. Two mods may share a
/// library only as the same file.
pub(super) fn wanted(
    mods: &[Bundle],
    platform: Option<&str>,
    supplied: &Path,
) -> Result<Vec<Library>, String> {
    let mut out: Vec<Library> = Vec::new();
    for bundle in mods {
        let path = bundle.dir.join(MANIFEST);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let manifest: Manifest =
            toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let refuse = |what: String| format!("{}: {what}", path.display());
        for library in &manifest.library {
            if !valid_name(&library.name) {
                return Err(refuse(format!("invalid library name {:?}", library.name)));
            }
            if library.version.is_empty() {
                return Err(refuse(format!("{} has no version", library.name)));
            }
            if !PLATFORMS.contains(&library.platform.as_str()) {
                return Err(refuse(format!(
                    "{}: unknown platform {:?} (one of {})",
                    library.name,
                    library.platform,
                    PLATFORMS.join(", ")
                )));
            }
            if !valid_file(&library.file) {
                return Err(refuse(format!(
                    "{}: {:?} is not a plain file name",
                    library.name, library.file
                )));
            }
            if !valid_sha256(&library.sha256) {
                return Err(refuse(format!(
                    "{}: sha256 must be 64 lowercase hex digits",
                    library.name
                )));
            }
            if library.path.as_deref().is_some_and(|p| !inside(p)) {
                return Err(refuse(format!(
                    "{}: path must stay inside the mod's folder",
                    library.name
                )));
            }
        }
        let mut names: Vec<&str> = manifest.library.iter().map(|l| l.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        for name in names {
            let Some(platform) = platform else {
                return Err(refuse(format!(
                    "{name}: this build's platform has no name for a library"
                )));
            };
            let mut builds = manifest
                .library
                .iter()
                .filter(|l| l.name == name && l.platform == platform);
            let library = builds
                .next()
                .ok_or_else(|| refuse(format!("{name} has no file for {platform}")))?;
            if builds.next().is_some() {
                return Err(refuse(format!("{name} is declared twice for {platform}")));
            }
            let found = find(bundle, library, supplied)?;
            if let Some(other) = out
                .iter()
                .find(|o| o.name == found.name || o.file == found.file)
            {
                if (&other.name, &other.file, &other.sha256)
                    != (&found.name, &found.file, &found.sha256)
                {
                    return Err(format!(
                        "{} and {} declare different files for library {} ({}): only one can be installed.",
                        other.mod_id, found.mod_id, found.name, found.file
                    ));
                }
                continue;
            }
            out.push(found);
        }
    }
    Ok(out)
}

/// `library`'s file: the bundle's own, else the one the player supplied, and
/// only with the declared hash.
fn find(bundle: &Bundle, library: &Declared, supplied: &Path) -> Result<Library, String> {
    let carried = match &library.path {
        Some(path) => bundle.dir.join(path),
        None => bundle.dir.join("data").join(&library.file),
    };
    let source = [carried, supplied.join(&library.file)]
        .into_iter()
        .find(|p| fs::symlink_metadata(p).is_ok_and(|m| m.is_file()))
        .ok_or_else(|| {
            format!(
                "{id} needs the library {name} {version}, which is not installed. Horizon never downloads a library: get {file} yourself from a source you trust and put it in {dir}. It is used only if its SHA-256 is {sha256}.",
                id = bundle.id,
                name = library.name,
                version = library.version,
                file = library.file,
                dir = supplied.display(),
                sha256 = library.sha256,
            )
        })?;
    let actual = sha256_of(&source)?;
    if actual != library.sha256 {
        return Err(format!(
            "{id}: {path} is not the {name} {version} the mod declared: its SHA-256 is {actual}, the mod expects {expected}. The mod was not built.",
            id = bundle.id,
            path = source.display(),
            name = library.name,
            version = library.version,
            expected = library.sha256,
        ));
    }
    Ok(Library {
        mod_id: bundle.id.clone(),
        name: library.name.clone(),
        version: library.version.clone(),
        file: library.file.clone(),
        sha256: actual,
        source,
    })
}

/// What a build's record says of its libraries (`native-mods.json`).
pub(super) fn about(libraries: &[Library]) -> serde_json::Value {
    libraries
        .iter()
        .map(|l| {
            serde_json::json!({
                "mod": l.mod_id,
                "name": l.name,
                "version": l.version,
                "file": l.file,
                "sha256": l.sha256,
            })
        })
        .collect()
}

/// Copies `libraries` into version folder `version` and lists them for the
/// game. Each copy is hashed again: a file changed since [`wanted`] read it
/// is not installed.
pub(super) fn install(libraries: &[Library], version: &Path) -> Result<(), String> {
    if libraries.is_empty() {
        return Ok(());
    }
    let dir = version.join(DIR);
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for library in libraries {
        let to = dir.join(&library.file);
        fs::copy(&library.source, &to).map_err(|e| format!("{}: {e}", library.source.display()))?;
        let copied = sha256_of(&to)?;
        if copied != library.sha256 {
            return Err(format!(
                "{} changed while it was installed: its SHA-256 is {copied}, not {}.",
                library.source.display(),
                library.sha256
            ));
        }
    }
    let list = serde_json::json!({ "libraries": about(libraries) });
    let path = version.join(INSTALLED);
    fs::write(&path, list.to_string()).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // SHA-256 of "abc" and of "abd".
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const ABD: &str = "a52d159f262b2c6ddb724a61840befc36eb30c88877a4030b65cbe86298449c9";

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "horizon-native-libraries-{name}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        /// A bundle `id` with `manifest` as its `native-libraries.toml`.
        fn bundle(&self, id: &str, manifest: &str) -> Bundle {
            let dir = self.0.join(id);
            fs::create_dir_all(dir.join("data")).unwrap();
            fs::write(dir.join(MANIFEST), manifest).unwrap();
            Bundle {
                id: id.into(),
                version: "1.0.0".into(),
                dir,
                horizon: None,
                mixins: None,
                hooks: None,
                api: None,
                mixins_content: true,
                injections: Vec::new(),
            }
        }
        fn supplied(&self) -> PathBuf {
            let dir = self.0.join("supplied");
            fs::create_dir_all(&dir).unwrap();
            dir
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn declared(sha256: &str) -> String {
        format!(
            "[[library]]\nname = \"libmpv\"\nversion = \"0.40.0\"\nplatform = \"windows-x64\"\nfile = \"libmpv-2.dll\"\nsha256 = \"{sha256}\"\n"
        )
    }

    const WINDOWS: Option<&str> = Some("windows-x64");

    #[test]
    fn a_build_platform_is_named_from_its_rust_target() {
        assert_eq!(platform(Some("x86_64-pc-windows-gnu")), WINDOWS);
        assert_eq!(
            platform(Some("aarch64-pc-windows-gnullvm")),
            Some("windows-arm64")
        );
        assert_eq!(
            platform(Some("x86_64-unknown-linux-gnu")),
            Some("linux-x64")
        );
        assert_eq!(platform(Some("aarch64-apple-darwin")), Some("macos-arm64"));
        assert_eq!(platform(Some("riscv64gc-unknown-linux-gnu")), None);
        assert!(platform(None).is_none_or(|p| PLATFORMS.contains(&p)));
    }

    #[test]
    fn a_library_with_the_declared_hash_is_installed_and_listed() {
        let scratch = Scratch::new("install");
        let bundle = scratch.bundle("video", &declared(ABC));
        fs::write(bundle.dir.join("data").join("libmpv-2.dll"), b"abc").unwrap();
        let libraries = wanted(&[bundle], WINDOWS, &scratch.supplied()).unwrap();
        assert_eq!(libraries.len(), 1);
        assert_eq!(libraries[0].sha256, ABC);

        let version = scratch.0.join("version");
        install(&libraries, &version).unwrap();
        assert_eq!(
            fs::read(version.join(DIR).join("libmpv-2.dll")).unwrap(),
            b"abc"
        );
        let list: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(version.join(INSTALLED)).unwrap()).unwrap();
        assert_eq!(
            list,
            serde_json::json!({ "libraries": [{
                "mod": "video", "name": "libmpv", "version": "0.40.0",
                "file": "libmpv-2.dll", "sha256": ABC,
            }] })
        );
    }

    #[test]
    fn a_library_with_another_hash_stops_the_build() {
        let scratch = Scratch::new("hash");
        let bundle = scratch.bundle("video", &declared(ABC));
        fs::write(bundle.dir.join("data").join("libmpv-2.dll"), b"abd").unwrap();
        let error = wanted(&[bundle], WINDOWS, &scratch.supplied()).unwrap_err();
        assert!(error.contains(ABD) && error.contains(ABC), "{error}");
        assert!(error.contains("was not built"), "{error}");
    }

    #[test]
    fn a_library_the_bundle_lacks_comes_from_the_players_folder_or_not_at_all() {
        let scratch = Scratch::new("supplied");
        let bundle = scratch.bundle("video", &declared(ABC));
        let supplied = scratch.supplied();
        let error = wanted(std::slice::from_ref(&bundle), WINDOWS, &supplied).unwrap_err();
        assert!(error.contains("never downloads"), "{error}");
        assert!(error.contains(&supplied.display().to_string()), "{error}");

        fs::write(supplied.join("libmpv-2.dll"), b"abc").unwrap();
        let libraries = wanted(&[bundle], WINDOWS, &supplied).unwrap();
        assert_eq!(libraries[0].source, supplied.join("libmpv-2.dll"));
    }

    #[test]
    fn a_mod_without_a_file_for_the_platform_is_refused() {
        let scratch = Scratch::new("platform");
        let bundle = scratch.bundle("video", &declared(ABC));
        let error = wanted(&[bundle], Some("linux-x64"), &scratch.supplied()).unwrap_err();
        assert!(error.contains("no file for linux-x64"), "{error}");
        // A mod that declares nothing needs nothing.
        let plain = scratch.bundle("plain", "");
        assert!(wanted(&[plain], None, &scratch.supplied())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_manifest_cannot_name_a_file_outside_its_folders() {
        let scratch = Scratch::new("escape");
        for (key, value) in [
            ("file", "../libmpv-2.dll"),
            ("file", "a/b.dll"),
            ("file", "C:evil.dll"),
            ("path", "../../evil.dll"),
            ("path", "/etc/evil.so"),
            ("path", "C:/evil.dll"),
            ("sha256", "abc"),
            ("name", "Lib MPV"),
            ("platform", "amiga"),
        ] {
            let mut entry = toml::toml! {
                name = "libmpv"
                version = "0.40.0"
                platform = "windows-x64"
                file = "libmpv-2.dll"
                sha256 = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
            };
            entry.insert(key.into(), value.into());
            let mut manifest = toml::Table::new();
            manifest.insert("library".into(), toml::Value::Array(vec![entry.into()]));
            let bundle = scratch.bundle("video", &manifest.to_string());
            fs::write(bundle.dir.join("data").join("libmpv-2.dll"), b"abc").unwrap();
            assert!(
                wanted(&[bundle], WINDOWS, &scratch.supplied()).is_err(),
                "{key} = {value}"
            );
        }
        // And nothing but the keys above.
        let unknown = scratch.bundle("video", &format!("{}url = \"x\"\n", declared(ABC)));
        assert!(wanted(&[unknown], WINDOWS, &scratch.supplied()).is_err());
    }

    #[test]
    fn two_mods_share_a_library_only_as_the_same_file() {
        let scratch = Scratch::new("shared");
        let supplied = scratch.supplied();
        let first = scratch.bundle("first", &declared(ABC));
        fs::write(first.dir.join("data").join("libmpv-2.dll"), b"abc").unwrap();
        let same = scratch.bundle("same", &declared(ABC));
        fs::write(same.dir.join("data").join("libmpv-2.dll"), b"abc").unwrap();
        let other = scratch.bundle("other", &declared(ABD));
        fs::write(other.dir.join("data").join("libmpv-2.dll"), b"abd").unwrap();

        let shared = wanted(&[first.clone(), same], WINDOWS, &supplied).unwrap();
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0].mod_id, "first");
        let error = wanted(&[first, other], WINDOWS, &supplied).unwrap_err();
        assert!(error.contains("only one can be installed"), "{error}");
    }

    #[test]
    fn a_file_changed_after_the_check_is_not_installed() {
        let scratch = Scratch::new("changed");
        let bundle = scratch.bundle("video", &declared(ABC));
        let file = bundle.dir.join("data").join("libmpv-2.dll");
        fs::write(&file, b"abc").unwrap();
        let libraries = wanted(&[bundle], WINDOWS, &scratch.supplied()).unwrap();
        fs::write(&file, b"abd").unwrap();
        let error = install(&libraries, &scratch.0.join("version")).unwrap_err();
        assert!(error.contains("changed while it was installed"), "{error}");
    }
}
