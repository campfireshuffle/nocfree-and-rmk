//! Local, immutable factory originals. No factory image is embedded or downloaded.
use crate::{device, runtime_recovery::Role};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
const LIMIT: usize = 1728 * 512;
const PREFIX_HASH: &str = "52102c1f530069395431dab626cf618310dd54ce579b8bf25aba1908526afd0b";
fn hash(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}
fn name(role: Role) -> &'static str {
    match role {
        Role::Left => "left",
        Role::Right => "right",
        Role::Receiver => "receiver",
    }
}
fn known_app(role: Role, digest: &str) -> bool {
    let hashes = match role {
        Role::Left => [
            "7087c8c8612f12a3dc75c6db88801a781b4414492226ac99e70a684bc23dd54e",
            "68e9fb2bd45f40a98057be658c23309d704c70c7084d47cf8c66bba6c5138f35",
        ],
        Role::Right => [
            "fe7851a983813b731b0c99b2479fcd88d6f7b50c4d3d483997d1a93db1538e11",
            "3a1756b20eb488e1626e807547d251323334e0d5463b6eb36a7255e9a0609892",
        ],
        Role::Receiver => [
            "e03d64b296849cbb39a6a7a6e3eeb3b5f5e0674acd76193cb02b386330b356d7",
            "e10bba493620dfa6d92a9bc3fac676be8469d197b45ba5c9fb93dc05393d307c",
        ],
    };
    hashes.contains(&digest)
}
fn word(b: &[u8], n: usize) -> u32 {
    u32::from_le_bytes(b[n..n + 4].try_into().unwrap())
}
fn payload(archive: &[u8]) -> Result<Vec<u8>, String> {
    device::inspect_archive(archive)?;
    let mut p = vec![0; 0x6d000 - 0x1000];
    for b in archive.as_chunks::<512>().0 {
        let n = word(b, 12) as usize - 0x1000;
        p[n..n + 256].copy_from_slice(&b[32..288]);
    }
    Ok(p)
}
fn validate(role: Role, p: &[u8]) -> Result<(), String> {
    if p.len() != 0x6c000
        || hash(&p[..0x26000]) != PREFIX_HASH
        || !known_app(role, &hash(&p[0x26000..0x64000]))
    {
        return Err("This is not a recognized complete factory backup for this part. RMK backups cannot restore factory firmware.".into());
    }
    validate_vectors(p)
}
fn validate_vectors(p: &[u8]) -> Result<(), String> {
    if p.len() != 0x6c000 {
        return Err("The factory backup is missing firmware data.".into());
    }
    let sp = word(p, 0x26000);
    let pc = word(p, 0x26004);
    if !(0x20000000 < sp
        && sp <= 0x20020000
        && sp.is_multiple_of(8)
        && pc & 1 == 1
        && (0x27000..0x65000).contains(&(pc & !1)))
        || word(p, 0x26200) == 0x87eeb07c
    {
        return Err("The factory backup contains invalid startup or recovery data.".into());
    }
    Ok(())
}
fn encode(p: &[u8], family: u32) -> Vec<u8> {
    let count = p.len() / 256;
    let mut image = Vec::with_capacity(count * 512);
    for (i, data) in p.as_chunks::<256>().0.iter().enumerate() {
        let mut b = [0; 512];
        for (n, v) in [
            0x0a324655,
            0x9e5d5157,
            0x2000,
            0x1000 + i as u32 * 256,
            256,
            i as u32,
            count as u32,
            family,
        ]
        .into_iter()
        .enumerate()
        {
            b[n * 4..n * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        b[32..288].copy_from_slice(data);
        b[508..].copy_from_slice(&0x0ab16f30u32.to_le_bytes());
        image.extend_from_slice(&b);
    }
    image
}
fn private_root(root: &Path) -> Result<(), String> {
    match fs::symlink_metadata(root) {
        Ok(m) if !m.is_dir() || m.file_type().is_symlink() => {
            return Err("The factory backup folder cannot be a link to another folder.".into());
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(root).map_err(|_| "Could not create the factory backup folder.")?
        }
        Err(_) => return Err("Could not inspect the factory backup folder.".into()),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))
            .map_err(|_| "Could not restrict factory backup folder permissions.")?;
    }
    Ok(())
}
fn local_archive(path: &Path) -> Result<Vec<u8>, String> {
    if !fs::symlink_metadata(path)
        .map_err(|_| "Factory backup is unavailable.")?
        .file_type()
        .is_file()
    {
        return Err("The factory backup must be a file, not a link or folder.".into());
    }
    device::read_bounded(path, LIMIT)
}
fn save_original(root: &Path, path: &Path, archive: &[u8]) -> Result<(), String> {
    let temporary = root.join(format!(
        ".{}.partial",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "Could not record the factory backup time.")?
            .as_nanos()
    ));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        crate::host_storage::private_file_options(&mut options);
        let mut f = options
            .open(&temporary)
            .map_err(|_| "Could not create the private factory backup.")?;
        f.write_all(archive)
            .and_then(|_| f.sync_all())
            .map_err(|_| "Could not finish saving the factory backup.")?;
        // Atomic no-replace publication: a concurrent first original wins.
        fs::hard_link(&temporary, path)
            .map_err(|_| "The first factory backup already exists or could not be saved.")?;
        crate::host_storage::sync_directory(root)?;
        Ok(())
    })();
    let _ = fs::remove_file(&temporary);
    result
}
#[derive(Clone)]
pub struct FactoryImage {
    pub(crate) role: Role,
    pub(crate) bytes: Vec<u8>,
    pub(crate) uf2: Vec<u8>,
    pub(crate) sha256: String,
    captured: bool,
}
impl FactoryImage {
    pub(crate) fn checked(&self) -> Result<(), String> {
        if self.captured {
            validate_vectors(&self.bytes)?;
        } else {
            validate(self.role, &self.bytes)?;
        }
        if self.uf2 != encode(&self.bytes, 0x621e937a) || hash(&self.uf2) != self.sha256 {
            return Err("Factory restore image changed.".into());
        }
        Ok(())
    }
    fn new(role: Role, bytes: Vec<u8>) -> Result<Self, String> {
        validate(role, &bytes)?;
        Self::from_checked(role, bytes, false)
    }
    fn from_checked(role: Role, bytes: Vec<u8>, captured: bool) -> Result<Self, String> {
        validate_vectors(&bytes)?;
        let uf2 = encode(&bytes, 0x621e937a);
        let sha256 = hash(&uf2);
        Ok(Self {
            role,
            bytes,
            uf2,
            sha256,
            captured,
        })
    }
}
#[derive(Clone)]
pub struct FactoryRelease {
    root: PathBuf,
    images: Vec<FactoryImage>,
}
impl FactoryRelease {
    /// Used only when a durable factory-verification journal already binds the
    /// exact readback and target hashes; this does not authorize an imported image.
    pub(crate) fn verified_archive_image(
        role: Role,
        archive: &[u8],
    ) -> Result<FactoryImage, String> {
        FactoryImage::from_checked(role, payload(archive)?, true)
    }
    fn capture_record(role: Role, archive: &[u8]) -> Result<Vec<u8>, String> {
        if Self::archive_role(archive).is_some_and(|actual| actual != role) {
            return Err("Factory backup belongs to a different part.".into());
        }
        validate_vectors(&payload(archive)?)?;
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": 1, "role": name(role), "origin": "correlated-factory-recovery",
            "archive_sha256": hash(archive)
        }))
        .map_err(|_| "Could not save the factory backup record.".into())
    }
    pub(crate) fn record_capture(folder: &Path, role: Role, archive: &[u8]) -> Result<(), String> {
        save_original(
            folder,
            &folder.join("factory-capture.json"),
            &Self::capture_record(role, archive)?,
        )
    }
    fn recorded_image(role: Role, archive: &[u8], proof: &Path) -> Result<FactoryImage, String> {
        if !fs::symlink_metadata(proof)
            .map_err(|_| "The factory backup record is unavailable.")?
            .file_type()
            .is_file()
        {
            return Err("Factory backup record must be a regular file.".into());
        }
        let metadata: serde_json::Value =
            serde_json::from_slice(&device::read_bounded(proof, 4096)?)
                .map_err(|_| "Factory backup record is unreadable.")?;
        if metadata["schema"].as_u64() != Some(1)
            || metadata["role"].as_str() != Some(name(role))
            || metadata["origin"].as_str() != Some("correlated-factory-recovery")
            || metadata["archive_sha256"].as_str() != Some(hash(archive).as_str())
        {
            return Err("The factory backup has changed since it was saved.".into());
        }
        FactoryImage::from_checked(role, payload(archive)?, true)
    }
    pub(crate) fn archive_role(archive: &[u8]) -> Option<Role> {
        let p = payload(archive).ok()?;
        [Role::Left, Role::Right, Role::Receiver]
            .into_iter()
            .find(|r| validate(*r, &p).is_ok())
    }
    pub fn discover() -> Result<Self, String> {
        let root = crate::host_storage::application_root()?.join("factory-originals");
        Self::at(root)
    }
    pub(crate) fn at(root: PathBuf) -> Result<Self, String> {
        if fs::symlink_metadata(&root).is_ok() {
            private_root(&root)?;
        }
        let mut release = Self {
            root,
            images: vec![],
        };
        for role in [Role::Left, Role::Right, Role::Receiver] {
            let path = release.root.join(format!("{}.uf2", name(role)));
            if fs::symlink_metadata(&path).is_ok() {
                let raw = local_archive(&path)?;
                if let Some(image) = release.load_original(role, &raw)? {
                    release.images.push(image);
                }
            }
        }
        Ok(release)
    }
    /// An unknown original published without its proof is pending, not usable.
    /// Keep discovery available so a fresh correlated capture can finish it.
    fn load_original(&self, role: Role, archive: &[u8]) -> Result<Option<FactoryImage>, String> {
        let bytes = payload(archive)?;
        let proof = self.root.join(format!("{}.json", name(role)));
        match fs::symlink_metadata(&proof) {
            Ok(_) => Self::recorded_image(role, archive, &proof).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Ok(image) = FactoryImage::new(role, bytes.clone()) {
                    return Ok(Some(image));
                }
                validate_vectors(&bytes)?;
                if Self::archive_role(archive).is_some() {
                    return Err("Factory backup belongs to a different part.".into());
                }
                Ok(None)
            }
            Err(_) => Err("The factory backup record is unavailable.".into()),
        }
    }
    /// Only a fresh, correlated factory session may attest otherwise unknown
    /// originals. The record binds role and bytes, not a universal board identity.
    pub(crate) fn capture_original(&mut self, role: Role, archive: &[u8]) -> Result<(), String> {
        let bytes = payload(archive)?;
        validate_vectors(&bytes)?;
        if Self::archive_role(archive).is_some_and(|actual| actual != role) {
            return Err("Factory backup belongs to a different part.".into());
        }
        private_root(&self.root)?;
        let path = self.root.join(format!("{}.uf2", name(role)));
        let proof = self.root.join(format!("{}.json", name(role)));
        let record = Self::capture_record(role, archive)?;
        if fs::symlink_metadata(&path).is_ok() {
            let existing = local_archive(&path)?;
            // A new correlated session can finish only the exact interrupted capture.
            if existing == archive && fs::symlink_metadata(&proof).is_err() {
                save_original(&self.root, &proof, &record)?;
            }
            let original = self.load_original(role, &existing)?.ok_or(
                "An unfinished factory backup exists for this part. Reconnect its original factory firmware to finish saving it.",
            )?;
            if original.bytes[..0x64000] != bytes[..0x64000] {
                return Err("A different factory original is already saved for this part. Keep both backups before replacing it.".into());
            }
            self.images.retain(|image| image.role != role);
            self.images.push(original);
            return Ok(());
        }
        // A partial capture remains ineligible: unknown archives need both files.
        save_original(&self.root, &path, archive)?;
        save_original(&self.root, &proof, &record)?;
        self.images.retain(|image| image.role != role);
        self.images.push(
            self.load_original(role, archive)?
                .ok_or("The factory backup record is missing.")?,
        );
        Ok(())
    }
    pub fn missing(&self) -> Vec<Role> {
        [Role::Left, Role::Right, Role::Receiver]
            .into_iter()
            .filter(|r| self.image(*r).is_err())
            .collect()
    }
    pub fn complete(&self) -> bool {
        self.is_complete()
    }
    pub fn has(&self, role: Role) -> bool {
        self.image(role).is_ok()
    }
    pub fn is_complete(&self) -> bool {
        self.missing().is_empty()
    }
    pub(crate) fn image(&self, role: Role) -> Result<&FactoryImage, String> {
        self.images
            .iter()
            .find(|i| i.role == role)
            .ok_or_else(|| format!("Add a complete factory backup for the {}.", name(role)))
    }
    pub(crate) fn id(&self) -> String {
        let hashes = [Role::Left, Role::Right, Role::Receiver]
            .into_iter()
            .filter_map(|r| self.image(r).ok())
            .flat_map(|i| i.sha256.as_bytes().iter().copied())
            .collect::<Vec<_>>();
        hash(&hashes)
    }
    /// Full originals are retained once. App-only imports require that original
    /// donor and replace only application bytes in this in-memory restore target.
    pub fn import(&mut self, role: Role, path: &Path) -> Result<(), String> {
        let raw = device::read_bounded(path, LIMIT)?;
        if raw.len() == LIMIT {
            let image = if let Ok(image) = FactoryImage::new(role, payload(&raw)?) {
                self.retain_original(role, &raw)?;
                image
            } else {
                Self::recorded_image(
                    role,
                    &raw,
                    &path
                        .parent()
                        .ok_or("Factory backup folder is unavailable.")?
                        .join("factory-capture.json"),
                )?
            };
            self.images.retain(|i| i.role != role);
            self.images.push(image);
            return Ok(());
        }
        let mut bytes = self
            .image(role)
            .map_err(
                |_| "This app-only UF2 needs a complete factory backup containing S140 first.",
            )?
            .bytes
            .clone();
        if raw.is_empty() || raw.len() % 512 != 0 {
            return Err("Choose an official factory UF2 for this part.".into());
        }
        bytes[0x26000..0x64000].fill(0xff);
        let count = raw.len() / 512;
        let mut seen = vec![false; count];
        for b in raw.as_chunks::<512>().0 {
            let a = word(b, 12) as usize;
            let i = word(b, 20) as usize;
            if word(b, 0) != 0x0a324655
                || word(b, 4) != 0x9e5d5157
                || word(b, 508) != 0x0ab16f30
                || word(b, 8) != 0x2000
                || word(b, 16) != 256
                || word(b, 24) != count as u32
                || word(b, 28) != 0x621e937a
                || i >= count
                || seen[i]
                || a != 0x27000 + i * 256
                || a + 256 > 0x65000
            {
                return Err(
                    "The factory UF2 file has missing data or is for a different chip.".into(),
                );
            }
            seen[i] = true;
            bytes[a - 0x1000..a - 0x1000 + 256].copy_from_slice(&b[32..288]);
        }
        let image = FactoryImage::new(role, bytes)?;
        self.images.retain(|i| i.role != role);
        self.images.push(image);
        Ok(())
    }
    pub(crate) fn retain_original(&mut self, role: Role, archive: &[u8]) -> Result<bool, String> {
        let bytes = payload(archive)?;
        if validate(role, &bytes).is_err() {
            return Ok(false);
        }
        private_root(&self.root)?;
        let path = self.root.join(format!("{}.uf2", name(role)));
        if fs::symlink_metadata(&path).is_ok() {
            let original = local_archive(&path)?;
            self.load_original(role, &original)?
                .ok_or("The saved factory backup is unfinished. Reconnect its original factory firmware to finish saving it.")?;
            return Ok(true);
        }
        save_original(&self.root, &path, archive)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temporary() -> PathBuf {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "factory-restore-test-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }
    #[test]
    fn rmk_or_unclassified_archives_never_become_factory_originals() {
        let root = temporary();
        let mut r = FactoryRelease::at(root.clone()).unwrap();
        let raw = crate::device::tests::archive();
        assert!(!r.retain_original(Role::Left, &raw).unwrap());
        assert!(!root.exists());
        assert!(!r.complete());
        assert!(FactoryImage::new(Role::Left, payload(&raw).unwrap()).is_err());
        assert!(crate::firmware_journey::FirmwareJourney::factory(r).is_err());
    }
    fn unknown_factory() -> Vec<u8> {
        let mut bytes = payload(&crate::device::tests::archive()).unwrap();
        bytes[0x26000..0x26004].copy_from_slice(&0x20020000u32.to_le_bytes());
        bytes[0x26004..0x26008].copy_from_slice(&0x27101u32.to_le_bytes());
        encode(&bytes, 0x239a0029)
    }
    #[test]
    fn captured_unknown_original_roundtrips_but_import_does_not_attest_it() {
        let root = temporary();
        let raw = unknown_factory();
        assert!(FactoryRelease::archive_role(&raw).is_none());
        let source = root.with_extension("uf2");
        fs::write(&source, &raw).unwrap();
        let mut release = FactoryRelease::at(root.clone()).unwrap();
        assert!(release.import(Role::Left, &source).is_err());
        release.capture_original(Role::Left, &raw).unwrap();
        let restored = FactoryRelease::at(root.clone()).unwrap();
        let image = restored.image(Role::Left).unwrap();
        image.checked().unwrap();
        assert_eq!(image.bytes, payload(&raw).unwrap());
        // Role cannot be changed independently of its durable capture record.
        fs::rename(root.join("left.uf2"), root.join("right.uf2")).unwrap();
        fs::rename(root.join("left.json"), root.join("right.json")).unwrap();
        assert!(FactoryRelease::at(root.clone()).is_err());
        fs::remove_dir_all(root).unwrap();
        fs::remove_file(source).unwrap();
    }
    #[test]
    fn capture_rejects_corruption_and_different_original_preserves_first() {
        let root = temporary();
        let mut release = FactoryRelease::at(root.clone()).unwrap();
        let raw = unknown_factory();
        release.capture_original(Role::Right, &raw).unwrap();
        let mut other = raw.clone();
        other[32 + 100] ^= 1;
        assert!(release.capture_original(Role::Right, &other).is_err());
        assert_eq!(fs::read(root.join("right.uf2")).unwrap(), raw);
        fs::write(root.join("right.uf2"), other).unwrap();
        assert!(FactoryRelease::at(root.clone()).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn recorded_backup_can_restore_a_later_original_without_replacing_vault() {
        let root = temporary();
        let mut release = FactoryRelease::at(root.clone()).unwrap();
        let first = unknown_factory();
        release.capture_original(Role::Left, &first).unwrap();
        let mut later = first.clone();
        later[100] ^= 1;
        let backup = root.join("saved-backup");
        private_root(&backup).unwrap();
        let path = backup.join("CURRENT.UF2");
        fs::write(&path, &later).unwrap();
        FactoryRelease::record_capture(&backup, Role::Left, &later).unwrap();
        assert!(release.import(Role::Right, &path).is_err());
        release.import(Role::Left, &path).unwrap();
        assert_eq!(
            release.image(Role::Left).unwrap().bytes,
            payload(&later).unwrap()
        );
        assert_eq!(fs::read(root.join("left.uf2")).unwrap(), first);
        let mut corrupted = later;
        corrupted[101] ^= 1;
        fs::write(&path, corrupted).unwrap();
        assert!(release.import(Role::Left, &path).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn interrupted_capture_is_ineligible_until_same_factory_is_recaptured() {
        let root = temporary();
        private_root(&root).unwrap();
        let raw = unknown_factory();
        save_original(&root, &root.join("left.uf2"), &raw).unwrap();
        let mut release = FactoryRelease::at(root.clone()).unwrap();
        assert!(!release.has(Role::Left));
        assert!(
            crate::firmware_journey::FirmwareJourney::factory_scoped(
                release.clone(),
                crate::scope::Scope::Part(Role::Left)
            )
            .is_err()
        );
        assert!(release.import(Role::Left, &root.join("left.uf2")).is_err());
        let mut other = raw.clone();
        other[32 + 100] ^= 1;
        assert!(release.capture_original(Role::Left, &other).is_err());
        assert_eq!(fs::read(root.join("left.uf2")).unwrap(), raw);
        assert!(!root.join("left.json").exists());
        release.capture_original(Role::Left, &raw).unwrap();
        let reopened = FactoryRelease::at(root.clone()).unwrap();
        assert!(reopened.has(Role::Left));
        reopened.image(Role::Left).unwrap().checked().unwrap();
        assert_eq!(fs::read(root.join("left.uf2")).unwrap(), raw);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn pending_original_does_not_hide_other_roles_or_accept_invalid_proof() {
        let root = temporary();
        let raw = unknown_factory();
        let mut release = FactoryRelease::at(root.clone()).unwrap();
        release.capture_original(Role::Right, &raw).unwrap();
        save_original(&root, &root.join("left.uf2"), &raw).unwrap();
        let reopened = FactoryRelease::at(root.clone()).unwrap();
        assert!(!reopened.has(Role::Left));
        assert!(reopened.has(Role::Right));
        fs::write(root.join("left.json"), b"{incomplete").unwrap();
        assert!(FactoryRelease::at(root.clone()).is_err());
        assert!(release.capture_original(Role::Left, &raw).is_err());
        assert_eq!(fs::read(root.join("left.json")).unwrap(), b"{incomplete");
        fs::remove_file(root.join("left.json")).unwrap();
        fs::write(root.join("left.uf2"), b"incomplete UF2").unwrap();
        assert!(FactoryRelease::at(root.clone()).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn interrupted_temporary_write_does_not_publish_or_replace_original() {
        let root = temporary();
        private_root(&root).unwrap();
        let path = root.join("left.uf2");
        fs::write(&path, b"first original").unwrap();
        assert!(save_original(&root, &path, b"replacement").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"first original");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn symbolic_link_vault_is_rejected() {
        let root = temporary();
        let link = root.with_extension("link");
        fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(&root, &link).unwrap();
        assert!(FactoryRelease::at(link.clone()).is_err());
        fs::remove_file(link).unwrap();
        fs::remove_dir(root).unwrap();
    }
    #[test]
    #[ignore = "requires private original factory archives; offline only"]
    fn complete_originals_roundtrip_wrong_roles_and_immutable_first_copy() {
        let evidence = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".evidence");
        let root = temporary();
        let mut release = FactoryRelease::at(root.clone()).unwrap();
        for (role, folder) in [
            (Role::Left, "factory-left"),
            (Role::Right, "factory-right"),
            (Role::Receiver, "receiver-backup"),
        ] {
            let source = evidence.join(folder).join("CURRENT.UF2");
            let raw = fs::read(&source).unwrap();
            release.import(role, &source).unwrap();
            let image = release.image(role).unwrap();
            image.checked().unwrap();
            assert_eq!(payload(&raw).unwrap(), image.bytes);
            let archive = encode(&image.bytes, 0x239a0029);
            assert_eq!(payload(&archive).unwrap(), image.bytes);
            let original = fs::read(root.join(format!("{}.uf2", name(role)))).unwrap();
            let mut later = raw.clone();
            for b in later.as_chunks_mut::<512>().0 {
                if word(b, 12) == 0x65000 {
                    b[32 + 100] ^= 1;
                    break;
                }
            }
            assert!(release.retain_original(role, &later).unwrap());
            assert_eq!(
                original,
                fs::read(root.join(format!("{}.uf2", name(role)))).unwrap()
            );
            for other in [Role::Left, Role::Right, Role::Receiver] {
                if other != role {
                    assert!(FactoryImage::new(other, payload(&raw).unwrap()).is_err());
                }
            }
        }
        assert!(release.complete());
        let id = release.id();
        release
            .import(Role::Left, &evidence.join("factory-left/CURRENT.UF2"))
            .unwrap();
        assert_eq!(id, release.id());
        let journey = crate::firmware_journey::FirmwareJourney::factory(release).unwrap();
        assert!(journey.is_factory());
        assert_eq!(journey.role(), Role::Left);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[ignore = "requires private original archives and extracted supplied ANSI 2.4.5 UF2 fixtures; offline only"]
    fn supplied_official_targets_preserve_donors_and_switching_back_reloads_originals() {
        let evidence = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".evidence");
        let root = temporary();
        let mut supplied = FactoryRelease::at(root.clone()).unwrap();
        let mut originals = vec![];
        for (role, folder, file) in [
            (Role::Left, "factory-left", "left.uf2"),
            (Role::Right, "factory-right", "right.uf2"),
            (Role::Receiver, "receiver-backup", "dongle.uf2"),
        ] {
            let official = evidence.join("factory-source-tests").join(file);
            // An application alone cannot manufacture its S140/settings donor.
            assert!(supplied.import(role, &official).is_err());
            let source = evidence.join(folder).join("CURRENT.UF2");
            supplied.import(role, &source).unwrap();
            originals.push((role, supplied.image(role).unwrap().bytes.clone()));
            let saved = fs::read(root.join(format!("{}.uf2", name(role)))).unwrap();
            let original = payload(&saved).unwrap();
            supplied.import(role, &official).unwrap();
            let image = supplied.image(role).unwrap();
            image.checked().unwrap();
            assert_eq!(&image.bytes[..0x26000], &original[..0x26000]);
            assert_eq!(&image.bytes[0x64000..], &original[0x64000..]);
            assert_ne!(&image.bytes[0x26000..0x64000], &original[0x26000..0x64000]);
            assert_eq!(
                fs::read(root.join(format!("{}.uf2", name(role)))).unwrap(),
                saved
            );
            for other in [Role::Left, Role::Right, Role::Receiver] {
                if other != role && supplied.has(other) {
                    let before = supplied.id();
                    assert!(supplied.import(other, &official).is_err());
                    assert_eq!(supplied.id(), before);
                }
            }
        }
        assert!(supplied.complete());
        for (role, file) in [
            (Role::Left, "left.uf2"),
            (Role::Right, "right.uf2"),
            (Role::Receiver, "dongle.uf2"),
        ] {
            for other in [Role::Left, Role::Right, Role::Receiver] {
                if other != role {
                    let before = supplied.id();
                    assert!(
                        supplied
                            .import(other, &evidence.join("factory-source-tests").join(file))
                            .is_err()
                    );
                    assert_eq!(supplied.id(), before);
                }
            }
        }
        let backups = FactoryRelease::at(root.clone()).unwrap();
        assert!(backups.complete());
        assert_ne!(backups.id(), supplied.id());
        for (role, original) in originals {
            assert_eq!(backups.image(role).unwrap().bytes, original);
        }
        fs::remove_dir_all(root).unwrap();
    }
}
