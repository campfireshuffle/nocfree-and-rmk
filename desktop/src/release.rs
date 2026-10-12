//! The app's fixed local firmware release. Files and role provenance are pinned in code;
//! a public UF2, USB product name, or edited manifest can never become a trusted release.
//! This module only reads files. Device identity and installation consent are separate.
use crate::{
    keyboard_layout::KeyboardLayout,
    runtime_recovery::Role,
    update_image::{self, ImagePolicy, ValidatedImage},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
#[cfg(any(debug_assertions, test))]
use std::path::PathBuf;
use std::path::{Component, Path};

#[cfg(not(feature = "firmware-trial"))]
const MANIFEST_SHA256: &str = "f99e2fe2b748ef3ee01c2804ca5d0aefa7a07ae0cdceff544ad232a9a6b791f3";
#[cfg(feature = "firmware-trial")]
const MANIFEST_SHA256: &str = "336d6daaf028c897e5b6e0c2f70d78999d3b0ea6ce0bcd03b1d4d869eb162c92";
const MAX_FILE: u64 = 1024 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageMetadata {
    pub role: String,
    pub layout: String,
    pub keymap: String,
    pub uf2: String,
    pub binary: String,
    pub uf2_sha256: String,
    pub binary_sha256: String,
    pub origin: u32,
    pub end_exclusive: u32,
    pub binary_size: usize,
    pub policy: String,
    pub source_commit: String,
    pub rmk_revision: String,
    pub battery_calibration: String,
    pub storage_revision: String,
    pub storage_preserved: bool,
    pub recovery_evidence: String,
    pub current_image_runtime_recovery: bool,
    pub bootloader: String,
    pub board_id: String,
    pub family_id: u32,
    pub softdevice: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    release_id: String,
    version: String,
    images: Vec<ImageMetadata>,
}
#[derive(Clone, Debug)]
pub struct ReleaseImage {
    pub metadata: ImageMetadata,
    pub uf2: Vec<u8>,
    pub binary: Vec<u8>,
    pub proof: ValidatedImage,
}
#[derive(Clone, Debug)]
pub struct FirmwareRelease {
    id: String,
    version: String,
    images: Vec<ReleaseImage>,
    selected_layout: KeyboardLayout,
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn filename(name: &str) -> Result<(), String> {
    let path = Path::new(name);
    if name.contains('\\')
        || path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
    {
        return Err("Firmware file must be a plain filename".into());
    }
    Ok(())
}
fn read_file(dir: &Path, name: &str, limit: u64) -> Result<Vec<u8>, String> {
    filename(name)?;
    let path = dir.join(name);
    let metadata =
        std::fs::symlink_metadata(&path).map_err(|_| "Bundled firmware is unavailable")?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err("Invalid bundled firmware file".into());
    }
    let bytes = std::fs::read(path).map_err(|_| "Could not read bundled firmware")?;
    if bytes.len() as u64 > limit {
        return Err("Bundled firmware exceeds size limit".into());
    }
    Ok(bytes)
}
impl FirmwareRelease {
    /// Only bundled resources are searched in release builds. Development may use
    /// the generated, ignored dist directory; both routes require identical pinned bytes.
    pub fn bundled() -> Result<Self, String> {
        let executable = std::env::current_exe().map_err(|_| "Could not locate Companion")?;
        #[cfg(target_os = "macos")]
        let resources = executable.parent().and_then(Path::parent);
        #[cfg(not(target_os = "macos"))]
        let resources = executable.parent();
        let resources = resources
            .map(|directory| directory.join("Resources/Firmware"))
            .ok_or("Could not locate Companion resources")?;
        if resources.exists() {
            return Self::load_from(&resources);
        }
        #[cfg(debug_assertions)]
        {
            let directory = if cfg!(feature = "firmware-trial") {
                "../dist/native-trial-candidates/companion"
            } else {
                "../dist/companion-firmware"
            };
            Self::load_from(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(directory))
        }
        #[cfg(not(debug_assertions))]
        {
            Err("This Companion build has no bundled firmware release".into())
        }
    }
    pub fn bundled_for(layout: KeyboardLayout) -> Result<Self, String> {
        let mut release = Self::bundled()?;
        if !release.native_controls() {
            return Err("The bundled firmware could not be loaded. Reinstall Companion.".into());
        }
        if !release.supports_layout(layout) {
            return Err(format!(
                "{} firmware is not available in this app yet",
                layout.label()
            ));
        }
        release.selected_layout = layout;
        Ok(release)
    }

    pub(crate) fn native_controls(&self) -> bool {
        !self.images.is_empty()
            && self.images.iter().all(|image| {
                image.metadata.rmk_revision == "434ab4d7d29d8e9ba689837358c8a44996ba38cc"
            })
    }

    pub fn supports_layout(&self, layout: KeyboardLayout) -> bool {
        ["left", "right"].into_iter().all(|role| {
            self.images
                .iter()
                .filter(|image| image.metadata.role == role && image.metadata.layout == layout.id())
                .count()
                == 1
        }) && self
            .images
            .iter()
            .filter(|image| image.metadata.role == "receiver")
            .count()
            == 1
    }

    pub fn layouts(&self) -> Vec<KeyboardLayout> {
        KeyboardLayout::ALL
            .into_iter()
            .filter(|layout| self.supports_layout(*layout))
            .collect()
    }

    /// Loads only this code-reviewed release; not an arbitrary package import API.
    pub fn load_from(dir: &Path) -> Result<Self, String> {
        let bytes = read_file(dir, "manifest.json", 32768)?;
        if digest(&bytes) != MANIFEST_SHA256 {
            return Err("The firmware list is not recognized".into());
        }
        let manifest: Manifest =
            serde_json::from_slice(&bytes).map_err(|_| "The firmware list is damaged")?;
        if manifest.schema != 1 || !matches!(manifest.images.len(), 3 | 9) {
            return Err("Companion cannot read this firmware list format".into());
        }
        let legacy = manifest.images.len() == 3;
        let mut expected: Vec<(&str, &str)> = KeyboardLayout::ALL
            .into_iter()
            .filter(|layout| !legacy || *layout == KeyboardLayout::Ansi)
            .flat_map(|layout| [("left", layout.id()), ("right", layout.id())])
            .collect();
        expected.push(("receiver", "ansi"));
        let mut images = Vec::new();
        for (metadata, (name, layout)) in manifest.images.into_iter().zip(expected) {
            let (policy, stem) = match name {
                "left" => (
                    ImagePolicy::LeftStartup,
                    if legacy {
                        "left".to_owned()
                    } else {
                        format!("left-{layout}")
                    },
                ),
                "right" => (
                    ImagePolicy::RightStartup,
                    if legacy {
                        "right".to_owned()
                    } else {
                        format!("right-{layout}")
                    },
                ),
                _ => (ImagePolicy::ReceiverProtectedPage, "receiver".to_owned()),
            };
            if metadata.layout != layout
                || metadata.role != name
                || metadata.uf2 != format!("{stem}.uf2")
                || metadata.binary != format!("{stem}.bin")
            {
                return Err("The firmware file is for a different keyboard part".into());
            }
            let uf2 = read_file(dir, &metadata.uf2, MAX_FILE)?;
            let binary = read_file(dir, &metadata.binary, MAX_FILE)?;
            if digest(&uf2) != metadata.uf2_sha256 || digest(&binary) != metadata.binary_sha256 {
                return Err(format!(
                    "{name} firmware does not match the verified release"
                ));
            }
            let proof =
                update_image::validate_for(policy, &uf2, &binary).map_err(|e| e.to_string())?;
            if proof.start() != metadata.origin
                || proof.end_exclusive() != metadata.end_exclusive
                || proof.binary_size() != metadata.binary_size
                || proof.family_id() != metadata.family_id
            {
                return Err("The firmware file has an unexpected size or address range".into());
            }
            images.push(ReleaseImage {
                metadata,
                uf2,
                binary,
                proof,
            });
        }
        Ok(Self {
            id: manifest.release_id,
            version: manifest.version,
            images,
            selected_layout: KeyboardLayout::Ansi,
        })
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn version(&self) -> &str {
        &self.version
    }
    pub fn image(&self, role: Role) -> &ReleaseImage {
        let (name, layout) = match role {
            Role::Left => ("left", self.selected_layout.id()),
            Role::Right => ("right", self.selected_layout.id()),
            Role::Receiver => ("receiver", "ansi"),
        };
        self.images
            .iter()
            .find(|image| image.metadata.role == name && image.metadata.layout == layout)
            .expect("selected firmware layout was validated")
    }
}

/// Synthetic guarded images for offline installer tests only. These never pass the
/// production release allowlist and are unavailable outside test builds.
#[cfg(test)]
pub(crate) fn fixture() -> FirmwareRelease {
    let images = [Role::Left, Role::Right, Role::Receiver]
        .into_iter()
        .map(|role| {
            let (name, policy) = match role {
                Role::Left => ("left", ImagePolicy::LeftStartup),
                Role::Right => ("right", ImagePolicy::RightStartup),
                Role::Receiver => ("receiver", ImagePolicy::ReceiverProtectedPage),
            };
            let origin = policy.start();
            let mut binary = vec![0u8; 0x2008];
            binary[..4].copy_from_slice(&0x20020000u32.to_le_bytes());
            binary[4..8].copy_from_slice(&(origin + 0x205).to_le_bytes());
            binary[0x200..0x204].fill(0xff);
            let mut padded = binary.clone();
            padded.resize(0x3000, 0xff);
            let blocks = padded.len() / 256;
            let mut uf2 = vec![0u8; blocks * 512];
            for (index, block) in uf2.as_chunks_mut::<512>().0.iter_mut().enumerate() {
                for (offset, word) in [
                    (0, 0x0a324655),
                    (4, 0x9e5d5157),
                    (8, 0x2000),
                    (12, origin + index as u32 * 256),
                    (16, 256),
                    (20, index as u32),
                    (24, blocks as u32),
                    (28, 0x621e937a),
                    (508, 0x0ab16f30),
                ] {
                    block[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
                }
                block[32..288].copy_from_slice(&padded[index * 256..(index + 1) * 256]);
            }
            let proof = update_image::validate_for(policy, &uf2, &binary).unwrap();
            let metadata = ImageMetadata {
                role: name.into(),
                layout: "ansi".into(),
                keymap: "mac".into(),
                uf2: format!("{name}.uf2"),
                binary: format!("{name}.bin"),
                uf2_sha256: proof.sha256().into(),
                binary_sha256: proof.binary_sha256().into(),
                origin,
                end_exclusive: proof.end_exclusive(),
                binary_size: binary.len(),
                policy: name.into(),
                source_commit: "synthetic".into(),
                rmk_revision: "synthetic".into(),
                battery_calibration: "none".into(),
                storage_revision: "synthetic".into(),
                storage_preserved: true,
                recovery_evidence: "synthetic".into(),
                current_image_runtime_recovery: false,
                bootloader: "0.9.2-39-g0147d71".into(),
                board_id: "NocFree &".into(),
                family_id: proof.family_id(),
                softdevice: "synthetic".into(),
            };
            ReleaseImage {
                metadata,
                uf2,
                binary,
                proof,
            }
        })
        .collect();
    FirmwareRelease {
        id: "synthetic-test-release".into(),
        version: "test".into(),
        images,
        selected_layout: KeyboardLayout::Ansi,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_controls_require_every_pinned_image() {
        let mut release = fixture();
        assert!(!release.native_controls());
        for image in &mut release.images {
            image.metadata.rmk_revision = "434ab4d7d29d8e9ba689837358c8a44996ba38cc".into();
        }
        assert!(release.native_controls());
        release.images[0].metadata.rmk_revision = "legacy".into();
        assert!(!release.native_controls());
        release.images.clear();
        assert!(!release.native_controls());
    }
    #[test]
    fn release_layout_requires_both_halves_to_agree_with_a_shared_receiver() {
        let mut release = fixture();
        assert!(release.supports_layout(KeyboardLayout::Ansi));
        assert_eq!(release.layouts(), vec![KeyboardLayout::Ansi]);
        for layout in [KeyboardLayout::Iso, KeyboardLayout::Jis, KeyboardLayout::Kr] {
            assert!(!release.supports_layout(layout));
        }
        release.images[1].metadata.layout = "iso".into();
        assert!(release.layouts().is_empty());
        release.images[0].metadata.layout = "iso".into();
        assert_eq!(release.layouts(), vec![KeyboardLayout::Iso]);
        assert_eq!(release.images[2].metadata.layout, "ansi");
    }

    #[test]
    fn selected_layout_uses_its_two_halves_and_the_shared_receiver() {
        let mut release = fixture();
        for layout in [KeyboardLayout::Iso, KeyboardLayout::Jis, KeyboardLayout::Kr] {
            for role in [Role::Left, Role::Right] {
                let mut image = release.image(role).clone();
                image.metadata.layout = layout.id().into();
                release.images.push(image);
            }
        }
        assert_eq!(release.layouts(), KeyboardLayout::ALL);
        release.selected_layout = KeyboardLayout::Jis;
        assert_eq!(release.image(Role::Left).metadata.layout, "jis");
        assert_eq!(release.image(Role::Right).metadata.layout, "jis");
        assert_eq!(release.image(Role::Receiver).metadata.layout, "ansi");
    }

    #[test]
    fn rejects_paths() {
        for name in [
            "../left.uf2",
            "/left.uf2",
            "left/left.uf2",
            "left\\left.uf2",
            ".",
            "..",
            "",
        ] {
            assert!(filename(name).is_err());
        }
        assert!(filename("left.uf2").is_ok());
    }
    #[test]
    #[ignore = "requires the reviewed local release; run packaging then --include-ignored"]
    fn generated_release_and_corruption_guards() {
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dist/companion-firmware");
        // Packaging is required before this integration test, never silently skipped.
        let release = FirmwareRelease::load_from(&source).unwrap();
        assert_eq!(release.image(Role::Right).proof.role(), Role::Right);
        assert_eq!(release.version(), "1.1.0");
        assert!(release.supports_layout(KeyboardLayout::Iso));
        assert!(release.supports_layout(KeyboardLayout::Jis));
        assert!(release.supports_layout(KeyboardLayout::Kr));
        let temporary =
            std::env::temp_dir().join(format!("nocfree-release-test-{}", std::process::id()));
        std::fs::create_dir_all(&temporary).unwrap();
        std::fs::copy(
            source.join("manifest.json"),
            temporary.join("manifest.json"),
        )
        .unwrap();
        for image in &release.images {
            for name in [&image.metadata.uf2, &image.metadata.binary] {
                std::fs::copy(source.join(name), temporary.join(name)).unwrap();
            }
        }
        let original = std::fs::read(temporary.join("manifest.json")).unwrap();
        let mut corrupt = original.clone();
        corrupt[0] ^= 1;
        std::fs::write(temporary.join("manifest.json"), corrupt).unwrap();
        assert!(FirmwareRelease::load_from(&temporary).is_err());
        std::fs::write(temporary.join("manifest.json"), &original).unwrap();
        // Half images share an origin: explicit role hash pins must reject a swap.
        std::fs::copy(
            source.join("right-ansi.uf2"),
            temporary.join("left-ansi.uf2"),
        )
        .unwrap();
        assert!(FirmwareRelease::load_from(&temporary).is_err());
        std::fs::copy(
            source.join("left-ansi.uf2"),
            temporary.join("left-ansi.uf2"),
        )
        .unwrap();
        let mut binary = std::fs::read(temporary.join("left-ansi.bin")).unwrap();
        binary[100] ^= 1;
        std::fs::write(temporary.join("left-ansi.bin"), binary).unwrap();
        assert!(FirmwareRelease::load_from(&temporary).is_err());
        std::fs::remove_dir_all(temporary).unwrap();
    }
}
