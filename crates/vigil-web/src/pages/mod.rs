//! The pages of the UI, one module each.

mod errors;
mod home;
mod login;

pub(crate) use errors::{Refusal, forbidden, not_found};
pub(crate) use home::home;
pub(crate) use login::{login_form, login_submit, logout};
