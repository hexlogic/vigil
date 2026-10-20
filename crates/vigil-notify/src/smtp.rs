//! Email over SMTP.

use std::time::Duration;

use lettre::message::{Message, header::ContentType};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

use crate::channel::{Email, SmtpTls};
use crate::error::SendError;
use crate::message::Prepared;

/// Send `prepared` as a plain-text email, making one connection.
///
/// Rejections from the server (5xx, or no STARTTLS where it is required) are permanent;
/// network problems, timeouts and 4xx replies are worth another try.
pub(crate) async fn send_email(
    email: &Email,
    prepared: &Prepared,
    timeout: Duration,
) -> Result<(), SendError> {
    let mut message = Message::builder()
        .from(email.from.clone())
        .subject(prepared.title.clone())
        .header(ContentType::TEXT_PLAIN);
    for to in &email.to {
        message = message.to(to.clone());
    }
    let message = message
        .body(prepared.body.clone())
        .map_err(|_| SendError::permanent("the email could not be built"))?;

    let tls = || {
        TlsParameters::new(email.host.clone())
            .map_err(|_| SendError::permanent("could not set up TLS for the mail server"))
    };
    let tls = match email.tls {
        SmtpTls::Starttls => Tls::Required(tls()?),
        SmtpTls::Implicit => Tls::Wrapper(tls()?),
        SmtpTls::None => Tls::None,
    };
    // `builder_dangerous` only means "no TLS policy by default"; the policy is set right here.
    let mut transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&email.host)
        .port(email.port)
        .timeout(Some(timeout))
        .tls(tls);
    if let Some((user, password)) = &email.credentials {
        transport =
            transport.credentials(Credentials::new(user.clone(), password.expose().to_owned()));
    }

    transport
        .build()
        .send(message)
        .await
        .map(|_| ())
        .map_err(|error| {
            let mut text = error.to_string();
            if let Some((_, password)) = &email.credentials {
                text = text.replace(password.expose(), "***");
            }
            if error.is_permanent() || error.is_client() || error.is_tls() {
                SendError::permanent(text)
            } else {
                SendError::transient(text)
            }
        })
}
