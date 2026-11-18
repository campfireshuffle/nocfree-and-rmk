//! One PWM owner; immutable RAM sequences, bounded DMA waits.
use crate::board_backlight::OutputReceiver;
#[cfg(target_arch = "arm")]
use embassy_nrf::{
    gpio::Level,
    pwm::{Config, Prescaler, SequenceConfig, SequencePwm, SingleSequenceMode, SingleSequencer},
};
#[cfg(target_arch = "arm")]
use embassy_time::Timer;
use embassy_time::{Duration, with_timeout};
#[cfg(target_arch = "arm")]
use static_cell::StaticCell;

const RETRY_DELAY: Duration = Duration::from_millis(100);

async fn next_level(changes: &mut OutputReceiver, level: u8, applied: bool) -> u8 {
    if applied {
        changes.changed().await
    } else {
        // A new request takes priority, including zero on sleep or disconnect.
        with_timeout(RETRY_DELAY, changes.changed())
            .await
            .unwrap_or(level)
    }
}

#[cfg(target_arch = "arm")]
pub fn new(
    pwm: embassy_nrf::Peri<'static, embassy_nrf::peripherals::PWM0>,
    pin: embassy_nrf::Peri<'static, embassy_nrf::peripherals::P0_20>,
) -> SequencePwm<'static> {
    let mut config = Config::default();
    config.prescaler = Prescaler::Div2;
    config.max_duty = 20000;
    config.ch0_idle_level = if cfg!(feature = "backlight-active-low") {
        Level::High
    } else {
        Level::Low
    };
    SequencePwm::new_1ch(pwm, pin, config).unwrap()
}

#[cfg(target_arch = "arm")]
pub async fn run(mut pwm: SequencePwm<'static>) -> ! {
    static WORDS: StaticCell<[[u16; 1]; 16]> = StaticCell::new();
    let words: &'static [[u16; 1]; 16] = WORDS.init(core::array::from_fn(|level| {
        [(20000 * level as u32 / 15) as u16
            | if cfg!(feature = "backlight-active-low") {
                0
            } else {
                0x8000
            }]
    }));
    let mut changes = crate::board_backlight::output_receiver();
    let mut level = changes.changed().await;
    loop {
        let mut loaded = pwm.event_seq_end();
        loaded.clear();
        let mut stopped = pwm.event_stopped();
        // Sole playback owner triggers STOP only after DMA load.
        let mut stop = unsafe { pwm.task_stop() };
        let sequence = SingleSequencer::new(
            &mut pwm,
            &words[usize::from(level.min(15))],
            SequenceConfig::default(),
        );
        let applied = if sequence.start(SingleSequenceMode::Infinite).is_err() {
            defmt::warn!("PWM sequence could not start");
            false
        } else if with_timeout(Duration::from_millis(20), async {
            while !loaded.is_triggered() {
                Timer::after_micros(100).await;
            }
        })
        .await
        .is_err()
        {
            defmt::warn!("PWM load timeout");
            false
        } else if level == 0 {
            stopped.clear();
            stop.trigger();
            let stopped_in_time = with_timeout(Duration::from_millis(20), async {
                while !stopped.is_triggered() {
                    Timer::after_micros(100).await;
                }
            })
            .await
            .is_ok();
            if !stopped_in_time {
                defmt::warn!("PWM stop timeout");
            }
            stopped_in_time
        } else {
            true
        };
        if !applied || level == 0 {
            sequence.stop();
        }
        // Keep successful nonzero playback running while waiting for a change.
        level = next_level(&mut changes, level, applied).await;
        drop(sequence);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embassy_futures::join::join;
    use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, watch::Watch};
    use embassy_time::{Instant, Timer};
    use rmk::test_support::test_block_on;

    #[test]
    fn failed_output_retries_without_a_notification() {
        static OUTPUT: Watch<CriticalSectionRawMutex, u8, 1> = Watch::new();
        test_block_on(async {
            let mut changes = OUTPUT.receiver().unwrap();
            OUTPUT.sender().send(9);
            let level = changes.changed().await;

            let start = Instant::now();
            assert_eq!(next_level(&mut changes, level, false).await, 9);
            assert!(start.elapsed() >= RETRY_DELAY);

            // Once applied, the same brightness needs no further attempt.
            assert!(
                with_timeout(RETRY_DELAY * 2, next_level(&mut changes, 9, true))
                    .await
                    .is_err()
            );
        });
    }

    #[test]
    fn off_request_interrupts_retry_delay() {
        static OUTPUT: Watch<CriticalSectionRawMutex, u8, 1> = Watch::new();
        test_block_on(async {
            let mut changes = OUTPUT.receiver().unwrap();
            OUTPUT.sender().send(9);
            let level = changes.changed().await;

            join(
                async {
                    let start = Instant::now();
                    assert_eq!(next_level(&mut changes, level, false).await, 0);
                    assert!(start.elapsed() < RETRY_DELAY);
                },
                async {
                    Timer::after_millis(10).await;
                    OUTPUT.sender().send(0);
                },
            )
            .await;
        });
    }
}
