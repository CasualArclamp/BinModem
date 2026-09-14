//! V.34: up to 33 600 bit/s over a telephone line.
//!
//! Where V.32bis fixed everything in advance -- one symbol rate, one carrier,
//! one constellation for each rate -- V.34 measures the line first and then
//! picks: six symbol rates from 2400 to 3429 a second, a low and a high carrier
//! for most of them, eleven pre-emphasis filters, and a data rate chosen from
//! what the probing found. The start-up is where most of the modem is.
//!
//! Four phases (clause 11). Phase 1 is V.8, which already exists. Phase 2
//! probes and ranges: the two modems exchange capabilities in INFO0 sequences,
//! measure the round trip with phase reversals of tones A and B, send each
//! other the line probing signals L1 and L2, and settle symbol rates, carriers
//! and power in INFO1c and INFO1a. Phase 3 trains the equalisers and echo
//! cancellers, and phase 4 the rest, before data.
//!
//! Built from phase 2 up, and the start-up is done: the INFO sequences and the
//! DPSK they ride on, the probing signals and what they measure, the QAM of
//! phases 3 and 4 and a receiver that trains on them, the J and MP exchanges,
//! and the procedures for both ends, each checked against real modems where a
//! recording has it. The data mode is next.

pub mod constellation;
pub mod dpsk;
pub mod info;
pub mod mp;
pub mod phase2;
pub mod probe;
pub mod qam;
pub mod receiver;
pub mod signals;
pub mod startup;
pub mod training;
