#![no_std]

pub mod migration;

use migration::{MigrationKey, StakeInfoV2};
use shared::initializable;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, token, Address, Env, Symbol,
};

// ── Slash severity tiers ──────────────────────────────────────────────────────

/// Severity tier for a slashing event.
/// Controls what fraction of stake is burned.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum SlashSeverity {
    Minor = 0,
    Major = 1,
    Critical = 2,
}

/// On-chain configuration for how much stake each tier slashes.
/// Values are in basis points (10_000 = 100 %).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlashTierConfig {
    /// Basis points slashed for a Minor violation.
    pub minor_bps: u32,
    /// Basis points slashed for a Major violation.
    pub major_bps: u32,
    /// Basis points slashed for a Critical violation (typically 10_000 = full stake).
    pub critical_bps: u32,
}

impl SlashTierConfig {
    pub const fn default_config() -> Self {
        Self {
            minor_bps: 500,       // 5 %
            major_bps: 3_000,     // 30 %
            critical_bps: 10_000, // 100 %
        }
    }
}

const BPS_DENOMINATOR: i128 = 10_000;

/// Temporary-storage key for the reentrancy lock on `withdraw_stake`.
const EXECUTION_LOCK: &str = "WithdrawLock";

/// 24 hours in seconds — grace period for providers to top up stake.
const GRACE_PERIOD_SECS: u64 = 86_400;

/// 1 hour time-lock for large withdrawals (flash loan prevention).
const LARGE_WITHDRAWAL_TIMELOCK_SECS: u64 = 3_600;

/// Threshold above which a withdrawal is considered "large" and requires time-lock.
/// Set to SILVER tier (500M = 5 * 10^8 stroops).
const LARGE_WITHDRAWAL_THRESHOLD: i128 = 500_000_000;

pub const GOLD_TIER_STAKE: i128 = 1_000_000_000;
pub const SILVER_TIER_STAKE: i128 = GOLD_TIER_STAKE / 2;
pub const BRONZE_TIER_STAKE: i128 = GOLD_TIER_STAKE / 10;

fn stake_tier_for_amount(amount: i128) -> u32 {
    if amount >= GOLD_TIER_STAKE {
        3
    } else if amount >= SILVER_TIER_STAKE {
        2
    } else if amount >= BRONZE_TIER_STAKE {
        1
    } else {
        0
    }
}

fn emit_provider_tier_change(
    env: &Env,
    provider: &Address,
    old_tier: u32,
    new_tier: u32,
    stake_balance: i128,
) {
    if old_tier == new_tier {
        return;
    }

    let topic = if new_tier > old_tier {
        "provider_tier_upgraded"
    } else {
        "provider_tier_downgraded"
    };

    #[allow(deprecated)]
    env.events().publish(
        (Symbol::new(env, topic),),
        (provider.clone(), old_tier, new_tier, stake_balance),
    );
}

#[contracttype]
#[derive(Clone)]
pub enum StorageKey {
    Admin,
    StakeToken,
    SignalRegistry,
    /// Minimum stake required for a provider to submit signals.
    MinimumStake,
    /// Timestamp when a provider's stake first dropped below minimum.
    /// `None` means stake is currently at or above minimum.
    StakeBelowMinSince(Address),
    /// Emergency pause flag — when true all stake/unstake ops are blocked.
    Paused,
    /// Timestamp when a large-withdrawal request was initiated (per staker).
    LargeWithdrawalRequestedAt(Address),
    /// Ledger sequence at which a stake was last deposited (per staker).
    /// Used to detect same-ledger stake+unstake flash loan patterns.
    LastStakeLedger(Address),
    /// Admin-configurable slashing tier percentages.
    SlashTierConfig,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum StakeVaultError {
    NotInitialized = 1,
    Unauthorized = 2,
    NoStake = 3,
    StakeLocked = 4,
    ReentrancyDetected = 5,
    /// Provider stake is below minimum and grace period has expired.
    StakeBelowMinimum = 6,
    /// Contract is paused due to suspicious activity or admin action.
    ContractPaused = 7,
    /// Large withdrawal requires a pending time-lock request first.
    TimelockRequired = 8,
    /// Time-lock period has not yet elapsed.
    TimelockNotElapsed = 9,
    /// Stake and unstake in the same ledger — flash loan pattern detected.
    FlashLoanDetected = 10,
    /// Slash tier percentage would exceed 100% of stake.
    InvalidSlashTier = 11,
}

#[contract]
pub struct StakeVaultContract;

#[contractimpl]
impl StakeVaultContract {
    /// One-time initialization (uses shared::initializable guard, issue #584).
    pub fn initialize(env: Env, admin: Address, stake_token: Address, signal_registry: Address) {
        if initializable::is_initialized(&env) {
            panic!("already initialized");
        }
        env.storage().instance().set(&StorageKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&StorageKey::StakeToken, &stake_token);
        env.storage()
            .instance()
            .set(&StorageKey::SignalRegistry, &signal_registry);
        env.storage().instance().set(&StorageKey::Paused, &false);
        initializable::mark_initialized(&env);
    }

