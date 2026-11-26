//! Checked file access to the folders of the namespaces a mod's manifest owns
//! (`ConfigHost::mod_file_read` and its siblings).
//!
//! A mod that takes over another mod's ids also takes over its files, at the paths packs and
//! worlds already use: `config/<namespace>/…`, `<world>/<namespace>/…` and
//! `<world>/serverconfig/<namespace>-….` These calls reach exactly those folders, for the
//! calling mod's own id and every namespace its manifest's `namespaces` lists, and refuse
//! everything else before touching the disk.

/// Which folder a mod file call names, under the namespace it is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, sunrise_mod_api_macros::Wire)]
#[non_exhaustive]
pub enum ModFolder {
    /// `config/<namespace>/` in the game's config folder (`ConfigHost::config_dir`), as
    /// NeoForge mods keep their own folder in `FMLPaths.CONFIGDIR`.
    Config,
    /// `<world>/<namespace>/` in the open world's folder (`ConfigHost::world_dir`), carried by
    /// the world's backups.
    World,
    /// The open world's `serverconfig/` folder itself, for files named `<namespace>-…` only
    /// (NeoForge's per-world server configs, `<id>-server.toml`): `path` is a bare file name.
    ServerConfig,
}

/// The most bytes one mod file read or write moves: 16 MiB.
pub const MAX_MOD_FILE_BYTES: usize = 16 * 1024 * 1024;
/// The most names one [`ModFolder`] listing returns.
pub const MAX_MOD_FILE_LIST: usize = 4096;
/// The longest relative path a mod file call takes, in bytes.
pub const MAX_MOD_FILE_PATH: usize = 1024;

/// A mod file call was refused, or the disk failed it.
#[derive(Clone, Debug, PartialEq, Eq, sunrise_mod_api_macros::Wire)]
#[non_exhaustive]
pub enum ModFileError {
    /// Not inside a mod's call, so there is no mod to check ownership for.
    NoMod,
    /// The calling mod's manifest doesn't own this namespace (its own id is always owned).
    NotOwned,
    /// The namespace isn't a valid one (`[a-z0-9_.-]`, 1 to 64 bytes).
    BadNamespace,
    /// The path is empty, absolute, too long, has an empty, `.` or `..` part, a backslash,
    /// a colon, a NUL or a name Windows reserves (`CON`, `NUL`, `COM1` …), ends in a dot or
    /// space, or, for [`ModFolder::ServerConfig`], isn't a bare `<namespace>-…` file name.
    BadPath,
    /// [`ModFolder::World`] or [`ModFolder::ServerConfig`] with no world open on this side
    /// (a client of a remote server, the title screen).
    NoWorld,
    /// The file or folder isn't there.
    NotFound,
    /// The path leads through a symbolic link or names a folder where a file is wanted (or a
    /// file where a folder is wanted).
    NotAFile,
    /// More than [`MAX_MOD_FILE_BYTES`] to read or write, or more than [`MAX_MOD_FILE_LIST`]
    /// names in a folder.
    TooLarge,
    /// The disk refused the call; the text is the system's.
    Io(String),
}

impl std::fmt::Display for ModFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoMod => "mod file calls work only inside a mod's call",
            Self::NotOwned => "the calling mod's manifest doesn't own this namespace",
            Self::BadNamespace => "not a valid namespace",
            Self::BadPath => "not a valid path inside the namespace's folder",
            Self::NoWorld => "no world is open on this side",
            Self::NotFound => "no such file or folder",
            Self::NotAFile => "a symbolic link or the wrong kind of entry is in the way",
            Self::TooLarge => "over the mod file size or listing limit",
            Self::Io(error) => return write!(f, "mod file: {error}"),
        })
    }
}

impl std::error::Error for ModFileError {}
