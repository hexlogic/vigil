//! Helpers shared by the parsing tests.

#![allow(dead_code)]

use vigil_notify::{Channel, ParseError};

/// Parse a URL that must be valid.
pub fn ok(url: &str) -> Channel {
    match Channel::parse(url) {
        Ok(channel) => channel,
        Err(error) => panic!("{url} should parse: {error}"),
    }
}

/// Parse a URL that must be refused.
pub fn err(url: &str) -> ParseError {
    match Channel::parse(url) {
        Ok(channel) => panic!("{url} should not parse, got {channel:?}"),
        Err(error) => error,
    }
}
