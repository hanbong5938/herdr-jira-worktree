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
use std::collections::{HashMap, HashSet};

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

#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: Option<usize>,
    pub has_more: bool,
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

    pub fn search(&self, jql: &str) -> Result<Page<Issue>, String> {
        let count = self.max_results.min(50).to_string();
        let query = [
            ("jql", jql),
            ("maxResults", count.as_str()),
            ("fields", FIELDS),
        ];
        // Only an unsupported *first* Cloud request selects the classic API.
        let page = match self.get("/rest/api/2/search/jql", &query) {
            Ok(first) => self.cloud_search(jql, first),
            Err(e)
                if ["HTTP 404:", "HTTP 405:", "HTTP 410:"]
                    .iter()
                    .any(|status| e.starts_with(status)) =>
            {
                let path = "/rest/api/2/search";
                let classic_query = [("jql", jql), ("fields", FIELDS)];
                let first = self.get(
                    path,
                    &[
                        ("jql", jql),
                        ("fields", FIELDS),
                        ("maxResults", &count),
                        ("startAt", "0"),
                    ],
                )?;
                self.classic_pages(
                    path,
                    &classic_query,
                    "issues",
                    first,
                    self.max_results as usize,
                )
            }
            Err(e) => Err(e),
        }?;
        Ok(Page {
            items: page.items.iter().map(|v| self.parse_issue(v)).collect(),
            total: page.total,
            has_more: page.has_more,
        })
    }

    fn cloud_search(&self, jql: &str, mut response: Value) -> Result<Page<Value>, String> {
        let cap = self.max_results as usize;
        let budget = cap.saturating_add(1);
        let mut requests = 1usize; // The first successful request was made by search().
        let mut items = Vec::new();
        let mut seen_ids = HashSet::new();
        let mut seen_tokens = HashSet::new();
        loop {
            let total = page_total(&response)?;
            let last = optional_bool(&response, "isLast")?;
            let page = take_array(&mut response, "issues")?;
            let token = optional_str(&response, "nextPageToken")?;
            if last == Some(true) && token.is_some() {
                return Err("Jira search has conflicting terminal and next-page metadata".into());
            }
            let size = page.len();
            let remaining = cap - items.len();
            retain_page(&mut items, &mut seen_ids, page, remaining, "issues")?;
            let collected = items.len();
            if size <= remaining && last == Some(true) && total.is_some_and(|n| n != collected) {
                return Err("Jira search dataset changed; terminal total conflicts with fetched items; retry".into());
            }
            if total.is_some_and(|n| n < collected) {
                return Err(
                    "Jira search dataset changed; total is smaller than fetched items; retry"
                        .into(),
                );
            }
            if let Some(token) = token {
                if !seen_tokens.insert(token.to_string()) {
                    return Err("Jira search repeated its next-page token; retry".into());
                }
            } else if last == Some(false) {
                return Err("Jira search has no next-page token".into());
            } else if last != Some(true) {
                return Err("Jira search page lacks terminal or continuation metadata".into());
            }
            if size > remaining || collected == cap || last == Some(true) {
                return Ok(Page {
                    items,
                    total,
                    has_more: size > remaining || token.is_some(),
                });
            }
            let token = token.expect("continuation checked above");
            if requests >= budget {
                return Err("Jira search page budget exhausted; retry".into());
            }
            requests += 1;
            let count = (cap - collected).min(50).to_string();
            response = self
                .get(
                    "/rest/api/2/search/jql",
                    &[
                        ("jql", jql),
                        ("fields", FIELDS),
                        ("maxResults", &count),
                        ("nextPageToken", token),
                    ],
                )
                .map_err(|error| format!("Jira search later page: {error}"))?;
        }
    }

    fn classic_pages(
        &self,
        path: &str,
        base_query: &[(&str, &str)],
        field: &str,
        mut response: Value,
        cap: usize,
    ) -> Result<Page<Value>, String> {
        let mut items = Vec::new();
        let mut seen_ids = HashSet::new();
        loop {
            let offset = items.len();
            if let Some(start) = optional_usize(&response, "startAt")? {
                if start != offset {
                    return Err(format!(
                        "Jira {field} returned an unexpected startAt {start}"
                    ));
                }
            }
            let total = page_total(&response)?;
            let page = take_array(&mut response, field)?;
            let size = page.len();
            let remaining = cap - offset;
            retain_page(&mut items, &mut seen_ids, page, remaining, field)?;
            let collected = items.len();
            if total.is_some_and(|n| n < collected) {
                return Err(format!(
                    "Jira {field} dataset changed; total is smaller than fetched items; retry"
                ));
            }
            if size == 0 {
                if total.is_some_and(|n| n > collected) {
                    return Err(format!(
                        "Jira {field} dataset changed; empty page before advertised total; retry"
                    ));
                }
                return Ok(Page {
                    items,
                    total,
                    has_more: false,
                });
            }
            if size > remaining || collected == cap {
                return Ok(Page {
                    items,
                    total,
                    has_more: size > remaining || total.map_or(true, |n| n > collected),
                });
            }
            if total == Some(collected) {
                return Ok(Page {
                    items,
                    total,
                    has_more: false,
                });
            }
            let count = (cap - collected).min(50).to_string();
            let offset = collected.to_string();
            let mut query = Vec::with_capacity(base_query.len() + 2);
            query.extend_from_slice(base_query);
            query.push(("maxResults", &count));
            query.push(("startAt", &offset));
            response = self.get(path, &query).map_err(|error| {
                if field == "issues" {
                    format!("Jira search later page: {error}")
                } else {
                    error
                }
            })?;
        }
    }

    pub fn issue(&self, key: &str) -> Result<Issue, String> {
        let value = self.get(&format!("/rest/api/2/issue/{key}"), &[("fields", FIELDS)])?;
        if value["key"].as_str() != Some(key) {
            return Err(format!("Jira returned an unexpected issue key for {key}"));
        }
        if value.get("fields").and_then(Value::as_object).is_none() {
            return Err(format!("Jira issue {key} is missing a fields object"));
        }
        Ok(self.parse_issue(&value))
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

    pub fn comments(&self, key: &str) -> Result<Page<Comment>, String> {
        self.comments_with_limit(key, self.max_results as usize)
    }

    /// Fetch up to `limit` comments independently of the list/search cap.
    /// A zero limit probes one comment for total/has_more metadata but returns
    /// no comment text.
    pub fn comments_with_limit(&self, key: &str, limit: usize) -> Result<Page<Comment>, String> {
        let path = format!("/rest/api/2/issue/{key}/comment");
        let count = limit.max(1).min(50).to_string();
        let first = self.get(
            &path,
            &[
                ("orderBy", "-created"),
                ("maxResults", &count),
                ("startAt", "0"),
            ],
        )?;
        let page = self.classic_pages(
            &path,
            &[("orderBy", "-created")],
            "comments",
            first,
            limit.max(1),
        )?;
        if limit == 0 {
            return Ok(Page {
                has_more: !page.items.is_empty() || page.has_more,
                total: page.total,
                items: Vec::new(),
            });
        }
        // Server/DC may ignore orderBy. Sort across all collected pages, not
        // within individual pages, and retain has_more if the cap cuts off data.
        let mut names = author_names(&page.items);
        let mut comments = parse_comment_items(&page.items);
        // Cloud wiki bodies reference users as `[~accountid:…]`. Comment
        // authors name most of them for free; look up the rest in bulk.
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
        Ok(Page {
            items: comments,
            total: page.total,
            has_more: page.has_more,
        })
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

fn take_array(response: &mut Value, field: &str) -> Result<Vec<Value>, String> {
    response
        .get_mut(field)
        .and_then(Value::as_array_mut)
        .map(std::mem::take)
        .ok_or_else(|| format!("Jira response missing {field} array"))
}

/// Keep only the raw prefix that fits the cap; validate its identities before
/// reporting a complete or partial page. Rows past the cap are not consumed.
fn retain_page(
    items: &mut Vec<Value>,
    seen_ids: &mut HashSet<String>,
    page: Vec<Value>,
    remaining: usize,
    field: &str,
) -> Result<(), String> {
    let identity = if field == "issues" { "key" } else { "id" };
    for row in page.into_iter().take(remaining) {
        let id = row
            .get(identity)
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| format!("Jira {field} has missing or invalid {identity}; retry"))?;
        if !seen_ids.insert(id.to_string()) {
            return Err(format!("Jira {field} has repeated {identity} {id}; retry"));
        }
        items.push(row);
    }
    Ok(())
}

fn optional_usize(response: &Value, field: &str) -> Result<Option<usize>, String> {
    match response.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| format!("Jira response has invalid {field}")),
    }
}

