use jira_domain::Issue;

pub(crate) fn preserve_cached_detail(issue: &mut Issue, existing: Option<&Issue>) {
    if issue.detail_loaded {
        return;
    }
    let Some(existing) = existing else {
        return;
    };

    issue.description_text = existing.description_text.clone();
    issue.rich_description = existing.rich_description.clone();
    issue.linked_issues = existing.linked_issues.clone();
    issue.detail_loaded = existing.detail_loaded;
}
