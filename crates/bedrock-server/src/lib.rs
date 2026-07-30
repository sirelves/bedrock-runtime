//! Tick loop, session registry, packet handlers and orchestration.
//!
//! The only crate that depends on all the others.
//!
//! **Concurrency model (normative, see `docs/ARCHITECTURE.md`):** world state is
//! single-threaded. I/O is async and lives outside the tick; the boundary between
//! them is two queues, inbound and outbound. The tick never blocks on I/O. A feature
//! that needs two threads mutating the world needs an ADR before it needs code.
//!
//! **Client authority:** no client claim about game state is accepted without
//! server-side validation. This is a posture from M0, not an anti-cheat feature
//! bolted on later — see `SECURITY.md`.
//!
//! Status: not started.

/// Target tick rate, in ticks per second.
pub const TICKS_PER_SECOND: u32 = 20;

/// The budget for a single tick. Exceeding it is a dropped tick, visible to every
/// connected player at once — see `docs/PERFORMANCE.md`.
pub const TICK_BUDGET: std::time::Duration = std::time::Duration::from_millis(50);