fn optional_bool(response: &Value, field: &str) -> Result<Option<bool>, String> {
    match response.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| format!("Jira response has invalid {field}")),
    }
}

fn optional_str<'a>(response: &'a Value, field: &str) -> Result<Option<&'a str>, String> {
    match response.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .filter(|s| !s.is_empty())
            .map(Some)
            .ok_or_else(|| format!("Jira response has invalid {field}")),
    }
}

fn page_total(response: &Value) -> Result<Option<usize>, String> {
    optional_usize(response, "total")
}

/// `(accountId, displayName)` of a Cloud user object.
fn user_entry(u: &Value) -> Option<(String, String)> {
    Some((
        u["accountId"].as_str()?.to_string(),
        u["displayName"].as_str()?.to_string(),
    ))
}

/// accountId → display name for every comment author in the collected pages.
fn author_names(comments: &[Value]) -> HashMap<String, String> {
    comments
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

/// Convert fetched comments newest first. Server/DC may ignore `orderBy`,
/// so sort client-side on the full ISO timestamp across pages.
fn parse_comment_items(items: &[Value]) -> Vec<Comment> {
    let mut raw: Vec<(&str, Option<i64>, Comment)> = items
        .iter()
        .map(|c| {
            let created = c["created"].as_str().unwrap_or("");
            (
                created,
                utc_millis(created),
                Comment {
                    author: person(&c["author"]),
                    created: short_time(created),
                    body: description_text(&c["body"]),
                },
            )
        })
        .collect();
    raw.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(a.0)));
    raw.into_iter().map(|(_, _, c)| c).collect()
}

