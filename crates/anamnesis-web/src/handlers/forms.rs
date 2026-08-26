//! Form bodies for every mutating route, plus the one `serde` helper they
//! all need: an HTML number input left blank still submits its field as an
//! empty string, not an absent one, so `Option<u16>` needs a hand-rolled
//! `deserialize_with` to treat `""` as `None` rather than a parse error.

use serde::{Deserialize, Deserializer};

fn empty_string_as_none<'de, D>(deserializer: D) -> Result<Option<u16>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = <Option<String> as Deserialize>::deserialize(deserializer)?;
    match raw.as_deref() {
        None | Some("") => Ok(None),
        Some(value) => value
            .parse::<u16>()
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateBoardForm {
    pub csrf_token: String,
    pub title: String,
}

#[derive(Debug, Deserialize)]
pub struct AddColumnForm {
    pub csrf_token: String,
    pub title: String,
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub wip_limit: Option<u16>,
}

#[derive(Debug, Deserialize)]
pub struct AddCardForm {
    pub csrf_token: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Deserialize)]
pub struct MoveCardForm {
    pub csrf_token: String,
    pub to_column: uuid::Uuid,
    pub to_index: usize,
}

/// Shared by every mutating route that needs nothing but the CSRF token:
/// deleting a board, deleting a card, logging out.
#[derive(Debug, Deserialize)]
pub struct CsrfOnlyForm {
    pub csrf_token: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_wip_limit_parses_as_none() {
        let form: AddColumnForm =
            serde_urlencoded::from_str("csrf_token=tok&title=Todo&wip_limit=").unwrap();
        assert_eq!(form.wip_limit, None);
    }

    #[test]
    fn missing_wip_limit_field_parses_as_none() {
        let form: AddColumnForm = serde_urlencoded::from_str("csrf_token=tok&title=Todo").unwrap();
        assert_eq!(form.wip_limit, None);
    }

    #[test]
    fn numeric_wip_limit_parses() {
        let form: AddColumnForm =
            serde_urlencoded::from_str("csrf_token=tok&title=Todo&wip_limit=3").unwrap();
        assert_eq!(form.wip_limit, Some(3));
    }

    #[test]
    fn non_numeric_wip_limit_is_rejected() {
        let result: Result<AddColumnForm, _> =
            serde_urlencoded::from_str("csrf_token=tok&title=Todo&wip_limit=abc");
        assert!(result.is_err());
    }
}
