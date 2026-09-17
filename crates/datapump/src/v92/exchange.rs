//! The SUV/CP/E acknowledge-and-repeat exchange of Phase 4 (9.6.1.1, 9.6.2.1
//! and Figures 12-14), which rate renegotiation (9.8) and fast parameter
//! exchange (9.9) enter with a context of their own.
//!
//! One machine serves both sides, driven by the plain flag structs of the
//! parent module rather than by the wire types, so it never sees a bit layout.
//!
//! V92-14 fills this in.
