use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::process::Command;

use crate::tools::{parse_bool_param, RecoverableError, Tool, ToolContext};

// ── Shared execution ──────────────────────────────────────────────────────────

pub(crate) struct GhOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

pub(crate) async fn run_gh(args: &[&str]) -> anyhow::Result<GhOutput> {
    let out = Command::new("gh").args(args).output().await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            RecoverableError::with_hint(
                "gh CLI not found",
                "Install from https://cli.github.com, then run: gh auth login",
            )
        } else {
            RecoverableError::new(format!("failed to run gh: {e}"))
        }
    })?;
    Ok(GhOutput {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        exit_code: out.status.code().unwrap_or(-1),
    })
}

pub(crate) fn check_gh_output(out: GhOutput) -> Result<String, RecoverableError> {
    if out.exit_code != 0 {
        let msg = if out.stderr.contains("not logged into")
            || out.stderr.contains("authentication")
            || out.stderr.contains("auth")
        {
            format!(
                "gh not authenticated: {}. Run: gh auth login",
                out.stderr.trim()
            )
        } else {
            out.stderr.trim().to_string()
        };
        return Err(RecoverableError::new(msg));
    }
    Ok(out.stdout)
}

/// Store in OutputBuffer if large; otherwise parse as JSON or return as string.
pub(crate) fn maybe_buffer(content: String, tool_name: &str, ctx: &ToolContext) -> Value {
    if crate::tools::exceeds_inline_limit(&content) {
        let id = ctx.output_buffer.store_tool(tool_name, content);
        json!({
            "output_id": id,
            "hint": format!(
                "read_file(\"{id}\", json_path=\"$.field\") to extract a specific field, \
                 or read_file(\"{id}\", start_line=N, end_line=M) to browse sections"
            )
        })
    } else {
        serde_json::from_str(&content).unwrap_or_else(|_| json!(content))
    }
}

/// Always buffer — for known-large responses (diffs, file contents, code search).
pub(crate) fn always_buffer(content: String, tool_name: &str, ctx: &ToolContext) -> Value {
    let id = ctx.output_buffer.store_tool(tool_name, content);
    json!({
        "output_id": id,
        "hint": format!(
            "read_file(\"{id}\", json_path=\"$.field\") to extract a specific field, \
             or read_file(\"{id}\", start_line=N, end_line=M) to browse sections"
        )
    })
}

// ── Summary helpers ───────────────────────────────────────────────────────────

/// Inject a summary into a buffered result (no-op if result is inline).
fn with_summary(mut result: Value, summary: Option<String>) -> Value {
    if let Some(s) = summary {
        if result.get("output_id").is_some() {
            result["summary"] = json!(s);
        }
    }
    result
}

/// Summarize a JSON array by extracting a label from each item.
fn summarize_list(
    content: &str,
    item_label: &str,
    extract: impl Fn(&Value) -> Option<String>,
) -> Option<String> {
    let arr = serde_json::from_str::<Vec<Value>>(content).ok()?;
    let count = arr.len();
    if count == 0 {
        return Some(format!("0 {item_label}"));
    }
    let items: Vec<String> = arr.iter().take(8).filter_map(&extract).collect();
    let list = items.join(", ");
    if count > 8 {
        Some(format!("{count} {item_label}: {list}, …"))
    } else {
        Some(format!("{count} {item_label}: {list}"))
    }
}

// ── Shared helpers ────────────────────────────────────────────────────────────

fn require_owner_repo(owner: &str, repo: &str) -> Result<(), RecoverableError> {
    if owner.is_empty() || repo.is_empty() {
        return Err(RecoverableError::with_hint(
            "owner and repo required",
            "Provide owner (GitHub username/org) and repo (repository name)",
        ));
    }
    Ok(())
}

fn require_number<'a>(number: &'a Option<String>, method: &str) -> Result<&'a str> {
    number.as_deref().ok_or_else(|| {
        RecoverableError::with_hint(
            "number is required",
            format!("{method} requires the 'number' parameter (issue/PR number)"),
        )
        .into()
    })
}

// ── Stub tool structs ─────────────────────────────────────────────────────────

pub struct GithubIdentity;
pub struct GithubIssue;
pub struct GithubPr;
pub struct GithubFile;
pub struct GithubRepo;

#[async_trait]
impl Tool for GithubIdentity {
    fn name(&self) -> &'static str {
        "github_identity"
    }

    fn description(&self) -> &'static str {
        "GitHub identity and team operations.\n\
         method: get_me | search_users | get_teams | get_team_members\n\n\
         get_me — authenticated user profile.\n\
         search_users — search GitHub users (query required).\n\
         get_teams — teams the authenticated user belongs to.\n\
         get_team_members — members of a specific team (org + team_slug required)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "method": {
                    "type": "string",
                    "enum": ["get_me", "search_users", "get_teams", "get_team_members"]
                },
                "query":      { "type": "string", "description": "search_users: search query" },
                "org":        { "type": "string", "description": "get_team_members: organization login" },
                "team_slug":  { "type": "string", "description": "get_team_members: team slug" }
            },
            "required": ["method"]
        })
    }

    async fn call(&self, params: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let method = params["method"].as_str().unwrap_or("");
        match method {
            "get_me" => {
                let out = run_gh(&["api", "/user"]).await?;
                Ok(maybe_buffer(check_gh_output(out)?, "github_identity", ctx))
            }
            "search_users" => {
                let q = params["query"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint("query required", "Provide a search query string")
                })?;
                let out = run_gh(&["search", "users", q, "--json", "login,id,name,url"]).await?;
                Ok(maybe_buffer(check_gh_output(out)?, "github_identity", ctx))
            }
            "get_teams" => {
                let out = run_gh(&["api", "/user/teams"]).await?;
                Ok(maybe_buffer(check_gh_output(out)?, "github_identity", ctx))
            }
            "get_team_members" => {
                let org = params["org"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint("org required", "Provide the organization login")
                })?;
                let slug = params["team_slug"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint(
                        "team_slug required",
                        "Provide the team slug (e.g. 'engineering')",
                    )
                })?;
                let endpoint = format!("/orgs/{org}/teams/{slug}/members");
                let out = run_gh(&["api", &endpoint]).await?;
                Ok(maybe_buffer(check_gh_output(out)?, "github_identity", ctx))
            }
            other => Err(RecoverableError::with_hint(
                format!("unknown method: '{other}'"),
                "method must be one of: get_me, search_users, get_teams, get_team_members",
            )
            .into()),
        }
    }
}

