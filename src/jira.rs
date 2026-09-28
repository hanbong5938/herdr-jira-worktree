//! Minimal Jira REST client (blocking, ureq).
//!
//! Works against both Jira Cloud and Server/Data Center:
//! - search first tries the new `/rest/api/2/search/jql` endpoint (Cloud replaced
//!   the classic `/search` with it in 2025), then falls back to the classic
//!   `/rest/api/2/search` (Server/DC and older instances);
//! - the v2 API returns descriptions as plain text on Server/DC, but some Cloud
//!   responses carry Atlassian Document Format (ADF) objects — both are handled.

use base64::Engine;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Issue {
    pub key: String,
    pub summary: String,
    pub status: String,
    pub status_category: String, // "new" | "indeterminate" | "done"
    pub issue_type: String,
    pub priority: String,
    pub assignee: String,
    pub reporter: String,
    pub updated: String,
    pub labels: Vec<String>,
    pub description: String,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct Transition {
    pub id: String,
    pub name: String,
    pub to_status: String,
}

#[derive(Debug, Clone)]
pub struct Comment {
    pub author: String,
    pub created: String, // "YYYY-MM-DD HH:MM", like Issue.updated
    pub body: String,
}

#[derive(Clone)]
pub struct JiraClient {
    base: String,
    auth_header: String,
    max_results: u32,
    agent: ureq::Agent,
}

const FIELDS: &str =
    "summary,status,issuetype,priority,assignee,reporter,updated,labels,description";

impl JiraClient {
    pub fn new(cfg: &crate::config::Config) -> Result<Self, String> {
        let token = cfg.resolve_token()?;
        let auth_header = match cfg.jira.auth.as_str() {
            "bearer" => format!("Bearer {token}"),
            "basic" => {
                if cfg.jira.email.trim().is_empty() {
                    return Err("auth = \"basic\" requires [jira].email".into());
                }
                let creds = format!("{}:{}", cfg.jira.email.trim(), token);
                format!(
                    "Basic {}",
                    base64::engine::general_purpose::STANDARD.encode(creds)
                )
            }
            other => {
                return Err(format!(
                    "unknown [jira].auth \"{other}\" (use basic|bearer)"
                ))
            }
        };
        Ok(Self {
            base: cfg.jira.base_url.clone(),
            auth_header,
            max_results: cfg.jira.max_results.max(1),
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(20))
                .build(),
        })
    }

    fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
        let mut req = self
            .agent
            .get(&format!("{}{}", self.base, path))
            .set("Authorization", &self.auth_header)
            .set("Accept", "application/json");
        for (k, v) in query {
            req = req.query(k, v);
        }
        Self::finish(req.call())
    }

    fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        let req = self
            .agent
            .post(&format!("{}{}", self.base, path))
            .set("Authorization", &self.auth_header)
            .set("Accept", "application/json");
        Self::finish(req.send_json(body))
    }

    fn finish(res: Result<ureq::Response, ureq::Error>) -> Result<Value, String> {
        match res {
            Ok(resp) => {
                let text = resp.into_string().map_err(|e| format!("read body: {e}"))?;
                if text.trim().is_empty() {
                    return Ok(Value::Null);
                }
                serde_json::from_str(&text).map_err(|e| format!("bad JSON from Jira: {e}"))
            }
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                Err(format!("HTTP {code}: {}", extract_error(&body)))
            }
            Err(e) => Err(format!("request failed: {e}")),
        }
    }

    pub fn search(&self, jql: &str) -> Result<Vec<Issue>, String> {
        let max = self.max_results.to_string();
        let query: &[(&str, &str)] = &[("jql", jql), ("maxResults", &max), ("fields", FIELDS)];
        // New endpoint first (Jira Cloud), classic /search as fallback (Server/DC).
        let result = match self.get("/rest/api/2/search/jql", query) {
            Ok(v) => Ok(v),
            Err(e)
                if e.starts_with("HTTP 404")
                    || e.starts_with("HTTP 405")
                    || e.starts_with("HTTP 410") =>
            {
                self.get("/rest/api/2/search", query)
            }
            Err(e) => Err(e),
        }?;
        let issues = result["issues"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|v| self.parse_issue(v))
            .collect();
        Ok(issues)
    }

    fn parse_issue(&self, v: &Value) -> Issue {
        let f = &v["fields"];
        let key = v["key"].as_str().unwrap_or("?").to_string();
        Issue {
            url: format!("{}/browse/{}", self.base, key),
            key,
            summary: f["summary"].as_str().unwrap_or("").to_string(),
            status: f["status"]["name"].as_str().unwrap_or("?").to_string(),
            status_category: f["status"]["statusCategory"]["key"]
                .as_str()
                .unwrap_or("")
                .to_string(),
            issue_type: f["issuetype"]["name"].as_str().unwrap_or("").to_string(),
            priority: f["priority"]["name"].as_str().unwrap_or("—").to_string(),
            assignee: person(&f["assignee"]),
            reporter: person(&f["reporter"]),
            updated: f["updated"].as_str().map(short_time).unwrap_or_default(),
            labels: f["labels"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|l| l.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            description: description_text(&f["description"]),
        }
    }

    pub fn transitions(&self, key: &str) -> Result<Vec<Transition>, String> {
        let v = self.get(&format!("/rest/api/2/issue/{key}/transitions"), &[])?;
        Ok(v["transitions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|t| Transition {
                id: t["id"].as_str().unwrap_or("").to_string(),
                name: t["name"].as_str().unwrap_or("?").to_string(),
                to_status: t["to"]["name"].as_str().unwrap_or("?").to_string(),
            })
            .collect())
    }

    pub fn apply_transition(&self, key: &str, transition_id: &str) -> Result<(), String> {
        self.post(
            &format!("/rest/api/2/issue/{key}/transitions"),
            serde_json::json!({ "transition": { "id": transition_id } }),
        )?;
        Ok(())
    }

    pub fn comments(&self, key: &str) -> Result<Vec<Comment>, String> {
        let v = self.get(
            &format!("/rest/api/2/issue/{key}/comment"),
            &[("orderBy", "-created"), ("maxResults", "50")],
        )?;
        let mut comments = parse_comments(&v);
        // Cloud wiki bodies reference users as `[~accountid:…]`. Comment
        // authors name most of them for free; look up the rest in bulk.
        let mut names = author_names(&v);
        let mut unknown: Vec<&str> = Vec::new();
        for c in &comments {
            for id in mention_ids(&c.body) {
                if !names.contains_key(id) && !unknown.contains(&id) {
                    unknown.push(id);
                }
            }
        }
        let unknown: Vec<String> = unknown.into_iter().map(String::from).collect();
        for chunk in unknown.chunks(50) {
            // Best effort: on failure the raw mention stays visible.
            if let Ok(users) = self.users_bulk(chunk) {
                names.extend(users);
            }
        }
        for c in &mut comments {
            c.body = replace_mentions(&c.body, &names);
        }
        Ok(comments)
    }

    /// accountId → display name via Cloud's `/user/bulk` (≤ 50 ids per call).
    fn users_bulk(&self, ids: &[String]) -> Result<HashMap<String, String>, String> {
        let max = ids.len().to_string();
        let mut query: Vec<(&str, &str)> = vec![("maxResults", &max)];
        query.extend(ids.iter().map(|id| ("accountId", id.as_str())));
        let v = self.get("/rest/api/2/user/bulk", &query)?;
        Ok(v["values"]
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or_default()
            .iter()
            .filter_map(user_entry)
            .collect())
    }
}

