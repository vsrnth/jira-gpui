use jira_domain::UpdateEvent;

pub(crate) fn same_event_identity(left: &UpdateEvent, right: &UpdateEvent) -> bool {
    left.id == right.id
        && left.site_id == right.site_id
        && left.issue_id == right.issue_id
        && left.issue_key == right.issue_key
        && left.kind == right.kind
        && left.occurred_at == right.occurred_at
}

pub(crate) fn normalize_matching_user_set_ids(event: &mut UpdateEvent) {
    event.matching_user_set_ids.sort();
    event.matching_user_set_ids.dedup();
}

pub(crate) fn union_matching_user_set_ids(target: &mut UpdateEvent, source: &UpdateEvent) {
    target
        .matching_user_set_ids
        .extend(source.matching_user_set_ids.iter().cloned());
    normalize_matching_user_set_ids(target);
}
