# Jira API token scope inventory

This is the endpoint inventory for migrating Jira Desk to scoped Jira API tokens. The
inventory is derived from the request builders and call sites in
[`crates/jira-http/src/lib.rs`](../crates/jira-http/src/lib.rs). Paths in the table are
relative Jira REST v3 paths; after migration, every authenticated path must be appended to
`https://api.atlassian.com/ex/jira/{cloudId}`.

## Routing and authentication

1. Resolve the site Cloud ID with an unauthenticated request:
   `GET https://<site>.atlassian.net/_edge/tenant_info`. It needs no token and no Jira
   scope; the response contains `cloudId`. This endpoint is a bootstrap prerequisite and
   must remain unauthenticated. See Atlassian's [Cloud ID
   instructions](https://support.atlassian.com/jira/kb/retrieve-my-atlassian-sites-cloud-id/).
2. Send each authenticated Jira request to
   `https://api.atlassian.com/ex/jira/{cloudId}/rest/api/3/...` using HTTP Basic
   authentication with the Atlassian account email as the username and the scoped API
   token as the password. Atlassian documents both the gateway URL and Basic
   authentication in [API token guidance for your Atlassian
   account](https://support.atlassian.com/atlassian-account/docs/manage-api-tokens-for-your-atlassian-account/).
3. Direct site REST URLs (`https://<site>.atlassian.net/rest/api/3/...`) and unscoped
   tokens are unsupported after this migration. Scoped tokens use the gateway URL
   documented above.

The full-function token needs this union of classic scopes:

```text
read:jira-user
read:jira-work
write:jira-work
```

Scopes limit what the token can do; they do not grant Jira access. The authenticated
Atlassian account must still have the product, project, issue-security, and operation
permissions required by each request. See Atlassian's [Jira scope
reference](https://developer.atlassian.com/cloud/jira/platform/scopes-for-oauth-2-3lo-and-forge-apps/)
and [REST API authorization and permissions](https://developer.atlassian.com/cloud/jira/platform/rest/v3/intro#permissions).

## REST v3 operations

| Method and path | `jira-http` use | Access/policy | Classic scope | Atlassian reference |
| --- | --- | --- | --- | --- |
| `GET /rest/api/3/user/search` | `search_users`: search users for the assignee/user UI | Read | `read:jira-user` | [User search](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-user-search/#api-rest-api-3-user-search-get) |
| `GET /rest/api/3/user/assignable/search` | `search_assignable_users`: find users assignable to an issue | Read; preflight for an explicit assignment | `read:jira-user` | [User search](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-user-search/#api-rest-api-3-user-assignable-search-get) |
| `GET /rest/api/3/myself` | `fetch_current_user`: resolve the authenticated Jira user | Read | `read:jira-user` | [Myself](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-myself/#api-rest-api-3-myself-get) |
| `POST /rest/api/3/search/jql` | `fetch_issue_page` and `fetch_issues_by_id`: search/sync issues with JQL | Read | `read:jira-work` | [Issue search](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issue-search/#api-rest-api-3-search-jql-post) |
| `POST /rest/api/3/changelog/bulkfetch` | `fetch_issue_changelog`: fetch bounded issue history for updates | Read | `read:jira-work` | [Issues](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issues/#api-rest-api-3-changelog-bulkfetch-post) |
| `GET /rest/api/3/issue/{issueIdOrKey}` | `fetch_issue_detail`: load issue details | Read | `read:jira-work` | [Issues](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issues/#api-rest-api-3-issue-issueidorkey-get) |
| `GET /rest/api/3/issue/{issueIdOrKey}/comment` | `fetch_issue_comments_page` and `fetch_recent_issue_comments`: read comments | Read | `read:jira-work` | [Issue comments](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issue-comments/#api-rest-api-3-issue-issueidorkey-comment-get) |
| `GET /rest/api/3/issue/{issueIdOrKey}/transitions` | `fetch_issue_transitions`: list transitions available to the user | Read; preflight for an explicit status transition | `read:jira-work` | [Issues](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issues/#api-rest-api-3-issue-issueidorkey-transitions-get) |
| `GET /rest/api/3/attachment/thumbnail/{id}` | `fetch_attachment_image`: display an issue-attachment thumbnail | Read | `read:jira-work` | [Issue attachments](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issue-attachments/#api-rest-api-3-attachment-thumbnail-id-get) |
| `GET /rest/api/3/attachment/content/{id}` | `fetch_attachment_content`: explicitly download attachment content | Read | `read:jira-work` | [Issue attachments](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issue-attachments/#api-rest-api-3-attachment-content-id-get) |
| `POST /rest/api/3/issue/{issueIdOrKey}/comment` | `create_comment`: create a Jira comment | Write; only after user confirmation, dispatched once | `write:jira-work` | [Issue comments](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issue-comments/#api-rest-api-3-issue-issueidorkey-comment-post) |
| `PUT /rest/api/3/issue/{issueIdOrKey}/assignee` | `assign_issue`: assign or unassign an issue | Write; only after user confirmation, dispatched once | `write:jira-work` | [Issues](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issues/#api-rest-api-3-issue-issueidorkey-assignee-put) |
| `POST /rest/api/3/issue/{issueIdOrKey}/transitions` | `transition_issue`: perform a workflow transition | Write; only after user confirmation, dispatched once | `write:jira-work` | [Issues](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issues/#api-rest-api-3-issue-issueidorkey-transitions-post) |

The write boundary is intentionally limited to comment creation, assignment changes, and
status transitions. There are no call sites for issue deletion, attachment upload or
deletion, or general issue edits. Automatic Jira writes and automatic write retries are
also absent; an uncertain write result is not silently retried.

## Scope validation

The `rg` audit of `crates/jira-http/src/lib.rs` found the 13 authenticated REST operations
listed above. Every one maps to one of the three required classic scopes; no authenticated
Jira endpoint falls outside that set. The only additional endpoint in this migration is
`GET /_edge/tenant_info`, which is unauthenticated and therefore has no Jira scope.

The migrated transport must construct these paths from the gateway base URL and the
resolved Cloud ID, and apply Basic authentication with the Atlassian account email and
scoped token to every authenticated request.
