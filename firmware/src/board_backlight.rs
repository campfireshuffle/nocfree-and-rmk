//! Board brightness; input events never wait for flash or PWM.
use embassy_futures::{
    join::join,
    select::{Either, Either4, select, select4},
};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    signal::Signal,
    watch::{Receiver, Watch},
};
use embassy_time::{Duration, Instant, Timer};
use rmk::{
    custom_message::{self, CustomMessage, CustomMessageTarget},
    event::{
        ActionEvent, CentralConnectedEvent, EventSubscriber, PeripheralConnectedEvent,
        SleepStateEvent, SubscribableEvent,
    },
    types::action::Action,
};

const SLOT: u8 = 0;
const MAGIC: u8 = 0xb1;
// LEFT refreshes every two seconds; tolerate missed heartbeats and scheduling delay.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(6);
#[derive(Clone, Copy, PartialEq, Eq)]
struct Brightness {
    level: u8,
    remembered: u8,
    revision: u32,
}
impl Brightness {
    const fn new() -> Self {
        Self {
            level: 0,
            remembered: 1,
            revision: 0,
        }
    }
    fn press(&mut self, action: u8) -> bool {
        let level = match action {
            13 => self.remembered,
            14 => 0,
            15 => {
                if self.level == 0 {
                    self.remembered
                } else {
                    0
                }
            }
            11 => self.level.saturating_sub(1),
            12 => (self.level + 1).min(15),
            16 => (self.level + 1) % 16,
            _ => return false,
        };
        // First supported press owns startup intent, even if it leaves the level unchanged.
        if level == self.level && self.revision != 0 {
            return false;
        }
        self.level = level;
        if level != 0 {
            self.remembered = level;
        }
        self.revision = self.revision.wrapping_add(1).max(1);
        true
    }
    fn restore(&mut self, level: u8, remembered: u8) -> bool {
        if self.revision != 0 || level > 15 || !(1..=15).contains(&remembered) {
            return false;
        }
        self.level = level;
        self.remembered = remembered;
        // A restore publishes output, but does not need another flash write.
        true
    }
    fn frame(self) -> [u8; 6] {
        let r = self.revision.to_le_bytes();
        [MAGIC, self.level, r[0], r[1], r[2], r[3]]
    }
    fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 6 || bytes[0] != MAGIC || bytes[1] > 15 {
            return None;
        }
        Some(Self {
            level: bytes[1],
            remembered: bytes[1].max(1),
            revision: u32::from_le_bytes(bytes[2..6].try_into().ok()?),
        })
    }
}
static SETTINGS: Watch<CriticalSectionRawMutex, Brightness, 1> = Watch::new();
static OUTPUT: Watch<CriticalSectionRawMutex, u8, 1> = Watch::new();
static RESTORE: Signal<CriticalSectionRawMutex, (u8, u8)> = Signal::new();
pub type OutputReceiver = Receiver<'static, CriticalSectionRawMutex, u8, 1>;
pub fn output_receiver() -> OutputReceiver {
    OUTPUT.receiver().expect("one PWM owner")
}
fn output(level: u8, sleeping: bool) {
    OUTPUT.sender().send(if sleeping { 0 } else { level });
}
#[cfg(test)]
static SNAPSHOTS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
fn snapshot(state: Brightness) {
    #[cfg(test)]
    SNAPSHOTS.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
    if let Ok(message) = CustomMessage::new(&state.frame(), CustomMessageTarget::Peripherals) {
        custom_message::send(message);
    }
}

