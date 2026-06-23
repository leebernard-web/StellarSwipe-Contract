#![no_std]

pub mod migration;

use migration::{MigrationKey, StakeInfoV2};
use soroban_sdk::{contract, contractimpl, contracttype, contracterror, token, Address, Env, Symbol};

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
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum StakeVaultError {
    NotInitialized    = 1,
    Unauthorized      = 2,
    NoStake           = 3,
    StakeLocked       = 4,
    ReentrancyDetected = 5,
    /// Provider stake is below minimum and grace period has expired.
    StakeBelowMinimum = 6,
    /// Contract is paused due to suspicious activity or admin action.
    ContractPaused    = 7,
    /// Large withdrawal requires a pending time-lock request first.
    TimelockRequired  = 8,
    /// Time-lock period has not yet elapsed.
    TimelockNotElapsed = 9,
    /// Stake and unstake in the same ledger — flash loan pattern detected.
    FlashLoanDetected = 10,
}

#[contract]
pub struct StakeVaultContract;

#[contractimpl]
impl StakeVaultContract {
    /// One-time initialization.
    pub fn initialize(env: Env, admin: Address, stake_token: Address, signal_registry: Address) {
        if env.storage().instance().has(&StorageKey::Admin) {
            panic!("already initialized");
        }
        env.storage().instance().set(&StorageKey::Admin, &admin);
        env.storage().instance().set(&StorageKey::StakeToken, &stake_token);
        env.storage().instance().set(&StorageKey::SignalRegistry, &signal_registry);
        env.storage().instance().set(&StorageKey::Paused, &false);
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
        env.events().publish(
            (Symbol::new(&env, "stake_vault"), Symbol::new(&env, "paused")),
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
        env.events().publish(
            (Symbol::new(&env, "stake_vault"), Symbol::new(&env, "unpaused")),
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
        token::Client::new(&env, &token).transfer(
            &staker,
            &env.current_contract_address(),
            &amount,
        );

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
        env.storage()
            .persistent()
            .set(&StorageKey::LargeWithdrawalRequestedAt(staker.clone()), &now);

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

        let info = stakes
            .get(staker.clone())
            .ok_or(StakeVaultError::NoStake)?;

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
        token::Client::new(env, &token).transfer(
            &env.current_contract_address(),
            staker,
            &amount,
        );

        Ok(amount)
    }

    // ── Slash ──────────────────────────────────────────────────────────────────

    pub fn slash_stake(
        env: Env,
        caller: Address,
        provider: Address,
        amount: i128,
        reason: Symbol,
    ) -> Result<(), StakeVaultError> {
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

        let mut stakes: soroban_sdk::Map<Address, StakeInfoV2> = env
            .storage()
            .persistent()
            .get(&MigrationKey::StakesV2)
            .unwrap_or_else(|| soroban_sdk::Map::new(&env));

        let mut info = stakes.get(provider.clone()).ok_or(StakeVaultError::NoStake)?;

        if amount <= 0 || amount > info.balance {
            return Err(StakeVaultError::NoStake);
        }

        info.balance = info
            .balance
            .checked_sub(amount)
            .ok_or(StakeVaultError::NoStake)?;
        info.last_updated = env.ledger().timestamp();
        stakes.set(provider.clone(), info);
        env.storage()
            .persistent()
            .set(&MigrationKey::StakesV2, &stakes);

        env.events().publish(
            (
                Symbol::new(&env, "stake_vault"),
                Symbol::new(&env, "stake_slashed"),
            ),
            (provider.clone(), amount, reason),
        );

        token::Client::new(&env, &token).burn(&env.current_contract_address(), &amount);

        Ok(())
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
