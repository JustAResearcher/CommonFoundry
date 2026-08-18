use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const COIN: u64 = 100_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonetaryPolicy {
    pub initial_subsidy: u64,
    pub halving_interval: u64,
    pub tail_height: u64,
    pub tail_subsidy: u64,
    pub steward_percent: u8,
    pub community_percent: u8,
}

pub const DEFAULT_MONETARY_POLICY: MonetaryPolicy = MonetaryPolicy {
    initial_subsidy: 500 * COIN,
    halving_interval: 2_102_400,
    tail_height: 14_716_800,
    tail_subsidy: 5 * COIN,
    steward_percent: 25,
    community_percent: 5,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Allocation {
    pub subsidy: u64,
    pub miner: u64,
    pub steward: u64,
    pub community: u64,
    pub fees_burned: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoinbaseClaim {
    pub miner: u64,
    pub steward: u64,
    pub community: u64,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EconomicsError {
    #[error("coinbase miner output is {actual}, expected {expected}")]
    Miner { actual: u64, expected: u64 },
    #[error("coinbase steward output is {actual}, expected {expected}")]
    Steward { actual: u64, expected: u64 },
    #[error("coinbase community output is {actual}, expected {expected}")]
    Community { actual: u64, expected: u64 },
    #[error("monetary policy percentages exceed 100")]
    InvalidPercentages,
}

impl MonetaryPolicy {
    pub fn validate(self) -> Result<(), EconomicsError> {
        if u16::from(self.steward_percent) + u16::from(self.community_percent) > 100 {
            return Err(EconomicsError::InvalidPercentages);
        }
        Ok(())
    }

    pub fn subsidy(self, height: u64) -> u64 {
        if height >= self.tail_height {
            return self.tail_subsidy;
        }

        let halvings = height / self.halving_interval;
        self.initial_subsidy
            .checked_shr(halvings as u32)
            .unwrap_or(0)
    }

    pub fn allocation(self, height: u64, fees: u64) -> Result<Allocation, EconomicsError> {
        self.validate()?;
        let subsidy = self.subsidy(height);

        if height >= self.tail_height {
            return Ok(Allocation {
                subsidy,
                miner: subsidy,
                steward: 0,
                community: 0,
                fees_burned: fees,
            });
        }

        let steward = subsidy * u64::from(self.steward_percent) / 100;
        let community = subsidy * u64::from(self.community_percent) / 100;
        let miner = subsidy - steward - community;
        Ok(Allocation {
            subsidy,
            miner,
            steward,
            community,
            fees_burned: fees,
        })
    }

    pub fn validate_coinbase(
        self,
        height: u64,
        fees: u64,
        claim: CoinbaseClaim,
    ) -> Result<Allocation, EconomicsError> {
        let expected = self.allocation(height, fees)?;
        if claim.miner != expected.miner {
            return Err(EconomicsError::Miner {
                actual: claim.miner,
                expected: expected.miner,
            });
        }
        if claim.steward != expected.steward {
            return Err(EconomicsError::Steward {
                actual: claim.steward,
                expected: expected.steward,
            });
        }
        if claim.community != expected.community {
            return Err(EconomicsError::Community {
                actual: claim.community,
                expected: expected.community,
            });
        }
        Ok(expected)
    }

    pub fn scheduled_supply_before_tail(self) -> u128 {
        let eras = self.tail_height / self.halving_interval;
        (0..eras)
            .map(|era| {
                u128::from(self.initial_subsidy.checked_shr(era as u32).unwrap_or(0))
                    * u128::from(self.halving_interval)
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emission_schedule_and_tail_are_exact() {
        let p = DEFAULT_MONETARY_POLICY;
        let expected = [500, 250, 125, 62, 31, 15, 7];
        for (era, whole_coins) in expected.into_iter().enumerate() {
            let subsidy = p.subsidy(era as u64 * p.halving_interval);
            if era < 3 {
                assert_eq!(subsidy, whole_coins * COIN);
            }
        }
        assert_eq!(p.subsidy(6 * p.halving_interval), 781_250_000);
        assert_eq!(p.subsidy(p.tail_height - 1), 781_250_000);
        assert_eq!(p.subsidy(p.tail_height), 5 * COIN);
        assert_eq!(p.scheduled_supply_before_tail(), 208_597_500_000_000_000);
    }

    #[test]
    fn fees_are_burned_and_never_enter_coinbase() {
        let p = DEFAULT_MONETARY_POLICY;
        let fees = 12 * COIN;
        let allocation = p.allocation(0, fees).unwrap();
        assert_eq!(allocation.miner, 350 * COIN);
        assert_eq!(allocation.steward, 125 * COIN);
        assert_eq!(allocation.community, 25 * COIN);
        assert_eq!(allocation.fees_burned, fees);

        let bad = CoinbaseClaim {
            miner: allocation.miner + fees,
            steward: allocation.steward,
            community: allocation.community,
        };
        assert_eq!(
            p.validate_coinbase(0, fees, bad),
            Err(EconomicsError::Miner {
                actual: 362 * COIN,
                expected: 350 * COIN,
            })
        );
    }

    #[test]
    fn tail_is_miner_only() {
        let p = DEFAULT_MONETARY_POLICY;
        let a = p.allocation(p.tail_height, COIN).unwrap();
        assert_eq!(a.miner, 5 * COIN);
        assert_eq!(a.steward, 0);
        assert_eq!(a.community, 0);
        assert_eq!(a.fees_burned, COIN);
    }
}
