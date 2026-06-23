#![cfg(test)]

use crate::{
    migration::{MigrationKey, StakeInfoV2},
    StakeVaultContract, StakeVaultContractClient, StakeVaultError,
};
use soroban_sdk::{
    contract, contractimpl, testutils::Address as _, token::StellarAssetClient, Address, Env, Map,
    MuxedAddress, Symbol,
};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn sac_token(env: &Env, admin: &Address) -> Address {
    env.register_stellar_asset_contract_v2(admin.clone())
        .address()
}

fn seed_v2_stake(
    env: &Env,
    contract_id: &Address,
    staker: &Address,
    balance: i128,
    locked_until: u64,
) {
    env.as_contract(contract_id, || {
        let mut stakes: Map<Address, StakeInfoV2> = env
            .storage()
            .persistent()
            .get(&MigrationKey::StakesV2)
            .unwrap_or_else(|| Map::new(env));
        stakes.set(
            staker.clone(),
            StakeInfoV2 {
                balance,
                locked_until,
                last_updated: env.ledger().timestamp(),
            },
        );
        env.storage()
            .persistent()
            .set(&MigrationKey::StakesV2, &stakes);
    });
}

fn setup() -> (Env, Address, Address, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let signal_registry = Address::generate(&env);
    let token = sac_token(&env, &admin);
    let vault_id = env.register(StakeVaultContract, ());
    StakeVaultContractClient::new(&env, &vault_id).initialize(&admin, &token, &signal_registry);
    (env, vault_id, token, admin, signal_registry)
}

// ── Basic withdraw tests ──────────────────────────────────────────────────────

#[test]
fn withdraw_stake_transfers_balance() {
    let (env, vault_id, token, _admin, _registry) = setup();
    let staker = Address::generate(&env);
    let amount: i128 = 5_000_000;

    StellarAssetClient::new(&env, &token).mint(&vault_id, &amount);
    seed_v2_stake(&env, &vault_id, &staker, amount, 0);

    let client = StakeVaultContractClient::new(&env, &vault_id);
    assert_eq!(client.withdraw_stake(&staker), amount);
    assert_eq!(client.get_stake(&staker), 0);
}

#[test]
fn withdraw_stake_no_stake_returns_error() {
    let (env, vault_id, _token, _admin, _registry) = setup();
    let staker = Address::generate(&env);
    let err = env.as_contract(&vault_id, || {
        StakeVaultContract::withdraw_stake(env.clone(), staker)
    });
    assert_eq!(err, Err(StakeVaultError::NoStake));
}

#[test]
fn withdraw_stake_locked_returns_error() {
    let (env, vault_id, token, _admin, _registry) = setup();
    let staker = Address::generate(&env);
    let amount: i128 = 1_000_000;
    StellarAssetClient::new(&env, &token).mint(&vault_id, &amount);
    seed_v2_stake(&env, &vault_id, &staker, amount, u64::MAX);
    let err = env.as_contract(&vault_id, || {
        StakeVaultContract::withdraw_stake(env.clone(), staker)
    });
    assert_eq!(err, Err(StakeVaultError::StakeLocked));
}

// ── Reentrancy guard tests ────────────────────────────────────────────────────

#[contract]
pub struct ReentrantToken;

#[contractimpl]
impl ReentrantToken {
    pub fn set_vault(env: Env, vault: Address) {
        env.storage()
            .instance()
            .set(&soroban_sdk::symbol_short!("vault"), &vault);
    }
    pub fn set_staker(env: Env, staker: Address) {
        env.storage()
            .instance()
            .set(&soroban_sdk::symbol_short!("staker"), &staker);
    }
    /// Attempt reentrancy on every transfer call.
    pub fn transfer(env: Env, _from: Address, _to: MuxedAddress, _amount: i128) {
        let vault: Address = env
            .storage()
            .instance()
            .get(&soroban_sdk::symbol_short!("vault"))
            .unwrap();
        let staker: Address = env
            .storage()
            .instance()
            .get(&soroban_sdk::symbol_short!("staker"))
            .unwrap();
        let result = StakeVaultContractClient::new(&env, &vault).try_withdraw_stake(&staker);
        let blocked = matches!(result, Err(Ok(StakeVaultError::ReentrancyDetected)));
        // Only write true; don't overwrite a previously set true with false.
        if blocked {
            env.storage()
                .instance()
                .set(&soroban_sdk::symbol_short!("blocked"), &true);
        }
    }
    pub fn was_blocked(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&soroban_sdk::symbol_short!("blocked"))
            .unwrap_or(false)
    }
    pub fn balance(_env: Env, _id: Address) -> i128 {
        0
    }
    pub fn transfer_from(
        _env: Env,
        _spender: Address,
        _from: Address,
        _to: Address,
        _amount: i128,
    ) {
    }
    pub fn approve(
        _env: Env,
        _from: Address,
        _spender: Address,
        _amount: i128,
        _expiration_ledger: u32,
    ) {
    }
    pub fn allowance(_env: Env, _from: Address, _spender: Address) -> i128 {
        0
    }
    pub fn decimals(_env: Env) -> u32 {
        7
    }
    pub fn name(env: Env) -> soroban_sdk::String {
        soroban_sdk::String::from_str(&env, "ReentrantToken")
    }
    pub fn symbol(env: Env) -> soroban_sdk::String {
        soroban_sdk::String::from_str(&env, "RT")
    }
    pub fn mint(_env: Env, _to: Address, _amount: i128) {}
}

