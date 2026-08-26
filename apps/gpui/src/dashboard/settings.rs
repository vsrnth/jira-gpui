use std::{future::Future, pin::Pin};

use jira_domain::{AccountId, User};

use crate::{
    live_workspace::{LiveWorkspace, RefreshResult},
    local_data::{
        LocalPreferences, MAX_TEAM_MEMBERS, PersistedTeamMember, normalize_issue_jql_scope,
        normalize_team_members, save_preferences,
    },
    presentation::{ScopeOutcomeKind, TeamInvalidInputKind, TeamOutcomeKind},
};

pub(super) type TransactionFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ()>> + Send + 'a>>;

/// The narrow, presentation-independent operation seam used by both settings transactions.
pub(super) trait SettingsPort: Send + Sync {
    fn set_scope<'a>(&'a self, scope: Option<String>) -> TransactionFuture<'a, ()>;
    fn refresh_scope<'a>(&'a self) -> TransactionFuture<'a, RefreshResult>;
    fn load_cached<'a>(&'a self) -> TransactionFuture<'a, ()>;
    fn search_users<'a>(&'a self, query: String) -> TransactionFuture<'a, Vec<User>>;
    fn configure_team<'a>(&'a self, members: Vec<AccountId>) -> TransactionFuture<'a, ()>;
    fn refresh_team<'a>(&'a self) -> TransactionFuture<'a, RefreshResult>;
}

/// The only preference-write seam used by the transactions.
pub(super) trait PreferencesPort: Send + Sync {
    fn save(&self, preferences: &LocalPreferences) -> Result<(), ()>;
}

pub(super) struct ProductionPreferences;

impl PreferencesPort for ProductionPreferences {
    fn save(&self, preferences: &LocalPreferences) -> Result<(), ()> {
        save_preferences(preferences).map_err(|_| ())
    }
}