pub async fn run_left() -> ! {
    join(left_events(), save_worker()).await;
    core::future::pending().await
}
async fn left_events() -> ! {
    let mut keys = ActionEvent::subscriber();
    let mut links = PeripheralConnectedEvent::subscriber();
    let mut sleep = SleepStateEvent::subscriber();
    let mut state = Brightness::new();
    let mut sleeping = false;
    let mut right_connected = false;
    SETTINGS.sender().send(state);
    output(state.level, sleeping);
    let mut next_snapshot = Instant::now() + Duration::from_secs(2);
    loop {
        // Check before biased event selection: even continuously ready input cannot starve refresh.
        if right_connected && !sleeping && Instant::now() >= next_snapshot {
            snapshot(state);
            next_snapshot = Instant::now() + Duration::from_secs(2);
        }
        // Periodic absolute snapshots recover messages lost during reconnect registration.
        match select(
            select4(
                keys.next_event(),
                links.next_event(),
                sleep.next_event(),
                RESTORE.wait(),
            ),
            async {
                if right_connected && !sleeping {
                    Timer::at(next_snapshot).await
                } else {
                    core::future::pending().await
                }
            },
        )
        .await
        {
            Either::First(Either4::First(event)) => {
                if event.keyboard_event.pressed
                    && let Action::User(action) = event.action
                    && state.press(action)
                {
                    SETTINGS.sender().send(state);
                    output(state.level, sleeping);
                    if right_connected && !sleeping {
                        snapshot(state);
                    }
                }
            }
            Either::First(Either4::Second(event)) => {
                if event.id == 0 {
                    right_connected = event.connected;
                    next_snapshot = Instant::now() + Duration::from_secs(2);
                    if right_connected && !sleeping {
                        snapshot(state);
                    }
                }
            }
            Either::First(Either4::Third(event)) => {
                sleeping = event.0;
                next_snapshot = Instant::now() + Duration::from_secs(2);
                output(state.level, sleeping);
                if right_connected && !sleeping {
                    snapshot(state);
                }
            }
            Either::First(Either4::Fourth(level)) => {
                if state.restore(level.0, level.1) {
                    output(state.level, sleeping);
                    if right_connected && !sleeping {
                        snapshot(state);
                    }
                }
            }
            Either::Second(_) => {
                snapshot(state);
                next_snapshot = Instant::now() + Duration::from_secs(2);
            }
        }
    }
}
async fn save_worker() -> ! {
    let mut changes = SETTINGS.receiver().expect("one save worker");
    if let Some(bytes) = rmk::storage::read_user_data(SLOT).await
        && bytes.len() == 3
        && bytes[0] == MAGIC
        && bytes[1] < 16
        && (1..=15).contains(&bytes[2])
    {
        RESTORE.signal((bytes[1], bytes[2]));
    }
    let mut state = changes.changed().await;
    loop {
        if state.revision == 0 {
            state = changes.changed().await;
            continue;
        }
        match select(changes.changed(), Timer::after_secs(2)).await {
            Either::First(new) => {
                state = new;
                continue;
            }
            Either::Second(_) => {}
        }
        let bytes = [MAGIC, state.level, state.remembered];
        // Ordered readback proves this queued write reached the actual storage task.
        let verified = rmk::storage::store_user_data(SLOT, &bytes).await.is_ok()
            && rmk::storage::read_user_data(SLOT).await.as_deref() == Some(bytes.as_slice());
        if verified {
            state = changes.changed().await;
        } else {
            Timer::after(Duration::from_secs(1)).await;
        }
    }
}

