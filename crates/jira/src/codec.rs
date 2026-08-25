//! JSON codecs for Jira operations that are shared by transports.
//!
//! The HTTP crate owns request dispatch, response limits, and error policy. This module owns the
//! Jira-specific JSON shapes and the conversion of transition responses into application values.

use jira_application::IssueTransition;
use jira_domain::IssueComment;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{IssueMapper, JiraComment};

/// A malformed or semantically invalid Jira operation payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum JiraCodecError {
    #[error("Jira JSON is malformed")]
    MalformedJson,
    #[error("Jira JSON contains invalid operation data")]
    InvalidData,
}

/// Encodes Jira's comment-create body as plain text in a minimal ADF document.
pub fn comment_create_request_body(text: &str) -> Value {
    json!({
        "body": {
            "type": "doc",
            "version": 1,
            "content": [{
                "type": "paragraph",
                "content": [{
                    "type": "text",
                    "text": text,
                }]
            }]
        }
    })
}

/// Encodes Jira's assignee body. `null` explicitly clears the assignee and must not be omitted.
pub fn assignee_request_body(account_id: Option<&str>) -> Value {
    json!({"accountId": account_id})
}

/// Encodes Jira's transition body for a transition ID already validated by the application port.
pub fn transition_request_body(transition_id: &str) -> Value {
    json!({"transition": {"id": transition_id}})
}

/// Decodes and maps Jira's transition response without exposing its transport DTOs.
pub fn decode_transitions_response(body: &[u8]) -> Result<Vec<IssueTransition>, JiraCodecError> {
    let payload: JiraTransitionsResponse =
        serde_json::from_slice(body).map_err(|_| JiraCodecError::MalformedJson)?;
    payload
        .transitions
        .into_iter()
        .map(map_transition)
        .collect()
}

/// Decodes and maps a comment returned by Jira after creation.
pub fn decode_created_comment_response(body: &[u8]) -> Result<IssueComment, JiraCodecError> {
    let comment: JiraComment =
        serde_json::from_slice(body).map_err(|_| JiraCodecError::MalformedJson)?;
    IssueMapper
        .map_comment(comment)
        .map_err(|_| JiraCodecError::InvalidData)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JiraTransitionsResponse {
    transitions: Vec<JiraTransitionResponse>,
}

#[derive(Debug, Deserialize)]
struct JiraTransitionResponse {
    id: String,
    name: String,
    to: JiraTransitionStatusResponse,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JiraTransitionStatusResponse {
    id: String,
    name: String,
    #[serde(default)]
    status_category: Option<JiraStatusCategoryResponse>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JiraStatusCategoryResponse {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    key: Option<String>,
}

fn map_transition(transition: JiraTransitionResponse) -> Result<IssueTransition, JiraCodecError> {
    validate_string_id(&transition.id)
        .then_some(())
        .ok_or(JiraCodecError::InvalidData)?;
    validate_string_id(&transition.name)
        .then_some(())
        .ok_or(JiraCodecError::InvalidData)?;
    validate_string_id(&transition.to.id)
        .then_some(())
        .ok_or(JiraCodecError::InvalidData)?;
    validate_string_id(&transition.to.name)
        .then_some(())
        .ok_or(JiraCodecError::InvalidData)?;

    let category = transition.to.status_category.and_then(|category| {
        category
            .name
            .filter(|value| !value.trim().is_empty())
            .or(category.key.filter(|value| !value.trim().is_empty()))
    });
    if category
        .as_ref()
        .is_some_and(|value| value.chars().count() > 255 || value.chars().any(char::is_control))
    {
        return Err(JiraCodecError::InvalidData);
    }

    Ok(IssueTransition {
        id: transition.id,
        name: transition.name,
        to: jira_domain::Status {
            id: transition.to.id,
            name: transition.to.name,
            category,
        },
    })
}

fn validate_string_id(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 255 && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_create_codec_preserves_plain_text_and_exact_adf_shape() {
        let body = comment_create_request_body("<b>hello & goodbye</b>\nsecond \"line\"");
        assert_eq!(
            body,
            json!({
                "body": {
                    "type": "doc",
                    "version": 1,
                    "content": [{
                        "type": "paragraph",
                        "content": [{
                            "type": "text",
                            "text": "<b>hello & goodbye</b>\nsecond \"line\"",
                        }]
                    }]
                }
            })
        );
        let serialized = serde_json::to_string(&body).expect("serialize ADF");
        assert!(serialized.contains("\\\"line\\\""));
        assert!(!serialized.contains("visibility"));
        assert!(!serialized.contains("properties"));
    }

    #[test]
    fn edit_write_codecs_preserve_account_id_null_and_transition_shape() {
        assert_eq!(
            assignee_request_body(Some("557058:abc-123")),
            json!({"accountId": "557058:abc-123"})
        );
        assert_eq!(assignee_request_body(None), json!({"accountId": null}));
        assert_eq!(
            transition_request_body("31"),
            json!({"transition": {"id": "31"}})
        );
    }

    #[test]
    fn created_comment_codec_decodes_adf_into_the_public_domain_comment() {
        let body = br#"{
            "id": "20001",
            "author": {"accountId": "557058:commenter", "displayName": "Asha", "active": true},
            "body": {"type":"doc","version":1,"content":[{"type":"paragraph","content":[{"type":"text","text":"Created & approved"}]}]},
            "created": "2026-08-16T10:00:00.000+0000"
        }"#;
        let comment = decode_created_comment_response(body).expect("created comment");
        assert_eq!(comment.id.as_str(), "20001");
        assert_eq!(comment.body, "Created & approved");
        assert_eq!(
            comment
                .author
                .as_ref()
                .and_then(|author| author.display_name.as_deref()),
            Some("Asha")
        );
    }

    #[test]
    fn transition_response_codec_maps_category_name_before_key() {
        let body = br#"{
            "transitions": [{
                "id": "31",
                "name": "In progress",
                "to": {
                    "id": "3",
                    "name": "In Progress",
                    "statusCategory": {"name": "In Progress", "key": "indeterminate"}
                }
            }, {
                "id": "41",
                "name": "Done",
                "to": {
                    "id": "100",
                    "name": "Done",
                    "statusCategory": {"key": "done"}
                }
            }]
        }"#;
        let transitions = decode_transitions_response(body).expect("valid transitions");
        assert_eq!(transitions[0].to.category.as_deref(), Some("In Progress"));
        assert_eq!(transitions[1].to.category.as_deref(), Some("done"));
    }

    #[test]
    fn transition_response_codec_rejects_invalid_values_without_leaking_payload() {
        let invalid = br#"{"transitions":[{"id":"","name":"Done","to":{"id":"3","name":"Done"}}]}"#;
        assert_eq!(
            decode_transitions_response(invalid),
            Err(JiraCodecError::InvalidData)
        );
        let invalid_category = br#"{"transitions":[{"id":"31","name":"Done","to":{"id":"3","name":"Done","statusCategory":{"name":"bad\ncategory","key":"done"}}}]}"#;
        assert_eq!(
            decode_transitions_response(invalid_category),
            Err(JiraCodecError::InvalidData)
        );
        assert_eq!(
            decode_transitions_response(br#"{"transitions": [}"#),
            Err(JiraCodecError::MalformedJson)
        );
    }
}
