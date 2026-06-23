#![no_std]

pub mod auth;
#[allow(deprecated)]
pub mod cross_contract;
pub mod errors;
#[allow(deprecated)]
pub mod events;
pub mod math;
#[allow(deprecated)]
pub mod version;

pub use cross_contract::{
    CrossContractError, CrossContractMessage, CrossContractMessageReceiverClient,
    CrossContractVersionClient, MessageStatus, MAX_MESSAGE_SIZE,
};
pub use errors::{ErrorCategory, RecoveryStrategy};
pub use version::{ContractKind, VersionError};