impl SettingsPort for LiveWorkspace {
    fn set_scope<'a>(&'a self, scope: Option<String>) -> TransactionFuture<'a, ()> {
        Box::pin(async move { self.set_jql_scope(scope).await.map_err(|_| ()) })
    }

    fn refresh_scope<'a>(&'a self) -> TransactionFuture<'a, RefreshResult> {
        Box::pin(async move {
            self.refresh(&jira_application::CancellationToken::new())
                .await
                .map_err(|_| ())
        })
    }

    fn load_cached<'a>(&'a self) -> TransactionFuture<'a, ()> {
        Box::pin(async move {
            self.load_cached_for_authenticated_account()
                .await
                .map(|_| ())
                .map_err(|_| ())
        })
    }

    fn search_users<'a>(&'a self, query: String) -> TransactionFuture<'a, Vec<User>> {
        Box::pin(async move {
            self.search_users(query, 5, &jira_application::CancellationToken::new())
                .await
                .map_err(|_| ())
        })
    }

    fn configure_team<'a>(&'a self, members: Vec<AccountId>) -> TransactionFuture<'a, ()> {
        Box::pin(async move { self.configure_team_members(members).await.map_err(|_| ()) })
    }

    fn refresh_team<'a>(&'a self) -> TransactionFuture<'a, RefreshResult> {
        Box::pin(async move {
            self.refresh_team(&jira_application::CancellationToken::new())
                .await
                .map_err(|_| ())
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ScopeFailureCause {
    Refresh,
    PreferenceSave,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ScopeUnchangedFailure {
    Invalid,
    Preparation,
}

#[derive(Debug)]
pub(super) enum ScopeSaveFailure {
    Unchanged(ScopeUnchangedFailure),
    Restored(ScopeFailureCause),
    RestoreFailed(ScopeFailureCause),
}

#[derive(Debug)]
pub(super) enum ScopeSaveResult {
    Saved {
        refreshed: RefreshResult,
        normalized: Option<String>,
    },
    Failed(ScopeSaveFailure),
}

pub(super) async fn run_scope_transaction<P, Prefs>(
    port: &P,
    preferences: &Prefs,
    entered: String,
    previous_scope: Option<String>,
    team_members: Vec<PersistedTeamMember>,
) -> ScopeSaveResult
where
    P: SettingsPort,
    Prefs: PreferencesPort,
{
    let normalized = match normalize_issue_jql_scope(Some(entered.clone())) {
        Ok(normalized) => normalized,
        Err(_) => {
            return ScopeSaveResult::Failed(ScopeSaveFailure::Unchanged(
                ScopeUnchangedFailure::Invalid,
            ));
        }
    };

    if port.set_scope(normalized.clone()).await.is_err() {
        return ScopeSaveResult::Failed(ScopeSaveFailure::Unchanged(
            ScopeUnchangedFailure::Preparation,
        ));
    }

    let refreshed = match port.refresh_scope().await {
        Ok(refreshed) => refreshed,
        Err(_) => {
            return restore_scope(port, previous_scope, ScopeFailureCause::Refresh).await;
        }
    };

    if preferences
        .save(&LocalPreferences {
            issue_jql_scope: normalized.clone(),
            team_members,
        })
        .is_err()
    {
        return restore_scope(port, previous_scope, ScopeFailureCause::PreferenceSave).await;
    }

    ScopeSaveResult::Saved {
        refreshed,
        normalized,
    }
}

async fn restore_scope<P: SettingsPort>(
    port: &P,
    previous_scope: Option<String>,
    cause: ScopeFailureCause,
) -> ScopeSaveResult {
    if port.set_scope(previous_scope).await.is_err() {
        return ScopeSaveResult::Failed(ScopeSaveFailure::RestoreFailed(cause));
    }
    // This is deliberately best effort and happens only after the old scope is active again.
    let _ = port.load_cached().await;
    ScopeSaveResult::Failed(ScopeSaveFailure::Restored(cause))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TeamFailureCause {
    Refresh,
    PreferenceSave,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TeamUnchangedFailure {
    InvalidInput(TeamInvalidInputKind),
    Search,
    EmailNotFound,
    EmailAmbiguous,
    Normalization,
    Preparation,
}

#[derive(Debug)]
pub(super) enum TeamSaveFailure {
    Unchanged(TeamUnchangedFailure),
    Restored(TeamFailureCause),
    RestoreFailed(TeamFailureCause),
}

#[derive(Debug)]
pub(super) enum TeamSaveResult {
    Saved {
        members: Vec<PersistedTeamMember>,
        refreshed: RefreshResult,
    },
    Failed(TeamSaveFailure),
}

pub(super) async fn run_team_transaction<P, Prefs>(
    port: &P,
    preferences: &Prefs,
    entered: String,
    previous_accounts: Vec<AccountId>,
    issue_jql_scope: Option<String>,
) -> TeamSaveResult
where
    P: SettingsPort,
    Prefs: PreferencesPort,
{
    let identifiers = match team_identifier_lines(&entered) {
        Ok(identifiers) => identifiers,
        Err(message) => {
            return TeamSaveResult::Failed(TeamSaveFailure::Unchanged(
                TeamUnchangedFailure::InvalidInput(message),
            ));
        }
    };

    let mut resolved = Vec::new();
    for identifier in identifiers {
        if identifier.contains('@') {
            let users = match port.search_users(identifier.clone()).await {
                Ok(users) => users
                    .into_iter()
                    .filter(|user| user.active)
                    .collect::<Vec<_>>(),
                Err(_) => {
                    return TeamSaveResult::Failed(TeamSaveFailure::Unchanged(
                        TeamUnchangedFailure::Search,
                    ));
                }
            };
            match users.as_slice() {
                [] => {
                    return TeamSaveResult::Failed(TeamSaveFailure::Unchanged(
                        TeamUnchangedFailure::EmailNotFound,
                    ));
                }
                [_] => {
                    let user = &users[0];
                    resolved.push(PersistedTeamMember {
                        identifier,
                        account_id: user.account_id.to_string(),
                        display_name: user.display_name.clone(),
                    });
                }
                _ => {
                    return TeamSaveResult::Failed(TeamSaveFailure::Unchanged(
                        TeamUnchangedFailure::EmailAmbiguous,
                    ));
                }
            }
        } else {
            match persisted_direct_team_member(identifier) {
                Ok(member) => resolved.push(member),
                Err(message) => {
                    return TeamSaveResult::Failed(TeamSaveFailure::Unchanged(
                        TeamUnchangedFailure::InvalidInput(message),
                    ));
                }
            }
        }
    }

    let normalized = match normalize_team_members(resolved) {
        Ok(normalized) => normalized,
        Err(_) => {
            return TeamSaveResult::Failed(TeamSaveFailure::Unchanged(
                TeamUnchangedFailure::Normalization,
            ));
        }
    };
    let accounts = normalized
        .iter()
        .filter_map(|member| AccountId::new(member.account_id.clone()).ok())
        .collect::<Vec<_>>();

    if port.configure_team(accounts).await.is_err() {
        return TeamSaveResult::Failed(TeamSaveFailure::Unchanged(
            TeamUnchangedFailure::Preparation,
        ));
    }

    let refreshed = match port.refresh_team().await {
        Ok(refreshed) => refreshed,
        Err(_) => {
            return restore_team(port, previous_accounts, TeamFailureCause::Refresh).await;
        }
    };

    if preferences
        .save(&LocalPreferences {
            issue_jql_scope,
            team_members: normalized.clone(),
        })
        .is_err()
    {
        return restore_team(port, previous_accounts, TeamFailureCause::PreferenceSave).await;
    }

    TeamSaveResult::Saved {
        members: normalized,
        refreshed,
    }
}

async fn restore_team<P: SettingsPort>(
    port: &P,
    previous_accounts: Vec<AccountId>,
    cause: TeamFailureCause,
) -> TeamSaveResult {
    if port.configure_team(previous_accounts).await.is_err() {
        return TeamSaveResult::Failed(TeamSaveFailure::RestoreFailed(cause));
    }
    TeamSaveResult::Failed(TeamSaveFailure::Restored(cause))
}

pub(super) fn team_identifier_lines(value: &str) -> Result<Vec<String>, TeamInvalidInputKind> {
    let lines = value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if lines.len() > MAX_TEAM_MEMBERS {
        return Err(TeamInvalidInputKind::TooManyMembers);
    }
    for line in &lines {
        if line.chars().any(char::is_control) || line.len() > 320 {
            return Err(TeamInvalidInputKind::InvalidEntry);
        }
        if !line.contains('@') {
            let account =
                AccountId::new(line.clone()).map_err(|_| TeamInvalidInputKind::InvalidAccount)?;
            if account
                .as_str()
                .chars()
                .any(|character| matches!(character, '"' | '\\'))
            {
                return Err(TeamInvalidInputKind::UnsafeAccount);
            }
        }
    }
    Ok(lines)
}

pub(super) fn persisted_direct_team_member(
    identifier: String,
) -> Result<PersistedTeamMember, TeamInvalidInputKind> {
    let account_id =
        AccountId::new(identifier.clone()).map_err(|_| TeamInvalidInputKind::InvalidAccount)?;
    if account_id
        .as_str()
        .chars()
        .any(|character| matches!(character, '"' | '\\'))
    {
        return Err(TeamInvalidInputKind::UnsafeAccount);
    }
    Ok(PersistedTeamMember {
        identifier,
        account_id: account_id.into_inner(),
        display_name: "Unknown user".to_owned(),
    })
}

pub(super) fn scope_failure_kind(failure: &ScopeSaveFailure) -> ScopeOutcomeKind {
    match failure {
        ScopeSaveFailure::Unchanged(ScopeUnchangedFailure::Invalid) => ScopeOutcomeKind::Invalid,
        ScopeSaveFailure::Unchanged(ScopeUnchangedFailure::Preparation) => {
            ScopeOutcomeKind::Preparation
        }
        ScopeSaveFailure::Restored(ScopeFailureCause::Refresh) => ScopeOutcomeKind::RefreshRestored,
        ScopeSaveFailure::RestoreFailed(ScopeFailureCause::Refresh) => {
            ScopeOutcomeKind::RefreshRollbackFailed
        }
        ScopeSaveFailure::Restored(ScopeFailureCause::PreferenceSave) => {
            ScopeOutcomeKind::PreferenceSaveRestored
        }
        ScopeSaveFailure::RestoreFailed(ScopeFailureCause::PreferenceSave) => {
            ScopeOutcomeKind::PreferenceSaveRollbackFailed
        }
    }
}

pub(super) fn team_failure_kind(failure: &TeamSaveFailure) -> TeamOutcomeKind {
    match failure {
        TeamSaveFailure::Unchanged(TeamUnchangedFailure::InvalidInput(kind)) => {
            TeamOutcomeKind::InvalidInput(*kind)
        }
        TeamSaveFailure::Unchanged(TeamUnchangedFailure::Search) => TeamOutcomeKind::Search,
        TeamSaveFailure::Unchanged(TeamUnchangedFailure::EmailNotFound) => {
            TeamOutcomeKind::EmailNotFound
        }
        TeamSaveFailure::Unchanged(TeamUnchangedFailure::EmailAmbiguous) => {
            TeamOutcomeKind::EmailAmbiguous
        }
        TeamSaveFailure::Unchanged(TeamUnchangedFailure::Normalization) => {
            TeamOutcomeKind::Normalization
        }
        TeamSaveFailure::Unchanged(TeamUnchangedFailure::Preparation) => {
            TeamOutcomeKind::Preparation
        }
        TeamSaveFailure::Restored(TeamFailureCause::Refresh) => TeamOutcomeKind::RefreshRestored,
        TeamSaveFailure::RestoreFailed(TeamFailureCause::Refresh) => {
            TeamOutcomeKind::RefreshRollbackFailed
        }
        TeamSaveFailure::Restored(TeamFailureCause::PreferenceSave) => {
            TeamOutcomeKind::PreferenceSaveRestored
        }
        TeamSaveFailure::RestoreFailed(TeamFailureCause::PreferenceSave) => {
            TeamOutcomeKind::PreferenceSaveRollbackFailed
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use futures_lite::future::block_on;
    use jira_application::SyncOutcome;
    use jira_domain::JiraSiteId;
    use time::macros::datetime;

    use super::*;
    use crate::live_workspace::CachedWorkspace;
    use crate::presentation::{
        FeedbackCertainty, FeedbackSeverity, RecoveryDirective, scope_outcome_copy,
        team_outcome_copy,
    };

    #[derive(Clone, Debug, Eq, PartialEq)]
    enum Call {
        ScopeSet(Option<String>),
        ScopeRefresh,
        ScopeCacheLoad,
        UserSearch(String),
        TeamConfigure(Vec<AccountId>),
        TeamRefresh,
        SavePreferences(LocalPreferences),
    }

    #[derive(Clone, Default)]
    struct CallLog(Arc<Mutex<Vec<Call>>>);

    impl CallLog {
        fn push(&self, call: Call) {
            self.0.lock().expect("calls lock").push(call);
        }

        fn calls(&self) -> Vec<Call> {
            self.0.lock().expect("calls lock").clone()
        }
    }

    struct FakePort {
        calls: CallLog,
        scope_sets: Mutex<Vec<Result<(), ()>>>,
        scope_refreshes: Mutex<Vec<Result<RefreshResult, ()>>>,
        cached_loads: Mutex<Vec<Result<(), ()>>>,
        searches: Mutex<Vec<Result<Vec<User>, ()>>>,
        team_configures: Mutex<Vec<Result<(), ()>>>,
        team_refreshes: Mutex<Vec<Result<RefreshResult, ()>>>,
    }

    impl FakePort {
        fn new(calls: CallLog) -> Self {
            Self {
                calls,
                scope_sets: Mutex::new(Vec::new()),
                scope_refreshes: Mutex::new(Vec::new()),
                cached_loads: Mutex::new(Vec::new()),
                searches: Mutex::new(Vec::new()),
                team_configures: Mutex::new(Vec::new()),
                team_refreshes: Mutex::new(Vec::new()),
            }
        }

        fn push_scope_set(&self, result: Result<(), ()>) {
            self.scope_sets
                .lock()
                .expect("scope sets lock")
                .push(result);
        }

        fn push_scope_refresh(&self, result: Result<RefreshResult, ()>) {
            self.scope_refreshes
                .lock()
                .expect("scope refreshes lock")
                .push(result);
        }

        fn push_cached_load(&self, result: Result<(), ()>) {
            self.cached_loads
                .lock()
                .expect("cached loads lock")
                .push(result);
        }

        fn push_team_configure(&self, result: Result<(), ()>) {
            self.team_configures
                .lock()
                .expect("team configures lock")
                .push(result);
        }

        fn push_team_refresh(&self, result: Result<RefreshResult, ()>) {
            self.team_refreshes
                .lock()
                .expect("team refreshes lock")
                .push(result);
        }

        fn push_search(&self, result: Result<Vec<User>, ()>) {
            self.searches.lock().expect("searches lock").push(result);
        }
    }

    impl SettingsPort for FakePort {
        fn set_scope<'a>(&'a self, scope: Option<String>) -> TransactionFuture<'a, ()> {
            self.calls.push(Call::ScopeSet(scope));
            let result = self.scope_sets.lock().expect("scope sets lock").remove(0);
            Box::pin(async move { result })
        }

        fn refresh_scope<'a>(&'a self) -> TransactionFuture<'a, RefreshResult> {
            self.calls.push(Call::ScopeRefresh);
            let result = self
                .scope_refreshes
                .lock()
                .expect("scope refreshes lock")
                .remove(0);
            Box::pin(async move { result })
        }

        fn load_cached<'a>(&'a self) -> TransactionFuture<'a, ()> {
            self.calls.push(Call::ScopeCacheLoad);
            let result = self
                .cached_loads
                .lock()
                .expect("cached loads lock")
                .remove(0);
            Box::pin(async move { result })
        }

        fn search_users<'a>(&'a self, query: String) -> TransactionFuture<'a, Vec<User>> {
            self.calls.push(Call::UserSearch(query));
            let result = self.searches.lock().expect("searches lock").remove(0);
            Box::pin(async move { result })
        }

        fn configure_team<'a>(&'a self, members: Vec<AccountId>) -> TransactionFuture<'a, ()> {
            self.calls.push(Call::TeamConfigure(members));
            let result = self
                .team_configures
                .lock()
                .expect("team configures lock")
                .remove(0);
            Box::pin(async move { result })
        }

        fn refresh_team<'a>(&'a self) -> TransactionFuture<'a, RefreshResult> {
            self.calls.push(Call::TeamRefresh);
            let result = self
                .team_refreshes
                .lock()
                .expect("team refreshes lock")
                .remove(0);
            Box::pin(async move { result })
        }
    }

    struct FakePreferences {
        calls: CallLog,
        results: Mutex<Vec<Result<(), ()>>>,
    }

    impl FakePreferences {
        fn new(calls: CallLog) -> Self {
            Self {
                calls,
                results: Mutex::new(Vec::new()),
            }
        }
    }

    impl PreferencesPort for FakePreferences {
        fn save(&self, preferences: &LocalPreferences) -> Result<(), ()> {
            self.calls.push(Call::SavePreferences(preferences.clone()));
            self.results
                .lock()
                .expect("preference results lock")
                .remove(0)
        }
    }

    fn fakes() -> (FakePort, FakePreferences, CallLog) {
        let calls = CallLog::default();
        (
            FakePort::new(calls.clone()),
            FakePreferences::new(calls.clone()),
            calls,
        )
    }

    fn refresh_result() -> RefreshResult {
        RefreshResult {
            cached: CachedWorkspace {
                issues: Vec::new(),
                events: Vec::new(),
            },
            outcome: SyncOutcome {
                mode: jira_application::SyncMode::Baseline,
                pages_fetched: 1,
                issues_fetched: 0,
                events_inserted: 0,
                notifications_delivered: 0,
                notification_failures: 0,
                cursor: datetime!(2026-01-01 00:00 UTC),
            },
        }
    }

    fn member(identifier: &str, account_id: &str) -> PersistedTeamMember {
        PersistedTeamMember {
            identifier: identifier.to_owned(),
            account_id: account_id.to_owned(),
            display_name: "Member".to_owned(),
        }
    }

    fn account(value: &str) -> AccountId {
        AccountId::new(value).expect("account")
    }

    fn user(account_id: &str, active: bool) -> User {
        User::new(
            JiraSiteId::new("site").expect("site"),
            account(account_id),
            "Member",
            None,
            active,
        )
    }

    fn local_preferences(
        issue_jql_scope: Option<&str>,
        team_members: Vec<PersistedTeamMember>,
    ) -> LocalPreferences {
        LocalPreferences {
            issue_jql_scope: issue_jql_scope.map(str::to_owned),
            team_members,
        }
    }

    #[test]
    fn scope_success_preserves_team_preferences_and_logs_save_in_order() {
        let (port, preferences, calls) = fakes();
        port.push_scope_set(Ok(()));
        port.push_scope_refresh(Ok(refresh_result()));
        preferences
            .results
            .lock()
            .expect("results lock")
            .push(Ok(()));
        let team_members = vec![member("first", "first"), member("second", "second")];

        let result = block_on(run_scope_transaction(
            &port,
            &preferences,
            " project = APP ".to_owned(),
            None,
            team_members.clone(),
        ));

        assert!(matches!(result, ScopeSaveResult::Saved { .. }));
        assert_eq!(
            calls.calls(),
            vec![
                Call::ScopeSet(Some("project = APP".to_owned())),
                Call::ScopeRefresh,
                Call::SavePreferences(local_preferences(Some("project = APP"), team_members,)),
            ]
        );
    }

    #[test]
    fn scope_refresh_failure_restores_previous_scope_once_then_loads_cache() {
        let (port, preferences, calls) = fakes();
        port.push_scope_set(Ok(()));
        port.push_scope_refresh(Err(()));
        port.push_scope_set(Ok(()));
        port.push_cached_load(Ok(()));

        let result = block_on(run_scope_transaction(
            &port,
            &preferences,
            "project = APP".to_owned(),
            Some("project = OLD".to_owned()),
            Vec::new(),
        ));

        assert!(matches!(
            result,
            ScopeSaveResult::Failed(ScopeSaveFailure::Restored(ScopeFailureCause::Refresh))
        ));
        assert_eq!(
            calls.calls(),
            vec![
                Call::ScopeSet(Some("project = APP".to_owned())),
                Call::ScopeRefresh,
                Call::ScopeSet(Some("project = OLD".to_owned())),
                Call::ScopeCacheLoad,
            ]
        );
    }

    #[test]
    fn scope_refresh_restore_failure_is_typed_once_without_cache_reload_or_retry() {
        let (port, preferences, calls) = fakes();
        port.push_scope_set(Ok(()));
        port.push_scope_refresh(Err(()));
        port.push_scope_set(Err(()));

        let result = block_on(run_scope_transaction(
            &port,
            &preferences,
            "project = APP".to_owned(),
            None,
            Vec::new(),
        ));

        assert!(matches!(
            result,
            ScopeSaveResult::Failed(ScopeSaveFailure::RestoreFailed(ScopeFailureCause::Refresh))
        ));
        assert_eq!(
            calls.calls(),
            vec![
                Call::ScopeSet(Some("project = APP".to_owned())),
                Call::ScopeRefresh,
                Call::ScopeSet(None),
            ]
        );
    }

    #[test]
    fn scope_preference_failure_restores_previous_scope_and_loads_cache() {
        let (port, preferences, calls) = fakes();
        port.push_scope_set(Ok(()));
        port.push_scope_refresh(Ok(refresh_result()));
        port.push_scope_set(Ok(()));
        port.push_cached_load(Err(()));
        preferences
            .results
            .lock()
            .expect("results lock")
            .push(Err(()));
        let team_members = vec![member("existing", "existing")];

        let result = block_on(run_scope_transaction(
            &port,
            &preferences,
            "project = APP".to_owned(),
            Some("project = OLD".to_owned()),
            team_members.clone(),
        ));

        assert!(matches!(
            result,
            ScopeSaveResult::Failed(ScopeSaveFailure::Restored(
                ScopeFailureCause::PreferenceSave
            ))
        ));
        assert_eq!(
            calls.calls(),
            vec![
                Call::ScopeSet(Some("project = APP".to_owned())),
                Call::ScopeRefresh,
                Call::SavePreferences(local_preferences(Some("project = APP"), team_members,)),
                Call::ScopeSet(Some("project = OLD".to_owned())),
                Call::ScopeCacheLoad,
            ]
        );
    }

    #[test]
    fn scope_preference_failure_restore_failure_is_typed_once_without_cache_reload_or_retry() {
        let (port, preferences, calls) = fakes();
        port.push_scope_set(Ok(()));
        port.push_scope_refresh(Ok(refresh_result()));
        port.push_scope_set(Err(()));
        preferences
            .results
            .lock()
            .expect("results lock")
            .push(Err(()));

        let result = block_on(run_scope_transaction(
            &port,
            &preferences,
            "project = APP".to_owned(),
            Some("project = OLD".to_owned()),
            vec![member("existing", "existing")],
        ));

        assert!(matches!(
            result,
            ScopeSaveResult::Failed(ScopeSaveFailure::RestoreFailed(
                ScopeFailureCause::PreferenceSave
            ))
        ));
        assert_eq!(
            calls.calls(),
            vec![
                Call::ScopeSet(Some("project = APP".to_owned())),
                Call::ScopeRefresh,
                Call::SavePreferences(local_preferences(
                    Some("project = APP"),
                    vec![member("existing", "existing")],
                )),
                Call::ScopeSet(Some("project = OLD".to_owned())),
            ]
        );
    }

    #[test]
    fn team_success_logs_exact_queries_configuration_and_saved_normalized_order() {
        let (port, preferences, calls) = fakes();
        port.push_search(Ok(vec![user("bob-id", true)]));
        port.push_search(Ok(vec![user("alice-id", true)]));
        port.push_team_configure(Ok(()));
        port.push_team_refresh(Ok(refresh_result()));
        preferences
            .results
            .lock()
            .expect("results lock")
            .push(Ok(()));
        let members = vec![
            member("bob@example.com", "bob-id"),
            persisted_direct_team_member("account-direct".to_owned()).expect("direct member"),
            member("alice@example.com", "alice-id"),
        ];

        let result = block_on(run_team_transaction(
            &port,
            &preferences,
            " bob@example.com \naccount-direct\nalice@example.com".to_owned(),
            vec![account("old")],
            Some("project = OLD".to_owned()),
        ));

        assert!(matches!(result, TeamSaveResult::Saved { .. }));
        assert_eq!(
            calls.calls(),
            vec![
                Call::UserSearch("bob@example.com".to_owned()),
                Call::UserSearch("alice@example.com".to_owned()),
                Call::TeamConfigure(vec![
                    account("bob-id"),
                    account("account-direct"),
                    account("alice-id"),
                ]),
                Call::TeamRefresh,
                Call::SavePreferences(local_preferences(Some("project = OLD"), members,)),
            ]
        );
    }

    #[test]
    fn team_refresh_failure_restores_previous_accounts_once_without_retrying_refresh() {
        let (port, preferences, calls) = fakes();
        port.push_team_configure(Ok(()));
        port.push_team_refresh(Err(()));
        port.push_team_configure(Ok(()));

        let result = block_on(run_team_transaction(
            &port,
            &preferences,
            "new-account".to_owned(),
            vec![account("old-a"), account("old-b")],
            None,
        ));

        assert!(matches!(
            result,
            TeamSaveResult::Failed(TeamSaveFailure::Restored(TeamFailureCause::Refresh))
        ));
        assert_eq!(
            calls.calls(),
            vec![
                Call::TeamConfigure(vec![account("new-account")]),
                Call::TeamRefresh,
                Call::TeamConfigure(vec![account("old-a"), account("old-b")]),
            ]
        );
    }

    #[test]
    fn team_refresh_restore_failure_is_typed_once_without_retry() {
        let (port, preferences, calls) = fakes();
        port.push_team_configure(Ok(()));
        port.push_team_refresh(Err(()));
        port.push_team_configure(Err(()));

        let result = block_on(run_team_transaction(
            &port,
            &preferences,
            "new-account".to_owned(),
            vec![account("old")],
            None,
        ));

        assert!(matches!(
            result,
            TeamSaveResult::Failed(TeamSaveFailure::RestoreFailed(TeamFailureCause::Refresh))
        ));
        assert_eq!(
            calls.calls(),
            vec![
                Call::TeamConfigure(vec![account("new-account")]),
                Call::TeamRefresh,
                Call::TeamConfigure(vec![account("old")]),
            ]
        );
    }

    #[test]
    fn team_preference_failure_restores_previous_accounts_once() {
        let (port, preferences, calls) = fakes();
        port.push_team_configure(Ok(()));
        port.push_team_refresh(Ok(refresh_result()));
        port.push_team_configure(Ok(()));
        preferences
            .results
            .lock()
            .expect("results lock")
            .push(Err(()));
        let members =
            vec![persisted_direct_team_member("new-account".to_owned()).expect("direct member")];

        let result = block_on(run_team_transaction(
            &port,
            &preferences,
            "new-account".to_owned(),
            vec![account("old-a"), account("old-b")],
            Some("project = OLD".to_owned()),
        ));

        assert!(matches!(
            result,
            TeamSaveResult::Failed(TeamSaveFailure::Restored(TeamFailureCause::PreferenceSave))
        ));
        assert_eq!(
            calls.calls(),
            vec![
                Call::TeamConfigure(vec![account("new-account")]),
                Call::TeamRefresh,
                Call::SavePreferences(local_preferences(Some("project = OLD"), members,)),
                Call::TeamConfigure(vec![account("old-a"), account("old-b")]),
            ]
        );
    }

    #[test]
    fn team_preference_failure_restore_failure_is_typed_once_without_retry() {
        let (port, preferences, calls) = fakes();
        port.push_team_configure(Ok(()));
        port.push_team_refresh(Ok(refresh_result()));
        port.push_team_configure(Err(()));
        preferences
            .results
            .lock()
            .expect("results lock")
            .push(Err(()));

        let result = block_on(run_team_transaction(
            &port,
            &preferences,
            "new-account".to_owned(),
            vec![account("old")],
            None,
        ));

        assert!(matches!(
            result,
            TeamSaveResult::Failed(TeamSaveFailure::RestoreFailed(
                TeamFailureCause::PreferenceSave
            ))
        ));
        assert_eq!(
            calls.calls(),
            vec![
                Call::TeamConfigure(vec![account("new-account")]),
                Call::TeamRefresh,
                Call::SavePreferences(local_preferences(
                    None,
                    vec![
                        persisted_direct_team_member("new-account".to_owned())
                            .expect("direct member"),
                    ],
                )),
                Call::TeamConfigure(vec![account("old")]),
            ]
        );
    }

    #[test]
    fn scope_failure_mapping_covers_every_cause_and_directive() {
        let cases = [
            (
                ScopeSaveFailure::Unchanged(ScopeUnchangedFailure::Invalid),
                ScopeOutcomeKind::Invalid,
                "Scope is invalid; check the expression and ORDER BY rule",
                RecoveryDirective::Retry,
            ),
            (
                ScopeSaveFailure::Unchanged(ScopeUnchangedFailure::Preparation),
                ScopeOutcomeKind::Preparation,
                "Scope could not be prepared locally",
                RecoveryDirective::Retry,
            ),
            (
                ScopeSaveFailure::Restored(ScopeFailureCause::Refresh),
                ScopeOutcomeKind::RefreshRestored,
                "Jira rejected the scope; the previous scope remains active",
                RecoveryDirective::Retry,
            ),
            (
                ScopeSaveFailure::RestoreFailed(ScopeFailureCause::Refresh),
                ScopeOutcomeKind::RefreshRollbackFailed,
                "Jira rejected the scope and the previous scope could not be restored",
                RecoveryDirective::InvalidateWorkspace,
            ),
            (
                ScopeSaveFailure::Restored(ScopeFailureCause::PreferenceSave),
                ScopeOutcomeKind::PreferenceSaveRestored,
                "Scope applied remotely, but settings could not be saved locally",
                RecoveryDirective::Retry,
            ),
            (
                ScopeSaveFailure::RestoreFailed(ScopeFailureCause::PreferenceSave),
                ScopeOutcomeKind::PreferenceSaveRollbackFailed,
                "Settings could not be saved and the previous scope could not be restored",
                RecoveryDirective::InvalidateWorkspace,
            ),
        ];
        for (failure, expected_kind, message, recovery) in cases {
            assert_eq!(scope_failure_kind(&failure), expected_kind);
            let copy = scope_outcome_copy(expected_kind);
            assert_eq!(copy.message(), message);
            assert_eq!(copy.severity(), FeedbackSeverity::Error);
            assert_eq!(copy.certainty(), FeedbackCertainty::Definite);
            assert_eq!(copy.recovery(), recovery);
        }
    }

    #[test]
    fn team_failure_mapping_covers_every_variant_and_directive() {
        let cases = [
            (
                TeamSaveFailure::Unchanged(TeamUnchangedFailure::InvalidInput(
                    TeamInvalidInputKind::TooManyMembers,
                )),
                TeamOutcomeKind::InvalidInput(TeamInvalidInputKind::TooManyMembers),
                "Team tracker accepts at most 100 members",
                RecoveryDirective::Retry,
            ),
            (
                TeamSaveFailure::Unchanged(TeamUnchangedFailure::InvalidInput(
                    TeamInvalidInputKind::InvalidAccount,
                )),
                TeamOutcomeKind::InvalidInput(TeamInvalidInputKind::InvalidAccount),
                "Enter a valid Jira account ID or Atlassian email",
                RecoveryDirective::Retry,
            ),
            (
                TeamSaveFailure::Unchanged(TeamUnchangedFailure::InvalidInput(
                    TeamInvalidInputKind::UnsafeAccount,
                )),
                TeamOutcomeKind::InvalidInput(TeamInvalidInputKind::UnsafeAccount),
                "Jira account IDs cannot contain quote or backslash characters",
                RecoveryDirective::Retry,
            ),
            (
                TeamSaveFailure::Unchanged(TeamUnchangedFailure::InvalidInput(
                    TeamInvalidInputKind::InvalidEntry,
                )),
                TeamOutcomeKind::InvalidInput(TeamInvalidInputKind::InvalidEntry),
                "Team tracker entries must be short, single-line values",
                RecoveryDirective::Retry,
            ),
            (
                TeamSaveFailure::Unchanged(TeamUnchangedFailure::Search),
                TeamOutcomeKind::Search,
                "Jira user search failed; existing team remains active",
                RecoveryDirective::Retry,
            ),
            (
                TeamSaveFailure::Unchanged(TeamUnchangedFailure::EmailNotFound),
                TeamOutcomeKind::EmailNotFound,
                "Email did not resolve to one active Jira user",
                RecoveryDirective::Retry,
            ),
            (
                TeamSaveFailure::Unchanged(TeamUnchangedFailure::EmailAmbiguous),
                TeamOutcomeKind::EmailAmbiguous,
                "Email matched multiple active Jira users; enter an account ID instead",
                RecoveryDirective::Retry,
            ),
            (
                TeamSaveFailure::Unchanged(TeamUnchangedFailure::Normalization),
                TeamOutcomeKind::Normalization,
                "Team tracker entries are invalid or exceed the member limit",
                RecoveryDirective::Retry,
            ),
            (
                TeamSaveFailure::Unchanged(TeamUnchangedFailure::Preparation),
                TeamOutcomeKind::Preparation,
                "Team configuration could not be applied; existing team remains active",
                RecoveryDirective::Retry,
            ),
            (
                TeamSaveFailure::Restored(TeamFailureCause::Refresh),
                TeamOutcomeKind::RefreshRestored,
                "Team refresh failed; existing team remains active",
                RecoveryDirective::Retry,
            ),
            (
                TeamSaveFailure::RestoreFailed(TeamFailureCause::Refresh),
                TeamOutcomeKind::RefreshRollbackFailed,
                "Team refresh failed and the previous team could not be restored; team tracker paused",
                RecoveryDirective::PauseTeam,
            ),
            (
                TeamSaveFailure::Restored(TeamFailureCause::PreferenceSave),
                TeamOutcomeKind::PreferenceSaveRestored,
                "Team refreshed but could not be saved locally; existing team remains active",
                RecoveryDirective::Retry,
            ),
            (
                TeamSaveFailure::RestoreFailed(TeamFailureCause::PreferenceSave),
                TeamOutcomeKind::PreferenceSaveRollbackFailed,
                "Team settings could not be saved and the previous team could not be restored; team tracker paused",
                RecoveryDirective::PauseTeam,
            ),
        ];
        for (failure, expected_kind, message, recovery) in cases {
            assert_eq!(team_failure_kind(&failure), expected_kind);
            let copy = team_outcome_copy(expected_kind);
            assert_eq!(copy.message(), message);
            assert_eq!(copy.severity(), FeedbackSeverity::Error);
            assert_eq!(copy.certainty(), FeedbackCertainty::Definite);
            assert_eq!(copy.recovery(), recovery);
        }
    }

    #[test]
    fn inactive_email_users_do_not_satisfy_exactly_one_rule() {
        let (port, preferences, calls) = fakes();
        port.push_search(Ok(vec![user("inactive", false)]));

        let result = block_on(run_team_transaction(
            &port,
            &preferences,
            "person@example.com".to_owned(),
            Vec::new(),
            None,
        ));

        assert!(matches!(
            result,
            TeamSaveResult::Failed(TeamSaveFailure::Unchanged(
                TeamUnchangedFailure::EmailNotFound
            ))
        ));
        assert_eq!(
            calls.calls(),
            vec![Call::UserSearch("person@example.com".to_owned())]
        );
    }

    #[test]
    fn ambiguous_active_email_is_rejected_by_team_transaction_with_search_only() {
        let (port, preferences, calls) = fakes();
        port.push_search(Ok(vec![
            user("first-active", true),
            user("second-active", true),
        ]));

        let result = block_on(run_team_transaction(
            &port,
            &preferences,
            "ambiguous@example.com".to_owned(),
            vec![account("old")],
            Some("project = OLD".to_owned()),
        ));

        assert!(matches!(
            result,
            TeamSaveResult::Failed(TeamSaveFailure::Unchanged(
                TeamUnchangedFailure::EmailAmbiguous
            ))
        ));
        assert_eq!(
            calls.calls(),
            vec![Call::UserSearch("ambiguous@example.com".to_owned())]
        );
    }
}
