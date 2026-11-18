#[path = "../../../firmware/src/startup.rs"]
pub mod startup;

#[path = "../../../firmware/src/board_status_led.rs"]
pub mod board_status_led;

#[path = "../../../firmware/src/board_backlight.rs"]
pub mod board_backlight;

#[cfg(test)]
#[path = "../../../firmware/src/board_pwm.rs"]
mod board_pwm;

#[cfg(test)]
mod review {
    use embassy_futures::{
        join::join,
        select::{Either, select},
    };
    use embassy_time::Timer;
    use rmk::{
        config::StorageConfig,
        core_traits::Runnable,
        event::{ActionEvent, KeyboardEvent, publish_event},
        storage::{Storage, async_flash_wrapper, read_user_data, store_user_data},
        test_support::{InMemoryFlash, test_block_on},
        types::action::Action,
    };
    #[test]
    fn explicit_off_before_late_restore_must_keep_light_off() {
        test_block_on(async {
            let part = InMemoryFlash::<16384, 4096, 1>::new();
            let mut storage = Storage::<_, 1, 85, 1, 0>::new(
                async_flash_wrapper(part),
                &StorageConfig::default(),
            )
            .await;
            assert!(matches!(
                select(storage.run(), async {
                    store_user_data(0, &[0xb1, 12, 12]).await.unwrap();
                    assert_eq!(read_user_data(0).await.unwrap().as_slice(), &[0xb1, 12, 12]);
                })
                .await,
                Either::Second(())
            ));
            let mut output = super::board_backlight::output_receiver();
            let scenario = async {
                assert_eq!(output.changed().await, 0);
                publish_event(ActionEvent {
                    action: Action::User(14),
                    keyboard_event: KeyboardEvent::key(0, 0, true),
                });
                Timer::after_secs(4).await;
                assert_eq!(
                    output.try_get(),
                    Some(0),
                    "Explicit OFF before the delayed read must defeat startup restore"
                );
            };
            let delayed_storage = async {
                Timer::after_secs(1).await;
                storage.run().await
            };
            assert!(matches!(
                select(
                    join(super::board_backlight::run_left(), delayed_storage),
                    scenario
                )
                .await,
                Either::Second(())
            ));
        });
    }
}
#[cfg(test)]
mod retry_review {
    use embassy_futures::{
        join::join,
        select::{Either, select},
    };
    use embassy_time::Timer;
    use rmk::{
        config::StorageConfig,
        core_traits::Runnable,
        event::{ActionEvent, KeyboardEvent, publish_event},
        storage::{Storage, async_flash_wrapper, read_user_data, store_user_data},
        test_support::{InMemoryFlash, test_block_on},
        types::action::Action,
    };
    #[test]
    fn failed_save_retry_keeps_new_input_and_persists_latest_revision() {
        test_block_on(async {
            let part = InMemoryFlash::<16384, 4096, 1>::new();
            let mut storage = Storage::<_, 1, 85, 1, 0>::new(
                async_flash_wrapper(part.clone()),
                &StorageConfig::default(),
            )
            .await;
            assert!(matches!(
                select(storage.run(), async {
                    store_user_data(0, &[0xb1, 1, 1]).await.unwrap();
                    assert_eq!(read_user_data(0).await.unwrap().as_slice(), &[0xb1, 1, 1]);
                })
                .await,
                Either::Second(())
            ));
            let mut output = super::board_backlight::output_receiver();
            let scenario = async {
                while output.changed().await != 1 {}
                part.fail_writes(true);
                let press = ActionEvent {
                    action: Action::User(12),
                    keyboard_event: KeyboardEvent::key(0, 0, true),
                };
                publish_event(press);
                assert_eq!(output.changed().await, 2);
                Timer::after_secs(4).await;
                publish_event(press);
                assert_eq!(output.changed().await, 3);
                Timer::after_secs(4).await;
                assert_eq!(read_user_data(0).await.unwrap().as_slice(), &[0xb1, 1, 1]);
                part.fail_writes(false);
                Timer::after_secs(5).await;
                assert_eq!(read_user_data(0).await.unwrap().as_slice(), &[0xb1, 3, 3]);
            };
            assert!(matches!(
                select(
                    join(super::board_backlight::run_left(), storage.run()),
                    scenario
                )
                .await,
                Either::Second(())
            ));
        });
    }
}
