//! Signing in: the admin password, sessions, and the checks that keep other sites from acting
//! on a signed-in browser's behalf.

mod limiter;
mod middleware;
mod password;
pub(crate) mod session;

pub(crate) use limiter::LoginLimiter;
pub(crate) use middleware::{
    SessionKey, check_origin, current_session, login_url, require_session, safe_next,
};
pub(crate) use password::verify_admin_password;
pub use password::{AuthError, PasswordSource, change_password, ensure_admin_password};
