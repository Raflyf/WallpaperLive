use crate::core::{live_extension, supported_extension, validate_media, validate_path_syntax};
use std::{
    collections::BTreeMap,
    fs,
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
};
use windows::{Win32::Storage::FileSystem::GetDriveTypeW, core::PCWSTR};

pub const MAX_FILES: usize = 4096;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Newest,
    NameAsc,
    NameDesc,
}
impl Sort {
    pub const OPTIONS: [Self; 3] = [Self::Newest, Self::NameAsc, Self::NameDesc];

    pub fn label(self) -> &'static str {
        match self {
            Self::Newest => "Newest added",
            Self::NameAsc => "Name: A to Z",
            Self::NameDesc => "Name: Z to A",
        }
    }
}

fn key(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
        .collect::<Vec<_>>()
        .join("\\")
}
fn in_folder(path: &Path, folder: &Path) -> bool {
    let mut parts = path.components();
    folder.components().all(|parent| {
        parts.next().is_some_and(|part| {
            part.as_os_str().to_string_lossy().to_lowercase()
                == parent.as_os_str().to_string_lossy().to_lowercase()
        })
    })
}
fn clean_directory(path: &Path) -> Result<PathBuf, String> {
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("Could not read folder: {e}"))?;
    let text = canonical
        .to_str()
        .ok_or("Folder name is not valid Unicode")?;
    let clean = PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(text));
    validate_path_syntax(&clean)?;
    Ok(clean)
}

pub fn scan_folder(source: &Path) -> Result<Vec<PathBuf>, String> {
    validate_path_syntax(source)?;
    let metadata =
        fs::symlink_metadata(source).map_err(|e| format!("Could not read folder: {e}"))?;
    if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
        return Err("Choose a local folder, not a link or reparse point".into());
    }
    let root = clean_directory(source)?;
    let text = root.to_str().ok_or("Folder name is not valid Unicode")?;
    let drive: Vec<u16> = text[..3].encode_utf16().chain(Some(0)).collect();
    // SAFETY: validated canonical drive root; this query does not change the drive.
    if !matches!(unsafe { GetDriveTypeW(PCWSTR(drive.as_ptr())) }, 2 | 3 | 5) {
        return Err("Choose a folder on a local drive, not a network drive".into());
    }
    let mut pending = vec![root.clone()];
    let mut media = BTreeMap::new();
    let mut entries = 0;
    while let Some(folder) = pending.pop() {
        let metadata = fs::symlink_metadata(&folder)
            .map_err(|e| format!("Could not read {}: {e}", folder.display()))?;
        if metadata.file_attributes() & 0x400 != 0 || !metadata.is_dir() {
            continue;
        }
        let directory = fs::read_dir(&folder)
            .map_err(|e| format!("Could not scan {}: {e}", folder.display()))?;
        for entry in directory {
            entries += 1;
            // ponytail: scans stop at 65,536 entries; use a cancellable background inventory for larger trees.
            if entries > MAX_FILES * 16 {
                return Err("Folder scan exceeds 65,536 entries. Choose a smaller folder.".into());
            }
            let path = entry
                .map_err(|e| format!("Could not read a folder entry: {e}"))?
                .path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|e| format!("Could not read {}: {e}", path.display()))?;
            if metadata.file_attributes() & 0x400 != 0 || metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                let folder = clean_directory(&path)?;
                if !in_folder(&folder, &root) {
                    return Err("Folder changed during import. No files were imported.".into());
                }
                pending.push(folder);
            } else if metadata.is_file() && live_extension(&path) {
                let file = validate_media(&path)?;
                if !in_folder(&file, &root) {
                    return Err("File changed during import. No files were imported.".into());
                }
                media.entry(key(&file)).or_insert(file);
                if media.len() > MAX_FILES {
                    return Err(format!(
                        "Folder contains more than {MAX_FILES} media files. Choose a smaller folder."
                    ));
                }
            }
        }
    }
    Ok(media.into_values().collect())
}

pub fn filtered(paths: &[PathBuf], folder: Option<&Path>) -> Vec<PathBuf> {
    paths
        .iter()
        .filter(|path| folder.is_none_or(|folder| in_folder(path, folder)))
        .cloned()
        .collect()
}
pub fn view(paths: &[PathBuf], folder: Option<&Path>, query: &str, sort: Sort) -> Vec<PathBuf> {
    let query = query.trim().to_lowercase();
    let name = |path: &Path| {
        path.file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .to_lowercase()
    };
    let mut result = filtered(paths, folder);
    if !query.is_empty() {
        result.retain(|path| name(path).contains(&query));
    }
    if sort != Sort::Newest {
        result.sort_by_cached_key(|path| (name(path), key(path), path.clone()));
        if sort == Sort::NameDesc {
            result.reverse();
        }
    }
    result
}

pub fn folders(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut folders = BTreeMap::new();
    for path in paths {
        if let Some(parent) = path.parent() {
            folders
                .entry(key(parent))
                .or_insert_with(|| parent.to_owned());
        }
    }
    folders.into_values().collect()
}

