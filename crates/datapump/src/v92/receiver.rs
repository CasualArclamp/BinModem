//! The digital modem's upstream PCM receiver: hunting Ru and Su and their
//! reversals (8.5.5, 8.5.6), training on TRN1u (8.5.7) and TRN2u (8.7.6),
//! holding the 12-symbol frame alignment of 9.5.1.1.10, and handing out
//! equalised symbols.
//!
//! V92-22 fills this in.
