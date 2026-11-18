use std::{
    collections::{BTreeSet, HashSet},
    fs::{self, File, Metadata},
    io::{Cursor, ErrorKind, Read, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use tempfile::{Builder, NamedTempFile, TempDir};
use wasmparser::{Chunk, Parser, Payload};

const SECTION_NAME: &str = "isola:assets";
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_INDEX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Deserialize, Serialize)]
struct Manifest {
    mounts: Vec<Mount>,
}

#[derive(Deserialize, Serialize)]
struct Mount {
    // Keep the serialized keys in canonical CBOR order.
    guest: String,
    source: PathBuf,
}

#[derive(Deserialize, Serialize)]
struct IndexedArchive {
    mounts: Vec<Mount>,
    // The tar body is only checked when at least one mount is needed.
    inventory: Option<ArchiveInventory>,
}

#[derive(Deserialize, Serialize)]
struct ArchiveInventory {
    digest: [u8; blake3::OUT_LEN],
    directories: BTreeSet<PathBuf>,
    files: Vec<(PathBuf, u64)>,
}

#[derive(Default)]
pub struct EmbeddedAssets {
    pub mounts: Vec<EmbeddedMount>,
    /// Uncached extraction directories shared by the template and its
    /// sandboxes.
    pub temp_dirs: Vec<TempDir>,
}

pub struct EmbeddedMount {
    pub guest: String,
    pub path: PathBuf,
}

/// Extract the `isola:assets` archives embedded in `wasm`, skipping any guest
/// path covered by an explicit user mount.
/// With a cache, each archive is extracted into a directory keyed by its
/// content digest, and the runtime index records its expected files.
pub fn prepare(
    wasm: &[u8],
    cache: Option<(&Path, blake3::Hash)>,
    is_overridden: impl Fn(&str) -> bool,
) -> Result<EmbeddedAssets> {
    let mut assets = EmbeddedAssets::default();
    let mut index = Vec::new();
    for (bytes, mounts) in find_archives(wasm)? {
        let mut active = mounts
            .iter()
            .filter(|mount| !is_overridden(&mount.guest))
            .peekable();
        let mut inventory = None;
        if active.peek().is_some() {
            if let Some((root, _)) = cache {
                let (directory, archive_inventory) = prepare_cached_archive(bytes, root)?;
                append_mounts(&mut assets, &directory, active)?;
                inventory = Some(archive_inventory);
            } else {
                let temp = Builder::new().prefix("isola-assets-").tempdir()?;
                unpack_archive(bytes, temp.path(), false, None)?;
                append_mounts(&mut assets, temp.path(), active)?;
                assets.temp_dirs.push(temp);
            }
        }
        if cache.is_some() {
            index.push(IndexedArchive { mounts, inventory });
        }
    }
    if let Some((root, digest)) = cache {
        // The index is an optimization. Its absence always falls back to the
        // runtime.
        let _ = write_index(root, digest, index);
    }
    Ok(assets)
}

/// Use the index only when every active archive still passes its file checks.
pub fn prepare_from_index(
    root: &Path,
    digest: blake3::Hash,
    is_overridden: impl Fn(&str) -> bool,
) -> Result<EmbeddedAssets> {
    let index = read_index(root, digest)?;
    let cache = root.join("assets");
    let mut assets = EmbeddedAssets::default();
    let mut guests = HashSet::new();
    for archive in &index {
        validate_mounts(&archive.mounts)?;
        for mount in &archive.mounts {
            ensure!(
                guests.insert(&mount.guest),
                "duplicate embedded asset mount"
            );
        }
        let mut active = archive
            .mounts
            .iter()
            .filter(|mount| !is_overridden(&mount.guest))
            .peekable();
        if active.peek().is_none() {
            continue;
        }
        let inventory = archive
            .inventory
            .as_ref()
            .context("archive has not been extracted")?;
        ensure!(directory_exists(&cache)?, "missing asset cache");
        let destination = cache.join(blake3::Hash::from(inventory.digest).to_hex().as_str());
        ensure!(
            directory_exists(&destination)?,
            "missing cached asset archive"
        );
        inventory.check(&destination)?;
        append_mounts(&mut assets, &destination, active)?;
    }
    Ok(assets)
}

fn append_mounts<'a>(
    assets: &mut EmbeddedAssets,
    root: &Path,
    mounts: impl IntoIterator<Item = &'a Mount>,
) -> Result<()> {
    for Mount { guest, source } in mounts {
        let mut path = root.to_owned();
        for part in source.components() {
            path.push(part);
            ensure!(
                directory_exists(&path)?,
                "embedded asset mount {guest} has no directory at {}",
                source.display()
            );
        }
        assets.mounts.push(EmbeddedMount {
            guest: guest.clone(),
            path,
        });
    }
    Ok(())
}

