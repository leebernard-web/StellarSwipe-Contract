#![no_std]

pub mod assets;
pub mod commit_reveal;
pub mod constants;
pub mod emergency;
pub mod health;
pub mod multisig;
pub mod oracle;
pub mod perf;
pub mod rate_limit;
pub mod replay_protection;

pub use assets::{validate_asset_pair, Asset, AssetPair, AssetPairError};
pub use commit_reveal::hash_trade_intent;
pub use constants::{
    BASIS_POINTS_DENOMINATOR, BASIS_POINTS_DENOMINATOR_I128, CAT_ALL, CAT_SIGNALS, CAT_STAKES,
    CAT_TRADING, LEDGERS_PER_30_DAY_MONTH, LEDGERS_PER_DAY, PLACEHOLDER_ADMIN_STR,
    SECONDS_PER_30_DAY_MONTH, SECONDS_PER_DAY, SECONDS_PER_HOUR, SECONDS_PER_WEEK,
    STELLAR_AMOUNT_SCALE,
};
pub use emergency::PauseState;
pub use health::{health_uninitialized, placeholder_admin, HealthStatus};
pub use perf::{
    mark_operation, op_batch_execute, op_collect_fee, op_create_signal, op_execute_trade,
    regression_budget_limit, tx_cache_or_compute, BASELINE_AUTO_TRADE_INSTRUCTIONS,
    BASELINE_COPY_TRADE_INSTRUCTIONS, BASELINE_FEE_COLLECT_INSTRUCTIONS,
    BASELINE_SIGNAL_SUBMIT_INSTRUCTIONS, DEFAULT_INSTRUCTION_BUDGET, REGRESSION_BUDGET_PCT,
};
pub use multisig::{
    emit_approval_recorded, emit_proposal_approved, emit_proposal_cancelled,
    emit_proposal_created, emit_proposal_executed, emit_timelock_config_updated,
    get_multisig_stats, get_proposal, get_timelock_config, prepare_execution, propose, approve,
    cancel, set_timelock_config, store_proposal, validate_signer_config, ApprovalProposal,
    CriticalActionType, MultisigError, MultisigStorageKey, MultisigTimelockConfig, ProposalStatus,
    DEFAULT_ADMIN_TRANSFER_DELAY, DEFAULT_CONFIG_DELAY, DEFAULT_FEE_CHANGE_DELAY,
    DEFAULT_GUARDIAN_DELAY, DEFAULT_PARAMETER_DELAY, DEFAULT_PAUSE_DELAY, DEFAULT_UNPAUSE_DELAY,
    MAX_ACTIVE_PROPOSALS, MAX_SIGNERS,
};
pub use oracle::{
    oracle_price_to_i128, validate_freshness, validate_oracle_price, validate_price_bounds,
    IOracleClient, MAX_ORACLE_PRICE, MIN_ORACLE_PRICE, MockOracleClient, OnChainOracleClient,
    OracleError, OraclePrice,
};
pub use rate_limit::{
    check_rate_limit, record_action, set_config as set_rate_limit_config, ActionType, RateLimitConfig,
};
pub use replay_protection::{current_nonce, verify_and_commit, ReplayError};

#[cfg(test)]
mod storage_key_tests;