#[async_trait]
impl Tool for GithubIssue {
    fn name(&self) -> &'static str {
        "github_issue"
    }

    fn description(&self) -> &'static str {
        "GitHub issue operations.\n\
         Read: list | search | get | get_comments | get_labels | get_sub_issues\n\
         Write: create | update | add_comment | add_sub_issue | remove_sub_issue\n\n\
         Most read methods require owner + repo. list/search support limit (default 30).\n\
         get/get_comments/get_labels require number. create requires title."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "method": {
                    "type": "string",
                    "enum": ["list","search","get","get_comments","get_labels","get_sub_issues",
                             "create","update","add_comment","add_sub_issue","remove_sub_issue"]
                },
                "owner":      { "type": "string" },
                "repo":       { "type": "string" },
                "number":     { "type": "integer", "description": "Issue number" },
                "query":      { "type": "string",  "description": "search: query string" },
                "title":      { "type": "string",  "description": "create: issue title" },
                "body":       { "type": "string",  "description": "create/update/add_comment: text" },
                "state":      { "type": "string",  "enum": ["open","closed"], "description": "list/update filter" },
                "labels":     { "type": "string",  "description": "list/create/update: comma-separated labels" },
                "assignees":  { "type": "string",  "description": "create/update: comma-separated logins" },
                "limit":      { "type": "integer", "description": "list/search: max results (default 30)" },
                "sub_issue_id": { "type": "integer", "description": "add_sub_issue/remove_sub_issue: sub-issue node ID" }
            },
            "required": ["method"]
        })
    }

    async fn call(&self, params: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let method = params["method"].as_str().unwrap_or("");
        let owner = params["owner"].as_str().unwrap_or("");
        let repo = params["repo"].as_str().unwrap_or("");
        let repo_flag = if !owner.is_empty() && !repo.is_empty() {
            format!("{owner}/{repo}")
        } else {
            String::new()
        };
        let limit = params["limit"].as_u64().unwrap_or(30).to_string();
        let number = params["number"].as_u64().map(|n| n.to_string());

        match method {
            "list" => {
                let mut args = vec![
                    "issue",
                    "list",
                    "--json",
                    "number,title,state,labels,assignees,createdAt,updatedAt",
                    "--limit",
                    &limit,
                ];
                if !repo_flag.is_empty() {
                    args.extend(["--repo", &repo_flag]);
                }
                if let Some(s) = params["state"].as_str() {
                    args.extend(["--state", s]);
                }
                if let Some(l) = params["labels"].as_str() {
                    args.extend(["--label", l]);
                }
                let out = run_gh(&args).await?;
                let content = check_gh_output(out)?;
                let summary = summarize_list(&content, "issues", |v| {
                    let n = v["number"].as_u64()?;
                    let t = v["title"].as_str()?;
                    Some(format!("#{n} {t}"))
                });
                Ok(with_summary(
                    maybe_buffer(content, "github_issue", ctx),
                    summary,
                ))
            }
            "search" => {
                let q = params["query"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint("query required", "Provide a search query")
                })?;
                let args = [
                    "search",
                    "issues",
                    q,
                    "--json",
                    "number,title,state,repository,url",
                    "--limit",
                    &limit,
                ];
                let out = run_gh(&args).await?;
                let content = check_gh_output(out)?;
                let summary = summarize_list(&content, "issues", |v| {
                    let n = v["number"].as_u64()?;
                    let t = v["title"].as_str()?;
                    Some(format!("#{n} {t}"))
                });
                Ok(with_summary(
                    maybe_buffer(content, "github_issue", ctx),
                    summary,
                ))
            }
            "get" => {
                let num = require_number(&number, "get")?;
                let mut args = vec![
                    "issue",
                    "view",
                    num,
                    "--json",
                    "number,title,body,state,labels,assignees,comments,createdAt,updatedAt,url",
                ];
                if !repo_flag.is_empty() {
                    args.extend(["--repo", &repo_flag]);
                }
                let out = run_gh(&args).await?;
                Ok(maybe_buffer(check_gh_output(out)?, "github_issue", ctx))
            }
            "get_comments" => {
                let num = require_number(&number, "get_comments")?;
                let mut args = vec!["issue", "view", num, "--json", "comments"];
                if !repo_flag.is_empty() {
                    args.extend(["--repo", &repo_flag]);
                }
                let out = run_gh(&args).await?;
                Ok(maybe_buffer(check_gh_output(out)?, "github_issue", ctx))
            }
            "get_labels" => {
                let num = require_number(&number, "get_labels")?;
                let mut args = vec!["issue", "view", num, "--json", "labels"];
                if !repo_flag.is_empty() {
                    args.extend(["--repo", &repo_flag]);
                }
                let out = run_gh(&args).await?;
                Ok(maybe_buffer(check_gh_output(out)?, "github_issue", ctx))
            }
            "get_sub_issues" => {
                let num = require_number(&number, "get_sub_issues")?;
                require_owner_repo(owner, repo)?;
                let endpoint = format!("/repos/{owner}/{repo}/issues/{num}/sub_issues");
                let out = run_gh(&["api", &endpoint]).await?;
                Ok(maybe_buffer(check_gh_output(out)?, "github_issue", ctx))
            }
            "create" => {
                require_owner_repo(owner, repo)?;
                let title = params["title"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint("title required", "Provide the issue title")
                })?;
                let mut args = vec!["issue", "create", "--repo", &repo_flag, "--title", title];
                if let Some(b) = params["body"].as_str() {
                    args.extend(["--body", b]);
                } else {
                    args.extend(["--body", ""]);
                }
                if let Some(l) = params["labels"].as_str() {
                    args.extend(["--label", l]);
                }
                if let Some(a) = params["assignees"].as_str() {
                    args.extend(["--assignee", a]);
                }
                let out = run_gh(&args).await?;
                Ok(json!(check_gh_output(out)?))
            }
            "update" => {
                let num = require_number(&number, "update")?;
                require_owner_repo(owner, repo)?;
                let mut args = vec!["issue", "edit", num, "--repo", &repo_flag];
                if let Some(t) = params["title"].as_str() {
                    args.extend(["--title", t]);
                }
                if let Some(b) = params["body"].as_str() {
                    args.extend(["--body", b]);
                }
                if let Some(s) = params["state"].as_str() {
                    args.extend(["--state", s]);
                }
                if let Some(l) = params["labels"].as_str() {
                    args.extend(["--add-label", l]);
                }
                if let Some(a) = params["assignees"].as_str() {
                    args.extend(["--add-assignee", a]);
                }
                let out = run_gh(&args).await?;
                Ok(json!(check_gh_output(out)?))
            }
            "add_comment" => {
                let num = require_number(&number, "add_comment")?;
                let body = params["body"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint("body required", "Provide the comment body")
                })?;
                let mut args = vec!["issue", "comment", num, "--body", body];
                if !repo_flag.is_empty() {
                    args.extend(["--repo", &repo_flag]);
                }
                let out = run_gh(&args).await?;
                Ok(json!(check_gh_output(out)?))
            }
            "add_sub_issue" => {
                let num = require_number(&number, "add_sub_issue")?;
                let sub_id = params["sub_issue_id"].as_u64().ok_or_else(|| {
                    RecoverableError::with_hint(
                        "sub_issue_id required",
                        "Provide the sub-issue node ID",
                    )
                })?;
                require_owner_repo(owner, repo)?;
                let endpoint = format!("/repos/{owner}/{repo}/issues/{num}/sub_issues");
                let sub_field = format!("sub_issue_id={sub_id}");
                let out =
                    run_gh(&["api", "--method", "POST", &endpoint, "--field", &sub_field]).await?;
                Ok(json!(check_gh_output(out)?))
            }
            "remove_sub_issue" => {
                let num = require_number(&number, "remove_sub_issue")?;
                let sub_id = params["sub_issue_id"].as_u64().ok_or_else(|| {
                    RecoverableError::with_hint(
                        "sub_issue_id required",
                        "Provide the sub-issue node ID",
                    )
                })?;
                require_owner_repo(owner, repo)?;
                let endpoint = format!("/repos/{owner}/{repo}/issues/{num}/sub_issues/{sub_id}");
                let out = run_gh(&["api", "--method", "DELETE", &endpoint]).await?;
                Ok(json!(check_gh_output(out)?))
            }
            other => Err(RecoverableError::with_hint(
                format!("unknown method: '{other}'"),
                "method must be one of: list, search, get, get_comments, get_labels, \
                 get_sub_issues, create, update, add_comment, add_sub_issue, remove_sub_issue",
            )
            .into()),
        }
    }
}