fn index_path(root: &Path, digest: blake3::Hash) -> PathBuf {
    // Bump this version if archive validation changes.
    root.join(format!("{}.assets-v1", digest.to_hex()))
}

fn read_index(root: &Path, digest: blake3::Hash) -> Result<Vec<IndexedArchive>> {
    let path = index_path(root, digest);
    ensure!(
        fs::symlink_metadata(&path)?.is_file(),
        "asset index is not a regular file"
    );
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_INDEX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_INDEX_BYTES,
        "asset index is too large"
    );
    let (checksum, encoded) = bytes
        .split_at_checked(blake3::OUT_LEN)
        .context("incomplete asset index")?;
    ensure!(
        blake3::keyed_hash(digest.as_bytes(), encoded).as_bytes() == checksum,
        "invalid asset index checksum"
    );
    Ok(minicbor_serde::from_slice(encoded)?)
}

fn write_index(root: &Path, digest: blake3::Hash, mut index: Vec<IndexedArchive>) -> Result<()> {
    // Preserve inventories learned by earlier builds with different mount
    // overrides.
    if index.iter().any(|archive| archive.inventory.is_none())
        && let Ok(previous) = read_index(root, digest)
    {
        for (archive, previous) in index.iter_mut().zip(previous) {
            if archive.inventory.is_none() {
                archive.inventory = previous.inventory;
            }
        }
    }
    let encoded = minicbor_serde::to_vec(&index)?;
    ensure!(
        (encoded.len() + blake3::OUT_LEN) as u64 <= MAX_INDEX_BYTES,
        "asset index is too large"
    );
    fs::create_dir_all(root)?;
    let mut temporary = NamedTempFile::new_in(root)?;
    temporary.write_all(blake3::keyed_hash(digest.as_bytes(), &encoded).as_bytes())?;
    temporary.write_all(&encoded)?;
    temporary.persist(index_path(root, digest))?;
    Ok(())
}

impl ArchiveInventory {
    fn record(&mut self, path: PathBuf, size: Option<u64>) {
        let directories = if size.is_some() {
            path.parent()
        } else {
            Some(path.as_path())
        };
        for directory in directories.into_iter().flat_map(Path::ancestors) {
            if directory.as_os_str().is_empty() || !self.directories.insert(directory.to_owned()) {
                break;
            }
        }
        if let Some(size) = size {
            self.files.push((path, size));
        }
    }

    fn check(&self, root: &Path) -> Result<()> {
        let check_path = |path: &Path| -> Result<PathBuf> {
            validate_relative_path(path)?;
            if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
                ensure!(
                    self.directories.contains(parent),
                    "asset index is missing a parent directory"
                );
            }
            Ok(root.join(path))
        };
        // Path ordering places parents before their children.
        for directory in &self.directories {
            ensure!(
                directory_exists(&check_path(directory)?)?,
                "missing cached asset directory"
            );
        }
        for (path, size) in &self.files {
            let metadata = fs::symlink_metadata(check_path(path)?)?;
            ensure!(
                metadata.is_file() && metadata.len() == *size,
                "invalid cached asset file"
            );
        }
        Ok(())
    }
}

/// Find top-level `isola:assets` sections without descending into nested
/// modules or components.
fn find_archives(wasm: &[u8]) -> Result<Vec<(&[u8], Vec<Mount>)>> {
    let mut archives = Vec::new();
    let mut guests = HashSet::new();
    let mut parser = Parser::new(0);
    let mut offset = 0;
    loop {
        let Chunk::Parsed { consumed, payload } = parser.parse(&wasm[offset..], true)? else {
            unreachable!("parsing with eof = true never needs more data");
        };
        offset += consumed;
        let section = match payload {
            Payload::ModuleSection {
                unchecked_range, ..
            }
            | Payload::ComponentSection {
                unchecked_range, ..
            } => {
                offset = usize::try_from(unchecked_range.end)?;
                ensure!(offset <= wasm.len(), "nested component exceeds input");
                continue;
            }
            Payload::CustomSection(section) if section.name() == SECTION_NAME => section,
            Payload::End(_) => break,
            _ => continue,
        };
        let bytes = section.data();
        let mounts = read_manifest(bytes)?;
        for mount in &mounts {
            ensure!(
                guests.insert(mount.guest.clone()),
                "duplicate embedded asset mount {}",
                mount.guest
            );
        }
        archives.push((bytes, mounts));
    }
    Ok(archives)
}

