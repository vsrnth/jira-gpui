//! Immutable cache/synchronization specifications for live-workspace partitions.
//!
//! The builders deliberately keep the primary and team policies separate.  A
//! completed spec is the only input used to build a sync request and reload a
//! cache, so a state change between those two steps cannot mix partitions.

use jira_application::SyncRequest;
use jira_domain::{AccountId, UserSetId};

use super::{ScopeState, TEAM_STATUS_SCOPE, TeamState};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PartitionKind {
    Primary,
    Team,
}

/// The immutable policy and identity of one live-workspace cache partition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PartitionSpec {
    kind: PartitionKind,
    user_set_id: UserSetId,
    assignees: Option<Vec<AccountId>>,
    watchers: Option<Vec<AccountId>>,
    jql_scope: Option<String>,
    required_event_user_set_id: UserSetId,
    notification_assignees: Option<Vec<AccountId>>,
    empty_team_noop: bool,
}

pub(super) struct PrimaryPartitionSpecBuilder;

impl PrimaryPartitionSpecBuilder {
    pub(super) fn build(
        scope_state: &ScopeState,
        authenticated_account: Option<&AccountId>,
    ) -> PartitionSpec {
        let account = authenticated_account.cloned();
        PartitionSpec {
            kind: PartitionKind::Primary,
            user_set_id: scope_state.user_set_id.clone(),
            // Jira's primary view is the union of assigned and watched issues.
            assignees: account.clone().map(|account| vec![account]),
            watchers: authenticated_account.cloned().map(|account| vec![account]),
            jql_scope: Some(scope_state.normalized_scope.clone()),
            required_event_user_set_id: scope_state.user_set_id.clone(),
            notification_assignees: authenticated_account.cloned().map(|account| vec![account]),
            empty_team_noop: false,
        }
    }
}

pub(super) struct TeamPartitionSpecBuilder;

impl TeamPartitionSpecBuilder {
    pub(super) fn build(
        team_state: &TeamState,
        authenticated_account: Option<&AccountId>,
    ) -> PartitionSpec {
        PartitionSpec {
            kind: PartitionKind::Team,
            user_set_id: team_state.user_set_id.clone(),
            assignees: Some(team_state.members.clone()),
            watchers: None,
            jql_scope: Some(TEAM_STATUS_SCOPE.to_owned()),
            required_event_user_set_id: team_state.user_set_id.clone(),
            // Team membership controls fetching, but notifications remain account-scoped.
            notification_assignees: authenticated_account.cloned().map(|account| vec![account]),
            empty_team_noop: team_state.members.is_empty(),
        }
    }
}

impl PartitionSpec {
    #[cfg(test)]
    pub(super) fn kind(&self) -> PartitionKind {
        self.kind
    }

    pub(super) fn user_set_id(&self) -> &UserSetId {
        &self.user_set_id
    }

    pub(super) fn required_event_user_set_id(&self) -> &UserSetId {
        &self.required_event_user_set_id
    }

    pub(super) fn is_empty_team_noop(&self) -> bool {
        matches!(self.kind, PartitionKind::Team) && self.empty_team_noop
    }

    pub(super) fn as_sync_request(
        &self,
        site_id: &jira_domain::JiraSiteId,
        mode: jira_application::SyncMode,
    ) -> SyncRequest {
        SyncRequest {
            site_id: site_id.clone(),
            user_set_id: self.user_set_id.clone(),
            assignees: self.assignees.clone(),
            watchers: self.watchers.clone(),
            jql_scope: self.jql_scope.clone(),
            notification_assignees: self.notification_assignees.clone(),
            mode,
        }
    }

    #[cfg(test)]
    pub(super) fn fields(
        &self,
    ) -> (
        PartitionKind,
        &UserSetId,
        &Option<Vec<AccountId>>,
        &Option<Vec<AccountId>>,
        &Option<String>,
        &UserSetId,
        &Option<Vec<AccountId>>,
        bool,
    ) {
        (
            self.kind,
            &self.user_set_id,
            &self.assignees,
            &self.watchers,
            &self.jql_scope,
            &self.required_event_user_set_id,
            &self.notification_assignees,
            self.empty_team_noop,
        )
    }
}

#[cfg(test)]
mod tests {
    use jira_domain::AccountId;

