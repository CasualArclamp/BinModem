//! ITU-T V.42bis data compression (BTLZ).
//!
//! The compression half of `CONNECT 33600/V42BIS`. Strings read from the DTE
//! are matched against a dictionary of trees and sent as fixed-length
//! codewords; the dictionary adapts as it goes and both ends build the same one
//! from the same data, which is what makes a codeword reversible.

pub mod bits;
pub mod codec;
pub mod dictionary;

pub use codec::{Decoder, Encoder, Error, Mode};
pub use dictionary::{
    DEFAULT_N2, DEFAULT_N7, Dictionary, OFFERED_N2, OFFERED_N7, Params,
};