fn read_manifest(bytes: &[u8]) -> Result<Vec<Mount>> {
    let mut archive = tar::Archive::new(bytes);
    let mut entry = archive
        .entries()?
        .raw(true)
        .next()
        .context("embedded asset archive is empty")??;
    ensure!(
        entry.header().entry_type().is_file(),
        "embedded asset archive must begin with a regular file containing the manifest"
    );
    ensure!(
        entry.size() <= MAX_MANIFEST_BYTES,
        "embedded asset manifest exceeds {MAX_MANIFEST_BYTES} bytes"
    );
    let mut manifest_bytes = Vec::new();
    entry.read_to_end(&mut manifest_bytes)?;
    let manifest: Manifest =
        minicbor_serde::from_slice(&manifest_bytes).context("invalid embedded asset manifest")?;
    ensure!(
        minicbor_serde::to_vec(&manifest)? == manifest_bytes,
        "embedded asset manifest must use canonical CBOR"
    );
    validate_mounts(&manifest.mounts)?;
    Ok(manifest.mounts)
}

fn validate_mounts(mounts: &[Mount]) -> Result<()> {
    ensure!(!mounts.is_empty(), "embedded asset manifest has no mounts");
    for mount in mounts {
        validate_guest_path(&mount.guest)?;
        validate_relative_path(&mount.source)?;
        ensure!(
            mount
                .source
                .components()
                .any(|component| matches!(component, Component::Normal(_))),
            "embedded asset mount {} must name an archive subdirectory",
            mount.guest
        );
    }
    Ok(())
}

fn validate_guest_path(path: &str) -> Result<()> {
    let relative = path
        .strip_prefix('/')
        .context("embedded asset mount must be absolute")?;
    ensure!(
        !relative.contains(['\\', '\0'])
            && (relative.is_empty()
                || relative
                    .split('/')
                    .all(|part| !matches!(part, "" | "." | ".."))),
        "invalid embedded asset mount path {path}"
    );
    Ok(())
}

fn existing_metadata(path: &Path) -> std::io::Result<Option<Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn directory_exists(path: &Path) -> Result<bool> {
    let Some(metadata) = existing_metadata(path)? else {
        return Ok(false);
    };
    ensure!(
        metadata.is_dir(),
        "embedded asset path is not a directory: {}",
        path.display()
    );
    Ok(true)
}

fn prepare_cached_archive(
    archive: &[u8],
    cache_root: &Path,
) -> Result<(PathBuf, ArchiveInventory)> {
    let cache = cache_root.join("assets");
    fs::create_dir_all(&cache)?;
    ensure!(
        directory_exists(&cache)?,
        "asset cache directory is missing"
    );
    let digest = blake3::hash(archive);
    let destination = cache.join(digest.to_hex().as_str());
    let mut inventory = ArchiveInventory {
        digest: *digest.as_bytes(),
        directories: BTreeSet::new(),
        files: Vec::new(),
    };
    if directory_exists(&destination)? {
        unpack_archive(archive, &destination, true, Some(&mut inventory))?;
        return Ok((destination, inventory));
    }

    let temporary = Builder::new().prefix(".isola-assets-").tempdir_in(&cache)?;
    unpack_archive(archive, temporary.path(), false, Some(&mut inventory))?;

    // Publish the complete directory in one rename. Concurrent extractions can
    // reuse the winner's directory and discard their own temp directories.
    if let Err(error) = fs::rename(temporary.path(), &destination)
        && !directory_exists(&destination)?
    {
        return Err(error.into());
    }
    Ok((destination, inventory))
}

fn unpack_archive(
    archive: &[u8],
    destination: &Path,
    repair: bool,
    mut inventory: Option<&mut ArchiveInventory>,
) -> Result<()> {
    let mut archive = tar::Archive::new(Cursor::new(archive));
    archive.set_overwrite(false);
    archive.set_mask(0o7022);
    let mut extracted = HashSet::new();
    let mut checked_dirs = HashSet::new();
    let mut staging = None;
    // The first entry holds the manifest, which prepare reads before
    // extraction.
    let mut entries = archive.entries_with_seek()?;
    entries.next().transpose()?;
    for entry in entries {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        validate_relative_path(&path)?;
        ensure!(
            extracted.insert(path.clone()),
            "duplicate embedded asset entry {}",
            path.display()
        );
        let entry_type = entry.header().entry_type();
        ensure!(
            entry_type.is_file() || entry_type.is_dir(),
            "embedded asset archive contains unsupported entry {}",
            path.display()
        );
        let size = entry_type.is_file().then(|| entry.size());
        if repair {
            repair_entry(
                &mut entry,
                destination,
                &path,
                size,
                &mut checked_dirs,
                &mut staging,
            )?;
        } else {
            unpack_entry(&mut entry, destination, &path)?;
        }
        if let Some(inventory) = inventory.as_deref_mut() {
            inventory.record(path, size);
        }
    }
    Ok(())
}

