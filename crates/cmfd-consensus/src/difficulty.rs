use primitive_types::{U256, U512};
use thiserror::Error;

pub const TARGET_SPACING_SECONDS: u64 = 60;
pub const DGW_WINDOW: usize = 180;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeaderWork {
    pub timestamp: u64,
    pub target: [u8; 32],
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DifficultyError {
    #[error("proof-of-work limit cannot be zero")]
    ZeroPowLimit,
    #[error("header target cannot be zero")]
    ZeroTarget,
    #[error("timestamps must be ordered oldest to newest")]
    TimestampOrder,
}

pub fn next_work_target(
    history: &[HeaderWork],
    pow_limit: [u8; 32],
) -> Result<[u8; 32], DifficultyError> {
    let pow_limit = U256::from_big_endian(&pow_limit);
    if pow_limit.is_zero() {
        return Err(DifficultyError::ZeroPowLimit);
    }
    if history.is_empty() {
        return Ok(to_bytes(pow_limit));
    }

    let window_start = history.len().saturating_sub(DGW_WINDOW);
    let window = &history[window_start..];
    let mut target_sum = U512::zero();
    for pair in window.windows(2) {
        if pair[1].timestamp < pair[0].timestamp {
            return Err(DifficultyError::TimestampOrder);
        }
    }
    for header in window {
        let target = U256::from_big_endian(&header.target);
        if target.is_zero() {
            return Err(DifficultyError::ZeroTarget);
        }
        target_sum += U512::from(target);
    }

    if window.len() == 1 {
        return Ok(to_bytes(
            U256::from_big_endian(&window[0].target).min(pow_limit),
        ));
    }

    let count = window.len() as u64;
    let average = target_sum / U512::from(count);
    let expected_span = (count - 1) * TARGET_SPACING_SECONDS;
    let measured_span = window.last().expect("nonempty window").timestamp
        - window.first().expect("nonempty window").timestamp;
    let clamped_span = measured_span.clamp(expected_span / 3, expected_span * 3);
    let adjusted = average * U512::from(clamped_span) / U512::from(expected_span);
    let capped = adjusted.min(U512::from(pow_limit));
    let target = if capped.is_zero() {
        U256::one()
    } else {
        U256::try_from(capped).expect("target was capped to U256")
    };
    Ok(to_bytes(target))
}

fn to_bytes(value: U256) -> [u8; 32] {
    value.to_big_endian()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(value: u64) -> [u8; 32] {
        to_bytes(U256::from(value))
    }

    fn history(spacing: u64, target_value: u64) -> Vec<HeaderWork> {
        (0..DGW_WINDOW)
            .map(|height| HeaderWork {
                timestamp: height as u64 * spacing,
                target: target(target_value),
            })
            .collect()
    }

    #[test]
    fn stable_blocks_keep_target() {
        assert_eq!(
            next_work_target(&history(60, 1_000_000), target(10_000_000)).unwrap(),
            target(1_000_000)
        );
    }

    #[test]
    fn fast_blocks_make_work_harder() {
        assert_eq!(
            next_work_target(&history(30, 1_000_000), target(10_000_000)).unwrap(),
            target(500_000)
        );
    }

    #[test]
    fn slow_blocks_make_work_easier() {
        assert_eq!(
            next_work_target(&history(120, 1_000_000), target(10_000_000)).unwrap(),
            target(2_000_000)
        );
    }

    #[test]
    fn retarget_is_clamped_and_never_exceeds_pow_limit() {
        assert_eq!(
            next_work_target(&history(600, 1_000_000), target(2_500_000)).unwrap(),
            target(2_500_000)
        );
        assert_eq!(
            next_work_target(&history(1, 900), target(10_000)).unwrap(),
            target(300)
        );
    }
}