pub fn decode(text: &str) -> Result<(Vec<PathBuf>, Vec<PathBuf>), String> {
    let mut lines = text.lines();
    let version = lines.next().ok_or("Library is empty or invalid")?;
    if !matches!(version, "version=1" | "version=2") {
        return Err("Invalid library version".into());
    }
    let mut files = Vec::new();
    let mut roots = Vec::new();
    for line in lines {
        let (folder, text) = if version == "version=1" {
            (false, line)
        } else if let Some(text) = line.strip_prefix("file=") {
            (false, text)
        } else if let Some(text) = line.strip_prefix("folder=") {
            (true, text)
        } else {
            return Err("Invalid library entry".into());
        };
        let path = PathBuf::from(text);
        validate_path_syntax(&path)?;
        if folder {
            if !roots.contains(&path) {
                roots.push(path);
            }
        } else {
            if !supported_extension(&path) {
                return Err("Invalid library media format".into());
            }
            if !files.contains(&path) {
                files.push(path);
            }
        }
        if files.len() > MAX_FILES || roots.len() > MAX_FILES {
            return Err("Library exceeds 4096 entries".into());
        }
    }
    Ok((files, roots))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_combines_search_folder_and_stable_name_order() {
        let paths = [
            r"C:\media\Zulu.mp4",
            r"C:\other\alpha.GIF",
            r"C:\media\alpha.GIF",
            r"C:\media\nested\ALPHA.GIF",
        ]
        .map(PathBuf::from);
        assert_eq!(view(&paths, None, "", Sort::Newest), paths);
        assert_eq!(
            view(
                &paths,
                Some(Path::new(r"C:\MEDIA")),
                " alpha ",
                Sort::Newest
            ),
            paths[2..]
        );
        assert_eq!(
            view(&paths, None, "", Sort::NameAsc),
            [
                paths[2].clone(),
                paths[3].clone(),
                paths[1].clone(),
                paths[0].clone()
            ]
        );
        assert_eq!(
            view(&paths, None, "", Sort::NameDesc),
            [
                paths[0].clone(),
                paths[1].clone(),
                paths[3].clone(),
                paths[2].clone()
            ]
        );
        assert!(view(&paths, None, "not found", Sort::Newest).is_empty());
        assert_eq!(paths[0], PathBuf::from(r"C:\media\Zulu.mp4"));
    }

    #[test]
    fn library_versions_preserve_folder_roots_and_missing_files() {
        let file = PathBuf::from(r"C:\media\nested\missing.mp4");
        assert_eq!(
            decode("version=1\nC:\\media\\nested\\missing.mp4\n").unwrap(),
            (vec![file.clone()], vec![])
        );
        assert_eq!(
            decode("version=2\nfolder=C:\\media\nfile=C:\\media\\nested\\missing.mp4\n").unwrap(),
            (vec![file], vec![PathBuf::from(r"C:\media")])
        );
        assert!(decode("version=2\nfile=https://evil.invalid/a.mp4\n").is_err());
        assert!(decode("version=2\nfolder=C:\\media\nbad=value\n").is_err());
    }

    #[test]
    fn folder_filter_is_case_insensitive_and_component_bounded() {
        let paths = [
            r"C:\Media\one.mp4",
            r"c:\media\nested\two.GIF",
            r"C:\Media-other\three.mp4",
        ]
        .map(PathBuf::from);
        assert_eq!(filtered(&paths, None), paths);
        assert_eq!(filtered(&paths, Some(Path::new(r"C:\MEDIA"))), paths[..2]);
        let parents = folders(&[
            paths[0].clone(),
            PathBuf::from(r"c:\media\four.mp4"),
            paths[1].clone(),
        ]);
        assert_eq!(
            parents,
            [
                PathBuf::from(r"C:\Media"),
                PathBuf::from(r"c:\media\nested")
            ]
        );
    }

    #[test]
    fn recursive_import_keeps_originals_and_ignores_other_formats() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("scratch")
            .join(format!(
                "folder-import-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        fs::create_dir_all(root.join("nested")).unwrap();
        let original = root.join("one.mp4");
        fs::write(&original, b"original fixture, not decoder media").unwrap();
        fs::write(root.join("nested/two.GIF"), b"second fixture").unwrap();
        let preview_jpg = root.join("preview.jpg");
        fs::write(&preview_jpg, b"static jpg").unwrap();
        fs::write(root.join("nested/cover.JPEG"), b"static jpeg").unwrap();
        fs::write(root.join("still.png"), b"static png").unwrap();
        fs::write(root.join("frame.webp"), b"static webp").unwrap();
        fs::write(root.join("thumb.BMP"), b"static bmp").unwrap();
        fs::write(root.join("ignored.txt"), b"not media").unwrap();
        let imported = scan_folder(&root).unwrap();
        assert_eq!(imported.len(), 2);
        assert!(
            imported
                .iter()
                .all(|path| in_folder(path, &root) && live_extension(path))
        );
        assert!(validate_media(&preview_jpg).is_ok());
        assert_eq!(
            fs::read(&original).unwrap(),
            b"original fixture, not decoder media"
        );
        assert!(scan_folder(&original).is_err());
        assert!(scan_folder(Path::new(r"\\server\media")).is_err());
        let oversized = root.join("oversized");
        fs::create_dir(&oversized).unwrap();
        for index in 0..=MAX_FILES {
            fs::write(oversized.join(format!("{index:04}.mp4")), []).unwrap();
        }
        assert!(
            scan_folder(&oversized)
                .unwrap_err()
                .contains("more than 4096")
        );
        assert!(oversized.join("4096.mp4").is_file());
        // Fixtures are retained under scratch; this test never removes original files.
    }
}