pub async fn run_right() -> ! {
    let mut messages = CustomMessage::subscriber();
    let mut links = CentralConnectedEvent::subscriber();
    let mut sleep = SleepStateEvent::subscriber();
    let mut state: Option<Brightness> = None;
    let mut expires = None;
    let mut connected = false;
    let mut sleeping = false;
    output(0, false);
    loop {
        match select4(
            messages.next_event(),
            links.next_event(),
            sleep.next_event(),
            async move {
                if let Some(deadline) = expires {
                    Timer::at(deadline).await
                } else {
                    core::future::pending().await
                }
            },
        )
        .await
        {
            Either4::First(message) => {
                if connected
                    && message.target == CustomMessageTarget::Peripherals
                    && let Some(new) = Brightness::decode(&message.data)
                    && state.is_none_or(|old| new.revision.wrapping_sub(old.revision) < (1 << 31))
                {
                    state = Some(new);
                    expires = Some(Instant::now() + SNAPSHOT_TIMEOUT);
                }
            }
            Either4::Second(event) => {
                connected = event.connected;
                state = None;
                expires = None;
            }
            Either4::Third(event) => sleeping = event.0,
            Either4::Fourth(()) => {}
        }
        // Check after every event as well as the timer, so unrelated traffic
        // cannot starve expiry. Retain the revision until a real link event.
        if expires.is_some_and(|deadline| Instant::now() >= deadline) {
            expires = None;
        }
        let level = match state {
            Some(state) if connected && expires.is_some() => state.level,
            _ => 0,
        };
        output(level, sleeping);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn steps_are_bounded() {
        let mut state = Brightness::new();
        for _ in 0..30 {
            state.press(12);
        }
        assert_eq!(state.level, 15);
        for _ in 0..30 {
            state.press(11);
        }
        assert_eq!(state.level, 0);
        state.press(16);
        assert_eq!(state.level, 1);
    }
    #[test]
    fn late_restore_cannot_overwrite_input() {
        let mut state = Brightness::new();
        assert!(state.restore(12, 12));
        state.press(11);
        assert!(!state.restore(3, 3));
        assert_eq!(state.level, 11);
    }
    #[test]
    fn reject_foreign_truncated_and_invalid_snapshots() {
        let state = Brightness::new();
        assert_eq!(Brightness::decode(&state.frame()).unwrap().level, 0);
        assert!(Brightness::decode(&[MAGIC, 16, 0, 0, 0, 0]).is_none());
        assert!(Brightness::decode(&[MAGIC, 2]).is_none());
        assert!(Brightness::decode(&[0, 2, 0, 0, 0, 0]).is_none());
    }
    #[test]
    fn off_and_toggle_preserve_the_last_nonzero_level() {
        let mut state = Brightness::new();
        state.press(12);
        state.press(12);
        state.press(14);
        assert_eq!((state.level, state.remembered), (0, 2));
        state.press(15);
        assert_eq!(state.level, 2);
    }
    #[test]
    fn actual_tasks_keep_input_live_during_storage_delay_and_resync_right() {
        use rmk::{
            config::StorageConfig,
            core_traits::Runnable,
            event::publish_event,
            storage::{Storage, async_flash_wrapper},
            test_support::{InMemoryFlash, test_block_on},
        };
        test_block_on(async {
            let part = InMemoryFlash::<16384, 4096, 1>::new();
            let mut storage = Storage::<_, 1, 85, 1, 0>::new(
                async_flash_wrapper(part),
                &StorageConfig::default(),
            )
            .await;
            let mut out = output_receiver();
            let storage_task = async {
                Timer::after_secs(5).await;
                storage.run().await
            };
            let scenario = async {
                assert_eq!(out.changed().await, 0);
                let press = |pressed| ActionEvent {
                    action: Action::User(12),
                    keyboard_event: rmk::event::KeyboardEvent::key(0, 0, pressed),
                };
                publish_event(press(true));
                assert_eq!(out.changed().await, 1);
                Timer::after_secs(1).await;
                // Keep the key pressed: one press must not repeat while flash is unavailable.
                assert_eq!(out.try_get(), Some(1));
                publish_event(press(false));
                Timer::after_millis(1).await;
                assert_eq!(out.try_get(), Some(1));
                publish_event(press(true));
                assert_eq!(out.changed().await, 2);
                Timer::after_secs(8).await;
                assert_eq!(
                    rmk::storage::read_user_data(SLOT).await.unwrap().as_slice(),
                    &[MAGIC, 2, 2]
                );
            };
            assert!(matches!(
                select(join(run_left(), storage_task), scenario).await,
                Either::Second(())
            ));
            drop(out);
            let mut out = output_receiver();
            let scenario = async {
                assert_eq!(out.changed().await, 0);
                publish_event(CentralConnectedEvent { connected: true });
                assert_eq!(out.changed().await, 0);
                let message = |level, revision| {
                    CustomMessage::new(
                        &Brightness {
                            level,
                            remembered: level.max(1),
                            revision,
                        }
                        .frame(),
                        CustomMessageTarget::Peripherals,
                    )
                    .unwrap()
                };
                publish_event(message(9, 4));
                assert_eq!(out.changed().await, 9);
                publish_event(message(3, 2));
                assert_eq!(out.changed().await, 9); // stale snapshot cannot revert output
                publish_event(CentralConnectedEvent { connected: false });
                assert_eq!(out.changed().await, 0);
                publish_event(CentralConnectedEvent { connected: true });
                assert_eq!(out.changed().await, 0);
                publish_event(message(7, 1)); // LEFT reboot: first fresh snapshot accepted
                assert_eq!(out.changed().await, 7);
                publish_event(SleepStateEvent(true));
                assert_eq!(out.changed().await, 0);
                publish_event(SleepStateEvent(false));
                assert_eq!(out.changed().await, 7);
            };
            assert!(matches!(
                select(run_right(), scenario).await,
                Either::Second(())
            ));
        });
    }
}

#[cfg(test)]
mod freshness_tests {
    use super::*;
    use rmk::{event::publish_event, test_support::test_block_on};

    fn message(level: u8, revision: u32) -> CustomMessage {
        CustomMessage::new(
            &Brightness {
                level,
                remembered: level.max(1),
                revision,
            }
            .frame(),
            CustomMessageTarget::Peripherals,
        )
        .unwrap()
    }

    #[test]
    fn silence_expires_without_losing_revision_order_and_sleep_does_not_refresh() {
        test_block_on(async {
            let mut out = output_receiver();
            let scenario = async {
                assert_eq!(out.changed().await, 0);
                publish_event(CentralConnectedEvent { connected: true });
                assert_eq!(out.changed().await, 0);
                publish_event(message(9, 4));
                assert_eq!(out.changed().await, 9);
                Timer::after_secs(7).await;
                assert_eq!(
                    out.try_get(),
                    Some(0),
                    "silence must expire while connected"
                );
                publish_event(message(3, 2));
                assert_eq!(
                    out.changed().await,
                    0,
                    "expiry must not admit an older revision"
                );
                publish_event(message(9, 4));
                assert_eq!(
                    out.changed().await,
                    9,
                    "an equal-revision heartbeat restores output"
                );
                publish_event(SleepStateEvent(true));
                assert_eq!(out.changed().await, 0);
                Timer::after_secs(7).await;
                assert_eq!(out.try_get(), Some(0));
                publish_event(SleepStateEvent(false));
                assert_eq!(
                    out.changed().await,
                    0,
                    "wake must not revive an expired snapshot"
                );
                publish_event(message(10, 5));
                assert_eq!(out.changed().await, 10);
            };
            assert!(matches!(
                select(run_right(), scenario).await,
                Either::Second(())
            ));
        });
    }

    #[test]
    fn equal_revision_heartbeats_extend_freshness_and_reconnect_accepts_reboot() {
        test_block_on(async {
            let mut out = output_receiver();
            let scenario = async {
                assert_eq!(out.changed().await, 0);
                publish_event(CentralConnectedEvent { connected: true });
                assert_eq!(out.changed().await, 0);
                publish_event(message(9, 4));
                assert_eq!(out.changed().await, 9);
                for _ in 0..4 {
                    Timer::after_secs(2).await;
                    assert_eq!(out.try_get(), Some(9));
                    publish_event(message(9, 4));
                    assert_eq!(out.changed().await, 9);
                }
                Timer::after_secs(5).await;
                assert_eq!(
                    out.try_get(),
                    Some(9),
                    "deadline follows the last valid heartbeat"
                );
                Timer::after_secs(2).await;
                assert_eq!(out.try_get(), Some(0));
                publish_event(message(9, 4));
                assert_eq!(out.changed().await, 9);
                publish_event(CentralConnectedEvent { connected: false });
                assert_eq!(out.changed().await, 0);
                publish_event(CentralConnectedEvent { connected: true });
                assert_eq!(out.changed().await, 0);
                publish_event(message(6, 1)); // LEFT rebooted during disconnection.
                assert_eq!(out.changed().await, 6);
            };
            assert!(matches!(
                select(run_right(), scenario).await,
                Either::Second(())
            ));
        });
    }

    #[test]
    fn continuous_invalid_or_stale_traffic_cannot_postpone_expiry() {
        test_block_on(async {
            let mut out = output_receiver();
            let scenario = async {
                assert_eq!(out.changed().await, 0);
                publish_event(CentralConnectedEvent { connected: true });
                assert_eq!(out.changed().await, 0);
                publish_event(message(9, 4));
                assert_eq!(out.changed().await, 9);
                for i in 0..70 {
                    let message = match i % 4 {
                        0 => CustomMessage::new(&[MAGIC, 9], CustomMessageTarget::Peripherals)
                            .unwrap(),
                        1 => CustomMessage::new(
                            &[MAGIC, 16, 5, 0, 0, 0],
                            CustomMessageTarget::Peripherals,
                        )
                        .unwrap(),
                        2 => message(3, 2),
                        _ => CustomMessage::new(&message(12, 5).data, CustomMessageTarget::Central)
                            .unwrap(),
                    };
                    publish_event(message);
                    Timer::after_millis(100).await;
                }
                assert_eq!(
                    out.try_get(),
                    Some(0),
                    "only accepted snapshots can refresh the deadline"
                );
            };
            assert!(matches!(
                select(run_right(), scenario).await,
                Either::Second(())
            ));
        });
    }
}

#[cfg(test)]
mod deadline_review {
    use super::*;
    use rmk::{
        event::{KeyboardEvent, publish_event},
        test_support::test_block_on,
    };
    #[test]
    fn ordinary_typing_cannot_postpone_snapshot_refresh() {
        test_block_on(async {
            let scenario = async {
                Timer::after_millis(1).await;
                publish_event(PeripheralConnectedEvent {
                    id: 0,
                    connected: true,
                });
                for _ in 0..60 {
                    Timer::after_millis(100).await;
                    publish_event(ActionEvent {
                        action: Action::No,
                        keyboard_event: KeyboardEvent::key(0, 0, true),
                    });
                }
                assert!(
                    SNAPSHOTS.load(core::sync::atomic::Ordering::SeqCst) >= 3,
                    "Expected reconnect plus periodic snapshots during typing; got {}",
                    SNAPSHOTS.load(core::sync::atomic::Ordering::SeqCst)
                );
            };
            assert!(matches!(
                select(left_events(), scenario).await,
                Either::Second(())
            ));
        });
    }
}

#[cfg(test)]
mod ownership_review {
    use super::*;
    #[test]
    fn initial_noop_claims_ownership_but_repeated_noop_is_not_dirty() {
        for (level, action) in [(0, 14), (15, 12), (1, 13)] {
            let mut state = Brightness {
                level,
                remembered: level.max(1),
                revision: 0,
            };
            assert!(state.press(action));
            assert_eq!(state.level, level);
            let revision = state.revision;
            assert!(!state.press(action));
            assert_eq!(state.revision, revision);
            assert!(!state.restore(12, 12));
        }
    }
}