fn utc_millis(timestamp: &str) -> Option<i64> {
    let part = |range: std::ops::Range<usize>| timestamp.get(range)?.parse::<i64>().ok();
    let year = part(0..4)?;
    let month = part(5..7)?;
    let day = part(8..10)?;
    let hour = part(11..13)?;
    let minute = part(14..16)?;
    let second = part(17..19)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let mut fraction = 0_i64;
    if let Some(rest) = timestamp.get(19..)?.strip_prefix('.') {
        let mut digits = 0;
        for byte in rest.bytes().take_while(u8::is_ascii_digit).take(3) {
            fraction = fraction * 10 + i64::from(byte - b'0');
            digits += 1;
        }
        if digits == 0 {
            return None;
        }
        for _ in digits..3 {
            fraction *= 10;
        }
    }
    let offset = match timestamp.get(19..)?.rfind(|c| c == '+' || c == '-') {
        Some(relative) => {
            let sign_at = 19 + relative;
            let hours = part(sign_at + 1..sign_at + 3)?;
            let minutes = if timestamp.get(sign_at + 3..sign_at + 4) == Some(":") {
                part(sign_at + 4..sign_at + 6)?
            } else {
                part(sign_at + 3..sign_at + 5)?
            };
            if hours > 23 || minutes > 59 {
                return None;
            }
            (hours * 60 + minutes)
                * if timestamp.as_bytes()[sign_at] == b'+' {
                    1
                } else {
                    -1
                }
        }
        None if timestamp.ends_with('Z') => 0,
        None => return None,
    };
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + (153 * mp + 2) / 5 + day - 1;
    Some((((days * 24 + hour) * 60 + minute - offset) * 60 + second) * 1000 + fraction)
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
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::{Duration, Instant};

    type Reply = (&'static str, Vec<&'static str>, u16, Value);

    fn loopback(cap: u32, replies: Vec<Reply>) -> (JiraClient, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            for (path, params, status, json) in replies {
                let deadline = Instant::now() + Duration::from_secs(3);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e)
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(e) => panic!("missing expected request {path}: {e}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut bytes = [0u8; 4096];
                let mut received = Vec::new();
                while !received.ends_with(b"\r\n\r\n") {
                    let count = stream.read(&mut bytes).unwrap();
                    assert!(
                        count > 0 && received.len() + count < 16_384,
                        "incomplete request headers"
                    );
                    received.extend_from_slice(&bytes[..count]);
                    if received.windows(4).any(|part| part == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = std::str::from_utf8(&received).unwrap();
                let target = request
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap();
                assert_eq!(target.split('?').next(), Some(path));
                assert!(
                    request.contains("Authorization: Bearer test")
                        || request.contains("authorization: Bearer test")
                );
                for param in params {
                    assert!(target.contains(param), "missing {param} in {target}");
                }
                let body = json.to_string();
                let reply = format!(
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(reply.as_bytes()).unwrap();
            }
        });
        (
            JiraClient {
                base,
                auth_header: "Bearer test".into(),
                max_results: cap,
                agent: ureq::AgentBuilder::new()
                    .timeout(Duration::from_secs(3))
                    .build(),
            },
            server,
        )
    }

    fn issues(start: usize, count: usize) -> Vec<Value> {
        (start..start + count)
            .map(|n| {
                serde_json::json!({
                    "key": format!("ABC-{n}"),
                    "fields": {"summary": format!("Summary {n}"), "description": "text"}
                })
            })
            .collect()
    }

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
        let cs = parse_comment_items(v["comments"].as_array().unwrap());
        assert_eq!(cs.len(), 3);
        assert_eq!(cs[0].author, "Ann Lee");
        assert_eq!(cs[0].created, "2024-03-05 08:30");
        assert_eq!(cs[0].body, "newer adf");
        assert_eq!(cs[1].author, "jdoe");
        assert_eq!(cs[1].body, "older plain");
        assert_eq!(cs[2].author, "—");
        assert_eq!(cs[2].body, "");
        assert!(parse_comment_items(&[]).is_empty());
    }

    #[test]
    fn comments_sort_by_instant_across_timezones() {
        let payload = serde_json::json!({"comments": [
            {"created": "2024-01-01T11:00:00.000+0200", "body": "older"},
            {"created": "2024-01-01T10:00:00.000+0000", "body": "newer"}
        ]});
        let comments = parse_comment_items(payload["comments"].as_array().unwrap());
        assert_eq!(comments[0].body, "newer");
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

    #[test]
    fn cloud_cap_is_total_limit_and_reports_partial() {
        let (client, server) = loopback(
            50,
            vec![(
                "/rest/api/2/search/jql",
                vec!["jql=", "fields=", "maxResults=50"],
                200,
                serde_json::json!({"issues": issues(0, 50), "total": 73, "isLast": false, "nextPageToken": "t1"}),
            )],
        );
        let page = client.search("project = ABC").unwrap();
        server.join().unwrap();
        assert_eq!(page.items.len(), 50);
        assert_eq!(page.total, Some(73));
        assert!(page.has_more);
        assert_eq!(page.items[49].key, "ABC-49");
    }

    #[test]
    fn cloud_fetches_successive_tokens_to_cap() {
        let (client, server) = loopback(
            120,
            vec![
                (
                    "/rest/api/2/search/jql",
                    vec!["jql=", "fields=", "maxResults=50"],
                    200,
                    serde_json::json!({"issues": issues(0, 50), "total": 120, "isLast": false, "nextPageToken": "t1"}),
                ),
                (
                    "/rest/api/2/search/jql",
                    vec!["jql=", "fields=", "maxResults=50", "nextPageToken=t1"],
                    200,
                    serde_json::json!({"issues": issues(50, 50), "total": 120, "isLast": false, "nextPageToken": "t2"}),
                ),
                (
                    "/rest/api/2/search/jql",
                    vec!["jql=", "fields=", "maxResults=20", "nextPageToken=t2"],
                    200,
                    serde_json::json!({"issues": issues(100, 20), "total": 120, "isLast": true}),
                ),
            ],
        );
        let page = client.search("project = ABC").unwrap();
        server.join().unwrap();
        assert_eq!(page.items.len(), 120);
        assert_eq!(page.items[119].key, "ABC-119");
        assert_eq!(page.total, Some(120));
        assert!(!page.has_more);
    }

    #[test]
    fn classic_fallback_advances_offset_and_preserves_total() {
        let (client, server) = loopback(
            120,
            vec![
                (
                    "/rest/api/2/search/jql",
                    vec!["maxResults=50"],
                    404,
                    serde_json::json!({"errorMessages": ["not supported"]}),
                ),
                (
                    "/rest/api/2/search",
                    vec!["jql=", "fields=", "startAt=0", "maxResults=50"],
                    200,
                    serde_json::json!({"issues": issues(0, 50), "startAt": 0, "total": 73}),
                ),
                (
                    "/rest/api/2/search",
                    vec!["jql=", "fields=", "startAt=50", "maxResults=50"],
                    200,
                    serde_json::json!({"issues": issues(50, 23), "startAt": 50, "total": 73}),
                ),
            ],
        );
        let page = client.search("project = ABC").unwrap();
        server.join().unwrap();
        assert_eq!(page.items.len(), 73);
        assert_eq!(page.total, Some(73));
        assert!(!page.has_more);
    }

    #[test]
    fn unauthorized_search_does_not_fallback_or_turn_into_empty() {
        let (client, server) = loopback(
            50,
            vec![(
                "/rest/api/2/search/jql",
                vec!["maxResults=50"],
                401,
                serde_json::json!({"errorMessages": ["invalid token"]}),
            )],
        );
        let error = client.search("project = ABC").unwrap_err();
        server.join().unwrap();
        assert!(error.contains("HTTP 401: invalid token"));
    }

    #[test]
    fn later_cloud_error_is_not_replaced_with_partial_results_or_classic_fallback() {
        let (client, server) = loopback(
            100,
            vec![
                (
                    "/rest/api/2/search/jql",
                    vec!["maxResults=50"],
                    200,
                    serde_json::json!({"issues": issues(0, 50), "isLast": false, "nextPageToken": "t1"}),
                ),
                (
                    "/rest/api/2/search/jql",
                    vec!["nextPageToken=t1"],
                    404,
                    serde_json::json!({"errorMessages": ["page expired"]}),
                ),
            ],
        );
        let error = client.search("project = ABC").unwrap_err();
        server.join().unwrap();
        assert!(error.contains("HTTP 404: page expired"));
    }

    #[test]
    fn later_classic_error_discards_collected_results() {
        let (client, server) = loopback(
            100,
            vec![
                ("/rest/api/2/search/jql", vec![], 410, serde_json::json!({})),
                (
                    "/rest/api/2/search",
                    vec!["startAt=0"],
                    200,
                    serde_json::json!({"issues": issues(0, 50), "startAt": 0, "total": 73}),
                ),
                (
                    "/rest/api/2/search",
                    vec!["startAt=50"],
                    503,
                    serde_json::json!({"errorMessages": ["try again"]}),
                ),
            ],
        );
        let error = client.search("project = ABC").unwrap_err();
        server.join().unwrap();
        assert!(error.contains("HTTP 503: try again"));
    }

    #[test]
    fn later_page_parent_400_cannot_masquerade_as_first_page_unsupported_jql() {
        let payload = serde_json::json!({"errorMessages": ["Field 'parent' does not exist"]});
        let (client, server) = loopback(
            100,
            vec![
                (
                    "/rest/api/2/search/jql",
                    vec!["maxResults=50"],
                    200,
                    serde_json::json!({"issues": issues(0, 50), "isLast": false, "nextPageToken": "t1"}),
                ),
                (
                    "/rest/api/2/search/jql",
                    vec!["nextPageToken=t1"],
                    400,
                    payload.clone(),
                ),
            ],
        );
        let error = client
            .search("parent = ABC-1 ORDER BY created ASC")
            .unwrap_err();
        server.join().unwrap();
        assert_eq!(
            error,
            "Jira search later page: HTTP 400: Field 'parent' does not exist"
        );
        assert!(!error.starts_with("HTTP 400:"));

        let (client, server) = loopback(
            100,
            vec![
                ("/rest/api/2/search/jql", vec![], 404, serde_json::json!({})),
                (
                    "/rest/api/2/search",
                    vec!["startAt=0"],
                    200,
                    serde_json::json!({"issues": issues(0, 50), "startAt": 0, "total": 73}),
                ),
                (
                    "/rest/api/2/search",
                    vec!["startAt=50"],
                    400,
                    payload.clone(),
                ),
            ],
        );
        let error = client
            .search("parent = ABC-1 ORDER BY created ASC")
            .unwrap_err();
        server.join().unwrap();
        assert_eq!(
            error,
            "Jira search later page: HTTP 400: Field 'parent' does not exist"
        );
        assert!(!error.starts_with("HTTP 400:"));

        let (client, server) =
            loopback(100, vec![("/rest/api/2/search/jql", vec![], 400, payload)]);
        let first_page_error = client
            .search("parent = ABC-1 ORDER BY created ASC")
            .unwrap_err();
        server.join().unwrap();
        assert_eq!(first_page_error, "HTTP 400: Field 'parent' does not exist");
    }

    #[test]
    fn malformed_pages_and_repeated_tokens_are_errors_but_zero_is_valid() {
        let (client, server) = loopback(
            100,
            vec![(
                "/rest/api/2/search/jql",
                vec![],
                200,
                serde_json::json!({"isLast": true}),
            )],
        );
        assert!(client
            .search("project = ABC")
            .unwrap_err()
            .contains("issues array"));
        server.join().unwrap();

        let (client, server) = loopback(
            100,
            vec![
                (
                    "/rest/api/2/search/jql",
                    vec![],
                    200,
                    serde_json::json!({"issues": issues(0, 1), "nextPageToken": "same"}),
                ),
                (
                    "/rest/api/2/search/jql",
                    vec!["nextPageToken=same"],
                    200,
                    serde_json::json!({"issues": issues(1, 1), "nextPageToken": "same"}),
                ),
            ],
        );
        assert!(client
            .search("project = ABC")
            .unwrap_err()
            .contains("repeated"));
        server.join().unwrap();

        let (client, server) = loopback(
            100,
            vec![(
                "/rest/api/2/search/jql",
                vec![],
                200,
                serde_json::json!({"issues": [], "isLast": true, "total": 0}),
            )],
        );
        let page = client.search("project = ABC").unwrap();
        server.join().unwrap();
        assert!(page.items.is_empty());
        assert_eq!(page.total, Some(0));
        assert!(!page.has_more);
    }

    #[test]
    fn oversized_cloud_page_stays_bounded_and_partial() {
        let (client, server) = loopback(
            50,
            vec![(
                "/rest/api/2/search/jql",
                vec!["maxResults=50"],
                200,
                serde_json::json!({"issues": issues(0, 73), "isLast": true, "total": 73}),
            )],
        );
        let page = client.search("project = ABC").unwrap();
        server.join().unwrap();
        assert_eq!(page.items.len(), 50);
        assert_eq!(page.total, Some(73));
        assert!(page.has_more);
    }

    #[test]
    fn oversized_classic_page_respects_cap() {
        let (client, server) = loopback(
            50,
            vec![
                ("/rest/api/2/search/jql", vec![], 404, serde_json::json!({})),
                (
                    "/rest/api/2/search",
                    vec!["maxResults=50", "startAt=0"],
                    200,
                    serde_json::json!({"issues": issues(0, 73), "total": 73, "startAt": 0}),
                ),
            ],
        );
        let page = client.search("project = ABC").unwrap();
        server.join().unwrap();
        assert_eq!(page.items.len(), 50);
        assert_eq!(page.total, Some(73));
        assert!(page.has_more);
    }

    #[test]
    fn stale_terminal_total_cannot_claim_complete() {
        let (client, server) = loopback(
            100,
            vec![(
                "/rest/api/2/search/jql",
                vec![],
                200,
                serde_json::json!({"issues": issues(0, 10), "isLast": true, "total": 73}),
            )],
        );
        let error = client.search("project = ABC").unwrap_err();
        assert!(error.contains("dataset changed") && error.contains("retry"));
        server.join().unwrap();
    }

    #[test]
    fn classic_empty_before_total_is_not_a_successful_search() {
        let (client, server) = loopback(
            100,
            vec![
                ("/rest/api/2/search/jql", vec![], 405, serde_json::json!({})),
                (
                    "/rest/api/2/search",
                    vec!["startAt=0"],
                    200,
                    serde_json::json!({"issues": issues(0, 50), "startAt": 0, "total": 73}),
                ),
                (
                    "/rest/api/2/search",
                    vec!["startAt=50"],
                    200,
                    serde_json::json!({"issues": [], "startAt": 50, "total": 73}),
                ),
            ],
        );
        let error = client.search("project = ABC").unwrap_err();
        assert!(error.contains("dataset changed") && error.contains("retry"));
        server.join().unwrap();
    }

    #[test]
    fn comments_single_page_retains_body_and_author() {
        let (client, server) = loopback(
            10,
            vec![(
                "/rest/api/2/issue/ABC-1/comment",
                vec!["orderBy=-created", "maxResults=10", "startAt=0"],
                200,
                serde_json::json!({"comments": [{
                    "id": "1",
                    "created": "2024-01-01T10:00:00.000+0000",
                    "author": {"displayName": "Ann"},
                    "body": "hello"
                }], "startAt": 0, "total": 1}),
            )],
        );
        let page = client.comments("ABC-1").unwrap();
        server.join().unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].author, "Ann");
        assert_eq!(page.items[0].body, "hello");
        assert_eq!(page.total, Some(1));
        assert!(!page.has_more);
    }

    #[test]
    fn comments_complete_pages_sort_and_resolve_cross_page_author() {
        let (client, server) = loopback(
            10,
            vec![
                (
                    "/rest/api/2/issue/ABC-1/comment",
                    vec!["orderBy=-created", "maxResults=10", "startAt=0"],
                    200,
                    serde_json::json!({"comments": [{
                        "id": "1",
                        "created": "2024-01-01T10:00:00.000+0000",
                        "body": "older [~accountid:a1]"
                    }], "startAt": 0, "total": 2}),
                ),
                (
                    "/rest/api/2/issue/ABC-1/comment",
                    vec!["orderBy=-created", "maxResults=9", "startAt=1"],
                    200,
                    serde_json::json!({"comments": [{
                        "id": "2",
                        "created": "2024-01-01T13:00:00.000+0200",
                        "author": {"accountId": "a1", "displayName": "Ann"},
                        "body": "newer"
                    }], "startAt": 1, "total": 2}),
                ),
            ],
        );
        let page = client.comments("ABC-1").unwrap();
        server.join().unwrap();
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.items[0].body, "newer");
        assert_eq!(page.items[1].body, "older @Ann");
        assert_eq!(page.total, Some(2));
        assert!(!page.has_more);
    }

    #[test]
    fn comments_later_page_failure_is_fatal() {
        let (client, server) = loopback(
            10,
            vec![
                (
                    "/rest/api/2/issue/ABC-1/comment",
                    vec!["orderBy=-created", "maxResults=10", "startAt=0"],
                    200,
                    serde_json::json!({"comments": [{"id": "1", "body": "first"}], "startAt": 0, "total": 2}),
                ),
                (
                    "/rest/api/2/issue/ABC-1/comment",
                    vec!["orderBy=-created", "maxResults=9", "startAt=1"],
                    503,
                    serde_json::json!({"errorMessages": ["unavailable"]}),
                ),
            ],
        );
        let error = client.comments("ABC-1").unwrap_err();
        server.join().unwrap();
        assert!(error.contains("HTTP 503: unavailable"));
    }

    #[test]
    fn comments_cap_sorts_across_pages_and_replaces_known_authors() {
        let mut first = vec![serde_json::json!({
            "id": "0",
            "created": "2024-01-01T10:00:00.000+0000",
            "author": {"accountId": "a1", "displayName": "Ann"},
            "body": "first"
        })];
        first.extend((1..50).map(|n| {
            serde_json::json!({
                "id": n.to_string(),
                "created": "2024-01-01T09:00:00.000+0000", "body": "older"
            })
        }));
        let (client, server) = loopback(
            51,
            vec![
                (
                    "/rest/api/2/issue/ABC-1/comment",
                    vec!["orderBy=-created", "maxResults=50", "startAt=0"],
                    200,
                    serde_json::json!({"comments": first, "startAt": 0, "total": 73}),
                ),
                (
                    "/rest/api/2/issue/ABC-1/comment",
                    vec!["orderBy=-created", "maxResults=1", "startAt=50"],
                    200,
                    serde_json::json!({"comments": [{
                    "id": "50",
                    "created": "2024-01-01T13:00:00.000+0200",
                    "body": {"type": "doc", "content": [{"type": "paragraph", "content": [
                        {"type": "text", "text": "newest [~accountid:a1]"}
                    ]}]}
                }], "startAt": 50, "total": 73}),
                ),
            ],
        );
        let page = client.comments("ABC-1").unwrap();
        server.join().unwrap();
        assert_eq!(page.items.len(), 51);
        assert_eq!(page.items[0].body, "newest @Ann");
        assert_eq!(page.items[1].body, "first");
        assert_eq!(page.total, Some(73));
        assert!(page.has_more);
    }

    #[test]
    fn comment_display_cap_and_handoff_cap_are_independent() {
        let (client, server) = loopback(
            2,
            vec![
                (
                    "/rest/api/2/issue/ABC-1/comment",
                    vec!["orderBy=-created", "maxResults=2", "startAt=0"],
                    200,
                    serde_json::json!({"comments": [
                        {"id": "1", "body": "first"}, {"id": "2", "body": "second"}
                    ], "startAt": 0, "total": 4}),
                ),
                (
                    "/rest/api/2/issue/ABC-1/comment",
                    vec!["orderBy=-created", "maxResults=4", "startAt=0"],
                    200,
                    serde_json::json!({"comments": [
                        {"id": "1", "body": "first"}, {"id": "2", "body": "second"}
                    ], "startAt": 0, "total": 4}),
                ),
                (
                    "/rest/api/2/issue/ABC-1/comment",
                    vec!["orderBy=-created", "maxResults=2", "startAt=2"],
                    200,
                    serde_json::json!({"comments": [
                        {"id": "3", "body": "third"}, {"id": "4", "body": "fourth"}
                    ], "startAt": 2, "total": 4}),
                ),
            ],
        );
        let display = client.comments("ABC-1").unwrap();
        let handoff = client.comments_with_limit("ABC-1", 4).unwrap();
        server.join().unwrap();
        assert_eq!(display.items.len(), 2);
        assert_eq!(handoff.items.len(), 4);
        assert_eq!(display.total, Some(4));
        assert_eq!(handoff.total, Some(4));
        assert!(display.has_more);
        assert!(!handoff.has_more);
    }

    #[test]
    fn zero_comment_budget_probes_metadata_without_returning_content() {
        let (client, server) = loopback(
            50,
            vec![
                (
                    "/rest/api/2/issue/ABC-1/comment",
                    vec!["orderBy=-created", "maxResults=1", "startAt=0"],
                    200,
                    serde_json::json!({"comments": [{"id": "1", "body": "secret"}], "startAt": 0, "total": 7}),
                ),
                (
                    "/rest/api/2/issue/ABC-2/comment",
                    vec!["orderBy=-created", "maxResults=1", "startAt=0"],
                    200,
                    serde_json::json!({"comments": [], "startAt": 0, "total": 0}),
                ),
            ],
        );
        let populated = client.comments_with_limit("ABC-1", 0).unwrap();
        let empty = client.comments_with_limit("ABC-2", 0).unwrap();
        server.join().unwrap();
        assert!(populated.items.is_empty());
        assert_eq!(populated.total, Some(7));
        assert!(populated.has_more);
        assert!(empty.items.is_empty());
        assert_eq!(empty.total, Some(0));
        assert!(!empty.has_more);
    }

    #[test]
    fn handoff_comments_later_page_failure_is_terminal() {
        let (client, server) = loopback(
            1,
            vec![
                (
                    "/rest/api/2/issue/ABC-1/comment",
                    vec!["orderBy=-created", "maxResults=4", "startAt=0"],
                    200,
                    serde_json::json!({"comments": [{"id": "1", "body": "first"}], "startAt": 0, "total": 4}),
                ),
                (
                    "/rest/api/2/issue/ABC-1/comment",
                    vec!["orderBy=-created", "maxResults=3", "startAt=1"],
                    503,
                    serde_json::json!({"errorMessages": ["unavailable"]}),
                ),
            ],
        );
        let error = client.comments_with_limit("ABC-1", 4).unwrap_err();
        server.join().unwrap();
        assert!(error.contains("HTTP 503: unavailable"));
    }

    #[test]
    fn detail_fetch_is_fresh_authenticated_and_checks_key() {
        let (client, server) = loopback(
            1,
            vec![
                (
                    "/rest/api/2/issue/ABC-1",
                    vec!["fields="],
                    200,
                    serde_json::json!({
                        "key": "ABC-1", "fields": {"summary": "fresh", "description": {
                            "type": "doc", "content": [{"type": "paragraph", "content": [
                                {"type": "text", "text": "current"}
                            ]}]
                        }}
                    }),
                ),
                (
                    "/rest/api/2/issue/ABC-2",
                    vec!["fields="],
                    200,
                    serde_json::json!({"key": "ABC-3", "fields": {"summary": "wrong"}}),
                ),
            ],
        );
        let issue = client.issue("ABC-1").unwrap();
        assert_eq!(issue.summary, "fresh");
        assert_eq!(issue.description, "current");
        assert!(issue.url.ends_with("/browse/ABC-1"));
        assert!(client
            .issue("ABC-2")
            .unwrap_err()
            .contains("unexpected issue key"));
        server.join().unwrap();
    }
    #[test]
    fn latest_total_controls_cloud_and_classic_completion() {
        for classic in [false, true] {
            for last_total in [Some(70), None] {
                let mut replies = Vec::new();
                let path = if classic {
                    "/rest/api/2/search"
                } else {
                    "/rest/api/2/search/jql"
                };
                if classic {
                    replies.push(("/rest/api/2/search/jql", vec![], 404, serde_json::json!({})));
                }
                replies.push((path, vec![], 200, if classic {
                    serde_json::json!({"issues": issues(0, 50), "startAt": 0, "total": 73})
                } else {
                    serde_json::json!({"issues": issues(0, 50), "total": 73, "nextPageToken": "next"})
                }));
                let mut second = if classic {
                    serde_json::json!({"issues": issues(50, 20), "startAt": 50})
                } else {
                    serde_json::json!({"issues": issues(50, 20), "isLast": true})
                };
                if let Some(total) = last_total {
                    second["total"] = total.into();
                }
                replies.push((
                    path,
                    vec![if classic {
                        "startAt=50"
                    } else {
                        "nextPageToken=next"
                    }],
                    200,
                    second,
                ));
                if classic && last_total.is_none() {
                    replies.push((
                        path,
                        vec!["startAt=70"],
                        200,
                        serde_json::json!({"issues": [], "startAt": 70}),
                    ));
                }
                let (client, server) = loopback(100, replies);
                let page = client.search("project = ABC").unwrap();
                server.join().unwrap();
                assert_eq!(page.items.len(), 70);
                assert_eq!(page.total, last_total);
                assert!(!page.has_more);
            }
        }
    }

    #[test]
    fn classic_growth_and_changed_dataset_boundaries() {
        let (client, server) = loopback(
            100,
            vec![
                ("/rest/api/2/search/jql", vec![], 405, serde_json::json!({})),
                (
                    "/rest/api/2/search",
                    vec!["startAt=0"],
                    200,
                    serde_json::json!({"issues": issues(0, 50), "startAt": 0, "total": 73}),
                ),
                (
                    "/rest/api/2/search",
                    vec!["startAt=50"],
                    200,
                    serde_json::json!({"issues": issues(50, 20), "startAt": 50, "total": 75}),
                ),
                (
                    "/rest/api/2/search",
                    vec!["startAt=70"],
                    200,
                    serde_json::json!({"issues": issues(70, 5), "startAt": 70, "total": 75}),
                ),
            ],
        );
        let page = client.search("project = ABC").unwrap();
        server.join().unwrap();
        assert_eq!(
            (page.items.len(), page.total, page.has_more),
            (75, Some(75), false)
        );
        for (second, diagnostic) in [
            (
                serde_json::json!({"issues": [], "startAt": 50, "total": 70}),
                "dataset changed",
            ),
            (
                serde_json::json!({"issues": [], "startAt": 50, "total": 45}),
                "dataset changed",
            ),
        ] {
            let (client, server) = loopback(
                100,
                vec![
                    ("/rest/api/2/search/jql", vec![], 404, serde_json::json!({})),
                    (
                        "/rest/api/2/search",
                        vec![],
                        200,
                        serde_json::json!({"issues": issues(0, 50), "startAt": 0, "total": 73}),
                    ),
                    ("/rest/api/2/search", vec!["startAt=50"], 200, second),
                ],
            );
            let error = client.search("project = ABC").unwrap_err();
            server.join().unwrap();
            assert!(error.contains(diagnostic) && error.contains("retry"));
        }
        let (client, server) = loopback(
            100,
            vec![
                ("/rest/api/2/search/jql", vec![], 404, serde_json::json!({})),
                (
                    "/rest/api/2/search",
                    vec![],
                    200,
                    serde_json::json!({"issues": issues(0, 50), "startAt": 0, "total": 73}),
                ),
                (
                    "/rest/api/2/search",
                    vec!["startAt=50"],
                    200,
                    serde_json::json!({"issues": [], "startAt": 50, "total": 50}),
                ),
            ],
        );
        let page = client.search("project = ABC").unwrap();
        server.join().unwrap();
        assert_eq!(
            (page.items.len(), page.total, page.has_more),
            (50, Some(50), false)
        );
    }

    #[test]
    fn cloud_empty_advancing_pages_and_request_budget() {
        for first_empty in [true, false] {
            let (client, server) = loopback(
                2,
                vec![
                    (
                        "/rest/api/2/search/jql",
                        vec![],
                        200,
                        if first_empty {
                            serde_json::json!({"issues": [], "isLast": false, "nextPageToken": "a"})
                        } else {
                            serde_json::json!({"issues": issues(0, 1), "nextPageToken": "a"})
                        },
                    ),
                    (
                        "/rest/api/2/search/jql",
                        vec!["nextPageToken=a"],
                        200,
                        if first_empty {
                            serde_json::json!({"issues": issues(0, 1), "nextPageToken": "b"})
                        } else {
                            serde_json::json!({"issues": [], "nextPageToken": "b"})
                        },
                    ),
                    (
                        "/rest/api/2/search/jql",
                        vec!["nextPageToken=b"],
                        200,
                        serde_json::json!({"issues": issues(1, 1), "isLast": true, "total": 2}),
                    ),
                ],
            );
            let page = client.search("project = ABC").unwrap();
            server.join().unwrap();
            assert_eq!(
                (page.items.len(), page.total, page.has_more),
                (2, Some(2), false)
            );
        }
        let (client, server) = loopback(
            2,
            vec![
                (
                    "/rest/api/2/search/jql",
                    vec![],
                    200,
                    serde_json::json!({"issues": [], "nextPageToken": "a"}),
                ),
                (
                    "/rest/api/2/search/jql",
                    vec!["nextPageToken=a"],
                    200,
                    serde_json::json!({"issues": [], "nextPageToken": "b"}),
                ),
                (
                    "/rest/api/2/search/jql",
                    vec!["nextPageToken=b"],
                    200,
                    serde_json::json!({"issues": [], "nextPageToken": "c"}),
                ),
            ],
        );
        let error = client.search("project = ABC").unwrap_err();
        server.join().unwrap();
        assert!(error.contains("budget exhausted") && error.contains("retry"));
    }

    #[test]
    fn cloud_continuation_metadata_and_cap_boundaries() {
        for (first, second, expected) in [
            (
                serde_json::json!({"issues": [], "nextPageToken": "a"}),
                Some(serde_json::json!({"issues": [], "nextPageToken": "a"})),
                "repeated",
            ),
            (
                serde_json::json!({"issues": [], "isLast": false}),
                None,
                "no next-page token",
            ),
            (
                serde_json::json!({"issues": [], "total": 0}),
                None,
                "terminal or continuation",
            ),
            (
                serde_json::json!({"issues": issues(0, 1), "isLast": true, "nextPageToken": "a"}),
                None,
                "conflicting",
            ),
            (
                serde_json::json!({"issues": issues(0, 1), "nextPageToken": "a"}),
                Some(serde_json::json!({"issues": [], "isLast": true, "total": 2})),
                "dataset changed",
            ),
        ] {
            let mut replies = vec![("/rest/api/2/search/jql", vec![], 200, first)];
            if let Some(second) = second {
                replies.push((
                    "/rest/api/2/search/jql",
                    vec!["nextPageToken=a"],
                    200,
                    second,
                ));
            }
            let (client, server) = loopback(2, replies);
            let error = client.search("project = ABC").unwrap_err();
            server.join().unwrap();
            assert!(error.contains(expected), "{error}");
        }
        for (metadata, more) in [
            (serde_json::json!({"nextPageToken": "a", "total": 1}), true),
            (serde_json::json!({"isLast": true, "total": 1}), false),
        ] {
            let mut payload = metadata;
            payload["issues"] = serde_json::json!(issues(0, 1));
            let (client, server) =
                loopback(1, vec![("/rest/api/2/search/jql", vec![], 200, payload)]);
            let page = client.search("project = ABC").unwrap();
            server.join().unwrap();
            assert_eq!(page.items.len(), 1);
            assert_eq!(page.has_more, more);
        }
    }

    #[test]
    fn repeated_and_invalid_issue_identifiers_reject_complete_and_capped_pages() {
        for (rows, metadata) in [
            (
                serde_json::json!([issues(0, 1)[0].clone(), issues(0, 1)[0].clone()]),
                serde_json::json!({"isLast": true, "total": 2}),
            ),
            (
                serde_json::json!([issues(0, 1)[0].clone(), issues(0, 1)[0].clone()]),
                serde_json::json!({"nextPageToken": "a", "total": 2}),
            ),
            (
                serde_json::json!([{"fields": {}}]),
                serde_json::json!({"isLast": true}),
            ),
            (
                serde_json::json!([{"key": 42, "fields": {}}]),
                serde_json::json!({"isLast": true}),
            ),
            (
                serde_json::json!([{"key": "", "fields": {}}]),
                serde_json::json!({"isLast": true}),
            ),
        ] {
            let mut payload = metadata;
            payload["issues"] = rows;
            let (client, server) =
                loopback(2, vec![("/rest/api/2/search/jql", vec![], 200, payload)]);
            let error = client.search("project = ABC").unwrap_err();
            server.join().unwrap();
            assert!(error.contains("key"), "{error}");
        }
        for classic in [false, true] {
            let mut replies = Vec::new();
            let path = if classic {
                "/rest/api/2/search"
            } else {
                "/rest/api/2/search/jql"
            };
            if classic {
                replies.push(("/rest/api/2/search/jql", vec![], 404, serde_json::json!({})));
            }
            replies.push((
                path,
                vec![],
                200,
                if classic {
                    serde_json::json!({"issues": issues(0, 1), "startAt": 0, "total": 2})
                } else {
                    serde_json::json!({"issues": issues(0, 1), "nextPageToken": "a"})
                },
            ));
            replies.push((
                path,
                vec![],
                200,
                if classic {
                    serde_json::json!({"issues": issues(0, 1), "startAt": 1, "total": 2})
                } else {
                    serde_json::json!({"issues": issues(0, 1), "isLast": true, "total": 2})
                },
            ));
            let (client, server) = loopback(2, replies);
            let error = client.search("project = ABC").unwrap_err();
            server.join().unwrap();
            assert!(error.contains("repeated key"), "{error}");
        }
        // Excluded rows are never validated.
        let (client, server) = loopback(
            1,
            vec![(
                "/rest/api/2/search/jql",
                vec![],
                200,
                serde_json::json!({"issues": [issues(0, 1)[0].clone(), {"key": "ABC-0"}], "isLast": true, "total": 2}),
            )],
        );
        let page = client.search("project = ABC").unwrap();
        server.join().unwrap();
        assert_eq!(page.items.len(), 1);
        assert!(page.has_more);
    }

    #[test]
    fn comment_ids_are_required_unique_and_independent_of_body() {
        for rows in [
            serde_json::json!([{"id": "1", "body": "same"}, {"id": "1", "body": "same"}]),
            serde_json::json!([{"body": "same"}]),
            serde_json::json!([{"id": null, "body": "same"}]),
            serde_json::json!([{"id": "", "body": "same"}]),
            serde_json::json!([{"id": 7, "body": "same"}]),
        ] {
            let (client, server) = loopback(
                2,
                vec![(
                    "/rest/api/2/issue/ABC-1/comment",
                    vec![],
                    200,
                    serde_json::json!({"comments": rows, "startAt": 0, "total": 2}),
                )],
            );
            let error = client.comments("ABC-1").unwrap_err();
            server.join().unwrap();
            assert!(error.contains("id"), "{error}");
        }
        let (client, server) = loopback(
            2,
            vec![
                (
                    "/rest/api/2/issue/ABC-1/comment",
                    vec![],
                    200,
                    serde_json::json!({"comments": [{"id": "1", "body": "same"}], "startAt": 0, "total": 2}),
                ),
                (
                    "/rest/api/2/issue/ABC-1/comment",
                    vec!["startAt=1"],
                    200,
                    serde_json::json!({"comments": [{"id": "1", "body": "same"}], "startAt": 1, "total": 2}),
                ),
            ],
        );
        let error = client.comments("ABC-1").unwrap_err();
        server.join().unwrap();
        assert!(error.contains("repeated id"), "{error}");
        let (client, server) = loopback(
            2,
            vec![
                (
                    "/rest/api/2/issue/ABC-1/comment",
                    vec![],
                    200,
                    serde_json::json!({"comments": [{"id": "1", "body": "same"}], "startAt": 0, "total": 2}),
                ),
                (
                    "/rest/api/2/issue/ABC-1/comment",
                    vec!["startAt=1"],
                    200,
                    serde_json::json!({"comments": [{"id": "2", "body": "same"}], "startAt": 1, "total": 2}),
                ),
            ],
        );
        let page = client.comments("ABC-1").unwrap();
        server.join().unwrap();
        assert_eq!(page.items.len(), 2);
        assert!(!page.has_more);
        let (client, server) = loopback(
            1,
            vec![(
                "/rest/api/2/issue/ABC-1/comment",
                vec![],
                200,
                serde_json::json!({"comments": [{"body": "secret"}], "total": 1}),
            )],
        );
        let error = client.comments_with_limit("ABC-1", 0).unwrap_err();
        server.join().unwrap();
        assert!(error.contains("id"));
    }

    #[test]
    fn fresh_issue_requires_fields_object_but_not_description() {
        for fields in [
            None,
            Some(Value::Null),
            Some(serde_json::json!([])),
            Some(serde_json::json!("wrong")),
            Some(serde_json::json!(1)),
        ] {
            let mut payload = serde_json::json!({"key": "ABC-1"});
            if let Some(fields) = fields {
                payload["fields"] = fields;
            }
            let (client, server) =
                loopback(1, vec![("/rest/api/2/issue/ABC-1", vec![], 200, payload)]);
            let error = client.issue("ABC-1").unwrap_err();
            server.join().unwrap();
            assert!(error.contains("fields object"), "{error}");
        }
        for fields in [
            serde_json::json!({}),
            serde_json::json!({"description": null}),
        ] {
            let (client, server) = loopback(
                1,
                vec![(
                    "/rest/api/2/issue/ABC-1",
                    vec![],
                    200,
                    serde_json::json!({"key": "ABC-1", "fields": fields}),
                )],
            );
            let issue = client.issue("ABC-1").unwrap();
            server.join().unwrap();
            assert_eq!(issue.key, "ABC-1");
            assert!(issue.description.is_empty());
        }
    }
    #[test]
    fn comments_shrinking_total_and_exact_cap_remain_ordered() {
        let (client, server) = loopback(
            2,
            vec![
                (
                    "/rest/api/2/issue/ABC-1/comment",
                    vec!["startAt=0"],
                    200,
                    serde_json::json!({"comments": [{"id": "1", "body": "older", "created": "2024-01-01T10:00:00.000+0000"}],
                    "startAt": 0, "total": 3}),
                ),
                (
                    "/rest/api/2/issue/ABC-1/comment",
                    vec!["startAt=1"],
                    200,
                    serde_json::json!({"comments": [{"id": "2", "body": "newer", "created": "2024-01-02T10:00:00.000+0000"}],
                    "startAt": 1, "total": 2}),
                ),
            ],
        );
        let page = client.comments("ABC-1").unwrap();
        server.join().unwrap();
        assert_eq!(
            (page.items.len(), page.total, page.has_more),
            (2, Some(2), false)
        );
        assert_eq!(page.items[0].body, "newer");
        let (client, server) = loopback(
            2,
            vec![(
                "/rest/api/2/issue/ABC-1/comment",
                vec![],
                200,
                serde_json::json!({"comments": [{"id": "1", "body": "first"},
                                                {"id": "1", "body": "second"}],
                    "startAt": 0, "total": 2}),
            )],
        );
        let error = client.comments("ABC-1").unwrap_err();
        server.join().unwrap();
        assert!(error.contains("repeated id"));
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