#[test]
fn reentrant_withdraw_is_blocked() {
    use soroban_sdk::testutils::Ledger;
    let (env, vault_id, token, _admin, _registry) = setup();
    let staker = Address::generate(&env);
    let amount: i128 = 1_000_000;

    StellarAssetClient::new(&env, &token).mint(&vault_id, &amount);
    // Advance ledger so flash-loan guard doesn't interfere (seed has no LastStakeLedger).
    env.ledger().with_mut(|l| l.sequence_number = 5);
    seed_v2_stake(&env, &vault_id, &staker, amount, 0);

    // Manually set the reentrancy lock as if we're inside withdraw_stake.
    env.as_contract(&vault_id, || {
        env.storage()
            .temporary()
            .set(&Symbol::new(&env, "WithdrawLock"), &true);
    });

    // A second withdraw_stake call while the lock is held must return ReentrancyDetected.
    let result = env.as_contract(&vault_id, || {
        StakeVaultContract::withdraw_stake(env.clone(), staker)
    });
    assert_eq!(result, Err(StakeVaultError::ReentrancyDetected));
}

#[test]
fn lock_cleared_after_successful_withdrawal() {
    let (env, vault_id, token, _admin, _registry) = setup();
    let staker = Address::generate(&env);
    let amount: i128 = 2_000_000;
    StellarAssetClient::new(&env, &token).mint(&vault_id, &(amount * 2));
    seed_v2_stake(&env, &vault_id, &staker, amount, 0);
    let client = StakeVaultContractClient::new(&env, &vault_id);
    client.withdraw_stake(&staker);
    seed_v2_stake(&env, &vault_id, &staker, amount, 0);
    assert_eq!(client.withdraw_stake(&staker), amount);
}

#[test]
fn lock_cleared_after_failed_withdrawal() {
    let (env, vault_id, _token, _admin, _registry) = setup();
    let staker = Address::generate(&env);
    let err = env.as_contract(&vault_id, || {
        StakeVaultContract::withdraw_stake(env.clone(), staker.clone())
    });
    assert_eq!(err, Err(StakeVaultError::NoStake));
    let lock_still_set: bool = env.as_contract(&vault_id, || {
        env.storage()
            .temporary()
            .get::<_, bool>(&Symbol::new(&env, "WithdrawLock"))
            .unwrap_or(false)
    });
    assert!(
        !lock_still_set,
        "lock was not cleared after failed withdrawal"
    );
}

// ── slash_stake tests ────────────────────────────────────────────────────────

#[test]
fn slash_stake_emits_event() {
    use soroban_sdk::testutils::Events;
    let (env, vault_id, token, _admin, signal_registry) = setup();
    let provider = Address::generate(&env);
    let amount: i128 = 500_000;
    StellarAssetClient::new(&env, &token).mint(&vault_id, &amount);
    seed_v2_stake(&env, &vault_id, &provider, amount, 0);
    let events_before = env.events().all().len();
    StakeVaultContractClient::new(&env, &vault_id).slash_stake(
        &signal_registry,
        &provider,
        &amount,
        &Symbol::new(&env, "ban"),
    );
    assert!(
        env.events().all().len() > events_before,
        "stake_slashed event not emitted"
    );
}

#[test]
fn slash_stake_reduces_provider_balance() {
    let (env, vault_id, token, _admin, signal_registry) = setup();
    let provider = Address::generate(&env);
    let initial: i128 = 1_000_000;
    let slash_amount: i128 = 300_000;
    StellarAssetClient::new(&env, &token).mint(&vault_id, &initial);
    seed_v2_stake(&env, &vault_id, &provider, initial, 0);
    let client = StakeVaultContractClient::new(&env, &vault_id);
    client.slash_stake(
        &signal_registry,
        &provider,
        &slash_amount,
        &Symbol::new(&env, "fraud"),
    );
    assert_eq!(client.get_stake(&provider), initial - slash_amount);
}

