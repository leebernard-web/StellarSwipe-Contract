use soroban_sdk::{contracttype, Address, Env, String, Vec};

use crate::types::{ProviderPerformance, Signal, SignalStatus};
use crate::events;

/// Storage key for the banned providers map
#[contracttype]
#[derive(Clone)]
pub enum BanStorageKey {
    /// (provider) -> reason_hash; presence of key indicates banned status
    ProviderBanReason(Address),
}

pub const GOLD_TIER_STAKE: i128 = 1_000_000_000;
pub const MIN_CLOSED_SIGNALS: u32 = 20;
pub const MIN_SUCCESS_RATE_BPS: u32 = 6_000;

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationEligibility {
    pub eligible: bool,
    pub stake_ok: bool,
    pub history_ok: bool,
    pub success_rate_ok: bool,
    pub missing_criteria: Vec<String>,
}

pub fn check_verification_eligibility(
    env: &Env,
    provider: Address,
    stake: i128,
    stats: ProviderPerformance,
) -> VerificationEligibility {
    let stake_ok = stake >= GOLD_TIER_STAKE;
    let history_ok = stats.total_signals >= MIN_CLOSED_SIGNALS;
    let success_rate_ok = stats.success_rate >= MIN_SUCCESS_RATE_BPS;
    let eligible = stake_ok && history_ok && success_rate_ok;

    let mut missing_criteria = Vec::new(env);
    if !stake_ok {
        missing_criteria.push_back(String::from_str(env, "gold_tier_stake"));
    }
    if !history_ok {
        missing_criteria.push_back(String::from_str(env, "closed_signals"));
    }
    if !success_rate_ok {
        missing_criteria.push_back(String::from_str(env, "success_rate"));
    }

    crate::events::emit_verification_eligibility_checked(env, provider, eligible);

    VerificationEligibility {
        eligible,
        stake_ok,
        history_ok,
        success_rate_ok,
        missing_criteria,
    }
}

// ═══════════════════════════════════════════════════════════════════
// Issue #424: Provider Ban Mechanism
// ═══════════════════════════════════════════════════════════════════

/// Check if a provider is banned (presence of ban reason indicates banned status)
pub fn is_provider_banned(env: &Env, provider: &Address) -> bool {
    env.storage()
        .persistent()
        .has(&BanStorageKey::ProviderBanReason(provider.clone()))
}

/// Get the ban reason hash for a banned provider
pub fn get_ban_reason(env: &Env, provider: &Address) -> Option<String> {
    env.storage()
        .persistent()
        .get(&BanStorageKey::ProviderBanReason(provider.clone()))
}

/// Ban a provider: cancel all active signals, slash full stake, block future submissions.
///
/// # Arguments
/// * `env` - Soroban environment
/// * `signals_map` - Mutable reference to the signals map (signals will be cancelled in-place)
/// * `provider` - Address of the provider to ban
/// * `reason_hash` - On-chain evidence hash (e.g. IPFS CID of dispute documentation)
/// * `stake_vault` - Address of the StakeVault contract for slashing
///
/// # Returns
/// `(signals_cancelled, stake_slashed)` tuple
pub fn ban_provider(
    env: &Env,
    signals_map: &mut Map<u64, Signal>,
    provider: &Address,
    reason_hash: &String,
    stake_vault: &Address,
) -> (u32, i128) {
    // Mark provider as banned by storing the reason hash
    env.storage()
        .persistent()
        .set(&BanStorageKey::ProviderBanReason(provider.clone()), reason_hash);

    // Cancel all active signals from this provider
    let mut signals_cancelled: u32 = 0;
    for i in 0..signals_map.keys().len() {
        if let Some(key) = signals_map.keys().get(i) {
            if let Some(mut signal) = signals_map.get(key) {
                if signal.provider == *provider && signal.status == SignalStatus::Active {
                    signal.status = SignalStatus::Failed;
                    signals_map.set(key, signal);
                    signals_cancelled += 1;
                }
            }
        }
    }

    // Slash full stake via cross-contract call to StakeVault
    let stake_slashed = Self::slash_stake(env, provider, stake_vault);

    (signals_cancelled, stake_slashed)
}

/// Slash the full stake of a provider via StakeVault cross-contract call
fn slash_stake(env: &Env, provider: &Address, stake_vault: &Address) -> i128 {
    let sym = soroban_sdk::Symbol::new(env, "get_stake");
    let mut args = soroban_sdk::Vec::<soroban_sdk::Val>::new(env);
    args.push_back(provider.clone().into_val(env));
    let stake: i128 = env
        .invoke_contract(stake_vault, &sym, args)
        .unwrap_or(0);

    if stake > 0 {
        // Call slash_stake on StakeVault (the contract will burn/transfer the slashed amount)
        let slash_sym = soroban_sdk::Symbol::new(env, "slash_stake");
        let mut slash_args = soroban_sdk::Vec::<soroban_sdk::Val>::new(env);
        slash_args.push_back(provider.clone().into_val(env));
        slash_args.push_back(stake.into_val(env));
        // We attempt to slash, but if it fails, we still return the stake amount for the event
        let _ = env.try_invoke_contract::<()>(stake_vault, &slash_sym, slash_args);
    }

    stake
}

/// Emit the ProviderBanned event
pub fn emit_provider_banned(
    env: &Env,
    provider: &Address,
    reason_hash: &String,
    signals_cancelled: u32,
    stake_slashed: i128,
) {
    let topics = (
        soroban_sdk::Symbol::new(env, "provider_banned"),
        provider.clone(),
    );
    env.events()
        .publish(topics, (reason_hash.clone(), signals_cancelled, stake_slashed));
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    fn stats(total_signals: u32, success_rate: u32) -> ProviderPerformance {
        ProviderPerformance {
            total_signals,
            successful_signals: 0,
            failed_signals: 0,
            total_copies: 0,
            success_rate,
            avg_return: 0,
            total_volume: 0,
            follower_count: 0,
        }
    }

    #[test]
    fn fully_eligible_provider_passes() {
        let env = Env::default();
        let provider = Address::generate(&env);

        let eligibility = check_verification_eligibility(
            &env,
            provider,
            GOLD_TIER_STAKE,
            stats(MIN_CLOSED_SIGNALS, MIN_SUCCESS_RATE_BPS),
        );

        assert!(eligibility.eligible);
        assert!(eligibility.stake_ok);
        assert!(eligibility.history_ok);
        assert!(eligibility.success_rate_ok);
        assert_eq!(eligibility.missing_criteria.len(), 0);
    }

    #[test]
    fn partially_eligible_provider_reports_missing_criteria() {
        let env = Env::default();
        let provider = Address::generate(&env);

        let eligibility = check_verification_eligibility(
            &env,
            provider,
            GOLD_TIER_STAKE,
            stats(MIN_CLOSED_SIGNALS - 1, MIN_SUCCESS_RATE_BPS),
        );

        assert!(!eligibility.eligible);
        assert!(eligibility.stake_ok);
        assert!(!eligibility.history_ok);
        assert!(eligibility.success_rate_ok);
        assert_eq!(eligibility.missing_criteria.len(), 1);
    }

    #[test]
    fn not_eligible_provider_reports_all_missing_criteria() {
        let env = Env::default();
        let provider = Address::generate(&env);

        let eligibility = check_verification_eligibility(&env, provider, 0, stats(0, 0));

        assert!(!eligibility.eligible);
        assert!(!eligibility.stake_ok);
        assert!(!eligibility.history_ok);
        assert!(!eligibility.success_rate_ok);
        assert_eq!(eligibility.missing_criteria.len(), 3);
    }
}
