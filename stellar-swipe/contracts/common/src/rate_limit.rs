//! Per-user rate limiting for key contract actions.
//!
//! Storage layout:
//!   RateLimitTimestamps(Address, ActionType) -> Vec<u64>  (sliding window, max 100 entries)
//!   RateLimitConfig(ActionType)              -> RateLimitConfig
//!   UserFirstAction(Address)                 -> u64  (timestamp of first recorded action)
//!
//! Tier multipliers:
//!   New user  (< 30 days): 1x
//!   Established (>= 30 days, trust_score >= 60): 2x

#![allow(dead_code)]

use crate::constants::{SECONDS_PER_DAY, SECONDS_PER_HOUR};
use soroban_sdk::{contracttype, symbol_short, Address, Env, Symbol, Vec};

// ── Constants ────────────────────────────────────────────────────────────────

const ESTABLISHED_DAYS: u64 = 30;
const ESTABLISHED_TRUST_SCORE: u32 = 60;
const MAX_STORED_TIMESTAMPS: u32 = 100;

// ── Types ────────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionType {
    SignalSubmission,
    TradeExecution,
    StakeChange,
    FollowAction,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct RateLimitConfig {
    /// Rolling window in seconds (e.g. 3600 for hourly, 86400 for daily)
    pub window_secs: u64,
    /// Maximum actions allowed within the window for a standard user
    pub max_actions: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct RateLimitWindow {
    pub window_start: u64,
    pub count: u32,
}

#[contracttype]
#[derive(Clone)]
pub enum RateLimitKey {
    Timestamps(Address, ActionType),
    Window(Address, ActionType),
    Config(ActionType),
    UserFirstAction(Address),
}

// ── Default configs ──────────────────────────────────────────────────────────

pub fn default_config(action: &ActionType) -> RateLimitConfig {
    match action {
        ActionType::SignalSubmission => RateLimitConfig {
            window_secs: SECONDS_PER_HOUR,
            max_actions: 10,
        },
        ActionType::TradeExecution => RateLimitConfig {
            window_secs: SECONDS_PER_HOUR,
            max_actions: 20,
        },
        ActionType::StakeChange => RateLimitConfig {
            window_secs: SECONDS_PER_DAY,
            max_actions: 5,
        },
        ActionType::FollowAction => RateLimitConfig {
            window_secs: SECONDS_PER_DAY,
            max_actions: 50,
        },
    }
}

// ── Storage helpers ──────────────────────────────────────────────────────────

fn get_config(env: &Env, action: &ActionType) -> RateLimitConfig {
    env.storage()
        .instance()
        .get(&RateLimitKey::Config(action.clone()))
        .unwrap_or_else(|| default_config(action))
}

pub fn set_config(env: &Env, action: ActionType, config: RateLimitConfig) {
    env.storage()
        .instance()
        .set(&RateLimitKey::Config(action), &config);
}

fn get_window(env: &Env, user: &Address, action: &ActionType) -> Option<RateLimitWindow> {
    env.storage()
        .persistent()
        .get(&RateLimitKey::Window(user.clone(), action.clone()))
}

fn save_window(env: &Env, user: &Address, action: &ActionType, window: &RateLimitWindow) {
    env.storage()
        .persistent()
        .set(&RateLimitKey::Window(user.clone(), action.clone()), window);
}

/// O(1) counter-based window count; migrates legacy timestamp vectors on first read.
fn current_window_count(
    env: &Env,
    user: &Address,
    action: &ActionType,
    config: &RateLimitConfig,
    now: u64,
) -> u32 {
    if let Some(window) = get_window(env, user, action) {
        if now.saturating_sub(window.window_start) >= config.window_secs {
            return 0;
        }
        return window.count;
    }

    // Legacy migration: count timestamps still inside the window once, then discard.
    let timestamps = get_timestamps(env, user, action);
    let count = timestamps
        .iter()
        .filter(|t| now.saturating_sub(*t) < config.window_secs)
        .count() as u32;
    if count > 0 {
        let window = RateLimitWindow {
            window_start: now,
            count,
        };
        save_window(env, user, action, &window);
    }
    count
}

fn get_timestamps(env: &Env, user: &Address, action: &ActionType) -> Vec<u64> {
    env.storage()
        .persistent()
        .get(&RateLimitKey::Timestamps(user.clone(), action.clone()))
        .unwrap_or_else(|| Vec::new(env))
}

fn save_timestamps(env: &Env, user: &Address, action: &ActionType, ts: &Vec<u64>) {
    env.storage()
        .persistent()
        .set(&RateLimitKey::Timestamps(user.clone(), action.clone()), ts);
}

fn get_first_action(env: &Env, user: &Address) -> u64 {
    env.storage()
        .persistent()
        .get(&RateLimitKey::UserFirstAction(user.clone()))
        .unwrap_or(0)
}

fn record_first_action_if_new(env: &Env, user: &Address, now: u64) {
    let key = RateLimitKey::UserFirstAction(user.clone());
    if env.storage().persistent().get::<_, u64>(&key).is_none() {
        env.storage().persistent().set(&key, &now);
    }
}

// ── Tier logic ───────────────────────────────────────────────────────────────

/// Returns the effective max_actions for this user based on their tier.
/// `trust_score`: pass the provider's trust score (0-100); use 0 if unknown.
fn effective_max(config: &RateLimitConfig, first_action: u64, now: u64, trust_score: u32) -> u32 {
    if first_action == 0 {
        return config.max_actions; // brand-new user, standard limits
    }
    let days = (now.saturating_sub(first_action)) / SECONDS_PER_DAY;
    if days >= ESTABLISHED_DAYS && trust_score >= ESTABLISHED_TRUST_SCORE {
        config.max_actions.saturating_mul(2)
    } else {
        config.max_actions
    }
}

// ── Core API ─────────────────────────────────────────────────────────────────

/// Check whether `user` may perform `action`.
/// Returns `Err(())` when the rate limit is exceeded and emits a `rate_limit_hit` event.
/// `trust_score`: caller should pass the user's current trust score (0-100).
pub fn check_rate_limit(
    env: &Env,
    user: &Address,
    action: ActionType,
    trust_score: u32,
) -> Result<(), ()> {
    let now = env.ledger().timestamp();
    let config = get_config(env, &action);
    let first_action = get_first_action(env, user);

    let max = effective_max(&config, first_action, now, trust_score);

    let recent_count = current_window_count(env, user, &action, &config, now);

    if recent_count >= max {
        emit_rate_limit_hit(env, user.clone(), action, recent_count, max);
        return Err(());
    }

    Ok(())
}

/// Record that `user` performed `action` right now.
/// Call this **after** a successful `check_rate_limit`.
pub fn record_action(env: &Env, user: &Address, action: ActionType) {
    let now = env.ledger().timestamp();
    record_first_action_if_new(env, user, now);

    let config = get_config(env, &action);

    let mut window = get_window(env, user, &action).unwrap_or(RateLimitWindow {
        window_start: now,
        count: 0,
    });

    if now.saturating_sub(window.window_start) >= config.window_secs {
        window.window_start = now;
        window.count = 0;
    }

    window.count = window.count.saturating_add(1);
    save_window(env, user, &action, &window);
}

// ── Event ────────────────────────────────────────────────────────────────────

fn emit_rate_limit_hit(env: &Env, user: Address, action: ActionType, count: u32, limit: u32) {
    let action_sym: Symbol = match action {
        ActionType::SignalSubmission => symbol_short!("sig_sub"),
        ActionType::TradeExecution => symbol_short!("trade"),
        ActionType::StakeChange => symbol_short!("stake"),
        ActionType::FollowAction => symbol_short!("follow"),
    };
    let topics = (Symbol::new(env, "rate_limit_hit"),);
    env.events()
        .publish(topics, (user, action_sym, count, limit));
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        contract, contractimpl,
        testutils::{Address as _, Ledger},
        Address, Env,
    };

    #[contract]
    struct RateLimitHarness;

    #[contractimpl]
    impl RateLimitHarness {}

    fn setup() -> (Env, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register(RateLimitHarness, ());
        let user = Address::generate(&env);
        (env, contract_id, user)
    }

    fn run<F>(env: &Env, contract_id: &Address, f: F)
    where
        F: FnOnce(),
    {
        env.as_contract(contract_id, f);
    }

    #[test]
    fn test_signal_submission_limit() {
        let (env, contract_id, user) = setup();
        run(&env, &contract_id, || {
            for _ in 0..10 {
                assert!(check_rate_limit(&env, &user, ActionType::SignalSubmission, 0).is_ok());
                record_action(&env, &user, ActionType::SignalSubmission);
            }
            assert!(check_rate_limit(&env, &user, ActionType::SignalSubmission, 0).is_err());
        });
    }

    #[test]
    fn test_window_reset_after_expiry() {
        let (env, contract_id, user) = setup();
        run(&env, &contract_id, || {
            for _ in 0..10 {
                check_rate_limit(&env, &user, ActionType::SignalSubmission, 0).unwrap();
                record_action(&env, &user, ActionType::SignalSubmission);
            }
            env.ledger()
                .set_timestamp(env.ledger().timestamp() + SECONDS_PER_HOUR + 1);
            assert!(check_rate_limit(&env, &user, ActionType::SignalSubmission, 0).is_ok());
        });
    }

    #[test]
    fn test_trade_execution_limit() {
        let (env, contract_id, user) = setup();
        run(&env, &contract_id, || {
            for _ in 0..20 {
                assert!(check_rate_limit(&env, &user, ActionType::TradeExecution, 0).is_ok());
                record_action(&env, &user, ActionType::TradeExecution);
            }
            assert!(check_rate_limit(&env, &user, ActionType::TradeExecution, 0).is_err());
        });
    }

    #[test]
    fn test_stake_change_daily_limit() {
        let (env, contract_id, user) = setup();
        run(&env, &contract_id, || {
            for _ in 0..5 {
                assert!(check_rate_limit(&env, &user, ActionType::StakeChange, 0).is_ok());
                record_action(&env, &user, ActionType::StakeChange);
            }
            assert!(check_rate_limit(&env, &user, ActionType::StakeChange, 0).is_err());
        });
    }

    #[test]
    fn test_follow_action_daily_limit() {
        let (env, contract_id, user) = setup();
        run(&env, &contract_id, || {
            for _ in 0..50 {
                assert!(check_rate_limit(&env, &user, ActionType::FollowAction, 0).is_ok());
                record_action(&env, &user, ActionType::FollowAction);
            }
            assert!(check_rate_limit(&env, &user, ActionType::FollowAction, 0).is_err());
        });
    }

    #[test]
    fn test_established_user_gets_2x_limit() {
        let (env, contract_id, user) = setup();
        run(&env, &contract_id, || {
            env.ledger().set_timestamp(100_000_000);
            let now = env.ledger().timestamp();
            let first = now.saturating_sub(31 * SECONDS_PER_DAY);
            env.storage()
                .persistent()
                .set(&RateLimitKey::UserFirstAction(user.clone()), &first);

            // Established user with trust_score >= 60 gets 20 signal submissions per hour
            for _ in 0..20 {
                assert!(check_rate_limit(&env, &user, ActionType::SignalSubmission, 60).is_ok());
                record_action(&env, &user, ActionType::SignalSubmission);
            }
            assert!(check_rate_limit(&env, &user, ActionType::SignalSubmission, 60).is_err());
        });
    }

    #[test]
    fn test_admin_config_update_applies_immediately() {
        let (env, contract_id, user) = setup();
        run(&env, &contract_id, || {
            set_config(
                &env,
                ActionType::SignalSubmission,
                RateLimitConfig {
                    window_secs: SECONDS_PER_HOUR,
                    max_actions: 3,
                },
            );
            for _ in 0..3 {
                assert!(check_rate_limit(&env, &user, ActionType::SignalSubmission, 0).is_ok());
                record_action(&env, &user, ActionType::SignalSubmission);
            }
            assert!(check_rate_limit(&env, &user, ActionType::SignalSubmission, 0).is_err());
        });
    }

    #[test]
    fn test_different_users_independent() {
        let (env, contract_id, user1) = setup();
        let user2 = Address::generate(&env);
        run(&env, &contract_id, || {
            for _ in 0..10 {
                check_rate_limit(&env, &user1, ActionType::SignalSubmission, 0).unwrap();
                record_action(&env, &user1, ActionType::SignalSubmission);
            }
            assert!(check_rate_limit(&env, &user2, ActionType::SignalSubmission, 0).is_ok());
        });
    }
}