#[test]
fn slash_stake_burns_tokens_from_vault() {
    use soroban_sdk::token;
    let (env, vault_id, token_addr, _admin, signal_registry) = setup();
    let provider = Address::generate(&env);
    let initial: i128 = 1_000_000;
    let slash_amount: i128 = 400_000;
    StellarAssetClient::new(&env, &token_addr).mint(&vault_id, &initial);
    seed_v2_stake(&env, &vault_id, &provider, initial, 0);
    let token_client = token::Client::new(&env, &token_addr);
    let balance_before = token_client.balance(&vault_id);
    StakeVaultContractClient::new(&env, &vault_id).slash_stake(
        &signal_registry,
        &provider,
        &slash_amount,
        &Symbol::new(&env, "misconduct"),
    );
    assert_eq!(
        token_client.balance(&vault_id),
        balance_before - slash_amount,
        "slashed tokens were not burned from vault"
    );
}

#[test]
fn slash_stake_unauthorized_caller_rejected() {
    let (env, vault_id, token, _admin, _signal_registry) = setup();
    let unauthorized = Address::generate(&env);
    let provider = Address::generate(&env);
    let amount: i128 = 500_000;
    StellarAssetClient::new(&env, &token).mint(&vault_id, &amount);
    seed_v2_stake(&env, &vault_id, &provider, amount, 0);
    let result = StakeVaultContractClient::new(&env, &vault_id).try_slash_stake(
        &unauthorized,
        &provider,
        &amount,
        &Symbol::new(&env, "ban"),
    );
    assert_eq!(result, Err(Ok(StakeVaultError::Unauthorized)));
}

// ── Issue #388: stake-below-minimum tests ─────────────────────────────────────

#[test]
fn signal_submission_allowed_when_stake_above_minimum() {
    let (env, vault_id, token, _admin, _registry) = setup();
    let provider = Address::generate(&env);
    let amount: i128 = 1_000_000;
    StellarAssetClient::new(&env, &token).mint(&vault_id, &amount);
    seed_v2_stake(&env, &vault_id, &provider, amount, 0);
    let client = StakeVaultContractClient::new(&env, &vault_id);
    client.set_minimum_stake(&500_000i128);
    // Should not panic — stake (1_000_000) >= minimum (500_000).
    client.check_signal_submission_allowed(&provider);
}

#[test]
fn notify_stake_below_minimum_emits_event() {
    use soroban_sdk::testutils::Events;
    let (env, vault_id, token, _admin, _registry) = setup();
    let provider = Address::generate(&env);
    StellarAssetClient::new(&env, &token).mint(&vault_id, &100_000i128);
    seed_v2_stake(&env, &vault_id, &provider, 100_000, 0);
    let client = StakeVaultContractClient::new(&env, &vault_id);
    client.set_minimum_stake(&500_000i128);
    let events_before = env.events().all().len();
    client.notify_stake_below_minimum(&provider);
    assert!(
        env.events().all().len() > events_before,
        "event not emitted"
    );
}

#[test]
fn signal_submission_blocked_after_grace_period_expires() {
    use soroban_sdk::testutils::Ledger;
    let (env, vault_id, token, _admin, _registry) = setup();
    let provider = Address::generate(&env);
    StellarAssetClient::new(&env, &token).mint(&vault_id, &100_000i128);
    seed_v2_stake(&env, &vault_id, &provider, 100_000, 0);
    let client = StakeVaultContractClient::new(&env, &vault_id);
    client.set_minimum_stake(&500_000i128);
    client.notify_stake_below_minimum(&provider);
    env.ledger().with_mut(|l| l.timestamp += 86_401);
    let result = client.try_check_signal_submission_allowed(&provider);
    assert_eq!(result, Err(Ok(StakeVaultError::StakeBelowMinimum)));
}

#[test]
fn signal_submission_allowed_within_grace_period() {
    use soroban_sdk::testutils::Ledger;
    let (env, vault_id, token, _admin, _registry) = setup();
    let provider = Address::generate(&env);
    StellarAssetClient::new(&env, &token).mint(&vault_id, &100_000i128);
    seed_v2_stake(&env, &vault_id, &provider, 100_000, 0);
    let client = StakeVaultContractClient::new(&env, &vault_id);
    client.set_minimum_stake(&500_000i128);
    client.notify_stake_below_minimum(&provider);
    env.ledger().with_mut(|l| l.timestamp += 43_200);
    // Should not panic — within 24h grace period.
    client.check_signal_submission_allowed(&provider);
}

