//! The BTLZ dictionary (V.42bis clause 6).
//!
//! A set of 256 trees, one per character of the alphabet, each node standing
//! for one known string. A node's codeword identifies it, and because both ends
//! build their dictionaries by the same rules from the same data, a codeword is
//! a reversible encoding of the string it names.

use std::collections::HashMap;

/// Character size in bits (V.42bis clause 10, N3).
pub const N3: usize = 8;
/// Characters in the alphabet, 2^N3 (N4).
pub const N4: u16 = 256;
/// Control codewords: ETM, FLUSH and STEPUP (N6).
pub const N6: u16 = 3;
/// First codeword available for a multi-character string, N4 + N6 (N5).
pub const N5: u16 = N4 + N6;

/// Control codewords used in compressed mode (V.42bis Table 2).
pub const ETM: u16 = 0;
pub const FLUSH: u16 = 1;
pub const STEPUP: u16 = 2;

/// Command codes used in transparent mode, following the escape character.
pub const ECM: u8 = 0;
pub const EID: u8 = 1;
pub const RESET: u8 = 2;

/// Total codewords, N2. V.42bis 5.1 gives P1 a default and minimum of 512.
pub const DEFAULT_N2: u16 = 512;
/// Maximum string length, N7. P2 defaults to 6 and ranges from 6 to 250.
pub const DEFAULT_N7: u8 = 6;

/// Negotiable parameters (V.42bis clause 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Params {
    /// N2, total codewords.
    pub n2: u16,
    /// N7, maximum string length.
    pub n7: u8,
}

impl Default for Params {
    fn default() -> Self {
        Self { n2: DEFAULT_N2, n7: DEFAULT_N7 }
    }
}

impl Params {
    /// N1, the maximum codeword size in bits: enough to address N2 codewords.
    pub fn max_code_bits(&self) -> u32 {
        let mut bits = N3 as u32 + 1;
        while (1u32 << bits) < self.n2 as u32 {
            bits += 1;
        }
        bits
    }