fn repair_entry(
    entry: &mut tar::Entry<'_, impl Read>,
    destination: &Path,
    path: &Path,
    size: Option<u64>,
    checked_dirs: &mut HashSet<PathBuf>,
    staging: &mut Option<TempDir>,
) -> Result<()> {
    let target = destination.join(path);
    let parent = target
        .parent()
        .context("embedded asset entry has no parent")?;
    for dir in parent.ancestors().take_while(|dir| *dir != destination) {
        if checked_dirs.contains(dir) {
            break;
        }
        directory_exists(dir)?;
        checked_dirs.insert(dir.to_owned());
    }
    let Some(size) = size else {
        if !directory_exists(&target)? {
            unpack_entry(entry, destination, path)?;
        }
        return Ok(());
    };
    if let Some(metadata) = existing_metadata(&target)? {
        ensure!(
            metadata.is_file(),
            "embedded asset path is not a file: {}",
            target.display()
        );
        if metadata.len() == size {
            return Ok(());
        }
    }
    // Replace individual files atomically to keep existing preopens valid.
    let stage = match staging {
        Some(stage) => stage,
        None => staging.insert(Builder::new().prefix(".repair-").tempdir_in(destination)?),
    };
    unpack_entry(entry, stage.path(), path)?;
    fs::create_dir_all(parent)?;
    fs::rename(stage.path().join(path), target)?;
    Ok(())
}

fn unpack_entry(
    entry: &mut tar::Entry<'_, impl std::io::Read>,
    destination: &Path,
    path: &Path,
) -> Result<()> {
    ensure!(
        entry.unpack_in(destination)?,
        "embedded asset entry was not extracted: {}",
        path.display()
    );
    Ok(())
}

