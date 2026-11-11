//! libmpv in the game's process (docs/native-libraries.md).
//!
//! Nothing links against libmpv: [`Mpv::open`] opens the library by its full
//! path, and [`library::verified`] gives that path only for a file the
//! launcher installed in the version folder's `native-libraries/` and whose
//! SHA-256 still matches what the mod's manifest declared. The library is
//! never looked for on `PATH` or in the working directory, and never
//! downloaded.
//!
//! A [`Player`] plays one local file and renders it with mpv's software
//! render API on its own thread into RGBA frames ([`Player::take_frame`]),
//! which the game uploads into an atlas slot. mpv plays the sound itself
//! through its own audio output.
//!
//! The bindings are written against mpv's documented client API (`client.h`,
//! `render.h`, version 2); no mpv code is part of this crate.

pub mod library;
mod audio_pipe;
mod player;
mod sys;

pub use player::{MAX_SIDE, Options, Player, Stats, local_file};

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Why libmpv could not be opened or a player could not be made.
#[derive(Debug)]
pub enum Error {
    /// The version folder has no `native-libraries.json`, or it does not list the library.
    NotInstalled(String),
    /// The manifest or the path in it is not what the launcher writes.
    Manifest(String),
    /// The file's SHA-256 is not the declared one.
    Hash {
        file: PathBuf,
        expected: String,
        actual: String,
    },
    /// The path is not absolute: a relative one would be searched for.
    Relative(PathBuf),
    /// The system could not load the library or one of its functions.
    Load(String),
    /// The library is not a libmpv this crate knows (client API 2).
    Version(u32),
    /// An mpv call failed: what was called and mpv's error text.
    Mpv(&'static str, String),
    /// Not a local file mpv may play (`local_file`).
    NotLocal(PathBuf),
    /// A frame size mpv cannot render or the game cannot show.
    Size(u32, u32),
    Io(PathBuf, std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInstalled(what) => write!(f, "native library not installed: {what}"),
            Self::Manifest(what) => write!(f, "native library manifest: {what}"),
            Self::Hash {
                file,
                expected,
                actual,
            } => write!(
                f,
                "{}: SHA-256 is {actual}, the mod declared {expected}",
                file.display()
            ),
            Self::Relative(path) => write!(
                f,
                "{}: a native library is opened by its full path only",
                path.display()
            ),
            Self::Load(what) => write!(f, "could not load libmpv: {what}"),
            Self::Version(version) => write!(
                f,
                "libmpv client API {}.{} is not supported (needs 2.x)",
                version >> 16,
                version & 0xffff
            ),
            Self::Mpv(call, what) => write!(f, "{call}: {what}"),
            Self::NotLocal(path) => write!(
                f,
                "{}: only a local file on this computer is played",
                path.display()
            ),
            Self::Size(width, height) => write!(f, "a {width}x{height} frame cannot be rendered"),
            Self::Io(path, error) => write!(f, "{}: {error}", path.display()),
        }
    }
}

impl std::error::Error for Error {}

/// A loaded libmpv. Cheap to clone; the library stays loaded while a clone
/// or a [`Player`] lives.
#[derive(Clone)]
pub struct Mpv {
    api: Arc<sys::Api>,
}

impl Mpv {
    /// Opens the libmpv at `path`, which must be absolute. On Windows its own
    /// dependencies are looked for only beside it and in System32.
    pub fn open(path: &Path) -> Result<Self, Error> {
        if !path.is_absolute() {
            return Err(Error::Relative(path.to_owned()));
        }
        let api = sys::Api::load(path)?;
        let version = api.client_api_version();
        if version >> 16 != 2 {
            return Err(Error::Version(version));
        }
        Ok(Self { api: Arc::new(api) })
    }

    /// Opens library `name` as the launcher installed it in the game's
    /// version folder `version_dir`, after checking its SHA-256
    /// ([`library::verified`]).
    pub fn open_installed(version_dir: &Path, name: &str) -> Result<Self, Error> {
        Self::open(&library::verified(version_dir, name)?)
    }

    /// `mpv_client_api_version()`: (major, minor).
    pub fn client_api_version(&self) -> (u16, u16) {
        let version = self.api.client_api_version();
        ((version >> 16) as u16, version as u16)
    }

    /// A player that renders `width` x `height` RGBA frames.
    pub fn player(&self, width: u32, height: u32, options: &Options) -> Result<Player, Error> {
        Player::new(self.api.clone(), width, height, options)
    }
}
