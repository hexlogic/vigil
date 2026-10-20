//! Email over SMTP: `mailto://[user:password@]host[:port]?to=a@b.c` and `mailtos://`.

use std::str::FromStr;

use lettre::Address;
use lettre::message::Mailbox;

use super::Secret;
use super::parse::{authority_of, is_loopback, split_list, split_url};
use crate::error::{ParseError, invalid};

/// How the connection to the SMTP server is secured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpTls {
    /// Connect in plaintext and require an upgrade with `STARTTLS` (`mailto://`, port 587).
    Starttls,
    /// TLS from the first byte (`mailtos://`, port 465).
    Implicit,
    /// No encryption (`mailto://...?tls=none`), for a relay on the local machine or network.
    None,
}

/// A message sent through an SMTP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email {
    /// How the connection is secured.
    pub tls: SmtpTls,
    /// The SMTP server.
    pub host: String,
    /// The SMTP port.
    pub port: u16,
    /// User name and password, if the server wants them.
    pub credentials: Option<(String, Secret)>,
    /// The sender.
    pub from: Mailbox,
    /// The recipients; at least one.
    pub to: Vec<Mailbox>,
}

pub(super) fn parse(wrapped: bool, input: &str) -> Result<Email, ParseError> {
    let scheme = if wrapped { "mailtos" } else { "mailto" };
    let url = split_url(scheme, input)?;
    if !url.segments.is_empty() {
        return Err(invalid(scheme, "takes no path; options go after ?"));
    }
    let mut options = url.options(scheme);

    let tls = match (wrapped, options.take("tls")?.as_deref()) {
        (true, Some(_)) => return Err(invalid(scheme, "always uses TLS; remove tls=")),
        (true, None) => SmtpTls::Implicit,
        (false, None | Some("starttls")) => SmtpTls::Starttls,
        (false, Some("none")) => SmtpTls::None,
        (false, Some(_)) => return Err(invalid(scheme, "tls= must be starttls or none")),
    };
    let port = url.port.unwrap_or(match tls {
        SmtpTls::Starttls => 587,
        SmtpTls::Implicit => 465,
        SmtpTls::None => 25,
    });
    let credentials = url.credentials(scheme)?;
    if tls == SmtpTls::None && credentials.is_some() && !is_loopback(&url.host) {
        return Err(invalid(
            scheme,
            "refuses to send a password without TLS: drop tls=none, or relay through this machine",
        ));
    }

    let name = options.take("name")?;
    let from = options.take("from")?;
    let to: Vec<String> = options
        .take_all("to")
        .iter()
        .flat_map(|value| split_list(value))
        .collect();
    options.finish("to, from, name, tls")?;

    let from = match (from, &credentials) {
        (Some(from), _) => mailbox(scheme, "from=", &from, name.as_deref())?,
        (None, Some((user, _))) if Address::from_str(user).is_ok() => {
            mailbox(scheme, "the user name", user, name.as_deref())?
        }
        (None, _) => {
            return Err(invalid(
                scheme,
                "needs from=you@example.com (the user name is not an email address)",
            ));
        }
    };
    let to = if to.is_empty() {
        vec![from.clone()]
    } else {
        to.iter()
            .map(|address| mailbox(scheme, "to=", address, None))
            .collect::<Result<_, _>>()?
    };
    Ok(Email {
        tls,
        host: url.host,
        port,
        credentials,
        from,
        to,
    })
}

/// Parse `a@b.c` or `Name <a@b.c>`; a given display `name` replaces the one in the text.
fn mailbox(
    scheme: &'static str,
    what: &str,
    text: &str,
    name: Option<&str>,
) -> Result<Mailbox, ParseError> {
    let parsed = Mailbox::from_str(text)
        .map_err(|_| invalid(scheme, format!("{what} is not a valid email address")))?;
    Ok(match name {
        Some(name) => Mailbox::new(Some(name.to_owned()), parsed.email),
        None => parsed,
    })
}

impl Email {
    /// The URL with the password replaced by `***`.
    pub(super) fn redacted(&self) -> String {
        let scheme = if self.tls == SmtpTls::Implicit {
            "mailtos"
        } else {
            "mailto"
        };
        let login = match &self.credentials {
            Some((user, _)) => format!("{user}:***@"),
            None => String::new(),
        };
        let to: Vec<String> = self.to.iter().map(|m| m.email.to_string()).collect();
        let tls = if self.tls == SmtpTls::None {
            "&tls=none"
        } else {
            ""
        };
        format!(
            "{scheme}://{login}{}?to={}{tls}",
            authority_of(&self.host, Some(self.port)),
            to.join(",")
        )
    }
}
