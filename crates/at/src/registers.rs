//! S-parameters (V.250 5.3.2 and clause 6).
//!
//! V.250 5.3.2: "If the number is not recognized as a valid parameter number,
//! an ERROR result code is issued." Many real modems instead expose a flat
//! array of 256 scratch registers; we follow the Recommendation.

/// Definition of one S-parameter.
#[derive(Debug, Clone, Copy)]
pub struct Spec {
    pub number: u8,
    pub default: u8,
    pub min: u8,
    pub max: u8,
    /// Clause of V.250 that defines it, or the origin if it is an extension.
    pub clause: &'static str,
    pub note: &'static str,
}

/// Every S-parameter this DCE implements.
///
/// Defaults are the Recommendation's "Recommended default setting" where one
/// exists. V.250 gives S7 and S10 a range but no default, so those two take the
/// long-standing Hayes values; that is convention, not conformance.
pub const SPECS: &[Spec] = &[
    Spec { number: 0,  default: 0,  min: 0, max: 255, clause: "V.250 6.3.8",
           note: "rings before automatic answer; 0 disables" },
    Spec { number: 2,  default: 43, min: 0, max: 255, clause: "extension",
           note: "escape character; values above 127 disable escaping. \
                  Hayes/TIA-602, absent from V.250" },
    Spec { number: 3,  default: 13, min: 0, max: 127, clause: "V.250 6.2.1",
           note: "command line termination character" },
    Spec { number: 4,  default: 10, min: 0, max: 127, clause: "V.250 6.2.2",
           note: "response formatting character" },
    Spec { number: 5,  default: 8,  min: 0, max: 127, clause: "V.250 6.2.3",
           note: "command line editing character" },
    Spec { number: 6,  default: 2,  min: 2, max: 10,  clause: "V.250 6.3.9",
           note: "seconds to pause before blind dialling" },
    Spec { number: 7,  default: 50, min: 1, max: 255, clause: "V.250 6.3.10",
           note: "seconds to wait for carrier; no ITU default, Hayes value used" },
    Spec { number: 8,  default: 2,  min: 0, max: 255, clause: "V.250 6.3.11",
           note: "seconds to pause for the comma dial modifier" },
    Spec { number: 10, default: 14, min: 1, max: 254, clause: "V.250 6.3.12",
           note: "tenths of a second to hold the line after carrier loss; \
                  no ITU default, Hayes value used" },
    Spec { number: 12, default: 50, min: 0, max: 255, clause: "extension",
           note: "escape sequence guard time in fiftieths of a second; \
                  Hayes/TIA-602, absent from V.250" },
];

pub fn spec_for(number: u8) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.number == number)
}

/// The S-parameter store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registers {
    values: [u8; 256],
}

impl Default for Registers {
    fn default() -> Self {
        let mut values = [0u8; 256];
        for s in SPECS {
            values[s.number as usize] = s.default;
        }
        Self { values }
    }
}

/// Why an S-parameter access was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegError {
    /// V.250 5.3.2: the parameter number is not implemented.
    Unknown(u8),
    /// V.250 5.6.2: the value lies outside the parameter's defined range.
    OutOfRange { number: u8, value: u32, min: u8, max: u8 },
}

impl Registers {
    /// Read a parameter. Errors if the number is not implemented.
    pub fn get(&self, number: u8) -> Result<u8, RegError> {
        if spec_for(number).is_none() {
            return Err(RegError::Unknown(number));
        }
        Ok(self.values[number as usize])
    }

    /// Write a parameter, range-checked against its definition.
    pub fn set(&mut self, number: u8, value: u32) -> Result<(), RegError> {
        let spec = spec_for(number).ok_or(RegError::Unknown(number))?;
        if value < spec.min as u32 || value > spec.max as u32 {
            return Err(RegError::OutOfRange {
                number,
                value,
                min: spec.min,
                max: spec.max,
            });
        }
        self.values[number as usize] = value as u8;
        Ok(())
    }

    /// Value without range checking, for internal use where the caller knows
    /// the parameter exists.
    pub fn raw(&self, number: u8) -> u8 {
        self.values[number as usize]
    }

    /// Command line termination character (S3).
    pub fn terminator(&self) -> u8 {
        self.values[3]
    }

    /// Response formatting character (S4).
    pub fn formatter(&self) -> u8 {
        self.values[4]
    }

    /// Command line editing character (S5).
    pub fn editor(&self) -> u8 {
        self.values[5]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_recommendation() {
        let r = Registers::default();
        assert_eq!(r.get(0).unwrap(), 0, "S0 automatic answer disabled");
        assert_eq!(r.get(3).unwrap(), 13, "S3 is CR");
        assert_eq!(r.get(4).unwrap(), 10, "S4 is LF");
        assert_eq!(r.get(5).unwrap(), 8, "S5 is BS");
        assert_eq!(r.get(6).unwrap(), 2, "S6 blind dial pause");
        assert_eq!(r.get(8).unwrap(), 2, "S8 comma pause");
    }

    #[test]
    fn unimplemented_parameters_are_rejected() {
        let mut r = Registers::default();
        assert_eq!(r.get(99), Err(RegError::Unknown(99)));
        assert_eq!(r.set(99, 1), Err(RegError::Unknown(99)));
    }

    #[test]
    fn values_are_range_checked() {
        let mut r = Registers::default();
        // S6 is defined as 2 to 10, so 1 and 11 are both out of range.
        assert!(r.set(6, 5).is_ok());
        assert_eq!(r.get(6).unwrap(), 5);
        assert!(matches!(r.set(6, 1), Err(RegError::OutOfRange { .. })));
        assert!(matches!(r.set(6, 11), Err(RegError::OutOfRange { .. })));
        assert_eq!(r.get(6).unwrap(), 5, "a rejected write must not alter the value");
    }

    #[test]
    fn s3_s4_s5_reject_values_above_127() {
        let mut r = Registers::default();
        for n in [3u8, 4, 5] {
            assert!(r.set(n, 127).is_ok(), "S{n} should accept 127");
            assert!(matches!(r.set(n, 128), Err(RegError::OutOfRange { .. })),
                    "S{n} should reject 128");
        }
    }

    #[test]
    fn every_default_lies_within_its_own_range() {
        for s in SPECS {
            assert!(s.default >= s.min && s.default <= s.max,
                    "S{} default {} outside {}..={}", s.number, s.default, s.min, s.max);
        }
    }
}
