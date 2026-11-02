//! Pure setup assessment. Unseen parts never make an entire keyboard current.
use crate::{
    device::Device,
    device_status::UsbKey,
    firmware_version::Observation,
    navigation::{Page, Readiness},
    runtime_recovery::Role,
    scope::Scope,
};

#[cfg(test)]
pub(crate) fn assess(
    page: Page,
    devices: &UsbKey,
    versions: &[Observation],
    latest: Option<&str>,
    fresh: bool,
) -> Readiness {
    assess_scoped(page, devices, versions, latest, fresh, Some(Scope::Whole))
}

pub(crate) fn assess_scoped(
    page: Page,
    devices: &UsbKey,
    versions: &[Observation],
    latest: Option<&str>,
    fresh: bool,
    scope: Option<Scope>,
) -> Readiness {
    let Some(scope) = scope else {
        return Readiness::Unknown;
    };
    if !fresh || devices.is_empty() {
        return Readiness::Unknown;
    }
    let devices: Vec<_> = devices
        .iter()
        .map(|(location, vendor, product, name)| Device {
            location: *location,
            vendor: *vendor,
            product: *product,
            name: name.clone(),
        })
        .collect();
    if scope == Scope::Whole && devices.iter().any(Device::bootloader) {
        return Readiness::Unknown;
    }
    if page == Page::Restore {
        if let Scope::Part(role) = scope {
            return if role == Role::Right
                && devices.iter().filter(|d| d.factory_right()).count() == 1
                && !devices.iter().any(|d| {
                    d.vendor == 0x4c4b
                        && d.product == u64::from(Role::Right.product())
                        && d.name == Role::Right.name()
                }) {
                Readiness::AlreadyFactory
            } else {
                // Left and dongle share a stock descriptor. Selection alone is
                // not physical identity; recovery correlation resolves it later.
                Readiness::Needed
            };
        }
        if scope != Scope::Whole {
            return Readiness::Needed;
        }
        if devices.iter().filter(|d| d.factory_left()).count() == 2
            && devices.iter().filter(|d| d.factory_right()).count() == 1
            && !devices.iter().any(|d| {
                d.rmk_left()
                    || d.rmk_receiver()
                    || (d.vendor == 0x4c4b
                        && d.product == u64::from(Role::Right.product())
                        && d.name == Role::Right.name())
            })
        {
            return Readiness::AlreadyFactory;
        }
        return Readiness::Needed;
    }
    if page != Page::Firmware {
        return Readiness::Unknown;
    }
    let Some(latest) = latest else {
        return Readiness::Unknown;
    };
    let roles = scope.roles();
    let includes_half = roles.iter().any(|role| *role != Role::Receiver);
    for role in roles {
        let matching: Vec<_> = devices
            .iter()
            .filter(|d| {
                d.vendor == 0x4c4b
                    && d.product == u64::from(role.product())
                    && d.name == role.name()
            })
            .collect();
        if matching.len() != 1 {
            return Readiness::Needed;
        }
        let matching_versions: Vec<_> = versions
            .iter()
            .filter(|v| !v.factory && v.role == role && v.location == matching[0].location)
            .collect();
        if matching_versions.len() != 1 || matching_versions[0].version != latest {
            return Readiness::Needed;
        }
    }
    // Versions do not identify the halves' physical layout. Recovery readback
    // must compare the selected image before deciding that no write is needed.
    if includes_half {
        Readiness::Needed
    } else {
        Readiness::AlreadyLatest
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn rmk() -> (UsbKey, Vec<Observation>) {
        let mut key = vec![];
        let mut versions = vec![];
        for (i, role) in [Role::Left, Role::Right, Role::Receiver]
            .into_iter()
            .enumerate()
        {
            key.push((
                i as u64,
                0x4c4b,
                u64::from(role.product()),
                role.name().into(),
            ));
            versions.push(Observation {
                location: i as u64,
                factory: false,
                role,
                version: "1.2.3".into(),
            });
        }
        (key, versions)
    }
    #[test]
    fn same_version_halves_can_start_every_layout_installation() {
        use crate::{
            keyboard_layout::KeyboardLayout,
            navigation::{Navigation, Start},
        };
        let (key, versions) = rmk();
        // Version observations carry no installed-layout identity. Every requested
        // layout, including ANSI, must reach recovery's exact-image comparison.
        for layout in KeyboardLayout::ALL {
            for scope in [
                Scope::Part(Role::Left),
                Scope::Part(Role::Right),
                Scope::Pair(Role::Left, Role::Right),
                Scope::Pair(Role::Left, Role::Receiver),
                Scope::Pair(Role::Right, Role::Receiver),
                Scope::Whole,
            ] {
                let mut nav = Navigation::with_layout(layout);
                nav.observe_available([true; 3]);
                nav.observe_layouts(&KeyboardLayout::ALL);
                nav.select_scope(scope);
                nav.observe(assess_scoped(
                    Page::Firmware,
                    &key,
                    &versions,
                    Some("1.2.3"),
                    true,
                    nav.scope(),
                ));
                assert!(nav.can_start(), "{layout:?} {scope:?}");
                assert_eq!(nav.next(), Some(Start::Firmware(scope)));
            }
        }
    }
    #[test]
    fn same_version_receiver_remains_current_without_layout_evidence() {
        let (key, versions) = rmk();
        assert_eq!(
            assess_scoped(
                Page::Firmware,
                &key,
                &versions,
                Some("1.2.3"),
                true,
                Some(Scope::Part(Role::Receiver)),
            ),
            Readiness::AlreadyLatest
        );
    }
    #[test]
    fn whole_release_requires_readback_even_with_matching_live_versions() {
        let (mut key, mut versions) = rmk();
        assert_eq!(
            assess(Page::Firmware, &key, &versions, Some("1.2.3"), true),
            Readiness::Needed
        );
        assert_eq!(
            assess(Page::Firmware, &key, &versions, Some("1.2.3"), false),
            Readiness::Unknown
        );
        versions[1].version = "1.2.2".into();
        assert_eq!(
            assess(Page::Firmware, &key, &versions, Some("1.2.3"), true),
            Readiness::Needed
        );
        versions[1].version = "1.2.3".into();
        versions[1].location = 100;
        assert_eq!(
            assess(Page::Firmware, &key, &versions, Some("1.2.3"), true),
            Readiness::Needed
        );
        key.pop();
        assert_eq!(
            assess(Page::Firmware, &key, &versions, Some("1.2.3"), true),
            Readiness::Needed
        );
    }
    #[test]
    fn part_preflight_needs_only_selected_fresh_role_and_never_guesses_shared_factory() {
        let (key, versions) = rmk();
        let key = vec![key[1].clone()];
        let versions = vec![versions[1].clone()];
        assert_eq!(
            assess_scoped(
                Page::Firmware,
                &key,
                &versions,
                Some("1.2.3"),
                true,
                Some(Scope::Part(Role::Right))
            ),
            Readiness::Needed
        );
        assert_eq!(
            assess_scoped(
                Page::Firmware,
                &key,
                &versions,
                Some("1.2.3"),
                true,
                Some(Scope::Part(Role::Left))
            ),
            Readiness::Needed
        );
        assert_eq!(
            assess_scoped(Page::Firmware, &key, &versions, Some("1.2.3"), true, None),
            Readiness::Unknown
        );
        let stock = vec![
            (10, 0x2886, 0x8029, "NocFree & ANSI".into()),
            (12, 0x239a, 0x80d8, "NocFree nRF52833 Right".into()),
        ];
        for role in [Role::Left, Role::Receiver] {
            assert_eq!(
                assess_scoped(
                    Page::Restore,
                    &stock,
                    &[],
                    None,
                    true,
                    Some(Scope::Part(role))
                ),
                Readiness::Needed
            );
        }
        assert_eq!(
            assess_scoped(
                Page::Restore,
                &stock,
                &[],
                None,
                true,
                Some(Scope::Part(Role::Right))
            ),
            Readiness::AlreadyFactory
        );
        let mut mixed_right = stock;
        mixed_right.extend(key);
        assert_eq!(
            assess_scoped(
                Page::Restore,
                &mixed_right,
                &[],
                None,
                true,
                Some(Scope::Part(Role::Right))
            ),
            Readiness::Needed
        );
    }
    #[test]
    fn factory_is_version_independent_but_mixed_or_missing_parts_are_not_complete() {
        let mut key = vec![
            (10, 0x2886, 0x8029, "NocFree & ANSI".into()),
            (11, 0x2886, 0x8029, "NocFree & ANSI".into()),
            (12, 0x239a, 0x80d8, "NocFree nRF52833 Right".into()),
        ];
        assert_eq!(
            assess(Page::Restore, &key, &[], None, true),
            Readiness::AlreadyFactory
        );
        key.pop();
        assert_eq!(
            assess(Page::Restore, &key, &[], None, true),
            Readiness::Needed
        );
        key.extend(rmk().0);
        assert_eq!(
            assess(Page::Restore, &key, &[], None, true),
            Readiness::Needed
        );
    }
}
