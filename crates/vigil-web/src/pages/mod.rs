//! The pages of the UI, one module each.

mod detail;
mod diff;
mod edit;
mod errors;
mod history;
mod login;
mod preview;
mod settings;
mod tag_form;
mod tags;
mod target_form;
mod targets;
mod watches;

pub(crate) use detail::show;
pub(crate) use diff::diff;
pub(crate) use edit::{create, edit_form, new_form, steps, update};
pub(crate) use errors::{Refusal, forbidden, not_found, server_error};
pub(crate) use history::{history, snapshot};
pub(crate) use login::{login_form, login_submit, logout};
pub(crate) use preview::preview;
pub(crate) use settings::{set_language, set_password, settings, sign_out_everywhere};
pub(crate) use tag_form::{
    create as create_tag, edit_form as edit_tag_form, new_form as new_tag_form,
    update as update_tag,
};
pub(crate) use tags::{confirm_delete_tag, delete_tag, tag_row, tags};
pub(crate) use target_form::{
    create as create_target, edit_form as edit_target_form, new_form as new_target_form,
    update as update_target,
};
pub(crate) use targets::{
    confirm_delete_target, delete_target, disable_target, enable_target, target_row, targets,
    test_target,
};
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{I18n, Lang};

    #[test]
    fn every_colour_of_the_palette_has_a_name_in_every_language() {
        let i18n = I18n::load().expect("locales parse");
        for lang in Lang::ALL {
            for color in TAG_COLORS {
                let key = format!("tags.color.{color}");
                assert!(
                    i18n.get(lang, &key).is_some(),
                    "{} lacks {key}",
                    lang.code()
                );
            }
        }
    }

    #[test]
    fn a_colour_outside_the_palette_is_gray() {
        assert_eq!(tag_class("red"), "red");
        assert_eq!(tag_class("url(javascript:x)"), "gray");
        assert_eq!(tag_class(""), "gray");
    }
}