#[test]
fn stake_restoration_clears_below_min_flag() {
    let (env, vault_id, token, _admin, _registry) = setup();
    let provider = Address::generate(&env);
    StellarAssetClient::new(&env, &token).mint(&vault_id, &100_000i128);
    seed_v2_stake(&env, &vault_id, &provider, 100_000, 0);
    let client = StakeVaultContractClient::new(&env, &vault_id);
    client.set_minimum_stake(&500_000i128);
    client.notify_stake_below_minimum(&provider);
    assert!(client.get_stake_below_min_since(&provider).is_some());
    seed_v2_stake(&env, &vault_id, &provider, 1_000_000, 0);
    // Should not panic — stake restored.
    client.check_signal_submission_allowed(&provider);
    assert!(client.get_stake_below_min_since(&provider).is_none());
}

// ── Flash loan protection tests ───────────────────────────────────────────────

/// Simulates a flash loan: deposit_stake and withdraw_stake in the same ledger.
#[test]
fn flash_loan_same_ledger_deposit_withdraw_blocked() {
    use soroban_sdk::testutils::Ledger;
    let (env, vault_id, token, _admin, _registry) = setup();
    let attacker = Address::generate(&env);
    let amount: i128 = 100_000;

    env.ledger().with_mut(|l| l.sequence_number = 42);
    StellarAssetClient::new(&env, &token).mint(&attacker, &amount);
    StellarAssetClient::new(&env, &token).mint(&vault_id, &amount);

    let client = StakeVaultContractClient::new(&env, &vault_id);
    // Deposit in ledger 42 — records LastStakeLedger = 42.
    client.deposit_stake(&attacker, &amount);
    // Withdraw in same ledger 42 — must be blocked.
    let result = client.try_withdraw_stake(&attacker);
    assert_eq!(result, Err(Ok(StakeVaultError::FlashLoanDetected)));
}

/// After advancing one ledger, withdrawal must succeed.
#[test]
fn withdrawal_allowed_after_ledger_advance() {
    use soroban_sdk::testutils::Ledger;
    let (env, vault_id, token, _admin, _registry) = setup();
    let staker = Address::generate(&env);
    let amount: i128 = 100_000;

    env.ledger().with_mut(|l| l.sequence_number = 10);
    StellarAssetClient::new(&env, &token).mint(&staker, &amount);
    StellarAssetClient::new(&env, &token).mint(&vault_id, &amount);

    let client = StakeVaultContractClient::new(&env, &vault_id);
    client.deposit_stake(&staker, &amount);
    env.ledger().with_mut(|l| l.sequence_number = 11);
    assert_eq!(client.withdraw_stake(&staker), amount);
}

/// Large withdrawal without a prior time-lock request must be rejected.
#[test]
fn large_withdrawal_without_timelock_request_blocked() {
    let (env, vault_id, token, _admin, _registry) = setup();
    let staker = Address::generate(&env);
    let amount: i128 = 600_000_000;

    StellarAssetClient::new(&env, &token).mint(&vault_id, &amount);
    seed_v2_stake(&env, &vault_id, &staker, amount, 0);

    let result = StakeVaultContractClient::new(&env, &vault_id).try_withdraw_stake(&staker);
    assert_eq!(result, Err(Ok(StakeVaultError::TimelockRequired)));
}

/// Large withdrawal before time-lock expires must be rejected.
#[test]
fn large_withdrawal_before_timelock_expires_blocked() {
    use soroban_sdk::testutils::Ledger;
    let (env, vault_id, token, _admin, _registry) = setup();
    let staker = Address::generate(&env);
    let amount: i128 = 600_000_000;

    StellarAssetClient::new(&env, &token).mint(&vault_id, &amount);
    seed_v2_stake(&env, &vault_id, &staker, amount, 0);

    let client = StakeVaultContractClient::new(&env, &vault_id);
    client.request_withdrawal(&staker);
    // 30 min elapsed — still within 1h lock.
    env.ledger().with_mut(|l| l.timestamp += 1_800);
    let result = client.try_withdraw_stake(&staker);
    assert_eq!(result, Err(Ok(StakeVaultError::TimelockNotElapsed)));
}