/// `(accountId, displayName)` of a Cloud user object.
fn user_entry(u: &Value) -> Option<(String, String)> {
    Some((
        u["accountId"].as_str()?.to_string(),
        u["displayName"].as_str()?.to_string(),
    ))
}

/// accountId → display name for every comment author in a comment payload.
fn author_names(v: &Value) -> HashMap<String, String> {
    v["comments"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|c| user_entry(&c["author"]))
        .collect()
}

const MENTION_OPEN: &str = "[~accountid:";

/// Account ids of `[~accountid:…]` mentions in a wiki-markup body.
fn mention_ids(body: &str) -> impl Iterator<Item = &str> {
    body.split(MENTION_OPEN)
        .skip(1)
        .filter_map(|rest| rest.split_once(']').map(|(id, _)| id))
}

/// Rewrite `[~accountid:…]` as `@Display Name`; unknown ids stay as-is.
fn replace_mentions(body: &str, names: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find(MENTION_OPEN) {
        out.push_str(&rest[..start]);
        let after = &rest[start + MENTION_OPEN.len()..];
        match after
            .split_once(']')
            .and_then(|(id, tail)| Some((names.get(id)?, tail)))
        {
            Some((name, tail)) => {
                out.push('@');
                out.push_str(name);
                rest = tail;
            }
            None => {
                out.push_str(MENTION_OPEN);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Display name of a Jira user object, falling back to the login name.
fn person(p: &Value) -> String {
    p["displayName"]
        .as_str()
        .or_else(|| p["name"].as_str())
        .unwrap_or("—")
        .to_string()
}

/// "2024-01-02T10:00:00.000+0000" → "2024-01-02 10:00".
fn short_time(s: &str) -> String {
    s.chars().take(16).collect::<String>().replace('T', " ")
}

/// Parse a `/issue/{key}/comment` payload, newest first. Server/DC may ignore
/// `orderBy`, so sort client-side on the raw ISO timestamp.
pub fn parse_comments(v: &Value) -> Vec<Comment> {
    let mut raw: Vec<(&str, Comment)> = v["comments"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|c| {
            let created = c["created"].as_str().unwrap_or("");
            (
                created,
                Comment {
                    author: person(&c["author"]),
                    created: short_time(created),
                    body: description_text(&c["body"]),
                },
            )
        })
        .collect();
    raw.sort_by(|a, b| b.0.cmp(a.0));
    raw.into_iter().map(|(_, c)| c).collect()
}

/// Jira error payloads look like {"errorMessages":[...],"errors":{...}}.
fn extract_error(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        let mut parts: Vec<String> = Vec::new();
        if let Some(msgs) = v["errorMessages"].as_array() {
            parts.extend(msgs.iter().filter_map(|m| m.as_str().map(String::from)));
        }
        if let Some(errs) = v["errors"].as_object() {
            parts.extend(
                errs.iter()
                    .map(|(k, val)| format!("{k}: {}", val.as_str().unwrap_or_default())),
            );
        }
        if !parts.is_empty() {
            return parts.join("; ");
        }
    }
    let trimmed = body.trim();
    if trimmed.is_empty() {
        "no error body".into()
    } else {
        trimmed.chars().take(300).collect()
    }
}

/// Description is a plain string on API v2 (Server/DC), but may arrive as an
/// ADF document object from Cloud — flatten either to displayable text.
fn description_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Object(_) => {
            let mut out = String::new();
            adf_walk(v, &mut out);
            out.trim().to_string()
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_plain_string_passes_through() {
        assert_eq!(
            description_text(&Value::String("hi\nthere".into())),
            "hi\nthere"
        );
        assert_eq!(description_text(&Value::Null), "");
    }

    #[test]
    fn description_adf_flattens_to_text() {
        let adf: Value = serde_json::json!({
            "type": "doc", "version": 1,
            "content": [
                {"type": "paragraph", "content": [
                    {"type": "text", "text": "first"},
                    {"type": "hardBreak"},
                    {"type": "text", "text": "second"}
                ]},
                {"type": "bulletList", "content": [
                    {"type": "listItem", "content": [
                        {"type": "paragraph", "content": [{"type": "text", "text": "item"}]}
                    ]}
                ]}
            ]
        });
        assert_eq!(description_text(&adf), "first\nsecond\n- item");
    }

    #[test]
    fn jira_error_payload_is_extracted() {
        let body = r#"{"errorMessages":["Issue does not exist"],"errors":{"status":"bad"}}"#;
        assert_eq!(extract_error(body), "Issue does not exist; status: bad");
        assert_eq!(extract_error("plain"), "plain");
    }

    #[test]
    fn comments_parse_plain_and_adf_newest_first() {
        let v = serde_json::json!({
            "comments": [
                {
                    "author": {"name": "jdoe"},
                    "created": "2024-01-02T10:00:00.000+0000",
                    "body": "older plain"
                },
                {
                    "author": {"displayName": "Ann Lee", "name": "alee"},
                    "created": "2024-03-05T08:30:00.000+0000",
                    "body": {"type": "doc", "version": 1, "content": [
                        {"type": "paragraph", "content": [{"type": "text", "text": "newer adf"}]}
                    ]}
                },
                {"created": "2023-12-31T23:59:00.000+0000", "body": null}
            ]
        });
        let cs = parse_comments(&v);
        assert_eq!(cs.len(), 3);
        assert_eq!(cs[0].author, "Ann Lee");
        assert_eq!(cs[0].created, "2024-03-05 08:30");
        assert_eq!(cs[0].body, "newer adf");
        assert_eq!(cs[1].author, "jdoe");
        assert_eq!(cs[1].body, "older plain");
        assert_eq!(cs[2].author, "—");
        assert_eq!(cs[2].body, "");
        assert!(parse_comments(&Value::Null).is_empty());
    }

    #[test]
    fn account_mentions_become_names_unknown_kept() {
        let names = HashMap::from([
            ("a1".to_string(), "류현욱".to_string()),
            ("b2".to_string(), "Ann Lee".to_string()),
        ]);
        let body = "[~accountid:a1] [~accountid:b2]\nhi [~accountid:zz] and [~accountid:a1]. [~accountid:open";
        assert_eq!(
            mention_ids(body).collect::<Vec<_>>(),
            ["a1", "b2", "zz", "a1"]
        );
        assert_eq!(
            replace_mentions(body, &names),
            "@류현욱 @Ann Lee\nhi [~accountid:zz] and @류현욱. [~accountid:open"
        );
    }
}

fn adf_walk(v: &Value, out: &mut String) {
    match v["type"].as_str() {
        Some("text") => {
            out.push_str(v["text"].as_str().unwrap_or(""));
            return;
        }
        Some("hardBreak") => {
            out.push('\n');
            return;
        }
        Some("listItem") => out.push_str("- "),
        _ => {}
    }
    if let Some(children) = v["content"].as_array() {
        for c in children {
            adf_walk(c, out);
        }
    }
    // Block-level nodes end with a blank line.
    if matches!(
        v["type"].as_str(),
        Some("paragraph" | "heading" | "codeBlock" | "blockquote" | "listItem")
    ) && !out.ends_with('\n')
    {
        out.push('\n');
    }
}