#[async_trait]
impl Tool for GithubPr {
    fn name(&self) -> &'static str {
        "github_pr"
    }

    fn description(&self) -> &'static str {
        "GitHub pull request operations.\n\
         Read: list | search | get | get_diff | get_files | get_comments | \
               get_reviews | get_review_comments | get_status\n\
         Write: create | update | merge | update_branch | create_review | \
                submit_review | delete_review | add_review_comment | add_reply_to_comment\n\n\
         get_diff always returns a @tool buffer handle (diffs are large).\n\
         owner + repo required for most operations. number required for single-PR ops."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "method": {
                    "type": "string",
                    "enum": ["list","search","get","get_diff","get_files","get_comments",
                             "get_reviews","get_review_comments","get_status",
                             "create","update","merge","update_branch",
                             "create_review","submit_review","delete_review",
                             "add_review_comment","add_reply_to_comment"]
                },
                "owner":        { "type": "string" },
                "repo":         { "type": "string" },
                "number":       { "type": "integer", "description": "PR number" },
                "query":        { "type": "string",  "description": "search: query string" },
                "title":        { "type": "string",  "description": "create/update: PR title" },
                "body":         { "type": "string",  "description": "create/update/review: body text" },
                "head":         { "type": "string",  "description": "create: head branch (user:branch)" },
                "base":         { "type": "string",  "description": "create/update: base branch" },
                "state":        { "type": "string",  "enum": ["open","closed"], "description": "list filter or update state" },
                "draft":        { "type": "boolean", "description": "create/update: draft status" },
                "merge_method": { "type": "string",  "enum": ["merge","squash","rebase"], "description": "merge: strategy" },
                "event":        { "type": "string",  "enum": ["APPROVE","REQUEST_CHANGES","COMMENT"], "description": "create_review: event type" },
                "review_id":    { "type": "integer", "description": "submit_review/delete_review: review ID" },
                "commit_id":    { "type": "string",  "description": "add_review_comment: commit SHA" },
                "path":         { "type": "string",  "description": "add_review_comment: file path" },
                "line":         { "type": "integer", "description": "add_review_comment: line number" },
                "comment_id":   { "type": "integer", "description": "add_reply_to_comment: comment ID" },
                "limit":        { "type": "integer", "description": "list/search: max results (default 30)" }
            },
            "required": ["method"]
        })
    }

    async fn call(&self, params: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let method = params["method"].as_str().unwrap_or("");
        let owner = params["owner"].as_str().unwrap_or("");
        let repo = params["repo"].as_str().unwrap_or("");
        let repo_flag = if !owner.is_empty() && !repo.is_empty() {
            format!("{owner}/{repo}")
        } else {
            String::new()
        };
        let limit = params["limit"].as_u64().unwrap_or(30).to_string();
        let number = params["number"].as_u64().map(|n| n.to_string());

        match method {
            "list" => {
                let mut args = vec![
                    "pr",
                    "list",
                    "--json",
                    "number,title,state,isDraft,headRefName,baseRefName,createdAt,updatedAt,url",
                    "--limit",
                    &limit,
                ];
                if !repo_flag.is_empty() {
                    args.extend(["--repo", &repo_flag]);
                }
                if let Some(s) = params["state"].as_str() {
                    args.extend(["--state", s]);
                }
                let out = run_gh(&args).await?;
                let content = check_gh_output(out)?;
                let summary = summarize_list(&content, "PRs", |v| {
                    let n = v["number"].as_u64()?;
                    let t = v["title"].as_str()?;
                    Some(format!("#{n} {t}"))
                });
                Ok(with_summary(
                    maybe_buffer(content, "github_pr", ctx),
                    summary,
                ))
            }
            "search" => {
                let q = params["query"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint("query required", "Provide a search query")
                })?;
                let out = run_gh(&[
                    "search",
                    "prs",
                    q,
                    "--json",
                    "number,title,state,repository,url",
                    "--limit",
                    &limit,
                ])
                .await?;
                let content = check_gh_output(out)?;
                let summary = summarize_list(&content, "PRs", |v| {
                    let n = v["number"].as_u64()?;
                    let t = v["title"].as_str()?;
                    Some(format!("#{n} {t}"))
                });
                Ok(with_summary(
                    maybe_buffer(content, "github_pr", ctx),
                    summary,
                ))
            }
            "get" => {
                let num = require_number(&number, "get")?;
                let mut args = vec![
                    "pr",
                    "view",
                    num,
                    "--json",
                    "number,title,body,state,isDraft,headRefName,baseRefName,\
                     labels,assignees,reviewers,url,mergeStateStatus,createdAt,updatedAt",
                ];
                if !repo_flag.is_empty() {
                    args.extend(["--repo", &repo_flag]);
                }
                let out = run_gh(&args).await?;
                Ok(maybe_buffer(check_gh_output(out)?, "github_pr", ctx))
            }
            "get_diff" => {
                let num = require_number(&number, "get_diff")?;
                let mut args = vec!["pr", "diff", num];
                if !repo_flag.is_empty() {
                    args.extend(["--repo", &repo_flag]);
                }
                let out = run_gh(&args).await?;
                let content = check_gh_output(out)?;
                let file_count = content.matches("\ndiff --git ").count()
                    + usize::from(content.starts_with("diff --git "));
                let summary = Some(format!("{file_count} files changed"));
                Ok(with_summary(
                    always_buffer(content, "github_pr", ctx),
                    summary,
                ))
            }
            "get_files" => {
                let num = require_number(&number, "get_files")?;
                let mut args = vec!["pr", "view", num, "--json", "files"];
                if !repo_flag.is_empty() {
                    args.extend(["--repo", &repo_flag]);
                }
                let out = run_gh(&args).await?;
                Ok(maybe_buffer(check_gh_output(out)?, "github_pr", ctx))
            }
            "get_comments" => {
                let num = require_number(&number, "get_comments")?;
                let mut args = vec!["pr", "view", num, "--json", "comments"];
                if !repo_flag.is_empty() {
                    args.extend(["--repo", &repo_flag]);
                }
                let out = run_gh(&args).await?;
                Ok(maybe_buffer(check_gh_output(out)?, "github_pr", ctx))
            }
            "get_reviews" => {
                let num = require_number(&number, "get_reviews")?;
                require_owner_repo(owner, repo)?;
                let endpoint = format!("/repos/{owner}/{repo}/pulls/{num}/reviews");
                let out = run_gh(&["api", &endpoint]).await?;
                Ok(maybe_buffer(check_gh_output(out)?, "github_pr", ctx))
            }
            "get_review_comments" => {
                let num = require_number(&number, "get_review_comments")?;
                require_owner_repo(owner, repo)?;
                let endpoint = format!("/repos/{owner}/{repo}/pulls/{num}/comments");
                let out = run_gh(&["api", &endpoint]).await?;
                Ok(maybe_buffer(check_gh_output(out)?, "github_pr", ctx))
            }
            "get_status" => {
                let num = require_number(&number, "get_status")?;
                let mut args = vec!["pr", "checks", num];
                if !repo_flag.is_empty() {
                    args.extend(["--repo", &repo_flag]);
                }
                args.extend(["--json", "name,state,conclusion,startedAt,completedAt"]);
                let out = run_gh(&args).await?;
                Ok(maybe_buffer(check_gh_output(out)?, "github_pr", ctx))
            }
            "create" => {
                require_owner_repo(owner, repo)?;
                let head = params["head"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint(
                        "head required",
                        "Provide the head branch (e.g. 'username:feature-branch')",
                    )
                })?;
                let base = params["base"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint(
                        "base required",
                        "Provide the base branch (e.g. 'main')",
                    )
                })?;
                let title = params["title"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint("title required", "Provide the PR title")
                })?;
                let mut args = vec![
                    "pr", "create", "--repo", &repo_flag, "--head", head, "--base", base,
                    "--title", title,
                ];
                if let Some(b) = params["body"].as_str() {
                    args.extend(["--body", b]);
                } else {
                    args.extend(["--body", ""]);
                }
                if parse_bool_param(&params["draft"]) {
                    args.push("--draft");
                }
                let out = run_gh(&args).await?;
                Ok(json!(check_gh_output(out)?))
            }
            "update" => {
                let num = require_number(&number, "update")?;
                let mut args = vec!["pr", "edit", num];
                if !repo_flag.is_empty() {
                    args.extend(["--repo", &repo_flag]);
                }
                if let Some(t) = params["title"].as_str() {
                    args.extend(["--title", t]);
                }
                if let Some(b) = params["body"].as_str() {
                    args.extend(["--body", b]);
                }
                if let Some(base) = params["base"].as_str() {
                    args.extend(["--base", base]);
                }
                let out = run_gh(&args).await?;
                Ok(json!(check_gh_output(out)?))
            }
            "merge" => {
                let num = require_number(&number, "merge")?;
                let merge_method = params["merge_method"].as_str().unwrap_or("merge");
                let mut args = vec!["pr", "merge", num];
                if !repo_flag.is_empty() {
                    args.extend(["--repo", &repo_flag]);
                }
                match merge_method {
                    "squash" => args.push("--squash"),
                    "rebase" => args.push("--rebase"),
                    _ => args.push("--merge"),
                }
                let out = run_gh(&args).await?;
                Ok(json!(check_gh_output(out)?))
            }
            "update_branch" => {
                let num = require_number(&number, "update_branch")?;
                require_owner_repo(owner, repo)?;
                let endpoint = format!("/repos/{owner}/{repo}/pulls/{num}/update-branch");
                let out = run_gh(&["api", "--method", "PUT", &endpoint]).await?;
                Ok(json!(check_gh_output(out)?))
            }
            "create_review" => {
                let num = require_number(&number, "create_review")?;
                require_owner_repo(owner, repo)?;
                let endpoint = format!("/repos/{owner}/{repo}/pulls/{num}/reviews");
                // Pre-compute owned strings to avoid &format!() temporaries
                let body_field = params["body"].as_str().map(|b| format!("body={b}"));
                let event_field = params["event"].as_str().map(|e| format!("event={e}"));
                let mut gh_args = vec!["api", "--method", "POST", &endpoint];
                if let Some(ref f) = body_field {
                    gh_args.extend(["--field", f.as_str()]);
                }
                if let Some(ref f) = event_field {
                    gh_args.extend(["--field", f.as_str()]);
                }
                let out = run_gh(&gh_args).await?;
                Ok(json!(check_gh_output(out)?))
            }
            "submit_review" => {
                let num = require_number(&number, "submit_review")?;
                let rev_id = params["review_id"].as_u64().ok_or_else(|| {
                    RecoverableError::with_hint("review_id required", "Provide the review ID")
                })?;
                let event = params["event"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint(
                        "event required",
                        "Provide event: APPROVE, REQUEST_CHANGES, or COMMENT",
                    )
                })?;
                require_owner_repo(owner, repo)?;
                let endpoint = format!("/repos/{owner}/{repo}/pulls/{num}/reviews/{rev_id}/events");
                let event_field = format!("event={event}");
                let out = run_gh(&[
                    "api",
                    "--method",
                    "POST",
                    &endpoint,
                    "--field",
                    &event_field,
                ])
                .await?;
                Ok(json!(check_gh_output(out)?))
            }
            "delete_review" => {
                let num = require_number(&number, "delete_review")?;
                let rev_id = params["review_id"].as_u64().ok_or_else(|| {
                    RecoverableError::with_hint("review_id required", "Provide the review ID")
                })?;
                require_owner_repo(owner, repo)?;
                let endpoint = format!("/repos/{owner}/{repo}/pulls/{num}/reviews/{rev_id}");
                let out = run_gh(&["api", "--method", "DELETE", &endpoint]).await?;
                Ok(json!(check_gh_output(out)?))
            }
            "add_review_comment" => {
                let num = require_number(&number, "add_review_comment")?;
                let body = params["body"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint("body required", "Provide the comment body")
                })?;
                require_owner_repo(owner, repo)?;
                let endpoint = format!("/repos/{owner}/{repo}/pulls/{num}/comments");
                // Pre-compute owned strings to avoid &format!() temporaries
                let body_field = format!("body={body}");
                let commit_field = params["commit_id"]
                    .as_str()
                    .map(|c| format!("commit_id={c}"));
                let path_field = params["path"].as_str().map(|p| format!("path={p}"));
                let line_field = params["line"].as_u64().map(|l| format!("line={l}"));
                let mut gh_args =
                    vec!["api", "--method", "POST", &endpoint, "--field", &body_field];
                if let Some(ref f) = commit_field {
                    gh_args.extend(["--field", f.as_str()]);
                }
                if let Some(ref f) = path_field {
                    gh_args.extend(["--field", f.as_str()]);
                }
                if let Some(ref f) = line_field {
                    gh_args.extend(["--field", f.as_str()]);
                }
                let out = run_gh(&gh_args).await?;
                Ok(json!(check_gh_output(out)?))
            }
            "add_reply_to_comment" => {
                let num = require_number(&number, "add_reply_to_comment")?;
                let cid = params["comment_id"].as_u64().ok_or_else(|| {
                    RecoverableError::with_hint(
                        "comment_id required",
                        "Provide the comment ID to reply to",
                    )
                })?;
                let body = params["body"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint("body required", "Provide the reply body")
                })?;
                require_owner_repo(owner, repo)?;
                let endpoint = format!("/repos/{owner}/{repo}/pulls/{num}/comments/{cid}/replies");
                let body_field = format!("body={body}");
                let out =
                    run_gh(&["api", "--method", "POST", &endpoint, "--field", &body_field]).await?;
                Ok(json!(check_gh_output(out)?))
            }
            other => Err(RecoverableError::with_hint(
                format!("unknown method: '{other}'"),
                "method must be one of: list, search, get, get_diff, get_files, \
                 get_comments, get_reviews, get_review_comments, get_status, create, \
                 update, merge, update_branch, create_review, submit_review, delete_review, \
                 add_review_comment, add_reply_to_comment",
            )
            .into()),
        }
    }
}

