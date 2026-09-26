// handlers/auth/mod.rs — re-exports the public surface of handlers::auth

mod handlers;
pub(crate) mod helpers;
pub(crate) mod password_links;
mod session;

pub use handlers::{
    Rejection, auth_status, change_password, list_sessions, login, logout, me, password_link_check,
    password_link_complete, revoke_all_sessions, revoke_session, session_responses, setup,
    setup_check, totp_confirm, totp_disable, totp_setup, totp_verify, verify_step_up,
};
pub use session::{
    MFA_PENDING_COOKIE, SESSION_COOKIE, extract_cookie, extract_session_token, validate_session,
    validate_session_with_id,
};
