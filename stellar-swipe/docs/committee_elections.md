# Committee Elections

This document describes the requirements, quorum rules, invalid-vote handling,
and failure modes for governance committee elections in the StellarSwipe
governance contract.

---

## Overview

A committee election replaces the current membership of a governance committee
with addresses chosen by stake-weighted community voting.  Elections are
managed through four on-chain operations:

| Step | Function | Caller |
|------|----------|--------|
| 1 | `start_committee_election` | Admin |
| 2 | `nominate_for_committee` | Any staked address |
| 3 | `vote_in_committee_election` | Any staked address |
| 4 | `finalize_committee_election` | Admin (after deadline) |

---

## Starting an Election

```
start_committee_election(
    admin,
    committee_id,
    positions_available,   // u32  — must be 3–max_members
    duration_days,         // u32  — must be > 0
    min_participation,     // u32  — minimum unique valid voters required
    quorum_stake_threshold // i128 — minimum total staked weight required
)
```

`min_participation` and `quorum_stake_threshold` are independent thresholds.
Set either to `0` to disable that check.

**Constraints:**
- `positions_available` must be ≥ 3 and ≤ the committee's `max_members`.
- `duration_days` must be ≥ 1.
- `quorum_stake_threshold` must be ≥ 0.
- A new election cannot be started while a previous one is still active
  (i.e. before `election_end`).

---

## Nomination

Any address with a positive staked balance may nominate a candidate.
Self-nomination is allowed (nominator and nominee can be the same address).
Duplicate nominations for the same candidate are silently de-duplicated.

---

## Casting a Vote

```
vote_in_committee_election(committee_id, voter, candidate)
```

A ballot is **accepted** when:
1. The election is currently active (`election_start ≤ now < election_end`).
2. `candidate` appears in `election.candidates` (was nominated).
3. `voter` has a staked balance > 0 at the time of the vote.
4. `voter` has not already cast a ballot in this election.

A ballot is **rejected** (returns `GovernanceError::InvalidElectionVote`) when:
- `candidate` is not on the ballot.
- `voter` has zero staked balance.

A duplicate ballot returns `GovernanceError::AlreadyVoted`.

**In all rejection cases the election state is not modified** — no phantom
votes are written, and previously recorded valid votes are preserved intact.

---

## Finalizing an Election

```
finalize_committee_election(admin, committee_id)
```

Can only be called after `election_end`.  Returns an `ElectionResult`:

```rust
pub struct ElectionResult {
    pub status: ElectionStatus,       // outcome variant (see below)
    pub winners: Vec<Address>,        // elected addresses; empty on failure
    pub valid_votes: u32,             // count of accepted ballots
    pub total_stake_weight: i128,     // sum of staked balances behind valid votes
    pub rejected_votes: u32,          // count of dropped invalid ballots
}
```

### ElectionStatus Variants

| Variant | Meaning | Committee updated? |
|---------|---------|-------------------|
| `Succeeded` | All checks passed; winners installed | **Yes** |
| `FailedQuorumParticipation` | `valid_votes < min_participation` | No |
| `FailedQuorumStake` | `total_stake_weight < quorum_stake_threshold` | No |
| `FailedInsufficientWinners` | Fewer than 3 candidates received any votes | No |
| `Pending` | Election not yet finalised (informational only) | No |

On any failure variant:
- The **existing committee membership is unchanged**.
- The failed election record is **removed** from state so a new election can
  be started immediately.
- No error is returned — the caller inspects `result.status`.

---

## Invalid Vote Handling

Invalid ballots are **rejected before being recorded**, not silently accepted
and later excluded.  This means:

- `election.votes` never contains a ballot for an address that was not staked
  or for a candidate that was not nominated.
- `election.votes.len()` reflects only valid, accepted ballots.
- `result.rejected_votes` counts ballots that were **dropped at finalization
  time** (e.g. a voter who unstaked after voting but before finalization).

This two-layer approach ensures election state is never corrupted:
- Ballots with obvious errors are refused at cast time.
- Ballots that become invalid between cast time and finalization
  (e.g. voter unstakes) are counted in `rejected_votes` but excluded from
  tallying and quorum math.

---

## Example: Quorum Failure and Recovery

```
// Election 1 — strict quorum, not met
start_committee_election(admin, id, 3, 7, min_participation=10, quorum_stake=0)
// ... only 3 voters participate ...
finalize_committee_election(admin, id)
// → ElectionResult { status: FailedQuorumParticipation, winners: [], valid_votes: 3 }
// Committee is unchanged; election record is cleared.

// Election 2 — relaxed quorum, succeeds
start_committee_election(admin, id, 3, 7, min_participation=1, quorum_stake=0)
// ... 3+ voters participate ...
finalize_committee_election(admin, id)
// → ElectionResult { status: Succeeded, winners: [...], valid_votes: N }
```

---

## Error Reference

| Error | Trigger |
|-------|---------|
| `CommitteeElectionNotFound` | No election exists for `committee_id` |
| `CommitteeElectionNotActive` | Voting outside `[election_start, election_end)`, or finalizing before deadline |
| `InvalidElectionVote` | Vote for un-nominated candidate, or voter has no staked balance |
| `AlreadyVoted` | Voter has already cast a ballot in this election |
| `InvalidCommitteeConfig` | `positions_available < 3`, `duration_days == 0`, negative `quorum_stake_threshold`, or election already running |
| `ElectionQuorumNotMet` | Reserved for direct call paths that need a hard error; prefer checking `ElectionResult.status` after `finalize_committee_election` |
