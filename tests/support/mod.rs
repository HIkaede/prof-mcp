#![allow(dead_code)]

use std::{io::Cursor, path::PathBuf};

use prof_mcp::profile::{BuildLimits, Profile, ProfileBuilder};

pub fn profile(input: &str) -> Profile {
    ProfileBuilder::new(BuildLimits::default())
        .from_reader(
            Cursor::new(input.as_bytes()),
            PathBuf::from("/synthetic/profile.folded"),
            input.len() as u64,
            Some(1),
        )
        .expect("synthetic profile must parse")
}

pub fn frame(profile: &Profile, name: &str) -> u32 {
    profile.frame_id(name).expect("fixture frame")
}

pub const DIAMOND: &str = "root;A;X 20\nroot;B;X 30\n";
pub const SHARED_CALLEE: &str = "root;A;X;leaf1 20\nroot;B;X;leaf2 30\n";
pub const RECURSION: &str = "root;foo;foo;bar 10\n";