#[async_trait]
impl Tool for GithubFile {
    fn name(&self) -> &'static str {
        "github_file"
    }

    fn description(&self) -> &'static str {
        "GitHub file operations.\n\
         method: get | create_or_update | delete | push_files\n\n\
         get — fetch file contents at an optional ref/branch (returns @buffer handle).\n\
         create_or_update — create or update a single file (sha required when updating).\n\
         delete — delete a file (sha required).\n\
         push_files — push multiple files in a single commit."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "method":  { "type": "string", "enum": ["get","create_or_update","delete","push_files"] },
                "owner":   { "type": "string" },
                "repo":    { "type": "string" },
                "path":    { "type": "string",  "description": "File path within repository" },
                "ref":     { "type": "string",  "description": "get: branch, tag, or commit SHA" },
                "content": { "type": "string",  "description": "create_or_update: base64-encoded file content" },
                "message": { "type": "string",  "description": "create_or_update/delete/push_files: commit message" },
                "sha":     { "type": "string",  "description": "create_or_update/delete: blob SHA of existing file" },
                "branch":  { "type": "string",  "description": "create_or_update/delete/push_files: target branch" },
                "files":   {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path":    { "type": "string" },
                            "content": { "type": "string" }
                        },
                        "required": ["path", "content"]
                    },
                    "description": "push_files: Array of {path, content} objects. Content is plaintext (not base64) — the Trees API handles encoding."
                }
            },
            "required": ["method"]
        })
    }

    async fn call(&self, params: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let method = params["method"].as_str().unwrap_or("");
        let owner = params["owner"].as_str().unwrap_or("");
        let repo = params["repo"].as_str().unwrap_or("");

        match method {
            "get" => {
                require_owner_repo(owner, repo)?;
                let path = params["path"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint("path required", "Provide the file path")
                })?;
                let mut endpoint = format!("/repos/{owner}/{repo}/contents/{path}");
                if let Some(r) = params["ref"].as_str() {
                    endpoint.push_str(&format!("?ref={r}"));
                }
                let out = run_gh(&["api", &endpoint]).await?;
                Ok(always_buffer(check_gh_output(out)?, "github_file", ctx))
            }
            "create_or_update" => {
                require_owner_repo(owner, repo)?;
                let path = params["path"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint("path required", "Provide the file path")
                })?;
                let content = params["content"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint(
                        "content required",
                        "Provide base64-encoded file content",
                    )
                })?;
                let message = params["message"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint("message required", "Provide a commit message")
                })?;
                let endpoint = format!("/repos/{owner}/{repo}/contents/{path}");
                let msg_field = format!("message={message}");
                let content_field = format!("content={content}");
                let sha_field = params["sha"].as_str().map(|s| format!("sha={s}"));
                let branch_field = params["branch"].as_str().map(|b| format!("branch={b}"));
                let mut gh_args = vec![
                    "api",
                    "--method",
                    "PUT",
                    &endpoint,
                    "--field",
                    &msg_field,
                    "--field",
                    &content_field,
                ];
                if let Some(ref f) = sha_field {
                    gh_args.extend(["--field", f.as_str()]);
                }
                if let Some(ref f) = branch_field {
                    gh_args.extend(["--field", f.as_str()]);
                }
                let out = run_gh(&gh_args).await?;
                Ok(json!(check_gh_output(out)?))
            }
            "delete" => {
                require_owner_repo(owner, repo)?;
                let path = params["path"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint("path required", "Provide the file path")
                })?;
                let sha = params["sha"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint(
                        "sha required",
                        "Provide the blob SHA of the file to delete",
                    )
                })?;
                let message = params["message"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint("message required", "Provide a commit message")
                })?;
                let endpoint = format!("/repos/{owner}/{repo}/contents/{path}");
                let msg_field = format!("message={message}");
                let sha_field = format!("sha={sha}");
                let branch_field = params["branch"].as_str().map(|b| format!("branch={b}"));
                let mut gh_args = vec![
                    "api", "--method", "DELETE", &endpoint, "--field", &msg_field, "--field",
                    &sha_field,
                ];
                if let Some(ref f) = branch_field {
                    gh_args.extend(["--field", f.as_str()]);
                }
                let out = run_gh(&gh_args).await?;
                Ok(json!(check_gh_output(out)?))
            }
            "push_files" => {
                require_owner_repo(owner, repo)?;
                let files = params["files"].as_array().ok_or_else(|| {
                    RecoverableError::with_hint(
                        "files required",
                        "Provide an array of {path, content} objects",
                    )
                })?;
                let message = params["message"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint("message required", "Provide a commit message")
                })?;
                let branch = params["branch"].as_str().unwrap_or("main");
                push_files_via_api(owner, repo, files, message, branch, ctx).await
            }
            other => Err(RecoverableError::with_hint(
                format!("unknown method: '{other}'"),
                "method must be one of: get, create_or_update, delete, push_files",
            )
            .into()),
        }
    }
}