fn validate_relative_path(path: &Path) -> Result<()> {
    let has_unsafe_byte = path
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .any(|byte| matches!(byte, b'\\' | b':' | 0));
    let has_unsafe_component = path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    });
    ensure!(
        !has_unsafe_byte && !has_unsafe_component,
        "embedded asset archive contains an unsafe path {}",
        path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use wasm_encoder::{Component, CustomSection, Module, ModuleSection};

    use super::*;

    fn manifest_bytes(mounts: &[(&str, &str)]) -> Result<Vec<u8>> {
        Ok(minicbor_serde::to_vec(Manifest {
            mounts: mounts
                .iter()
                .map(|&(guest, source)| Mount {
                    guest: guest.into(),
                    source: source.into(),
                })
                .collect(),
        })?)
    }

    fn append_file(archive: &mut tar::Builder<Vec<u8>>, path: &str, data: &[u8]) -> Result<()> {
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_size(data.len() as u64);
        archive.append_data(&mut header, path, data)?;
        Ok(())
    }

    #[test]
    fn one_archive_supplies_several_mounts() -> Result<()> {
        let mut tar = tar::Builder::new(Vec::new());
        append_file(
            &mut tar,
            "mounts/0/metadata.cbor",
            &manifest_bytes(&[("/lib", "mounts/0"), ("/data", "mounts/1")])?,
        )?;
        append_file(&mut tar, "mounts/0/file", b"library")?;
        append_file(&mut tar, "mounts/1/file", b"data")?;
        let mut component = Component::new();
        component.section(&CustomSection {
            name: SECTION_NAME.into(),
            data: tar.into_inner()?.into(),
        });
        let wasm = component.finish();

        let cache_root = tempfile::tempdir()?;
        let digest = blake3::hash(&wasm);
        let cache = Some((cache_root.path(), digest));
        assert!(prepare(&wasm, cache, |_| true)?.mounts.is_empty());
        assert!(
            prepare_from_index(cache_root.path(), digest, |_| true)?
                .mounts
                .is_empty()
        );
        assert!(prepare_from_index(cache_root.path(), digest, |_| false).is_err());

        let assets = prepare(&wasm, cache, |_| false)?;
        assert_eq!(assets.mounts.len(), 2);
        let lib = &assets.mounts[0];
        let data = &assets.mounts[1];
        assert_eq!(lib.guest, "/lib");
        assert_eq!(data.guest, "/data");
        assert_eq!(lib.path.parent(), data.path.parent());
        assert!(lib.path.starts_with(cache_root.path().join("assets")));
        assert!(lib.path.ends_with("mounts/0"));
        assert!(data.path.ends_with("mounts/1"));
        assert!(!lib.path.join("metadata.cbor").exists());
        assert_eq!(fs::read(lib.path.join("file"))?, b"library");
        assert_eq!(fs::read(data.path.join("file"))?, b"data");

        let indexed = prepare_from_index(cache_root.path(), digest, |guest| guest == "/lib")?;
        assert_eq!(indexed.mounts.len(), 1);
        assert_eq!(indexed.mounts[0].path, data.path);
        prepare(&wasm, cache, |_| true)?;
        assert_eq!(
            prepare_from_index(cache_root.path(), digest, |_| false)?
                .mounts
                .len(),
            2
        );

        fs::remove_file(data.path.join("file"))?;
        fs::write(lib.path.join("file"), b"truncated")?;
        assert!(prepare_from_index(cache_root.path(), digest, |guest| guest == "/lib").is_err());
        let overridden = prepare(&wasm, cache, |guest| guest == "/lib")?;
        assert_eq!(overridden.mounts.len(), 1);
        assert_eq!(overridden.mounts[0].guest, "/data");
        assert_eq!(overridden.mounts[0].path, data.path);
        assert_eq!(fs::read(data.path.join("file"))?, b"data");
        assert_eq!(fs::read(lib.path.join("file"))?, b"library");

        let index = index_path(cache_root.path(), digest);
        let mut bytes = fs::read(&index)?;
        bytes[blake3::OUT_LEN] ^= 1;
        fs::write(&index, bytes)?;
        assert!(prepare_from_index(cache_root.path(), digest, |_| false).is_err());
        prepare(&wasm, cache, |_| false)?;
        assert_eq!(
            prepare_from_index(cache_root.path(), digest, |_| false)?
                .mounts
                .len(),
            2
        );
        Ok(())
    }

    #[test]
    fn rejects_duplicate_archive_entries() -> Result<()> {
        let mut tar = tar::Builder::new(Vec::new());
        append_file(
            &mut tar,
            "manifest.cbor",
            &manifest_bytes(&[("/lib", "mounts/0")])?,
        )?;
        append_file(&mut tar, "file", b"first")?;
        append_file(&mut tar, "file", b"second")?;
        let directory = tempfile::tempdir()?;
        let error = unpack_archive(&tar.into_inner()?, directory.path(), false, None).unwrap_err();
        assert!(error.to_string().contains("duplicate embedded asset entry"));
        assert_eq!(fs::read(directory.path().join("file"))?, b"first");
        Ok(())
    }

    #[test]
    fn rejects_manifest_paths_outside_the_bundle() -> Result<()> {
        let mut tar = tar::Builder::new(Vec::new());
        append_file(
            &mut tar,
            "manifest.cbor",
            &manifest_bytes(&[("/lib", "../outside")])?,
        )?;
        let error = read_manifest(&tar.into_inner()?)
            .err()
            .context("expected the manifest path to be rejected")?;
        assert!(error.to_string().contains("contains an unsafe path"));
        Ok(())
    }

    #[test]
    fn rejects_noncanonical_manifests() -> Result<()> {
        let canonical = manifest_bytes(&[("/lib", "mounts/0")])?;
        let trailing_data = [canonical.as_slice(), &[0]].concat();
        let long_map_length = [&[0xb8, 1], &canonical[1..]].concat();
        let reversed_keys = b"\xa1\x66mounts\x81\xa2\x66source\x68mounts/0\x65guest\x64/lib";
        for bytes in [
            trailing_data.as_slice(),
            long_map_length.as_slice(),
            reversed_keys.as_slice(),
        ] {
            let mut tar = tar::Builder::new(Vec::new());
            append_file(&mut tar, "manifest.cbor", bytes)?;
            let error = read_manifest(&tar.into_inner()?)
                .err()
                .context("expected the noncanonical manifest to be rejected")?;
            assert!(error.to_string().contains("must use canonical CBOR"));
        }
        Ok(())
    }

    #[test]
    fn ignores_asset_sections_inside_core_modules() -> Result<()> {
        let mut module = Module::new();
        module.section(&CustomSection {
            name: SECTION_NAME.into(),
            data: b"invalid tar".as_slice().into(),
        });
        let mut component = Component::new();
        component.section(&ModuleSection(&module));

        assert!(
            prepare(&component.finish(), None, |_| false)?
                .mounts
                .is_empty()
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_cache_directories() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let symlink = directory.path().join("alias");
        std::os::unix::fs::symlink(directory.path(), &symlink)?;
        assert!(directory_exists(&symlink).is_err());
        Ok(())
    }
}
