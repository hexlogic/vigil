//! The pages of the UI, one module each.

mod detail;
mod diff;
mod edit;
mod errors;
mod history;
mod login;
mod preview;
mod watches;

pub(crate) use detail::show;
pub(crate) use diff::diff;
pub(crate) use edit::{create, edit_form, new_form, steps, update};
pub(crate) use errors::{Refusal, forbidden, not_found, server_error};
pub(crate) use history::{history, snapshot};
pub(crate) use login::{login_form, login_submit, logout};
pub(crate) use preview::preview;
pub(crate) use watches::{
    check_watch, confirm_delete, delete_watch, parse_id, pause_watch, resume_watch, watch_row,
    watches,
};

/// The colours a tag can have: the suffixes of the `tag-*` classes in `vigil.css`. A palette,
/// not a free choice, because the page policy forbids `style=` attributes.
pub(crate) const TAG_COLORS: [&str; 9] = [
    "gray", "red", "orange", "yellow", "green", "teal", "blue", "purple", "pink",
];

/// The class suffix for a stored colour; gray for one that is not in the palette.
pub(crate) fn tag_class(color: &str) -> &'static str {
    TAG_COLORS
        .iter()
        .copied()
        .find(|known| *known == color)
        .unwrap_or("gray")
}
