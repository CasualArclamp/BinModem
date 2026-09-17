//! The digital modem's data-mode decoder: clause 6.4 read backwards, a Viterbi
//! search over the 4D subsets and the modulus decoder, plus the RM watch that
//! spots a fast parameter exchange (8.7.4, 9.9.1.1.2).
//!
//! V92-20 fills this in.