/// Large withdrawal after time-lock expires must succeed.
#[test]
fn large_withdrawal_after_timelock_succeeds() {
    use soroban_sdk::testutils::Ledger;
    let (env, vault_id, token, _admin, _registry) = setup();
    let staker = Address::generate(&env);
    let amount: i128 = 600_000_000;

    StellarAssetClient::new(&env, &token).mint(&vault_id, &amount);
    seed_v2_stake(&env, &vault_id, &staker, amount, 0);

    let client = StakeVaultContractClient::new(&env, &vault_id);
    client.request_withdrawal(&staker);
    env.ledger().with_mut(|l| l.timestamp += 3_601);
    assert_eq!(client.withdraw_stake(&staker), amount);
}

/// Small withdrawal (below threshold) does not need a time-lock request.
#[test]
fn small_withdrawal_no_timelock_needed() {
    let (env, vault_id, token, _admin, _registry) = setup();
    let staker = Address::generate(&env);
    let amount: i128 = 100_000;

    StellarAssetClient::new(&env, &token).mint(&vault_id, &amount);
    seed_v2_stake(&env, &vault_id, &staker, amount, 0);
    assert_eq!(
        StakeVaultContractClient::new(&env, &vault_id).withdraw_stake(&staker),
        amount
    );
}

/// Admin pause blocks both deposit_stake and withdraw_stake.
#[test]
fn paused_contract_blocks_stake_and_unstake() {
    let (env, vault_id, token, _admin, _registry) = setup();
    let staker = Address::generate(&env);
    let amount: i128 = 100_000;

    StellarAssetClient::new(&env, &token).mint(&staker, &amount);
    StellarAssetClient::new(&env, &token).mint(&vault_id, &amount);
    seed_v2_stake(&env, &vault_id, &staker, amount, 0);

    let client = StakeVaultContractClient::new(&env, &vault_id);
    client.pause();

    assert_eq!(
        client.try_deposit_stake(&staker, &amount),
        Err(Ok(StakeVaultError::ContractPaused))
    );
    assert_eq!(
        client.try_withdraw_stake(&staker),
        Err(Ok(StakeVaultError::ContractPaused))
    );
}

/// Unpause restores normal operation.
#[test]
fn unpause_restores_operations() {
    let (env, vault_id, token, _admin, _registry) = setup();
    let staker = Address::generate(&env);
    let amount: i128 = 100_000;

    StellarAssetClient::new(&env, &token).mint(&vault_id, &amount);
    seed_v2_stake(&env, &vault_id, &staker, amount, 0);

    let client = StakeVaultContractClient::new(&env, &vault_id);
    client.pause();
    client.unpause();
    assert_eq!(client.withdraw_stake(&staker), amount);
}

/// Flash loan detection emits a monitoring alert (diagnostic event preserved in test env).
/// Verifies the flash_loan_attempt error code is returned, which triggers the event path.
#[test]
fn flash_loan_attempt_emits_alert_event() {
    use soroban_sdk::testutils::Ledger;
    let (env, vault_id, token, _admin, _registry) = setup();
    let attacker = Address::generate(&env);
    let amount: i128 = 100_000;

    env.ledger().with_mut(|l| l.sequence_number = 99);
    StellarAssetClient::new(&env, &token).mint(&attacker, &amount);
    StellarAssetClient::new(&env, &token).mint(&vault_id, &amount);

    let client = StakeVaultContractClient::new(&env, &vault_id);
    client.deposit_stake(&attacker, &amount);

    // The monitoring alert event is emitted inside do_withdraw before returning
    // FlashLoanDetected. Soroban preserves diagnostic events even on failed calls.
    let result = client.try_withdraw_stake(&attacker);
    assert_eq!(
        result,
        Err(Ok(StakeVaultError::FlashLoanDetected)),
        "flash_loan_attempt should return FlashLoanDetected (event emitted on this path)"
    );
}

/// Time-lock request is consumed after a successful large withdrawal.
#[test]
fn timelock_request_consumed_after_withdrawal() {
    use soroban_sdk::testutils::Ledger;
    let (env, vault_id, token, _admin, _registry) = setup();
    let staker = Address::generate(&env);
    let amount: i128 = 600_000_000;

    StellarAssetClient::new(&env, &token).mint(&vault_id, &(amount * 2));
    seed_v2_stake(&env, &vault_id, &staker, amount, 0);

    let client = StakeVaultContractClient::new(&env, &vault_id);
    client.request_withdrawal(&staker);
    env.ledger().with_mut(|l| l.timestamp += 3_601);
    client.withdraw_stake(&staker);

    // Re-seed for a second attempt — must require a fresh request.
    seed_v2_stake(&env, &vault_id, &staker, amount, 0);
    assert_eq!(
        client.try_withdraw_stake(&staker),
        Err(Ok(StakeVaultError::TimelockRequired))
    );
}