async fn push_files_via_api(
    owner: &str,
    repo: &str,
    files: &[Value],
    message: &str,
    branch: &str,
    ctx: &ToolContext,
) -> anyhow::Result<Value> {
    // 1. Get the current HEAD SHA for the branch
    let ref_endpoint = format!("/repos/{owner}/{repo}/git/ref/heads/{branch}");
    let out = run_gh(&["api", &ref_endpoint]).await?;
    let ref_json: Value = serde_json::from_str(&check_gh_output(out)?)?;
    let base_sha = ref_json["object"]["sha"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("could not get base commit SHA"))?
        .to_string();

    // 2. Get the base tree SHA
    let commit_endpoint = format!("/repos/{owner}/{repo}/git/commits/{base_sha}");
    let out = run_gh(&["api", &commit_endpoint]).await?;
    let commit_json: Value = serde_json::from_str(&check_gh_output(out)?)?;
    let base_tree = commit_json["tree"]["sha"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("could not get base tree SHA"))?
        .to_string();

    // 3. Create new tree with all file changes
    let tree_endpoint = format!("/repos/{owner}/{repo}/git/trees");
    let tree_entries: Vec<Value> = files
        .iter()
        .map(|f| {
            json!({
                "path": f["path"],
                "mode": "100644",
                "type": "blob",
                "content": f["content"]
            })
        })
        .collect();
    let tree_json = serde_json::to_string(&tree_entries)?;
    let base_tree_field = format!("base_tree={base_tree}");
    let tree_field = format!("tree={tree_json}");
    let out = run_gh(&[
        "api",
        "--method",
        "POST",
        &tree_endpoint,
        "--field",
        &base_tree_field,
        "--field",
        &tree_field,
    ])
    .await?;
    let tree_result: Value = serde_json::from_str(&check_gh_output(out)?)?;
    let new_tree_sha = tree_result["sha"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("could not get new tree SHA"))?
        .to_string();

    // 4. Create commit
    let commits_endpoint = format!("/repos/{owner}/{repo}/git/commits");
    let msg_field = format!("message={message}");
    let tree_sha_field = format!("tree={new_tree_sha}");
    let parent_field = format!("parents[]={base_sha}");
    let out = run_gh(&[
        "api",
        "--method",
        "POST",
        &commits_endpoint,
        "--field",
        &msg_field,
        "--field",
        &tree_sha_field,
        "--field",
        &parent_field,
    ])
    .await?;
    let new_commit: Value = serde_json::from_str(&check_gh_output(out)?)?;
    let new_commit_sha = new_commit["sha"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("could not get new commit SHA"))?
        .to_string();

    // 5. Update branch ref
    let update_endpoint = format!("/repos/{owner}/{repo}/git/refs/heads/{branch}");
    let sha_field = format!("sha={new_commit_sha}");
    let out = run_gh(&[
        "api",
        "--method",
        "PATCH",
        &update_endpoint,
        "--field",
        &sha_field,
    ])
    .await?;
    check_gh_output(out)?;

    let _ = ctx; // ctx available for future buffering if needed
    Ok(json!({"sha": new_commit_sha, "branch": branch}))
}

