//! `ConfigHost::mod_file_read`, `mod_file_write`, `mod_file_remove` and `mod_file_list`: checked
//! file access to `config/<namespace>/`, `<world>/<namespace>/` and the world's
//! `serverconfig/<namespace>-…` files, for the namespaces the calling mod owns
//! (`crash::current_native_mod`, `owns_namespace`). Every refusal happens before the disk is
//! touched, and a path never leaves its folder: no `..`, no absolute or drive path, and no
//! symbolic link on the way.
use super::Game;
use mod_api::{MAX_MOD_FILE_BYTES, MAX_MOD_FILE_LIST, MAX_MOD_FILE_PATH, ModFileError, ModFolder};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

/// A namespace as `ResourceLocation` allows one: `[a-z0-9_.-]`, 1 to 64 bytes here.
fn valid_namespace(namespace: &str) -> bool {
    (1..=64).contains(&namespace.len())
        && namespace
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-'))
        && namespace != "."
        && namespace != ".."
}

/// One part of a relative path that names the same entry on every system: not empty, `.` or
/// `..`, without a separator, colon or control character, not ending in a dot or space (Windows
/// drops those), and not a name Windows reserves for a device.
fn valid_part(part: &str) -> bool {
    if part.is_empty() || part == "." || part == ".." || part.ends_with(['.', ' ']) {
        return false;
    }
    if part
        .chars()
        .any(|c| c.is_control() || matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
    {
        return false;
    }
    let stem = part.split('.').next().unwrap_or(part).to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
    !device
}

/// The parts of `path`, or `BadPath`. An empty path is allowed only where `empty_ok` (a listing
/// of the namespace's folder itself).
fn parts(path: &str, empty_ok: bool) -> Result<Vec<&str>, ModFileError> {
    if path.is_empty() {
        return if empty_ok {
            Ok(Vec::new())
        } else {
            Err(ModFileError::BadPath)
        };
    }
    if path.len() > MAX_MOD_FILE_PATH {
        return Err(ModFileError::BadPath);
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.iter().all(|part| valid_part(part)) {
        Ok(parts)
    } else {
        Err(ModFileError::BadPath)
    }
}

/// Where a call lands: the folder it can't leave (`base`, under `root`) and the entry in it.
#[derive(Debug, PartialEq)]
struct Target {
    /// The first folder below `root` the call may touch (`<root>/<namespace>` or
    /// `<world>/serverconfig`); it and everything under it is checked for links.
    base: PathBuf,
    /// The parts below `base`.
    parts: Vec<String>,
    /// A `serverconfig/` call: only `<namespace>-…` files, no folders.
    prefix: Option<String>,
}

impl Target {
    fn new(
        root: &Path,
        folder: ModFolder,
        namespace: &str,
        path: &str,
        listing: bool,
    ) -> Result<Self, ModFileError> {
        let prefix = format!("{namespace}-");
        let (base, prefix) = match folder {
            ModFolder::ServerConfig => {
                let parts = parts(path, listing)?;
                let bare = match parts.as_slice() {
                    [] => listing,
                    [name] => !listing && name.starts_with(&prefix) && name.len() > prefix.len(),
                    _ => false,
                };
                if !bare {
                    return Err(ModFileError::BadPath);
                }
                (root.join("serverconfig"), Some(prefix))
            }
            // `Config` and `World`; a later folder is a namespace's own folder too.
            _ => (root.join(namespace), None),
        };
        let parts = parts(path, listing)?
            .into_iter()
            .map(str::to_owned)
            .collect();
        Ok(Self {
            base,
            parts,
            prefix,
        })
    }

    fn path(&self) -> PathBuf {
        let mut path = self.base.clone();
        path.extend(&self.parts);
        path
    }

    /// `NotAFile` when `base` or an entry below it on the way is a symbolic link (or a
    /// Windows junction); entries not there yet are fine.
    fn check_links(&self) -> Result<(), ModFileError> {
        let mut path = self.base.clone();
        for part in std::iter::once(None).chain(self.parts.iter().map(Some)) {
            if let Some(part) = part {
                path.push(part);
            }
            match fs::symlink_metadata(&path) {
                Ok(meta) if is_link(&meta) => return Err(ModFileError::NotAFile),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(io_error(error)),
            }
        }
        Ok(())
    }
}

fn is_link(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_REPARSE_POINT: symbolic links and junctions alike.
        meta.file_type().is_symlink() || meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

fn io_error(error: io::Error) -> ModFileError {
    match error.kind() {
        io::ErrorKind::NotFound => ModFileError::NotFound,
        io::ErrorKind::IsADirectory | io::ErrorKind::NotADirectory => ModFileError::NotAFile,
        _ => ModFileError::Io(error.to_string()),
    }
}

fn read(target: &Target) -> Result<Vec<u8>, ModFileError> {
    target.check_links()?;
    let path = target.path();
    let meta = fs::metadata(&path).map_err(io_error)?;
    if !meta.is_file() {
        return Err(ModFileError::NotAFile);
    }
    if meta.len() > MAX_MOD_FILE_BYTES as u64 {
        return Err(ModFileError::TooLarge);
    }
    let bytes = fs::read(&path).map_err(io_error)?;
    // It may have grown since the length was read.
    if bytes.len() > MAX_MOD_FILE_BYTES {
        return Err(ModFileError::TooLarge);
    }
    Ok(bytes)
}

fn write(target: &Target, bytes: &[u8]) -> Result<(), ModFileError> {
    if bytes.len() > MAX_MOD_FILE_BYTES {
        return Err(ModFileError::TooLarge);
    }
    target.check_links()?;
    let path = target.path();
    if path.is_dir() {
        return Err(ModFileError::NotAFile);
    }
    let parent = path.parent().ok_or(ModFileError::BadPath)?;
    fs::create_dir_all(parent).map_err(io_error)?;
    // Made folders are checked again: another program could have put a link there meanwhile.
    target.check_links()?;
    let name = path.file_name().ok_or(ModFileError::BadPath)?;
    let mut temp_name = std::ffi::OsString::from(".");
    temp_name.push(name);
    temp_name.push(format!(".sunrise-{}.tmp", std::process::id()));
    let temp = parent.join(temp_name);
    let written = (|| {
        let mut file = fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        // `rename` replaces an existing file on every platform the game runs on.
        fs::rename(&temp, &path)
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&temp);
        return Err(io_error(error));
    }
    Ok(())
}

fn remove(target: &Target) -> Result<bool, ModFileError> {
    target.check_links()?;
    let path = target.path();
    match fs::symlink_metadata(&path) {
        Ok(meta) if !meta.is_file() => return Err(ModFileError::NotAFile),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(io_error(error)),
    }
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(error)),
    }
}

fn list(target: &Target) -> Result<Vec<String>, ModFileError> {
    target.check_links()?;
    let path = target.path();
    let entries = match fs::read_dir(&path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error(error)),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(io_error)?;
        let meta = entry.metadata().map_err(io_error)?;
        // A name this host can't spell as text can't be named back either.
        let (false, Some(name)) = (
            is_link(&meta),
            entry.file_name().to_str().map(str::to_owned),
        ) else {
            continue;
        };
        let name = match &target.prefix {
            Some(prefix) if meta.is_file() && name.starts_with(prefix.as_str()) => name,
            Some(_) => continue,
            None if meta.is_dir() => format!("{name}/"),
            None => name,
        };
        if names.len() == MAX_MOD_FILE_LIST {
            return Err(ModFileError::TooLarge);
        }
        names.push(name);
    }
    names.sort();
    Ok(names)
}

