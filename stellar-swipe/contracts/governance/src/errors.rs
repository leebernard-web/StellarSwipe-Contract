use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum GovernanceError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    InvalidSupply = 4,
    InvalidAmount = 5,
    InvalidDuration = 6,
    DuplicateSchedule = 7,
    VestingScheduleNotFound = 8,
    CliffNotReached = 9,
    NothingToRelease = 10,
    InsufficientBalance = 11,
    InsufficientStakedBalance = 12,
    ActiveVoteLock = 13,
    BelowMinimumClaim = 14,
    LiquidityPoolExhausted = 15,
    DuplicateRecipient = 16,
    ArithmeticOverflow = 17,
    InvalidRewardConfig = 18,
    InvalidMetadata = 19,
    BudgetNotFound = 20,
    BudgetExceeded = 21,
    BudgetPeriodEnded = 22,
    MissingAssetPrice = 23,
    InvalidTreasuryConfig = 24,
    InvalidCommitteeConfig = 25,
    CommitteeNotFound = 26,
    CommitteeTermEnded = 27,
    CommitteeDecisionNotFound = 28,
    CommitteeDecisionNotOpen = 29,
    AlreadyVoted = 30,
    NoCommitteeAuthority = 31,
    CommitteeElectionNotFound = 32,
    CommitteeElectionNotActive = 33,
    NotCommitteeCandidate = 34,
    CrossCommitteeRequestNotFound = 35,
    CommitteeInactive = 36,
    InvalidCommitteeAction = 37,
    InvalidApprovalRating = 38,
    InvalidGovernanceConfig = 39,
    InvalidProposal = 40,
    ProposalNotFound = 41,
    ProposalNotActive = 42,
    ProposalNotApproved = 43,
    VotingNotStarted = 44,
    VotingEnded = 45,
    NoVotingPower = 46,
    TimelockNotInitialized = 47,
    ActionNotFound = 48,
    InvalidTimelockConfig = 49,
    ConvictionPoolNotFound = 50,
 feat/treasury-budget-caps
    /// Fewer eligible voters participated in a committee election than the
    /// configured minimum quorum requires.  The election is voided and the
    /// committee keeps its existing members.
    ElectionQuorumNotMet = 51,
    /// A vote submitted to a committee election was structurally invalid —
    /// for example the candidate was not on the ballot.  This error is
    /// returned so callers can diagnose the problem; internally the ballot is
    /// rejected without mutating election state.
    InvalidElectionVote = 52,
    /// A treasury spend was attempted for a budget category that has not yet
    /// received a governance-approved budget cap via `approve_treasury_budget`.
    BudgetApprovalRequired = 53,
    /// The requested spend would cause the category's total governance-approved
    /// cap to be exceeded.
    ApprovedCapExceeded = 54,

    InvalidCalibrationConfig = 51,
main
}