    // ── Emergency pause ────────────────────────────────────────────────────────

    /// Admin: pause all stake/unstake operations.
    pub fn pause(env: Env) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&StorageKey::Admin)
            .expect("not initialized");
        admin.require_auth();
        env.storage().instance().set(&StorageKey::Paused, &true);
        #[allow(deprecated)]
        env.events().publish(
            (
                Symbol::new(&env, "stake_vault"),
                Symbol::new(&env, "paused"),
            ),
            (),
        );
    }

    /// Admin: resume operations.
    pub fn unpause(env: Env) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&StorageKey::Admin)
            .expect("not initialized");
        admin.require_auth();
        env.storage().instance().set(&StorageKey::Paused, &false);
        #[allow(deprecated)]
        env.events().publish(
            (
                Symbol::new(&env, "stake_vault"),
                Symbol::new(&env, "unpaused"),
            ),
            (),
        );
    }

    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&StorageKey::Paused)
            .unwrap_or(false)
    }

    // ── Helpers ────────────────────────────────────────────────────────────────

    fn require_not_paused(env: &Env) -> Result<(), StakeVaultError> {
        if env
            .storage()
            .instance()
            .get::<_, bool>(&StorageKey::Paused)
            .unwrap_or(false)
        {
            return Err(StakeVaultError::ContractPaused);
        }
        Ok(())
    }

    // ── Deposit stake (records ledger for flash-loan detection) ────────────────

    /// Deposit `amount` of stake tokens from `staker`.
    ///
    /// Records the current ledger sequence to detect same-ledger withdraw
    /// attempts (flash loan pattern).
    pub fn deposit_stake(env: Env, staker: Address, amount: i128) -> Result<(), StakeVaultError> {
        staker.require_auth();
        Self::require_not_paused(&env)?;

        if amount <= 0 {
            return Err(StakeVaultError::NoStake);
        }

        let token: Address = env
            .storage()
            .instance()
            .get(&StorageKey::StakeToken)
            .ok_or(StakeVaultError::NotInitialized)?;

        let mut stakes: soroban_sdk::Map<Address, StakeInfoV2> = env
            .storage()
            .persistent()
            .get(&MigrationKey::StakesV2)
            .unwrap_or_else(|| soroban_sdk::Map::new(&env));

        let now = env.ledger().timestamp();
        let current = stakes.get(staker.clone()).unwrap_or(StakeInfoV2 {
            balance: 0,
            locked_until: 0,
            last_updated: 0,
        });

        let old_tier = stake_tier_for_amount(current.balance);
        let new_balance = current.balance.checked_add(amount).unwrap_or(i128::MAX);
        let new_tier = stake_tier_for_amount(new_balance);

        stakes.set(
            staker.clone(),
            StakeInfoV2 {
                balance: new_balance,
                locked_until: current.locked_until,
                last_updated: now,
            },
        );
        env.storage()
            .persistent()
            .set(&MigrationKey::StakesV2, &stakes);

        // Record the ledger sequence at deposit time for flash-loan detection.
        let ledger_seq = env.ledger().sequence();
        env.storage()
            .temporary()
            .set(&StorageKey::LastStakeLedger(staker.clone()), &ledger_seq);

        emit_provider_tier_change(&env, &staker, old_tier, new_tier, new_balance);

        // Transfer tokens into the vault (after state update — CEI pattern).
        token::Client::new(&env, &token).transfer(&staker, env.current_contract_address(), &amount);

        Ok(())
    }

    // ── Minimum stake ──────────────────────────────────────────────────────────

    pub fn set_minimum_stake(env: Env, minimum: i128) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&StorageKey::Admin)
            .expect("not initialized");
        admin.require_auth();
        env.storage()
            .instance()
            .set(&StorageKey::MinimumStake, &minimum);
    }

    pub fn get_minimum_stake(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&StorageKey::MinimumStake)
            .unwrap_or(0)
    }

    pub fn notify_stake_below_minimum(env: Env, provider: Address) {
        let minimum: i128 = env
            .storage()
            .instance()
            .get(&StorageKey::MinimumStake)
            .unwrap_or(0);

        let current_stake = Self::get_stake(env.clone(), provider.clone());

        if current_stake >= minimum {
            return;
        }

        let key = StorageKey::StakeBelowMinSince(provider.clone());
        if !env.storage().persistent().has(&key) {
            let now = env.ledger().timestamp();
            env.storage().persistent().set(&key, &now);

            #[allow(deprecated)]
            env.events().publish(
                (
                    Symbol::new(&env, "stake_vault"),
                    Symbol::new(&env, "stake_below_min"),
                ),
                (provider, current_stake, minimum),
            );
        }
    }

    pub fn check_signal_submission_allowed(
        env: Env,
        provider: Address,
    ) -> Result<(), StakeVaultError> {
        let minimum: i128 = env
            .storage()
            .instance()
            .get(&StorageKey::MinimumStake)
            .unwrap_or(0);

        let current_stake = Self::get_stake(env.clone(), provider.clone());

        if current_stake >= minimum {
            let key = StorageKey::StakeBelowMinSince(provider);
            env.storage().persistent().remove(&key);
            return Ok(());
        }

        let key = StorageKey::StakeBelowMinSince(provider.clone());
        let below_since: u64 = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| env.ledger().timestamp());

        let now = env.ledger().timestamp();
        if now.saturating_sub(below_since) > GRACE_PERIOD_SECS {
            Err(StakeVaultError::StakeBelowMinimum)
        } else {
            Ok(())
        }
    }

    pub fn get_stake_below_min_since(env: Env, provider: Address) -> Option<u64> {
        env.storage()
            .persistent()
            .get(&StorageKey::StakeBelowMinSince(provider))
    }

    // ── Large-withdrawal time-lock ─────────────────────────────────────────────

    /// Initiate a time-locked withdrawal request for a large stake.
    ///
    /// After calling this, the staker must wait `LARGE_WITHDRAWAL_TIMELOCK_SECS`
    /// before `withdraw_stake` will succeed for amounts >= `LARGE_WITHDRAWAL_THRESHOLD`.
    pub fn request_withdrawal(env: Env, staker: Address) -> Result<(), StakeVaultError> {
        staker.require_auth();
        Self::require_not_paused(&env)?;

        let balance = Self::get_stake(env.clone(), staker.clone());
        if balance < LARGE_WITHDRAWAL_THRESHOLD {
            // Small withdrawals don't need a time-lock request.
            return Ok(());
        }

        let now = env.ledger().timestamp();
        env.storage().persistent().set(
            &StorageKey::LargeWithdrawalRequestedAt(staker.clone()),
            &now,
        );

        #[allow(deprecated)]
        env.events().publish(
            (
                Symbol::new(&env, "stake_vault"),
                Symbol::new(&env, "withdrawal_requested"),
            ),
            (staker, balance, now + LARGE_WITHDRAWAL_TIMELOCK_SECS),
        );

        Ok(())
    }

    pub fn get_withdrawal_unlock_time(env: Env, staker: Address) -> Option<u64> {
        env.storage()
            .persistent()
            .get::<_, u64>(&StorageKey::LargeWithdrawalRequestedAt(staker))
            .map(|t| t + LARGE_WITHDRAWAL_TIMELOCK_SECS)
    }

    // ── Withdraw ───────────────────────────────────────────────────────────────

    /// Withdraw all unlocked stake for `staker`.
    ///
    /// Flash loan protections applied:
    /// 1. Reentrancy guard (temporary storage lock).
    /// 2. Same-ledger deposit+withdraw detection.
    /// 3. Time-lock for large withdrawals (>= LARGE_WITHDRAWAL_THRESHOLD).
    pub fn withdraw_stake(env: Env, staker: Address) -> Result<i128, StakeVaultError> {
        staker.require_auth();
        Self::require_not_paused(&env)?;

        // ── Reentrancy guard ──────────────────────────────────────────────────
        let lock_key = Symbol::new(&env, EXECUTION_LOCK);
        if env
            .storage()
            .temporary()
            .get::<_, bool>(&lock_key)
            .unwrap_or(false)
        {
            return Err(StakeVaultError::ReentrancyDetected);
        }
        env.storage().temporary().set(&lock_key, &true);

        let result = Self::do_withdraw(&env, &staker);

        env.storage().temporary().remove(&lock_key);
        result
    }

    fn do_withdraw(env: &Env, staker: &Address) -> Result<i128, StakeVaultError> {
        let token: Address = env
            .storage()
            .instance()
            .get(&StorageKey::StakeToken)
            .ok_or(StakeVaultError::NotInitialized)?;

        let mut stakes: soroban_sdk::Map<Address, StakeInfoV2> = env
            .storage()
            .persistent()
            .get(&MigrationKey::StakesV2)
            .unwrap_or_else(|| soroban_sdk::Map::new(env));

        let info = stakes.get(staker.clone()).ok_or(StakeVaultError::NoStake)?;

        if info.balance == 0 {
            return Err(StakeVaultError::NoStake);
        }

        let now = env.ledger().timestamp();
        if now < info.locked_until {
            return Err(StakeVaultError::StakeLocked);
        }

        // ── Flash loan detection: same-ledger stake + unstake ─────────────────
        let current_ledger = env.ledger().sequence();
        let last_stake_ledger = env
            .storage()
            .temporary()
            .get::<_, u32>(&StorageKey::LastStakeLedger(staker.clone()))
            .unwrap_or(0);
        if last_stake_ledger == current_ledger && current_ledger != 0 {
            // Emit alert event for monitoring system.
            #[allow(deprecated)]
            env.events().publish(
                (
                    Symbol::new(env, "stake_vault"),
                    Symbol::new(env, "flash_loan_attempt"),
                ),
                (staker.clone(), info.balance, current_ledger),
            );
            return Err(StakeVaultError::FlashLoanDetected);
        }

        // ── Time-lock for large withdrawals ───────────────────────────────────
        if info.balance >= LARGE_WITHDRAWAL_THRESHOLD {
            let requested_at: u64 = env
                .storage()
                .persistent()
                .get(&StorageKey::LargeWithdrawalRequestedAt(staker.clone()))
                .ok_or(StakeVaultError::TimelockRequired)?;

            if now < requested_at.saturating_add(LARGE_WITHDRAWAL_TIMELOCK_SECS) {
                return Err(StakeVaultError::TimelockNotElapsed);
            }

            // Consume the request so it can't be reused.
            env.storage()
                .persistent()
                .remove(&StorageKey::LargeWithdrawalRequestedAt(staker.clone()));
        }

        let amount = info.balance;
        let old_tier = stake_tier_for_amount(info.balance);
        let new_tier = stake_tier_for_amount(0);

        // Zero balance before transfer (checks-effects-interactions).
        stakes.set(
            staker.clone(),
            StakeInfoV2 {
                balance: 0,
                locked_until: info.locked_until,
                last_updated: now,
            },
        );
        env.storage()
            .persistent()
            .set(&MigrationKey::StakesV2, &stakes);

        emit_provider_tier_change(env, staker, old_tier, new_tier, 0);

        // Cross-contract call: transfer tokens back to staker.
        token::Client::new(env, &token).transfer(&env.current_contract_address(), staker, &amount);

        Ok(amount)
    }

    // ── Slash ──────────────────────────────────────────────────────────────────

    /// Admin: configure the slash percentage for each severity tier (in basis points).
    /// `minor_bps`, `major_bps`, `critical_bps` must all be <= 10_000.
    pub fn configure_slash_tiers(
        env: Env,
        minor_bps: u32,
        major_bps: u32,
        critical_bps: u32,
    ) -> Result<(), StakeVaultError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&StorageKey::Admin)
            .ok_or(StakeVaultError::NotInitialized)?;
        admin.require_auth();
        if minor_bps > 10_000 || major_bps > 10_000 || critical_bps > 10_000 {
            return Err(StakeVaultError::InvalidSlashTier);
        }
        let cfg = SlashTierConfig {
            minor_bps,
            major_bps,
            critical_bps,
        };
        env.storage()
            .instance()
            .set(&StorageKey::SlashTierConfig, &cfg);
        #[allow(deprecated)]
        env.events().publish(
            (
                Symbol::new(&env, "stake_vault"),
                Symbol::new(&env, "slash_tiers_updated"),
            ),
            (minor_bps, major_bps, critical_bps),
        );
        Ok(())
    }

    pub fn get_slash_tier_config(env: Env) -> SlashTierConfig {
        env.storage()
            .instance()
            .get(&StorageKey::SlashTierConfig)
            .unwrap_or_else(SlashTierConfig::default_config)
    }

    /// Slash `provider`'s stake according to `severity`.
    ///
    /// The slashed amount is computed from the configured tier percentages
    /// (default: minor=5%, major=30%, critical=100%).  Only the signal registry
    /// may call this.
    pub fn slash_stake(
        env: Env,
        caller: Address,
        provider: Address,
        severity: SlashSeverity,
        reason: Symbol,
    ) -> Result<i128, StakeVaultError> {
        caller.require_auth();
        let signal_registry: Address = env
            .storage()
            .instance()
            .get(&StorageKey::SignalRegistry)
            .ok_or(StakeVaultError::NotInitialized)?;
        if caller != signal_registry {
            return Err(StakeVaultError::Unauthorized);
        }

        let token: Address = env
            .storage()
            .instance()
            .get(&StorageKey::StakeToken)
            .ok_or(StakeVaultError::NotInitialized)?;

        let cfg: SlashTierConfig = env
            .storage()
            .instance()
            .get(&StorageKey::SlashTierConfig)
            .unwrap_or_else(SlashTierConfig::default_config);

        let tier_bps = match severity {
            SlashSeverity::Minor => cfg.minor_bps as i128,
            SlashSeverity::Major => cfg.major_bps as i128,
            SlashSeverity::Critical => cfg.critical_bps as i128,
        };

        let mut stakes: soroban_sdk::Map<Address, StakeInfoV2> = env
            .storage()
            .persistent()
            .get(&MigrationKey::StakesV2)
            .unwrap_or_else(|| soroban_sdk::Map::new(&env));

        let mut info = stakes
            .get(provider.clone())
            .ok_or(StakeVaultError::NoStake)?;

        if info.balance == 0 {
            return Err(StakeVaultError::NoStake);
        }

        // Compute slash amount from tier percentage; min 1 stroop.
        let slash_amount = core::cmp::max((info.balance * tier_bps) / BPS_DENOMINATOR, 1);
        let slash_amount = core::cmp::min(slash_amount, info.balance);

        info.balance = info.balance.saturating_sub(slash_amount);
        info.last_updated = env.ledger().timestamp();
        stakes.set(provider.clone(), info);
        env.storage()
            .persistent()
            .set(&MigrationKey::StakesV2, &stakes);

        // Event records severity tier and resulting slash amount for audit.
        #[allow(deprecated)]
        env.events().publish(
            (
                Symbol::new(&env, "stake_vault"),
                Symbol::new(&env, "stake_slashed"),
            ),
            (provider.clone(), severity as u32, slash_amount, reason),
        );

        token::Client::new(&env, &token).burn(&env.current_contract_address(), &slash_amount);

        Ok(slash_amount)
    }

    // ── Read ───────────────────────────────────────────────────────────────────

    pub fn get_stake(env: Env, staker: Address) -> i128 {
        let stakes: soroban_sdk::Map<Address, StakeInfoV2> = env
            .storage()
            .persistent()
            .get(&MigrationKey::StakesV2)
            .unwrap_or_else(|| soroban_sdk::Map::new(&env));
        stakes.get(staker).map(|s| s.balance).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests;
