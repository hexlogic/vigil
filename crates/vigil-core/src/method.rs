//! The HTTP method of a watch's request.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The method a watch's request is made with. Serialised in capitals, as `"POST"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    /// Read the page. This is the default.
    #[default]
    Get,
    /// Send a body, as a search form or an API query does.
    Post,
    /// Send a body to replace a resource.
    Put,
    /// Send a body to change a resource.
    Patch,
    /// Delete a resource.
    Delete,
}

/// The text is not a method a watch can use.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown request method {0:?}: use GET, POST, PUT, PATCH or DELETE")]
pub struct ParseMethodError(String);

impl Method {
    /// The method's name in capitals.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }

    /// Whether this is the default, so stored JSON can leave it out.
    pub fn is_default(&self) -> bool {
        *self == Self::Get
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Method {
    type Err = ParseMethodError;

    /// Reads a method name in any case.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_uppercase().as_str() {
            "GET" => Ok(Self::Get),
            "POST" => Ok(Self::Post),
            "PUT" => Ok(Self::Put),
            "PATCH" => Ok(Self::Patch),
            "DELETE" => Ok(Self::Delete),
            _ => Err(ParseMethodError(s.trim().to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Method;

    #[test]
    fn names_read_in_any_case_and_print_in_capitals() -> Result<(), Box<dyn std::error::Error>> {
        for (text, method) in [
            ("get", Method::Get),
            (" Post ", Method::Post),
            ("PUT", Method::Put),
            ("patch", Method::Patch),
            ("delete", Method::Delete),
        ] {
            assert_eq!(text.parse::<Method>()?, method);
            assert_eq!(method.to_string(), method.as_str());
        }
        let error = "TRACE".parse::<Method>().err().map(|e| e.to_string());
        assert_eq!(
            error.as_deref(),
            Some("unknown request method \"TRACE\": use GET, POST, PUT, PATCH or DELETE")
        );
        Ok(())
    }

    #[test]
    fn the_stored_form_is_in_capitals() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(serde_json::to_string(&Method::Patch)?, "\"PATCH\"");
        assert_eq!(serde_json::from_str::<Method>("\"POST\"")?, Method::Post);
        assert!(serde_json::from_str::<Method>("\"post\"").is_err());
        Ok(())
    }
}