impl Game<'_> {
    /// Where the calling mod's call lands, or why it is refused, before any disk access.
    fn mod_file_target(
        &self,
        folder: ModFolder,
        namespace: &str,
        path: &str,
        listing: bool,
    ) -> Result<Target, ModFileError> {
        let owner = mod_api::content::crash::current_native_mod().ok_or(ModFileError::NoMod)?;
        if !valid_namespace(namespace) {
            return Err(ModFileError::BadNamespace);
        }
        if !mod_api::__private::owns_namespace(owner, namespace) {
            return Err(ModFileError::NotOwned);
        }
        let root = match folder {
            ModFolder::Config => self.config_dir().to_path_buf(),
            _ => self.world_dir().ok_or(ModFileError::NoWorld)?,
        };
        Target::new(&root, folder, namespace, path, listing)
    }

    pub(in crate::game) fn mod_file_read(
        &self,
        folder: ModFolder,
        namespace: &str,
        path: &str,
    ) -> Result<Vec<u8>, ModFileError> {
        read(&self.mod_file_target(folder, namespace, path, false)?)
    }

    pub(in crate::game) fn mod_file_write(
        &self,
        folder: ModFolder,
        namespace: &str,
        path: &str,
        bytes: &[u8],
    ) -> Result<(), ModFileError> {
        write(
            &self.mod_file_target(folder, namespace, path, false)?,
            bytes,
        )
    }

    pub(in crate::game) fn mod_file_remove(
        &self,
        folder: ModFolder,
        namespace: &str,
        path: &str,
    ) -> Result<bool, ModFileError> {
        remove(&self.mod_file_target(folder, namespace, path, false)?)
    }

    pub(in crate::game) fn mod_file_list(
        &self,
        folder: ModFolder,
        namespace: &str,
        dir: &str,
    ) -> Result<Vec<String>, ModFileError> {
        list(&self.mod_file_target(folder, namespace, dir, true)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(root: &Path, folder: ModFolder, path: &str) -> Result<Target, ModFileError> {
        Target::new(root, folder, "ftbquests", path, false)
    }

    #[test]
    fn paths_that_could_leave_the_folder_are_refused() {
        let root = Path::new("root");
        for bad in [
            "",
            "/abs",
            "a//b",
            "a/./b",
            "../x",
            "a/../../x",
            "a\\b",
            "C:x",
            "nul.txt",
            "COM1",
            "lpt9.log",
            "trailing.",
            "trailing ",
            "a\0b",
        ] {
            assert_eq!(
                target(root, ModFolder::Config, bad),
                Err(ModFileError::BadPath),
                "{bad:?}"
            );
        }
        let ok = target(root, ModFolder::Config, "quests/chapters/one.snbt").unwrap();
        assert_eq!(
            ok.path(),
            Path::new("root/ftbquests/quests/chapters/one.snbt")
        );
        assert!(valid_part("console.txt") && valid_part("com10") && valid_part(".hidden"));
        assert!(!valid_namespace("FTB") && !valid_namespace("") && !valid_namespace(".."));
        assert!(valid_namespace("ftbquests") && valid_namespace("my_mod.extra-1"));
    }

    #[test]
    fn serverconfig_takes_only_the_namespaces_bare_file_names() {
        let root = Path::new("world");
        let ok = target(root, ModFolder::ServerConfig, "ftbquests-server.snbt").unwrap();
        assert_eq!(
            ok.path(),
            Path::new("world/serverconfig/ftbquests-server.snbt")
        );
        for bad in [
            "ftbquests-",
            "other-server.toml",
            "ftbquests-a/b",
            "ftbquests",
        ] {
            assert_eq!(
                target(root, ModFolder::ServerConfig, bad),
                Err(ModFileError::BadPath),
                "{bad:?}"
            );
        }
        assert!(Target::new(root, ModFolder::ServerConfig, "ftbquests", "", true).is_ok());
        assert_eq!(
            Target::new(root, ModFolder::ServerConfig, "ftbquests", "sub", true),
            Err(ModFileError::BadPath)
        );
    }

    #[test]
    fn a_write_replaces_the_file_whole_and_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = target(dir.path(), ModFolder::World, "teams/red.snbt").unwrap();
        assert_eq!(read(&file), Err(ModFileError::NotFound));
        write(&file, b"first").unwrap();
        write(&file, b"second").unwrap();
        assert_eq!(read(&file).unwrap(), b"second");
        let folder = dir.path().join("ftbquests/teams");
        let names: Vec<_> = fs::read_dir(&folder)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["red.snbt"], "no temporary file is left");

        let top = Target::new(dir.path(), ModFolder::World, "ftbquests", "", true).unwrap();
        assert_eq!(list(&top).unwrap(), ["teams/"]);
        let teams = Target::new(dir.path(), ModFolder::World, "ftbquests", "teams", true).unwrap();
        assert_eq!(list(&teams).unwrap(), ["red.snbt"]);
        let missing = Target::new(dir.path(), ModFolder::World, "ftbquests", "none", true).unwrap();
        assert_eq!(list(&missing).unwrap(), Vec::<String>::new());

        // A folder where a file is wanted is refused, and so is a file over the limit.
        let as_file = target(dir.path(), ModFolder::World, "teams").unwrap();
        assert_eq!(read(&as_file), Err(ModFileError::NotAFile));
        assert_eq!(write(&as_file, b"x"), Err(ModFileError::NotAFile));
        assert_eq!(remove(&as_file), Err(ModFileError::NotAFile));
        let big = vec![0; MAX_MOD_FILE_BYTES + 1];
        assert_eq!(write(&file, &big), Err(ModFileError::TooLarge));
        assert_eq!(
            read(&file).unwrap(),
            b"second",
            "a refused write changes nothing"
        );

        assert_eq!(remove(&file), Ok(true));
        assert_eq!(remove(&file), Ok(false));
    }

    #[test]
    fn serverconfig_listing_shows_only_the_namespaces_files() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("serverconfig");
        fs::create_dir_all(config.join("ftbquests-dir")).unwrap();
        for name in [
            "ftbquests-server.snbt",
            "ftbquests-common.toml",
            "other-server.toml",
        ] {
            fs::write(config.join(name), "x").unwrap();
        }
        let all = Target::new(dir.path(), ModFolder::ServerConfig, "ftbquests", "", true).unwrap();
        assert_eq!(
            list(&all).unwrap(),
            ["ftbquests-common.toml", "ftbquests-server.snbt"]
        );
    }

    #[test]
    fn a_link_on_the_way_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), "no").unwrap();
        fs::create_dir_all(dir.path().join("ftbquests")).unwrap();
        let link = dir.path().join("ftbquests/out");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        // Windows makes directory links only with Developer Mode on (or as an administrator).
        #[cfg(windows)]
        if let Err(error) = std::os::windows::fs::symlink_dir(outside.path(), &link) {
            eprintln!("skipping the link check: no symlink here ({error})");
            return;
        }
        let through = target(dir.path(), ModFolder::Config, "out/secret.txt").unwrap();
        assert_eq!(read(&through), Err(ModFileError::NotAFile));
        assert_eq!(write(&through, b"x"), Err(ModFileError::NotAFile));
        let listing = Target::new(dir.path(), ModFolder::Config, "ftbquests", "", true).unwrap();
        assert_eq!(
            list(&listing).unwrap(),
            Vec::<String>::new(),
            "links are left out"
        );
        assert_eq!(
            fs::read_to_string(outside.path().join("secret.txt")).unwrap(),
            "no"
        );
    }
}