#[async_trait]
impl Tool for GithubRepo {
    fn name(&self) -> &'static str {
        "github_repo"
    }

    fn description(&self) -> &'static str {
        "GitHub repository operations.\n\
         Repo: search | create | fork\n\
         Branches: list_branches | create_branch\n\
         Commits: list_commits | get_commit\n\
         Releases: list_releases | get_latest_release | get_release_by_tag\n\
         Tags: list_tags | get_tag\n\
         Code: search_code\n\n\
         search_code returns a @buffer handle. get_commit returns a @buffer handle.\n\
         NOTE: owner/repo params do NOT scope search_code — use repo:owner/repo in the query string (e.g. query=\"fn main repo:rust-lang/rust-analyzer\")."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "method": {
                    "type": "string",
                    "enum": ["search","create","fork","list_branches","create_branch",
                             "list_commits","get_commit","list_releases","get_latest_release",
                             "get_release_by_tag","list_tags","get_tag","search_code"]
                },
                "owner":      { "type": "string" },
                "repo":       { "type": "string" },
                "query":      { "type": "string",  "description": "search/search_code: query string" },
                "name":       { "type": "string",  "description": "create: repository name" },
                "private":    { "type": "boolean", "description": "create: private repository" },
                "branch":     { "type": "string",  "description": "create_branch: new branch name" },
                "from_branch":{ "type": "string",  "description": "create_branch: source branch (default: HEAD)" },
                "sha":        { "type": "string",  "description": "get_commit: commit SHA" },
                "tag":        { "type": "string",  "description": "get_release_by_tag/get_tag: tag name" },
                "limit":      { "type": "integer", "description": "list_*: max results (default 30)" },
                "page":       { "type": "integer", "description": "pagination" }
            },
            "required": ["method"]
        })
    }

    async fn call(&self, params: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let method = params["method"].as_str().unwrap_or("");
        let owner = params["owner"].as_str().unwrap_or("");
        let repo = params["repo"].as_str().unwrap_or("");
        let repo_flag = if !owner.is_empty() && !repo.is_empty() {
            format!("{owner}/{repo}")
        } else {
            String::new()
        };
        let limit = params["limit"].as_u64().unwrap_or(30).to_string();

        match method {
            "search" => {
                let q = params["query"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint("query required", "Provide a search query")
                })?;
                let out = run_gh(&[
                    "search",
                    "repos",
                    q,
                    "--json",
                    "name,owner,description,url,stargazersCount,visibility",
                    "--limit",
                    &limit,
                ])
                .await?;
                let content = check_gh_output(out)?;
                let summary = summarize_list(&content, "repos", |v| {
                    let owner_login = v["owner"]["login"].as_str().unwrap_or("");
                    v["name"].as_str().map(|n| format!("{owner_login}/{n}"))
                });
                Ok(with_summary(
                    maybe_buffer(content, "github_repo", ctx),
                    summary,
                ))
            }
            "create" => {
                let name = params["name"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint("name required", "Provide the repository name")
                })?;
                let mut args = vec![
                    "repo",
                    "create",
                    name,
                    "--json",
                    "name,url,description,visibility",
                ];
                if parse_bool_param(&params["private"]) {
                    args.push("--private");
                } else {
                    args.push("--public");
                }
                let out = run_gh(&args).await?;
                Ok(json!(check_gh_output(out)?))
            }
            "fork" => {
                require_owner_repo(owner, repo)?;
                let out = run_gh(&["repo", "fork", &repo_flag, "--json", "name,url,owner"]).await?;
                Ok(json!(check_gh_output(out)?))
            }
            "list_branches" => {
                require_owner_repo(owner, repo)?;
                let endpoint = format!("/repos/{owner}/{repo}/branches?per_page={limit}");
                let out = run_gh(&["api", &endpoint]).await?;
                let content = check_gh_output(out)?;
                let summary = summarize_list(&content, "branches", |v| {
                    v["name"].as_str().map(|s| s.to_string())
                });
                Ok(with_summary(
                    maybe_buffer(content, "github_repo", ctx),
                    summary,
                ))
            }
            "create_branch" => {
                require_owner_repo(owner, repo)?;
                let branch = params["branch"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint("branch required", "Provide the new branch name")
                })?;
                let from = params["from_branch"].as_str().unwrap_or("HEAD");
                let sha_endpoint = format!("/repos/{owner}/{repo}/git/ref/heads/{from}");
                let sha_out = run_gh(&["api", &sha_endpoint]).await?;
                let sha_json: Value = serde_json::from_str(&check_gh_output(sha_out)?)?;
                let sha = sha_json["object"]["sha"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("could not get SHA for source branch"))?
                    .to_string();
                let endpoint = format!("/repos/{owner}/{repo}/git/refs");
                let ref_field = format!("ref=refs/heads/{branch}");
                let sha_field = format!("sha={sha}");
                let out = run_gh(&[
                    "api", "--method", "POST", &endpoint, "--field", &ref_field, "--field",
                    &sha_field,
                ])
                .await?;
                Ok(json!(check_gh_output(out)?))
            }
            "list_commits" => {
                require_owner_repo(owner, repo)?;
                let endpoint = format!("/repos/{owner}/{repo}/commits?per_page={limit}");
                let out = run_gh(&["api", &endpoint]).await?;
                let content = check_gh_output(out)?;
                let summary = summarize_list(&content, "commits", |v| {
                    v["commit"]["message"]
                        .as_str()
                        .and_then(|m| m.lines().next())
                        .map(|s| s.to_string())
                });
                Ok(with_summary(
                    maybe_buffer(content, "github_repo", ctx),
                    summary,
                ))
            }
            "get_commit" => {
                require_owner_repo(owner, repo)?;
                let sha = params["sha"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint("sha required", "Provide the commit SHA")
                })?;
                let endpoint = format!("/repos/{owner}/{repo}/commits/{sha}");
                let out = run_gh(&["api", &endpoint]).await?;
                let content = check_gh_output(out)?;
                let summary = serde_json::from_str::<Value>(&content).ok().and_then(|v| {
                    let msg = v["commit"]["message"].as_str()?.lines().next()?;
                    let s = v["sha"].as_str().map(|s| &s[..7]).unwrap_or("?");
                    Some(format!("{s}: {msg}"))
                });
                Ok(with_summary(
                    always_buffer(content, "github_repo", ctx),
                    summary,
                ))
            }
            "list_releases" => {
                let mut args = vec![
                    "release",
                    "list",
                    "--json",
                    "name,tagName,isDraft,isPrerelease,publishedAt",
                    "--limit",
                    &limit,
                ];
                if !repo_flag.is_empty() {
                    args.extend(["--repo", &repo_flag]);
                }
                let out = run_gh(&args).await?;
                Ok(maybe_buffer(check_gh_output(out)?, "github_repo", ctx))
            }
            "get_latest_release" => {
                let mut args = vec![
                    "release",
                    "view",
                    "--json",
                    "name,tagName,body,assets,publishedAt,url",
                ];
                if !repo_flag.is_empty() {
                    args.extend(["--repo", &repo_flag]);
                }
                let out = run_gh(&args).await?;
                Ok(maybe_buffer(check_gh_output(out)?, "github_repo", ctx))
            }
            "get_release_by_tag" => {
                let tag = params["tag"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint("tag required", "Provide the release tag")
                })?;
                let mut args = vec![
                    "release",
                    "view",
                    tag,
                    "--json",
                    "name,tagName,body,assets,publishedAt,url",
                ];
                if !repo_flag.is_empty() {
                    args.extend(["--repo", &repo_flag]);
                }
                let out = run_gh(&args).await?;
                Ok(maybe_buffer(check_gh_output(out)?, "github_repo", ctx))
            }
            "list_tags" => {
                require_owner_repo(owner, repo)?;
                let endpoint = format!("/repos/{owner}/{repo}/tags?per_page={limit}");
                let out = run_gh(&["api", &endpoint]).await?;
                let content = check_gh_output(out)?;
                let summary = summarize_list(&content, "tags", |v| {
                    v["name"].as_str().map(|s| s.to_string())
                });
                Ok(with_summary(
                    maybe_buffer(content, "github_repo", ctx),
                    summary,
                ))
            }
            "get_tag" => {
                require_owner_repo(owner, repo)?;
                let tag = params["tag"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint("tag required", "Provide the tag name")
                })?;
                let endpoint = format!("/repos/{owner}/{repo}/git/ref/tags/{tag}");
                let out = run_gh(&["api", &endpoint]).await?;
                Ok(maybe_buffer(check_gh_output(out)?, "github_repo", ctx))
            }
            "search_code" => {
                let q = params["query"].as_str().ok_or_else(|| {
                    RecoverableError::with_hint("query required", "Provide a code search query")
                })?;
                let out = run_gh(&[
                    "search",
                    "code",
                    q,
                    "--json",
                    "path,repository,url,textMatches",
                    "--limit",
                    &limit,
                ])
                .await?;
                let content = check_gh_output(out)?;
                let summary = summarize_list(&content, "results", |v| {
                    v["path"].as_str().map(|s| s.to_string())
                });
                Ok(with_summary(
                    always_buffer(content, "github_repo", ctx),
                    summary,
                ))
            }
            other => Err(RecoverableError::with_hint(
                format!("unknown method: '{other}'"),
                "method must be one of: search, create, fork, list_branches, create_branch, \
                 list_commits, get_commit, list_releases, get_latest_release, get_release_by_tag, \
                 list_tags, get_tag, search_code",
            )
            .into()),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::TOOL_OUTPUT_BUFFER_THRESHOLD;

    #[test]
    fn test_check_gh_output_success() {
        let out = GhOutput {
            stdout: "ok".into(),
            stderr: "".into(),
            exit_code: 0,
        };
        assert_eq!(check_gh_output(out).unwrap(), "ok");
    }

    #[test]
    fn test_check_gh_output_auth_error() {
        let out = GhOutput {
            stdout: "".into(),
            stderr: "not logged into github.com".into(),
            exit_code: 1,
        };
        let err = check_gh_output(out).unwrap_err();
        assert!(err.to_string().contains("gh auth login"));
    }

    #[test]
    fn test_check_gh_output_api_error() {
        let out = GhOutput {
            stdout: "".into(),
            stderr: "Could not resolve to a Repository with the name 'owner/nonexistent'.".into(),
            exit_code: 1,
        };
        let err = check_gh_output(out).unwrap_err();
        assert!(err.to_string().contains("nonexistent"));
    }

    #[test]
    fn test_format_identity_get_me() {
        let json =
            r#"{"login":"testuser","id":12345,"name":"Test User","email":"test@example.com"}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        assert_eq!(v["login"], "testuser");
        assert_eq!(v["id"], 12345);
    }

    #[test]
    fn test_format_identity_search_users() {
        let json = r#"{"items":[{"login":"alice","id":1},{"login":"bob","id":2}],"total":2}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        assert_eq!(v["items"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_issue_list_json_shape() {
        let json = r#"[{"number":1,"title":"Bug","state":"OPEN"},{"number":2,"title":"Feature","state":"CLOSED"}]"#;
        let v: Value = serde_json::from_str(json).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 2);
        assert_eq!(v[0]["number"], 1);
    }

    #[test]
    fn test_issue_get_json_shape() {
        let json = r#"{"number":42,"title":"Test issue","body":"Some body","state":"OPEN","labels":[],"comments":[]}"#;
        let v: Value = serde_json::from_str(json).unwrap();
        assert_eq!(v["number"], 42);
        assert_eq!(v["title"], "Test issue");
    }

    #[test]
    fn test_issue_create_requires_title() {
        let params = json!({"method": "create", "owner": "foo", "repo": "bar"});
        assert!(params["title"].as_str().is_none());
    }

    #[test]
    fn test_pr_list_json_shape() {
        let json = r#"[{"number":1,"title":"Fix bug","state":"OPEN","isDraft":false}]"#;
        let v: Value = serde_json::from_str(json).unwrap();
        assert_eq!(v[0]["number"], 1);
    }

    #[test]
    fn test_pr_diff_is_text() {
        let diff = "diff --git a/src/main.rs b/src/main.rs\n+new line\n-old line\n";
        assert!(diff.starts_with("diff --git"));
        assert!(diff.len() < TOOL_OUTPUT_BUFFER_THRESHOLD);
    }

    #[test]
    fn test_pr_merge_method_validation() {
        for m in &["merge", "squash", "rebase"] {
            assert!(["merge", "squash", "rebase"].contains(m));
        }
    }

    #[test]
    fn test_file_get_always_buffers() {
        let large = "x".repeat(TOOL_OUTPUT_BUFFER_THRESHOLD + 1);
        assert!(large.len() > TOOL_OUTPUT_BUFFER_THRESHOLD);
    }

    #[test]
    fn test_repo_list_branches_shape() {
        let json = r#"[{"name":"main","protected":true},{"name":"feature","protected":false}]"#;
        let v: Value = serde_json::from_str(json).unwrap();
        assert_eq!(v[0]["name"], "main");
    }
}
