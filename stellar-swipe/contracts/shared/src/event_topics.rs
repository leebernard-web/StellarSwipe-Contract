/// Canonical event-topic constants (issue #585).
///
/// All `env.events().publish()` call sites must use these named constants as
/// their topic arguments instead of inline string/symbol literals. A CI check
/// (`scripts/check_event_topics.sh`) scans source for ad hoc topic strings and
/// fails if any are found outside this module.
///
/// # Adding a new topic
/// 1. Define a new `pub const` here following the naming convention.
/// 2. Update any call sites to reference the constant.
/// 3. Document the event in `docs/events.md`.
use soroban_sdk::{symbol_short, Symbol};

// ── Namespace topics ──────────────────────────────────────────────────────────

/// Top-level namespace for governance contract events.
pub const TOPIC_GOVERNANCE: fn() -> Symbol = || symbol_short!("gov");

/// Top-level namespace for stake vault events.
pub const TOPIC_STAKE_VAULT: fn() -> Symbol = || Symbol::short("stkvault");

/// Top-level namespace for fee collector events.
pub const TOPIC_FEE: fn() -> Symbol = || symbol_short!("fee");

/// Top-level namespace for signal registry events.
pub const TOPIC_SIGNAL: fn() -> Symbol = || symbol_short!("signal");

// ── Governance sub-topics ─────────────────────────────────────────────────────

pub const TOPIC_GOV_INIT: fn() -> Symbol = || symbol_short!("init");
pub const TOPIC_GOV_DIST: fn() -> Symbol = || symbol_short!("dist");
pub const TOPIC_GOV_VESTING_ADD: fn() -> Symbol = || symbol_short!("vestadd");
pub const TOPIC_GOV_PROP_NEW: fn() -> Symbol = || symbol_short!("propnew");
pub const TOPIC_GOV_ACCRUE: fn() -> Symbol = || symbol_short!("accrue");
pub const TOPIC_GOV_ADMIN: fn() -> Symbol = || symbol_short!("admin");
pub const TOPIC_GOV_UPGRADE: fn() -> Symbol = || symbol_short!("upgrade");
pub const TOPIC_GOV_UPGRADE_ANNOUNCED: fn() -> Symbol = || symbol_short!("announced");

// ── Shadow-mode sub-topics ────────────────────────────────────────────────────

pub const TOPIC_SHADOW: fn() -> Symbol = || symbol_short!("shadow");
pub const TOPIC_SHADOW_ENTER: fn() -> Symbol = || symbol_short!("enter");
pub const TOPIC_SHADOW_PROMOTE: fn() -> Symbol = || symbol_short!("promote");
pub const TOPIC_SHADOW_CANCEL: fn() -> Symbol = || symbol_short!("cancel");
pub const TOPIC_SHADOW_DISCREP: fn() -> Symbol = || symbol_short!("discrep");

// ── Deposit sub-topics ────────────────────────────────────────────────────────

pub const TOPIC_DEPOSIT: fn() -> Symbol = || symbol_short!("deposit");
pub const TOPIC_DEPOSIT_LOCKED: fn() -> Symbol = || symbol_short!("locked");
pub const TOPIC_DEPOSIT_REFUNDED: fn() -> Symbol = || symbol_short!("refunded");
pub const TOPIC_DEPOSIT_FORFEITED: fn() -> Symbol = || symbol_short!("forfeit");

// ── Stake vault sub-topics ────────────────────────────────────────────────────

pub const TOPIC_STAKE_PAUSED: fn() -> Symbol = || symbol_short!("paused");
pub const TOPIC_STAKE_UNPAUSED: fn() -> Symbol = || symbol_short!("unpaused");
pub const TOPIC_STAKE_BELOW_MIN: fn() -> Symbol = || symbol_short!("blwmin");

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn topic_namespace_governance_matches_expected_symbol() {
        let gov = TOPIC_GOVERNANCE();
        let expected = symbol_short!("gov");
        assert_eq!(gov, expected);
    }

    #[test]
    fn topic_shadow_discrep_matches_expected_symbol() {
        let discrep = TOPIC_SHADOW_DISCREP();
        let expected = symbol_short!("discrep");
        assert_eq!(discrep, expected);
    }

    #[test]
    fn topic_gov_prop_new_matches_expected_symbol() {
        let t = TOPIC_GOV_PROP_NEW();
        let expected = symbol_short!("propnew");
        assert_eq!(t, expected);
    }

    #[test]
    fn all_topic_fns_produce_values_without_panic() {
        let _ = TOPIC_GOVERNANCE();
        let _ = TOPIC_STAKE_VAULT();
        let _ = TOPIC_FEE();
        let _ = TOPIC_SIGNAL();
        let _ = TOPIC_GOV_INIT();
        let _ = TOPIC_GOV_DIST();
        let _ = TOPIC_GOV_VESTING_ADD();
        let _ = TOPIC_GOV_PROP_NEW();
        let _ = TOPIC_GOV_ACCRUE();
        let _ = TOPIC_GOV_ADMIN();
        let _ = TOPIC_GOV_UPGRADE();
        let _ = TOPIC_GOV_UPGRADE_ANNOUNCED();
        let _ = TOPIC_SHADOW();
        let _ = TOPIC_SHADOW_ENTER();
        let _ = TOPIC_SHADOW_PROMOTE();
        let _ = TOPIC_SHADOW_CANCEL();
        let _ = TOPIC_SHADOW_DISCREP();
        let _ = TOPIC_DEPOSIT();
        let _ = TOPIC_DEPOSIT_LOCKED();
        let _ = TOPIC_DEPOSIT_REFUNDED();
        let _ = TOPIC_DEPOSIT_FORFEITED();
        let _ = TOPIC_STAKE_PAUSED();
        let _ = TOPIC_STAKE_UNPAUSED();
        let _ = TOPIC_STAKE_BELOW_MIN();
    }
}