    use super::*;

    fn account(value: &str) -> AccountId {
        AccountId::new(value).expect("valid account")
    }

    fn user_set(value: &str) -> UserSetId {
        UserSetId::new(value).expect("valid user set")
    }

    #[test]
    fn primary_specs_capture_account_and_project_wide_policies_exactly() {
        let scope = ScopeState {
            user_set_id: user_set("primary-user-set"),
            normalized_scope: "project = APP".to_owned(),
        };
        let account_id = account("alice");
        let authenticated = PrimaryPartitionSpecBuilder::build(&scope, Some(&account_id));
        let project_wide = PrimaryPartitionSpecBuilder::build(&scope, None);

        assert_eq!(
            authenticated.fields(),
            (
                PartitionKind::Primary,
                &user_set("primary-user-set"),
                &Some(vec![account("alice")]),
                &Some(vec![account("alice")]),
                &Some("project = APP".to_owned()),
                &user_set("primary-user-set"),
                &Some(vec![account("alice")]),
                false,
            )
        );
        assert_eq!(
            project_wide.fields(),
            (
                PartitionKind::Primary,
                &user_set("primary-user-set"),
                &None,
                &None,
                &Some("project = APP".to_owned()),
                &user_set("primary-user-set"),
                &None,
                false,
            )
        );

        let request = authenticated.as_sync_request(
            &jira_domain::JiraSiteId::new("site").expect("valid site"),
            jira_application::SyncMode::Reconciliation,
        );
        assert_eq!(request.user_set_id, user_set("primary-user-set"));
        assert_eq!(request.assignees, Some(vec![account("alice")]));
        assert_eq!(request.watchers, Some(vec![account("alice")]));
        assert_eq!(request.jql_scope, Some("project = APP".to_owned()));
        assert_eq!(request.notification_assignees, Some(vec![account("alice")]));
        assert_eq!(request.mode, jira_application::SyncMode::Reconciliation);
    }

    #[test]
    fn team_specs_capture_membership_notification_and_empty_noop_policies() {
        let authenticated_account = account("alice");
        let nonempty = TeamState {
            user_set_id: user_set("team-user-set"),
            members: vec![account("bob"), account("carol")],
        };
        let empty = TeamState {
            user_set_id: user_set("empty-team-user-set"),
            members: Vec::new(),
        };

        let team = TeamPartitionSpecBuilder::build(&nonempty, Some(&authenticated_account));
        let team_without_account = TeamPartitionSpecBuilder::build(&nonempty, None);
        let empty_team = TeamPartitionSpecBuilder::build(&empty, Some(&authenticated_account));

        assert_eq!(
            team.fields(),
            (
                PartitionKind::Team,
                &user_set("team-user-set"),
                &Some(vec![account("bob"), account("carol")]),
                &None,
                &Some("statusCategory = \"In Progress\"".to_owned()),
                &user_set("team-user-set"),
                &Some(vec![account("alice")]),
                false,
            )
        );
        let request = team.as_sync_request(
            &jira_domain::JiraSiteId::new("site").expect("valid site"),
            jira_application::SyncMode::Baseline,
        );
        assert_eq!(
            request.assignees,
            Some(vec![account("bob"), account("carol")])
        );
        assert_eq!(request.watchers, None);
        assert_eq!(
            request.jql_scope,
            Some("statusCategory = \"In Progress\"".to_owned())
        );
        assert_eq!(request.notification_assignees, Some(vec![account("alice")]));
        assert_eq!(request.mode, jira_application::SyncMode::Baseline);
        assert_eq!(
            team_without_account.fields(),
            (
                PartitionKind::Team,
                &user_set("team-user-set"),
                &Some(vec![account("bob"), account("carol")]),
                &None,
                &Some("statusCategory = \"In Progress\"".to_owned()),
                &user_set("team-user-set"),
                &None,
                false,
            )
        );
        assert_eq!(empty_team.kind(), PartitionKind::Team);
        assert!(empty_team.is_empty_team_noop());
        assert_eq!(empty_team.user_set_id(), &user_set("empty-team-user-set"));
        assert_eq!(
            empty_team.required_event_user_set_id(),
            &user_set("empty-team-user-set")
        );
    }
}
