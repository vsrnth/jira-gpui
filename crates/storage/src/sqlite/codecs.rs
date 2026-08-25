use jira_application::{ApplicationError, ErrorKind};
use jira_domain::{NotificationDelivery, Timestamp, UpdateKind, UpdateReadState};
use rusqlite::types::Value;
use time::{OffsetDateTime, UtcOffset};

use super::storage_error;

pub(super) fn encode<T: serde::Serialize>(
    value: &T,
    what: &str,
) -> Result<String, ApplicationError> {
    serde_json::to_string(value).map_err(|_| storage_error(format!("could not encode {what}")))
}

pub(super) fn decode<T: serde::de::DeserializeOwned>(
    value: &str,
    what: &str,
) -> Result<T, ApplicationError> {
    serde_json::from_str(value)
        .map_err(|_| storage_error(format!("could not decode stored {what}")))
}

pub(super) fn stamp(timestamp: Timestamp) -> (i64, i32) {
    (timestamp.unix_timestamp(), timestamp.nanosecond() as i32)
}

pub(super) fn from_stamp(seconds: i64, nanos: i32) -> Result<Timestamp, ApplicationError> {
    OffsetDateTime::from_unix_timestamp(seconds)
        .map_err(|_| storage_error("stored timestamp is invalid"))?
        .replace_nanosecond(
            u32::try_from(nanos).map_err(|_| storage_error("stored timestamp is invalid"))?,
        )
        .map(|timestamp| timestamp.to_offset(UtcOffset::UTC))
        .map_err(|_| storage_error("stored timestamp is invalid"))
}

pub(super) fn optional_timestamp(
    seconds: Option<i64>,
    nanos: Option<i32>,
) -> Result<Option<Timestamp>, ApplicationError> {
    match (seconds, nanos) {
        (None, None) => Ok(None),
        (Some(seconds), Some(nanos)) => from_stamp(seconds, nanos).map(Some),
        _ => Err(storage_error("stored sync state is invalid")),
    }
}

pub(super) fn text(value: &str) -> Value {
    Value::Text(value.to_owned())
}

pub(super) fn escape_like(value: String) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

pub(super) fn kind_tag(kind: &UpdateKind) -> i64 {
    match kind {
        UpdateKind::IssueAddedToView => 0,
        UpdateKind::IssueRemovedFromView => 1,
        UpdateKind::IssueUpdated => 9,
        UpdateKind::FieldChanged { .. } => 10,
        UpdateKind::StatusChanged { .. } => 2,
        UpdateKind::AssigneeChanged { .. } => 3,
        UpdateKind::PriorityChanged { .. } => 4,
        UpdateKind::DueDateChanged { .. } => 5,
        UpdateKind::SummaryChanged { .. } => 6,
        UpdateKind::ParentChanged { .. } => 7,
        UpdateKind::CommentAdded { .. } => 8,
    }
}

pub(super) fn read_state_tag(state: UpdateReadState) -> i64 {
    i64::from(matches!(state, UpdateReadState::Read))
}

pub(super) fn read_state_from_i64(value: i64) -> Result<UpdateReadState, ApplicationError> {
    match value {
        0 => Ok(UpdateReadState::Unread),
        1 => Ok(UpdateReadState::Read),
        _ => Err(storage_error("stored update event is invalid")),
    }
}

pub(super) fn delivery_tag(delivery: NotificationDelivery) -> i64 {
    match delivery {
        NotificationDelivery::NotAttempted => 0,
        NotificationDelivery::Delivered => 1,
        NotificationDelivery::Unavailable => 2,
        NotificationDelivery::SuppressedByPolicy => 3,
    }
}

pub(super) fn delivery_from_i64(value: i64) -> Result<NotificationDelivery, ApplicationError> {
    match value {
        0 => Ok(NotificationDelivery::NotAttempted),
        1 => Ok(NotificationDelivery::Delivered),
        2 => Ok(NotificationDelivery::Unavailable),
        3 => Ok(NotificationDelivery::SuppressedByPolicy),
        _ => Err(storage_error("stored update event is invalid")),
    }
}
pub(super) fn error_kind_tag(kind: ErrorKind) -> i64 {
    match kind {
        ErrorKind::Authentication => 0,
        ErrorKind::Authorization => 1,
        ErrorKind::RateLimited => 2,
        ErrorKind::Offline => 3,
        ErrorKind::Cancelled => 4,
        ErrorKind::InvalidInput => 5,
        ErrorKind::NotFound => 6,
        // Comment-write outcomes are never sync-state categories. Keep this
        // defensive fallback on the existing Upstream tag for old databases.
        ErrorKind::UnknownOutcome => 8,
        ErrorKind::Storage => 7,
        ErrorKind::Upstream => 8,
        ErrorKind::Notification => 9,
        ErrorKind::Internal => 10,
    }
}

pub(super) fn error_kind_from_i32(value: i32) -> Result<ErrorKind, ApplicationError> {
    match value {
        0 => Ok(ErrorKind::Authentication),
        1 => Ok(ErrorKind::Authorization),
        2 => Ok(ErrorKind::RateLimited),
        3 => Ok(ErrorKind::Offline),
        4 => Ok(ErrorKind::Cancelled),
        5 => Ok(ErrorKind::InvalidInput),
        6 => Ok(ErrorKind::NotFound),
        7 => Ok(ErrorKind::Storage),
        8 => Ok(ErrorKind::Upstream),
        9 => Ok(ErrorKind::Notification),
        10 => Ok(ErrorKind::Internal),
        _ => Err(storage_error("stored sync state is invalid")),
    }
}
