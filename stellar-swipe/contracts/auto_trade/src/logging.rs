#![allow(dead_code)]

use soroban_sdk::{contracttype, Address, Env, String, Symbol, Vec};
use crate::admin::{require_admin};
use crate::errors::AutoTradeError;

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    pub schema_version: u32,
    pub timestamp: u64,
    pub level: LogLevel,
    pub category: String,
    pub message: String,
    pub correlation_id: Option<String>,
}

#[contracttype]
pub enum LoggingStorageKey {
    Config,
    RecentLogs,
}

pub fn set_log_level(env: &Env, caller: &Address, level: LogLevel) -> Result<(), AutoTradeError> {
    require_admin(env, caller)?;
    caller.require_auth();
    env.storage().instance().set(&LoggingStorageKey::Config, &level);
    Ok(())
}

pub fn get_log_level(env: &Env) -> LogLevel {
    env.storage()
        .instance()
        .get(&LoggingStorageKey::Config)
        .unwrap_or(LogLevel::Info)
}

pub fn is_info_logging_enabled(env: &Env) -> bool {
    matches!(
        get_log_level(env),
        LogLevel::Debug | LogLevel::Info
    )
}

pub fn emit_log(
    env: &Env,
    level: LogLevel,
    category: String,
    message: String,
    correlation_id: Option<String>,
) {
    let configured = get_log_level(env);
    if !should_log(&configured, &level) {
        return;
    }

    let entry = LogEntry {
        schema_version: 1,
        timestamp: env.ledger().timestamp(),
        level: level.clone(),
        category: category.clone(),
        message: message.clone(),
        correlation_id: correlation_id.clone(),
    };

    env.events().publish((Symbol::new(env, "log_entry"),), entry.clone());

    let mut logs: Vec<LogEntry> = env
        .storage()
        .instance()
        .get(&LoggingStorageKey::RecentLogs)
        .unwrap_or_else(|| Vec::new(env));

    if logs.len() >= 20 {
        logs.remove(0);
    }
    logs.push_back(entry);
    env.storage().instance().set(&LoggingStorageKey::RecentLogs, &logs);
}

fn should_log(configured: &LogLevel, event_level: &LogLevel) -> bool {
    matches!(
        (configured, event_level),
        (LogLevel::Debug, _)
            | (LogLevel::Info, LogLevel::Info)
            | (LogLevel::Info, LogLevel::Warn)
            | (LogLevel::Info, LogLevel::Error)
            | (LogLevel::Warn, LogLevel::Warn)
            | (LogLevel::Warn, LogLevel::Error)
            | (LogLevel::Error, LogLevel::Error)
    )
}