    /// Reject values V.42bis 5.1 calls procedural errors.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.n2 < DEFAULT_N2 {
            return Err("N2 below the minimum of 512");
        }
        if !(6..=250).contains(&self.n7) {
            return Err("N7 outside the permitted range of 6 to 250");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Node {
    /// Codeword of the parent, or `None` for a root.
    parent: Option<u16>,
    ch: u8,
    /// How many nodes hang off this one. Zero means a leaf, and only leaves may
    /// be recovered (V.42bis 6.5).
    children: u16,
    /// String length, so N7 can be enforced.
    depth: u8,
    in_use: bool,
}

/// The encoder's or decoder's dictionary.
#[derive(Debug)]
pub struct Dictionary {
    params: Params,
    nodes: Vec<Node>,
    /// Child lookup, keyed by parent codeword and appended character.
    index: HashMap<(u16, u8), u16>,
    /// C1, the next empty entry (V.42bis 6.5).
    c1: u16,
}

impl Dictionary {
    pub fn new(params: Params) -> Self {
        let mut d = Self {
            params,
            nodes: vec![Node::default(); params.n2 as usize],
            index: HashMap::new(),
            c1: N5,
        };
        d.reset();
        d
    }

    /// Return to the initial condition (V.42bis 6.2): every tree is a bare root
    /// and C1 is N5.
    pub fn reset(&mut self) {
        for n in &mut self.nodes {
            *n = Node::default();
        }
        self.index.clear();
        for c in 0..N4 {
            let code = Self::root_code(c as u8);
            self.nodes[code as usize] = Node {
                parent: None,
                ch: c as u8,
                children: 0,
                depth: 1,
                in_use: true,
            };
        }
        self.c1 = N5;
    }

    /// Codeword of the single-character string `ch`: N6 plus its ordinal
    /// (V.42bis 6.2).
    pub fn root_code(ch: u8) -> u16 {
        N6 + ch as u16
    }

    pub fn params(&self) -> Params {
        self.params
    }

    pub fn find_child(&self, parent: u16, ch: u8) -> Option<u16> {
        self.index.get(&(parent, ch)).copied()
    }

    pub fn depth(&self, code: u16) -> u8 {
        self.nodes.get(code as usize).map(|n| n.depth).unwrap_or(0)
    }

    pub fn in_use(&self, code: u16) -> bool {
        self.nodes.get(code as usize).map(|n| n.in_use).unwrap_or(false)
    }

    /// The string a codeword names, first character first.
    pub fn string(&self, code: u16) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cursor = Some(code);
        while let Some(c) = cursor {
            let Some(node) = self.nodes.get(c as usize) else { break };
            if !node.in_use {
                break;
            }
            out.push(node.ch);
            cursor = node.parent;
        }
        out.reverse();
        out
    }

    /// First character of the string a codeword names, without building it.
    pub fn first_char(&self, code: u16) -> Option<u8> {
        let mut cursor = code;
        loop {
            let node = self.nodes.get(cursor as usize)?;
            if !node.in_use {
                return None;
            }
            match node.parent {
                Some(p) => cursor = p,
                None => return Some(node.ch),
            }
        }
    }

    /// Add `parent + ch` as a new entry (V.42bis 6.4).
    ///
    /// Returns the new codeword, or `None` when the string is already present
    /// or would exceed N7.
    pub fn add(&mut self, parent: u16, ch: u8) -> Option<u16> {
        if self.depth(parent) as u16 + 1 > self.params.n7 as u16 {
            return None;
        }
        if self.find_child(parent, ch).is_some() {
            return None;
        }
        let code = self.c1;
        // The slot may hold an entry recovered earlier; clear it out properly.
        self.evict(code);
        self.nodes[code as usize] = Node {
            parent: Some(parent),
            ch,
            children: 0,
            depth: self.depth(parent) + 1,
            in_use: true,
        };
        self.index.insert((parent, ch), code);
        self.nodes[parent as usize].children += 1;

        // V.42bis 6.4: recovery runs immediately after every creation.
        self.recover();
        Some(code)
    }

    /// Free a single entry for re-use (V.42bis 6.5).
    fn recover(&mut self) {
        // Bounded so a dictionary with no recoverable leaf cannot spin.
        let span = self.params.n2 - N5;
        for _ in 0..span {
            self.c1 += 1;
            if self.c1 > self.params.n2 - 1 {
                self.c1 = N5;
            }
            let node = self.nodes[self.c1 as usize];
            if node.in_use && node.children > 0 {
                continue; // in use and not a leaf: keep looking
            }
            if node.in_use {
                self.evict(self.c1);
            }
            return;
        }
    }

    /// Detach a leaf from its parent and mark the slot free.
    fn evict(&mut self, code: u16) {
        let node = self.nodes[code as usize];
        if !node.in_use {
            return;
        }
        if let Some(parent) = node.parent {
            self.index.remove(&(parent, node.ch));
            let p = &mut self.nodes[parent as usize];
            p.children = p.children.saturating_sub(1);
        }
        self.nodes[code as usize] = Node::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_codewords_follow_the_recommendation() {
        // V.42bis 6.2: N6 plus the ordinal value of the character.
        assert_eq!(Dictionary::root_code(0), 3);
        assert_eq!(Dictionary::root_code(b'A'), 3 + 65);
        assert_eq!(Dictionary::root_code(255), 258);
        assert_eq!(N5, 259);
    }

    #[test]
    fn a_fresh_dictionary_holds_only_roots() {
        let d = Dictionary::new(Params::default());
        for c in 0..=255u8 {
            assert!(d.in_use(Dictionary::root_code(c)), "root {c} missing");
            assert_eq!(d.depth(Dictionary::root_code(c)), 1);
        }
        assert!(!d.in_use(N5), "no string entries should exist yet");
    }

    #[test]
    fn strings_reconstruct_from_their_codeword() {
        let mut d = Dictionary::new(Params::default());
        let b = Dictionary::root_code(b'B');
        let ba = d.add(b, b'A').unwrap();
        let bag = d.add(ba, b'G').unwrap();
        assert_eq!(d.string(b), b"B");
        assert_eq!(d.string(ba), b"BA");
        assert_eq!(d.string(bag), b"BAG");
        assert_eq!(d.first_char(bag), Some(b'B'));
    }

    #[test]
    fn a_duplicate_string_is_not_added_twice() {
        let mut d = Dictionary::new(Params::default());
        let b = Dictionary::root_code(b'B');
        let first = d.add(b, b'A').unwrap();
        assert_eq!(d.add(b, b'A'), None);
        assert_eq!(d.find_child(b, b'A'), Some(first));
    }

    #[test]
    fn the_maximum_string_length_is_enforced() {
        // V.42bis 6.4 a): a string may not exceed N7.
        let params = Params { n7: 6, ..Default::default() };
        let mut d = Dictionary::new(params);
        let mut code = Dictionary::root_code(b'a');
        for i in 1..6 {
            code = d.add(code, b'a' + i).unwrap_or_else(|| panic!("depth {i} refused"));
            assert_eq!(d.depth(code), i + 1);
        }
        assert_eq!(d.depth(code), 6);
        assert_eq!(d.add(code, b'z'), None, "a seventh character must be refused");
    }

    #[test]
    fn recovery_reuses_entries_once_the_dictionary_fills() {
        let params = Params { n2: 512, n7: 250 };
        let mut d = Dictionary::new(params);
        // Add far more strings than there are slots, so recovery must run.
        let mut added = 0;
        for a in 0..=255u8 {
            for b in 0..=255u8 {
                if d.add(Dictionary::root_code(a), b).is_some() {
                    added += 1;
                }
                if added > 2000 {
                    break;
                }
            }
            if added > 2000 {
                break;
            }
        }
        assert!(added > 512, "recovery should have kept slots available");
        // Every remaining entry must still describe a consistent string.
        for code in N5..params.n2 {
            if d.in_use(code) {
                let s = d.string(code);
                assert!(!s.is_empty(), "codeword {code} names an empty string");
                assert_eq!(s.len() as u8, d.depth(code));
            }
        }
    }

    #[test]
    fn a_parent_is_never_recovered_before_its_children() {
        // V.42bis 6.5 c): a node in use that is not a leaf is skipped.
        let params = Params { n2: 512, n7: 250 };
        let mut d = Dictionary::new(params);
        for a in 0..=255u8 {
            for b in 0..4u8 {
                d.add(Dictionary::root_code(a), b);
            }
        }
        for code in N5..params.n2 {
            if !d.in_use(code) {
                continue;
            }
            if let Some(parent) = (code >= N5).then(|| d.nodes[code as usize].parent).flatten() {
                assert!(
                    d.in_use(parent),
                    "codeword {code} outlived its parent {parent}"
                );
            }
        }
    }

    #[test]
    fn maximum_codeword_size_covers_the_dictionary() {
        assert_eq!(Params { n2: 512, n7: 6 }.max_code_bits(), 9);
        assert_eq!(Params { n2: 1024, n7: 6 }.max_code_bits(), 10);
        assert_eq!(Params { n2: 2048, n7: 6 }.max_code_bits(), 11);
        assert_eq!(Params { n2: 4096, n7: 6 }.max_code_bits(), 12);
    }

    #[test]
    fn out_of_range_parameters_are_refused() {
        // V.42bis 5.1 calls these procedural errors.
        assert!(Params { n2: 511, n7: 6 }.validate().is_err());
        assert!(Params { n2: 512, n7: 5 }.validate().is_err());
        assert!(Params { n2: 512, n7: 251 }.validate().is_err());
        assert!(Params::default().validate().is_ok());
    }
}
